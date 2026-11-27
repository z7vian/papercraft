//! Spreadsheet editing: cell input with recalculation, structure (rows, columns, sheets), styles, undo and save.
//! Legacy .xls/.et files are converted to OOXML on open and always save as .xlsx.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use betteroffice_xlsx::{
    CalculationOptions, CellAddress, CellRange, CellRef, HorizontalAlignment, NumberFormatKind, NumberFormatMutation,
    Op, SheetId, StylePatch, TextWrapping, VerticalAlignment, Workbook,
};

use crate::CoreError;
use crate::office::OfficeFamily;
use crate::office::convert::to_ooxml;

/// A cell whose shown value changed, with its new display text.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetCellChange {
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
    pub display: String,
}

/// What a cell holds: `input` is what the user typed (a formula starts with `=`), `display` the formatted value.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetCellContent {
    pub input: String,
    pub display: String,
    pub is_formula: bool,
}

/// An inclusive block of cells, zero-based.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct SheetCellRange {
    pub first_row: u32,
    pub first_col: u32,
    pub last_row: u32,
    pub last_col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum SheetEditHAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum SheetEditVAlign {
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SheetNumberFormat {
    General,
    Text,
    Number,
    Percent,
    Scientific,
    Currency,
    Date,
    Time,
    /// An Excel format code such as `0.000` or `dd/mm/yyyy`.
    Custom { pattern: String },
}

/// A cell's look. Setting: `None` leaves that property as it is. Reading: every property is filled.
/// Colours are `#rrggbb`.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SheetStyleEdit {
    #[uniffi(default)]
    pub bold: Option<bool>,
    #[uniffi(default)]
    pub italic: Option<bool>,
    #[uniffi(default)]
    pub text_color: Option<String>,
    #[uniffi(default)]
    pub fill_color: Option<String>,
    #[uniffi(default)]
    pub horizontal: Option<SheetEditHAlign>,
    #[uniffi(default)]
    pub vertical: Option<SheetEditVAlign>,
    #[uniffi(default)]
    pub wrap: Option<bool>,
    #[uniffi(default)]
    pub number_format: Option<SheetNumberFormat>,
}

type Job = Box<dyn FnOnce(&mut Workbook) + Send>;

// Formula evaluation and parsing recurse on nested content, as in the viewer's engine threads.
const ENGINE_STACK: usize = 32 * 1024 * 1024;

/// An editable workbook. One engine thread owns it and runs every call in order.
#[derive(uniffi::Object)]
pub struct SheetEditor {
    jobs: Sender<Job>,
}

fn unreadable(e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this spreadsheet: {e}"))
}

fn refused(e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't make this change: {e}"))
}

fn engine_stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped editing this file because it hit something it can't handle.".into())
}

/// The clock TODAY() and NOW() read, as an Excel serial day.
// Limit: TODAY() and NOW() use UTC, so near midnight they show the neighbouring day; passing the device's UTC offset from Kotlin lifts it.
fn options() -> CalculationOptions {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    CalculationOptions { now_serial: Some(secs / 86_400.0 + 25_569.0) }
}

fn cell_range(range: SheetCellRange) -> CellRange {
    CellRange::new(CellRef::new(range.first_row, range.first_col), CellRef::new(range.last_row, range.last_col))
}

fn display(book: &Workbook, sheet: u32, row: u32, col: u32) -> Result<String, CoreError> {
    let model = book.model();
    let sheet = model.sheets.get(sheet as usize).ok_or_else(|| refused(format!("there is no sheet {sheet}")))?;
    Ok(sheet
        .cell(CellRef::new(row, col))
        .map(|cell| xlsx_render::display_text(&model.styles, model.date_system, cell))
        .unwrap_or_default())
}

fn changes(book: &Workbook, cells: impl IntoIterator<Item = CellAddress>) -> Result<Vec<SheetCellChange>, CoreError> {
    cells
        .into_iter()
        .map(|CellAddress { sheet, cell }| {
            Ok(SheetCellChange { sheet: sheet.0, row: cell.row, col: cell.col, display: display(book, sheet.0, cell.row, cell.col)? })
        })
        .collect()
}

fn open_book(bytes: Vec<u8>) -> Result<Workbook, CoreError> {
    let bytes = if bytes.starts_with(b"PK") { bytes } else { to_ooxml(bytes, OfficeFamily::Spreadsheet)? };
    // Keeps the values the file cached; calculates only formulas it saved none for (converted files save none).
    Workbook::open_recalculating_stale(&bytes, options(), None).map_err(unreadable)
}

fn apply(book: &mut Workbook, ops: Vec<Op>) -> Result<(), CoreError> {
    book.apply_ops(ops, options()).map(drop).map_err(refused)
}

fn style_patch(style: &SheetStyleEdit) -> StylePatch {
    StylePatch {
        bold: style.bold,
        italic: style.italic,
        text_color: style.text_color.clone(),
        fill_color: style.fill_color.clone(),
        horizontal_alignment: style.horizontal.map(|h| match h {
            SheetEditHAlign::Left => HorizontalAlignment::Left,
            SheetEditHAlign::Center => HorizontalAlignment::Center,
            SheetEditHAlign::Right => HorizontalAlignment::Right,
        }),
        vertical_alignment: style.vertical.map(|v| match v {
            SheetEditVAlign::Top => VerticalAlignment::Top,
            SheetEditVAlign::Middle => VerticalAlignment::Middle,
            SheetEditVAlign::Bottom => VerticalAlignment::Bottom,
        }),
        text_wrapping: style.wrap.map(|wrap| if wrap { TextWrapping::Wrap } else { TextWrapping::Overflow }),
        ..StylePatch::default()
    }
}

fn number_format_mutation(format: SheetNumberFormat) -> NumberFormatMutation {
    match format {
        SheetNumberFormat::General => NumberFormatMutation::Automatic,
        SheetNumberFormat::Text => NumberFormatMutation::PlainText,
        SheetNumberFormat::Number => NumberFormatMutation::Number,
        SheetNumberFormat::Percent => NumberFormatMutation::Percent,
        SheetNumberFormat::Scientific => NumberFormatMutation::Scientific,
        SheetNumberFormat::Currency => NumberFormatMutation::Currency,
        SheetNumberFormat::Date => NumberFormatMutation::Date,
        SheetNumberFormat::Time => NumberFormatMutation::Time,
        SheetNumberFormat::Custom { pattern } => NumberFormatMutation::Custom { pattern },
    }
}

// Limit: a built-in format outside the named kinds (fractions, locale dates) reads as Custom with an empty pattern; resolving built-in format ids to their codes lifts it.
fn number_format(kind: NumberFormatKind, pattern: Option<String>) -> SheetNumberFormat {
    match kind {
        NumberFormatKind::Automatic => SheetNumberFormat::General,
        NumberFormatKind::PlainText => SheetNumberFormat::Text,
        NumberFormatKind::Number => SheetNumberFormat::Number,
        NumberFormatKind::Percent => SheetNumberFormat::Percent,
        NumberFormatKind::Scientific => SheetNumberFormat::Scientific,
        NumberFormatKind::Currency => SheetNumberFormat::Currency,
        NumberFormatKind::Date => SheetNumberFormat::Date,
        NumberFormatKind::Time => SheetNumberFormat::Time,
        NumberFormatKind::Custom => SheetNumberFormat::Custom { pattern: pattern.unwrap_or_default() },
    }
}

impl SheetEditor {
    fn ask<T: Send + 'static>(&self, f: impl FnOnce(&mut Workbook) -> Result<T, CoreError> + Send + 'static) -> Result<T, CoreError> {
        let (reply, answer) = channel();
        self.jobs.send(Box::new(move |book: &mut Workbook| drop(reply.send(f(book))))).map_err(|_| engine_stopped())?;
        answer.recv().map_err(|_| engine_stopped())?
    }
}

#[uniffi::export]
impl SheetEditor {
    /// Opens an .xlsx-family file, or a legacy .xls/.et one (converted; it saves as .xlsx).
    /// `family` must be `Spreadsheet`.
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, family: OfficeFamily) -> Result<Arc<Self>, CoreError> {
        if !matches!(family, OfficeFamily::Spreadsheet) {
            return Err(CoreError::BadFile("Only spreadsheets open in the sheet editor.".into()));
        }
        let (jobs, inbox) = channel::<Job>();
        let (opened_tx, opened) = channel();
        thread::Builder::new()
            .name("sheet-editor".into())
            .stack_size(ENGINE_STACK)
            .spawn(move || {
                let mut book = match open_book(bytes) {
                    Ok(book) => book,
                    Err(e) => return drop(opened_tx.send(Err(e))),
                };
                let _ = opened_tx.send(Ok(()));
                // Runs until the SheetEditor is dropped, which closes the channel.
                for job in inbox {
                    job(&mut book);
                }
            })
            .map_err(|e| CoreError::BadFile(format!("Can't start editing this file: {e}")))?;
        opened.recv().map_err(|_| engine_stopped())??;
        Ok(Arc::new(Self { jobs }))
    }

    pub fn sheet_names(&self) -> Result<Vec<String>, CoreError> {
        self.ask(|book| Ok(book.model().sheets.iter().map(|s| s.name.clone()).collect()))
    }

    /// Enters `input` as a user types it: a number, text, date, TRUE/FALSE, or a formula when it starts with `=`.
    /// Empty input clears the cell. Returns the edited cell and every cell recalculation changed, with their new display text.
    pub fn set_cell(&self, sheet: u32, row: u32, col: u32, input: String) -> Result<Vec<SheetCellChange>, CoreError> {
        self.ask(move |book| {
            let at = CellRef::new(row, col);
            let result = book.edit_cell(SheetId(sheet), at, &input, options()).map_err(refused)?;
            if !result.applied {
                return Ok(Vec::new());
            }
            changes(book, std::iter::once(CellAddress { sheet: SheetId(sheet), cell: at }).chain(result.changed))
        })
    }

    pub fn get_cell(&self, sheet: u32, row: u32, col: u32) -> Result<SheetCellContent, CoreError> {
        self.ask(move |book| {
            let cell = book.cell(SheetId(sheet), CellRef::new(row, col)).map_err(refused)?;
            Ok(SheetCellContent { input: cell.input, display: display(book, sheet, row, col)?, is_formula: cell.is_formula })
        })
    }

    /// Inserts `count` empty rows before row `at`; formulas that point below it shift down.
    pub fn insert_rows(&self, sheet: u32, at: u32, count: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::InsertRows { sheet: SheetId(sheet), at, count }]))
    }

    /// Deletes `count` rows from row `at`; references to them become `#REF!`.
    pub fn delete_rows(&self, sheet: u32, at: u32, count: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::DeleteRows { sheet: SheetId(sheet), at, count }]))
    }

    pub fn insert_columns(&self, sheet: u32, at: u32, count: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::InsertCols { sheet: SheetId(sheet), at, count }]))
    }

    pub fn delete_columns(&self, sheet: u32, at: u32, count: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::DeleteCols { sheet: SheetId(sheet), at, count }]))
    }

    /// Applies every property `style` sets to `range` as one undo step.
    pub fn set_cell_style(&self, sheet: u32, range: SheetCellRange, style: SheetStyleEdit) -> Result<(), CoreError> {
        self.ask(move |book| {
            let (sheet, range) = (SheetId(sheet), cell_range(range));
            let mut ops = Vec::with_capacity(2);
            let patch = style_patch(&style);
            if patch != StylePatch::default() {
                ops.push(Op::PatchRangeStyle { sheet, range, patch });
            }
            if let Some(format) = style.number_format {
                ops.push(Op::SetRangeNumberFormat { sheet, range, format: number_format_mutation(format) });
            }
            apply(book, ops)
        })
    }

    pub fn set_number_format(&self, sheet: u32, range: SheetCellRange, format: SheetNumberFormat) -> Result<(), CoreError> {
        self.ask(move |book| {
            book.set_range_number_format(SheetId(sheet), cell_range(range), number_format_mutation(format), options())
                .map(drop)
                .map_err(refused)
        })
    }

    pub fn get_cell_style(&self, sheet: u32, row: u32, col: u32) -> Result<SheetStyleEdit, CoreError> {
        self.ask(move |book| {
            let at = CellRef::new(row, col);
            let f = book.selection_formatting(SheetId(sheet), CellRange::new(at, at)).map_err(refused)?;
            Ok(SheetStyleEdit {
                bold: f.bold,
                italic: f.italic,
                text_color: f.text_color,
                fill_color: f.fill_color,
                horizontal: f.horizontal_alignment.map(|h| match h {
                    HorizontalAlignment::Left => SheetEditHAlign::Left,
                    HorizontalAlignment::Center => SheetEditHAlign::Center,
                    HorizontalAlignment::Right => SheetEditHAlign::Right,
                }),
                vertical: f.vertical_alignment.map(|v| match v {
                    VerticalAlignment::Top => SheetEditVAlign::Top,
                    VerticalAlignment::Middle => SheetEditVAlign::Middle,
                    VerticalAlignment::Bottom => SheetEditVAlign::Bottom,
                }),
                wrap: f.text_wrapping.map(|w| w == TextWrapping::Wrap),
                number_format: f.number_format.map(|kind| number_format(kind, f.number_format_pattern)),
            })
        })
    }

    /// Width in characters of the default font (Excel's unit); `None` resets to the sheet default.
    pub fn set_column_width(&self, sheet: u32, col: u32, width: Option<f64>) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::SetColWidth { sheet: SheetId(sheet), col, width }]))
    }

    /// Height in points; `None` resets to the sheet default.
    pub fn set_row_height(&self, sheet: u32, row: u32, height: Option<f64>) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::SetRowHeight { sheet: SheetId(sheet), row, height }]))
    }

    /// The width set on column `col`, or `None` when it uses the sheet default.
    pub fn column_width(&self, sheet: u32, col: u32) -> Result<Option<f64>, CoreError> {
        self.ask(move |book| Ok(book.sheet(SheetId(sheet)).map_err(refused)?.col_widths.get(&col).copied()))
    }

    /// The height set on row `row`, or `None` when it uses the sheet default.
    pub fn row_height(&self, sheet: u32, row: u32) -> Result<Option<f64>, CoreError> {
        self.ask(move |book| Ok(book.sheet(SheetId(sheet)).map_err(refused)?.row_heights.get(&row).copied()))
    }

    /// Adds an empty sheet named `name` at position `index` (0 = first, sheet count = last).
    pub fn add_sheet(&self, index: u32, name: String) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::AddSheet { index: index as usize, name }]))
    }

    /// Renames a sheet; formulas that name it follow.
    pub fn rename_sheet(&self, sheet: u32, name: String) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::RenameSheet { sheet: SheetId(sheet), name }]))
    }

    /// Deletes a sheet; formulas that read it become `#REF!`. The last sheet can't be deleted.
    pub fn delete_sheet(&self, sheet: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::RemoveSheet { index: sheet as usize }]))
    }

    /// Moves the sheet at position `from` to position `to`.
    pub fn move_sheet(&self, from: u32, to: u32) -> Result<(), CoreError> {
        self.ask(move |book| apply(book, vec![Op::MoveSheet { from: from as usize, to: to as usize }]))
    }

    /// Undoes the last edit; `false` when there is none. Reload the shown cells afterwards.
    pub fn undo(&self) -> Result<bool, CoreError> {
        self.ask(|book| book.undo(options()).map(|r| r.applied).map_err(refused))
    }

    /// Redoes the last undone edit; `false` when there is none. Reload the shown cells afterwards.
    pub fn redo(&self) -> Result<bool, CoreError> {
        self.ask(|book| book.redo(options()).map(|r| r.applied).map_err(refused))
    }

    pub fn can_undo(&self) -> Result<bool, CoreError> {
        self.ask(|book| Ok(book.can_undo()))
    }

    pub fn can_redo(&self) -> Result<bool, CoreError> {
        self.ask(|book| Ok(book.can_redo()))
    }

    /// The workbook as .xlsx bytes. An edited save drops `xl/calcChain.xml` (with its relationship and content
    /// type); the part is optional and Excel rebuilds it on open.
    pub fn save(&self) -> Result<Vec<u8>, CoreError> {
        self.ask(|book| book.save().map_err(|e| CoreError::BadFile(format!("Can't save this spreadsheet: {e}"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{Cursor, Read};

    // chain.xlsx: written for these tests (Apache-2.0, this repo). Budget: A1=2, A2=3, A3=A1+A2, A4=A3*10,
    // B1 "Total" (bold), B4=SUM(A1:A3); Notes: A1=Budget!A4; it carries a calcChain.xml.
    const CHAIN: &[u8] = include_bytes!("../tests/fixtures/sheet_edit/chain.xlsx");
    // SimpleWithFormula.xls: Apache POI test-data/spreadsheet (Apache-2.0).
    const LEGACY_XLS: &[u8] = include_bytes!("../tests/fixtures/sheet_edit/SimpleWithFormula.xls");

    fn chain() -> Arc<SheetEditor> {
        SheetEditor::open(CHAIN.to_vec(), OfficeFamily::Spreadsheet).unwrap()
    }

    fn shown(editor: &SheetEditor, sheet: u32, row: u32, col: u32) -> String {
        editor.get_cell(sheet, row, col).unwrap().display
    }

    fn input(editor: &SheetEditor, sheet: u32, row: u32, col: u32) -> String {
        editor.get_cell(sheet, row, col).unwrap().input
    }

    fn change(sheet: u32, row: u32, col: u32, display: &str) -> SheetCellChange {
        SheetCellChange { sheet, row, col, display: display.into() }
    }

    #[test]
    fn opens_with_cached_values() {
        let editor = chain();
        assert_eq!(editor.sheet_names().unwrap(), ["Budget", "Notes"]);
        assert_eq!(
            editor.get_cell(0, 2, 0).unwrap(),
            SheetCellContent { input: "=A1+A2".into(), display: "5".into(), is_formula: true }
        );
        assert_eq!(shown(&editor, 1, 0, 0), "50");
    }

    #[test]
    fn formula_chain_recalculates_after_an_edit() {
        let editor = chain();
        let changed = editor.set_cell(0, 0, 0, "10".into()).unwrap();
        assert_eq!(changed[0], change(0, 0, 0, "10"));
        for expected in [change(0, 2, 0, "13"), change(0, 3, 0, "130"), change(0, 3, 1, "26"), change(1, 0, 0, "130")] {
            assert!(changed.contains(&expected), "{expected:?} missing from {changed:?}");
        }
        assert_eq!(shown(&editor, 1, 0, 0), "130");
    }

    #[test]
    fn input_kinds_and_new_formulas() {
        let editor = chain();
        editor.set_cell(0, 5, 0, "hello".into()).unwrap();
        editor.set_cell(0, 5, 1, "TRUE".into()).unwrap();
        editor.set_cell(0, 5, 2, "=A4/2".into()).unwrap();
        assert_eq!(shown(&editor, 0, 5, 0), "hello");
        assert_eq!(shown(&editor, 0, 5, 1), "TRUE");
        assert_eq!(editor.get_cell(0, 5, 2).unwrap(), SheetCellContent { input: "=A4/2".into(), display: "25".into(), is_formula: true });
        editor.set_cell(0, 5, 0, String::new()).unwrap();
        assert_eq!(editor.get_cell(0, 5, 0).unwrap(), SheetCellContent { input: String::new(), display: String::new(), is_formula: false });
    }

    #[test]
    fn inserting_a_row_shifts_formula_references() {
        let editor = chain();
        editor.insert_rows(0, 0, 1).unwrap();
        assert_eq!(input(&editor, 0, 3, 0), "=A2+A3");
        assert_eq!(input(&editor, 0, 4, 1), "=SUM(A2:A4)");
        assert_eq!(input(&editor, 1, 0, 0), "=Budget!A5");
        assert_eq!(shown(&editor, 1, 0, 0), "50");
        editor.set_cell(0, 1, 0, "4".into()).unwrap();
        assert_eq!(shown(&editor, 1, 0, 0), "70");
    }

    #[test]
    fn inserting_and_deleting_columns_shift_references() {
        let editor = chain();
        editor.insert_columns(0, 0, 2).unwrap();
        assert_eq!(input(&editor, 0, 2, 2), "=C1+C2");
        assert_eq!(input(&editor, 1, 0, 0), "=Budget!C4");
        editor.delete_columns(0, 0, 2).unwrap();
        assert_eq!(input(&editor, 0, 2, 0), "=A1+A2");
        editor.delete_rows(0, 3, 1).unwrap();
        let notes = editor.get_cell(1, 0, 0).unwrap();
        assert!(notes.input.contains("#REF!") && notes.display == "#REF!", "{notes:?}");
    }

    #[test]
    fn undo_restores_and_redo_reapplies() {
        let editor = chain();
        assert!(!editor.can_undo().unwrap());
        editor.set_cell(0, 0, 0, "10".into()).unwrap();
        editor.insert_rows(0, 0, 1).unwrap();
        assert!(editor.undo().unwrap());
        assert!(editor.undo().unwrap());
        assert_eq!(input(&editor, 0, 0, 0), "2");
        assert_eq!(shown(&editor, 0, 3, 0), "50");
        assert_eq!(shown(&editor, 1, 0, 0), "50");
        assert!(!editor.can_undo().unwrap());
        assert!(editor.can_redo().unwrap());
        assert!(editor.redo().unwrap());
        assert_eq!(shown(&editor, 1, 0, 0), "130");
        assert!(editor.undo().unwrap());
        assert_eq!(shown(&editor, 1, 0, 0), "50");
    }

    #[test]
    fn sheets_add_rename_move_delete() {
        let editor = chain();
        editor.add_sheet(2, "Extra".into()).unwrap();
        editor.rename_sheet(0, "Plan".into()).unwrap();
        assert_eq!(input(&editor, 1, 0, 0), "=Plan!A4");
        editor.move_sheet(2, 0).unwrap();
        assert_eq!(editor.sheet_names().unwrap(), ["Extra", "Plan", "Notes"]);
        editor.delete_sheet(0).unwrap();
        assert_eq!(editor.sheet_names().unwrap(), ["Plan", "Notes"]);
        assert!(editor.undo().unwrap());
        assert_eq!(editor.sheet_names().unwrap(), ["Extra", "Plan", "Notes"]);
        assert!(editor.rename_sheet(1, "Notes".into()).is_err());
    }

    #[test]
    fn save_and_reopen_keeps_values_and_styles() {
        let editor = chain();
        editor.set_cell(0, 0, 0, "10".into()).unwrap();
        let look = SheetStyleEdit {
            bold: Some(true),
            italic: Some(true),
            text_color: Some("#C00000".into()),
            fill_color: Some("#ffff00".into()),
            horizontal: Some(SheetEditHAlign::Center),
            vertical: Some(SheetEditVAlign::Top),
            wrap: Some(true),
            number_format: None,
        };
        let a1 = SheetCellRange { first_row: 0, first_col: 0, last_row: 0, last_col: 0 };
        editor.set_cell_style(0, a1, look).unwrap();
        let a2 = SheetCellRange { first_row: 1, first_col: 0, last_row: 1, last_col: 0 };
        editor.set_number_format(0, a2, SheetNumberFormat::Percent).unwrap();
        editor.set_column_width(0, 0, Some(20.0)).unwrap();
        editor.set_row_height(0, 0, Some(30.0)).unwrap();
        assert_eq!(shown(&editor, 0, 1, 0), "300.00%");

        let saved = editor.save().unwrap();
        assert_package_is_whole(&saved);
        let reopened = SheetEditor::open(saved, OfficeFamily::Spreadsheet).unwrap();
        assert_eq!(reopened.sheet_names().unwrap(), ["Budget", "Notes"]);
        assert_eq!(shown(&reopened, 0, 0, 0), "10");
        assert_eq!(reopened.get_cell(0, 3, 0).unwrap().input, "=A3*10");
        assert_eq!(shown(&reopened, 0, 3, 0), "130");
        assert_eq!(shown(&reopened, 1, 0, 0), "130");
        assert_eq!(shown(&reopened, 0, 1, 0), "300.00%");
        assert_eq!(
            reopened.get_cell_style(0, 0, 0).unwrap(),
            SheetStyleEdit {
                bold: Some(true),
                italic: Some(true),
                text_color: Some("#c00000".into()),
                fill_color: Some("#ffff00".into()),
                horizontal: Some(SheetEditHAlign::Center),
                vertical: Some(SheetEditVAlign::Top),
                wrap: Some(true),
                number_format: Some(SheetNumberFormat::General),
            }
        );
        assert_eq!(reopened.get_cell_style(0, 0, 1).unwrap().bold, Some(true));
        assert_eq!(reopened.get_cell_style(0, 1, 0).unwrap().number_format, Some(SheetNumberFormat::Percent));
        assert_eq!(reopened.column_width(0, 0).unwrap(), Some(20.0));
        assert_eq!(reopened.row_height(0, 0).unwrap(), Some(30.0));
    }

    #[test]
    fn legacy_xls_opens_edits_and_saves_as_xlsx() {
        let editor = SheetEditor::open(LEGACY_XLS.to_vec(), OfficeFamily::Spreadsheet).unwrap();
        assert!(!editor.sheet_names().unwrap().is_empty());
        editor.set_cell(0, 0, 0, "42".into()).unwrap();
        let saved = editor.save().unwrap();
        assert!(saved.starts_with(b"PK"));
        assert_package_is_whole(&saved);
        let reopened = SheetEditor::open(saved, OfficeFamily::Spreadsheet).unwrap();
        assert_eq!(reopened.get_cell(0, 0, 0).unwrap().input, "42");
    }

    #[test]
    fn refuses_other_families_and_junk() {
        assert!(SheetEditor::open(CHAIN.to_vec(), OfficeFamily::Document).is_err());
        assert!(SheetEditor::open(b"not a spreadsheet".to_vec(), OfficeFamily::Spreadsheet).is_err());
    }

    /// Excel refuses (or "repairs") a package whose relationships or content types name a missing part,
    /// which is what a dropped calcChain.xml would leave behind if only the part went.
    fn assert_package_is_whole(bytes: &[u8]) {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: HashSet<String> = zip.file_names().map(|name| name.unwrap().into_owned()).collect();
        assert!(!names.contains("xl/calcChain.xml"), "an edited save drops the calc chain");
        let mut text = |name: &str| {
            let mut out = String::new();
            zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
            out
        };
        let content_types = text("[Content_Types].xml");
        for part in attr_values(&content_types, "PartName") {
            assert!(names.contains(part.trim_start_matches('/')), "content type for missing part {part}");
        }
        assert!(!content_types.contains("calcChain"));
        for rels in names.iter().filter(|n| n.ends_with(".rels")) {
            let xml = text(rels);
            let base = rels.replace("_rels/", "").trim_end_matches(".rels").rsplit_once('/').map_or(String::new(), |(dir, _)| format!("{dir}/"));
            for element in xml.split("<Relationship ").skip(1) {
                if element.contains("TargetMode=\"External\"") {
                    continue;
                }
                let target = attr_values(&format!(" {element}"), "Target").remove(0);
                let path = resolve(&base, &target);
                assert!(names.contains(&path), "{rels} points at missing part {path}");
            }
        }
    }

    fn attr_values(xml: &str, attr: &str) -> Vec<String> {
        let key = format!(" {attr}=\"");
        xml.match_indices(&key)
            .map(|(at, _)| {
                let rest = &xml[at + key.len()..];
                rest[..rest.find('"').unwrap()].to_owned()
            })
            .collect()
    }

    fn resolve(base: &str, target: &str) -> String {
        let joined = match target.strip_prefix('/') {
            Some(absolute) => absolute.to_owned(),
            None => format!("{base}{target}"),
        };
        let mut parts: Vec<&str> = Vec::new();
        for segment in joined.split('/') {
            match segment {
                ".." => drop(parts.pop()),
                "." | "" => {}
                s => parts.push(s),
            }
        }
        parts.join("/")
    }
}
