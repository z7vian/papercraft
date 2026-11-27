//! Spreadsheets as a cell grid: the cells, sizes, merges and links a spreadsheet screen draws.
//! (`office/xlsx.rs` still draws whole sheets as pictures for `OfficeDoc`.)
//!
//! Opening skips the editing workbook (`betteroffice_xlsx::Workbook`) and calls the engine's
//! parser directly: it inflates only the parts a grid reads, parses one sheet at a time, and reads
//! merged ranges in linear time. Pictures, charts, comments and formula recalculation are not here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use betteroffice_xlsx::GridGeometry;
use betteroffice_xlsx::fonts::{FontLoading, FontManager, FontSource, LastResortOffice};
use ooxml_text::{FontId, FontStore};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use xlsx_model::numfmt::{
    FormatPartKind, FormattedValue, builtin_format_code, fit_general, format_value, is_general,
};
use xlsx_model::{
    BorderEdge, BorderStyle, CellRange, CellRef, CellValue, DateSystem, Fill, FormatCode, HAlign, MAX_COLS, MAX_ROWS,
    RunUnderline, Sheet, Stylesheet, VAlign,
};

use crate::CoreError;
use crate::office::{OfficeFamily, convert};

/// Parsing recurses on nested content, as in `office/mod.rs`; Kotlin's calling threads have small stacks.
const ENGINE_STACK: usize = 32 * 1024 * 1024;

/// What an unparsed sheet's part holds while another sheet parses, so the engine skips it in no time.
const BLANK_WORKSHEET: &[u8] = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;

/// The engine's text inset from each side of a cell, in pixels at 96 dpi.
const TEXT_PAD_PX: f32 = 2.0;

/// The family a cell draws in when its font names none.
const DEFAULT_FAMILY: &str = "Calibri";

/// Parts a cell grid never reads (pictures, charts, pivot caches, macros, comments, metadata), left compressed.
const SKIPPED_PARTS: [&str; 24] = [
    "xl/media/",
    "xl/drawings/",
    "xl/charts/",
    "xl/embeddings/",
    "xl/pivotcache/",
    "xl/pivottables/",
    "xl/printersettings/",
    "xl/externallinks/",
    "xl/activex/",
    "xl/ctrlprops/",
    "xl/comments",
    "xl/threadedcomments/",
    "xl/persons/",
    "xl/richdata/",
    "xl/slicers/",
    "xl/slicercaches/",
    "xl/timelines/",
    "xl/timelinecaches/",
    "xl/model/",
    "xl/querytables/",
    "xl/diagrams/",
    "xl/calcchain.xml",
    "customxml/",
    "docprops/",
];

/// One sheet tab.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetTab {
    pub name: String,
    /// Hidden (or "very hidden") in the file; Excel lists it under "Unhide".
    pub hidden: bool,
}

/// A sheet's grid: how far it is used, its frozen panes and its default track sizes.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetLayout {
    /// Rows from row 1 to the last used one (cells, merges, links); 0 for an empty sheet.
    pub row_count: u32,
    /// Columns from column A to the last used one; 0 for an empty sheet.
    pub col_count: u32,
    /// Rows frozen at the top (0 when none).
    pub frozen_rows: u32,
    /// Columns frozen at the left (0 when none).
    pub frozen_cols: u32,
    /// Pixels at 96 dpi of a row that [`SheetDoc::row_heights`] does not list.
    pub default_row_height_px: f32,
    /// Pixels at 96 dpi of a column that [`SheetDoc::col_widths`] does not list.
    pub default_col_width_px: f32,
    pub show_gridlines: bool,
}

/// Tracks `first..=last` (0-based rows or columns) all `px` pixels at 96 dpi; hidden ones are 0.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct SheetSizeRun {
    pub first: u32,
    pub last: u32,
    pub px: f32,
}

/// A cell rectangle, 0-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SheetRange {
    pub first_row: u32,
    pub first_col: u32,
    pub last_row: u32,
    pub last_col: u32,
}

/// A link on a cell range: a web, mail or file address, a place in the workbook, or both.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetHyperlink {
    pub range: SheetRange,
    /// External target such as `https://…`, `mailto:…` or a file name.
    pub url: Option<String>,
    /// Place in this workbook, such as `Sheet1!A2`.
    pub location: Option<String>,
    pub tooltip: Option<String>,
}

/// The kind of value a cell stores (a formula cell reports its saved result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SheetValueKind {
    /// No value: a formatted blank cell, or a cell a merged range covers.
    Empty,
    Number,
    Text,
    Bool,
    Error,
}

/// Horizontal text alignment, with Excel's "General" already resolved by value kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SheetHAlign {
    Left,
    Center,
    Right,
    /// Repeat the text across the cell.
    Fill,
    Justify,
    /// Centre across the selection: over this cell and the empty cells to its right.
    CenterAcross,
    Distributed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SheetVAlign {
    Top,
    Center,
    Bottom,
    Justify,
    Distributed,
}

/// Excel's border line styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SheetLineStyle {
    Thin,
    Medium,
    Thick,
    Dashed,
    Dotted,
    Double,
    Hair,
    MediumDashed,
    DashDot,
    MediumDashDot,
    DashDotDot,
    MediumDashDotDot,
    SlantDashDot,
}

/// One side of a cell's border; `color` is ARGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SheetBorder {
    pub style: SheetLineStyle,
    pub color: u32,
}

/// How a cell draws. Colours are ARGB (`0xFFRRGGBB`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetCellStyle {
    pub font_name: Option<String>,
    pub font_size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// `None` is the automatic colour (black on white). A number format colour (`[Red]`) is applied.
    pub font_color: Option<u32>,
    /// `None` draws no fill. Pattern fills come as the average colour of their pattern.
    pub fill: Option<u32>,
    pub h_align: SheetHAlign,
    pub v_align: SheetVAlign,
    pub wrap: bool,
    pub border_left: Option<SheetBorder>,
    pub border_right: Option<SheetBorder>,
    pub border_top: Option<SheetBorder>,
    pub border_bottom: Option<SheetBorder>,
}

/// A cell with something to draw: text, a fill or a border.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetCell {
    /// 0-based row.
    pub row: u32,
    /// 0-based column.
    pub col: u32,
    /// The value as Excel displays it: number formats, dates, percentages and currency applied.
    /// `*` fill characters repeat across the cell, and a number too wide for its cell shows in fewer
    /// digits (General format) or as `###`, measured in the faces the engine draws the cell's font with.
    pub text: String,
    pub kind: SheetValueKind,
    pub style: SheetCellStyle,
}

/// A spreadsheet opened for a cell grid. Sheets parse on first use; every call is thread-safe.
#[derive(uniffi::Object)]
pub struct SheetDoc {
    tabs: Vec<SheetTab>,
    active: u32,
    book: Mutex<Book>,
}

#[uniffi::export]
impl SheetDoc {
    /// Opens .xlsx/.xlsm/.xltx/.xltm/.xlam, and .xls/.xlt/.xla/.et/.ett through the legacy converter.
    /// Parses the active sheet; the others parse when first asked for. `locale` is a BCP-47 tag such
    /// as `en-GB` that sets Excel's locale-dependent short date formats; `None` reads them as en-US.
    #[uniffi::constructor(default(locale = None))]
    pub fn open(bytes: Vec<u8>, family: OfficeFamily, locale: Option<String>) -> Result<Arc<Self>, CoreError> {
        if !matches!(family, OfficeFamily::Spreadsheet) {
            return Err(CoreError::BadFile("Only spreadsheets open as a cell grid.".into()));
        }
        let (mut book, entries, active) = on_engine_stack(move || Book::unpack(bytes))??;
        book.dates = short_date(locale.as_deref());
        let active = active.min(book.slots.len().saturating_sub(1));
        let names = book.parse(active)?;
        let listed = entries.len() == names.len();
        let tabs = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| SheetTab { hidden: listed && entries[index].hidden, name })
            .collect::<Vec<_>>();
        let active = active.min(tabs.len().saturating_sub(1)) as u32;
        Ok(Arc::new(Self { tabs, active, book: Mutex::new(book) }))
    }

    pub fn sheets(&self) -> Vec<SheetTab> {
        self.tabs.clone()
    }

    /// The sheet the file was saved on.
    pub fn active_sheet(&self) -> u32 {
        self.active
    }

    pub fn layout(&self, sheet: u32) -> Result<SheetLayout, CoreError> {
        self.with(sheet, |_, loaded| {
            let grid = &loaded.sheet;
            let used = grid.used_range();
            let frozen = grid.freeze_pane.as_ref();
            SheetLayout {
                row_count: used.map_or(0, |range| range.end.row + 1),
                col_count: used.map_or(0, |range| range.end.col + 1),
                frozen_rows: frozen.map_or(0, |pane| pane.rows),
                frozen_cols: frozen.map_or(0, |pane| pane.cols),
                default_row_height_px: loaded.default_row_px,
                default_col_width_px: loaded.default_col_px,
                show_gridlines: !grid.format.hide_gridlines,
            }
        })
    }

    /// Columns whose width differs from [`SheetLayout::default_col_width_px`], in order.
    pub fn col_widths(&self, sheet: u32) -> Result<Vec<SheetSizeRun>, CoreError> {
        self.with(sheet, |_, loaded| loaded.cols.clone())
    }

    /// Rows whose height differs from [`SheetLayout::default_row_height_px`], in order.
    pub fn row_heights(&self, sheet: u32) -> Result<Vec<SheetSizeRun>, CoreError> {
        self.with(sheet, |_, loaded| loaded.rows.clone())
    }

    /// The cells with something to draw inside the window, row by row. Cells a merged range covers
    /// keep their style but lose their value, as in Excel. Formatting applied to whole rows or
    /// columns shows on blank cells, so such a window costs time in proportion to its area.
    pub fn cells(
        &self,
        sheet: u32,
        first_row: u32,
        row_count: u32,
        first_col: u32,
        col_count: u32,
    ) -> Result<Vec<SheetCell>, CoreError> {
        self.with(sheet, |book, loaded| {
            let Some(window) = window(first_row, row_count, first_col, col_count) else {
                return Vec::new();
            };
            Styler::new(book).window(loaded, window)
        })
    }

    pub fn merges(&self, sheet: u32) -> Result<Vec<SheetRange>, CoreError> {
        self.with(sheet, |_, loaded| loaded.sheet.merges.iter().map(|&range| range.into()).collect())
    }

    pub fn hyperlinks(&self, sheet: u32) -> Result<Vec<SheetHyperlink>, CoreError> {
        self.with(sheet, |_, loaded| {
            loaded
                .sheet
                .hyperlinks
                .iter()
                .map(|link| SheetHyperlink {
                    range: link.range.into(),
                    url: link.external_target.clone(),
                    location: link.location.clone(),
                    tooltip: link.tooltip.clone(),
                })
                .collect()
        })
    }

    /// Every displayed value, cells split by tabs and rows by newlines, for search.
    pub fn sheet_text(&self, sheet: u32) -> Result<String, CoreError> {
        self.with(sheet, |book, loaded| {
            let mut text = String::new();
            let mut last_row = None;
            for (at, cell) in loaded.sheet.iter_cells() {
                let shown = book.display(cell.style.unwrap_or(0), &cell.value).text;
                if shown.is_empty() {
                    continue;
                }
                match last_row {
                    Some(row) if row == at.row => text.push('\t'),
                    Some(_) => text.push('\n'),
                    None => {}
                }
                last_row = Some(at.row);
                text.push_str(&shown);
            }
            text
        })
    }
}

impl SheetDoc {
    fn with<T>(&self, sheet: u32, read: impl FnOnce(&Book, &Loaded) -> T) -> Result<T, CoreError> {
        let mut book = self.book.lock().map_err(|_| stopped())?;
        let index = sheet as usize;
        if index >= book.slots.len() {
            return Err(CoreError::BadFile(format!("This workbook has no sheet {}.", sheet + 1)));
        }
        if book.slots[index].loaded.is_none() {
            book.parse(index)?;
        }
        let loaded = book.slots[index].loaded.as_ref().ok_or_else(stopped)?;
        Ok(read(&book, loaded))
    }
}

/// The unpacked file: its parts, one slot per sheet, and what its sheets share.
struct Book {
    parts: Vec<(String, Vec<u8>)>,
    slots: Vec<Slot>,
    /// The shared strings, styles and date system the first parse reads. Their parts leave
    /// [`Book::parts`] then, so later sheets parse without reading them again.
    strings: Vec<String>,
    styles: Stylesheet,
    date_system: DateSystem,
    workbook_read: bool,
    dates: ShortDate,
    /// Loaded when a cell is first measured.
    fonts: RefCell<Option<Fonts>>,
    /// Parses the shared strings part was given to.
    #[cfg(test)]
    string_reads: usize,
}

struct Slot {
    /// The sheet's worksheet part in [`Book::parts`], when the workbook names it unambiguously.
    /// Such a part holds [`BLANK_WORKSHEET`] while `xml` keeps the real one until the sheet parses.
    part: Option<usize>,
    xml: Option<Vec<u8>>,
    loaded: Option<Loaded>,
}

struct Loaded {
    sheet: Sheet,
    cols: Vec<SheetSizeRun>,
    rows: Vec<SheetSizeRun>,
    default_col_px: f32,
    default_row_px: f32,
}

/// A `<sheet>` entry of the workbook part.
struct Entry {
    hidden: bool,
    rid: Option<String>,
}

struct Relationship {
    target: String,
    kind: Option<String>,
    external: bool,
}

impl Book {
    /// Inflates the parts a grid reads and sets each worksheet aside. Returns the workbook part's
    /// sheet entries and its active tab.
    fn unpack(bytes: Vec<u8>) -> Result<(Self, Vec<Entry>, usize), CoreError> {
        let bytes = if bytes.starts_with(b"PK") { bytes } else { convert::to_ooxml(bytes, OfficeFamily::Spreadsheet)? };
        let mut parts = ooxml_opc::unzip_parts_where(&bytes, u64::MAX, keep_part).map_err(bad)?;
        drop(bytes);
        let (entries, active) = find(&parts, "xl/workbook.xml").and_then(workbook_entries).unwrap_or_default();
        let rels = find(&parts, "xl/_rels/workbook.xml.rels").and_then(relationships).unwrap_or_default();
        // Mirrors how the engine's parser finds each sheet's part.
        let mut targets: Vec<Option<usize>> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let rel = entry.rid.as_deref().and_then(|rid| rels.get(rid));
                if rel.is_some_and(|rel| !rel.is_worksheet()) {
                    return None;
                }
                let path = rel
                    .filter(|rel| !rel.external)
                    .map(|rel| resolve_part_path("xl", &rel.target))
                    .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", index + 1));
                position(&parts, &path)
            })
            .collect();
        // A part two sheets share can't be set aside for one of them.
        for index in 0..targets.len() {
            if let Some(part) = targets[index]
                && targets.iter().filter(|&&other| other == Some(part)).count() > 1
            {
                targets[index] = None;
            }
        }
        let slots = targets
            .into_iter()
            .map(|part| Slot {
                part,
                xml: part.map(|part| std::mem::replace(&mut parts[part].1, BLANK_WORKSHEET.to_vec())),
                loaded: None,
            })
            .collect();
        let book = Self {
            parts,
            slots,
            strings: Vec::new(),
            styles: Stylesheet::default(),
            date_system: DateSystem::default(),
            workbook_read: false,
            dates: short_date(None),
            fonts: RefCell::new(None),
            #[cfg(test)]
            string_reads: 0,
        };
        Ok((book, entries, active))
    }

    /// Parses sheet `index` (and any sheet that could not be set aside) and returns every sheet's name.
    fn parse(&mut self, index: usize) -> Result<Vec<String>, CoreError> {
        let part = self.slots.get(index).and_then(|slot| slot.part);
        let mut own_merges = None;
        if let Some(part) = part {
            let xml = self.slots[index].xml.take().ok_or_else(stopped)?;
            let split = split_merges(&xml);
            let body = match split {
                Ok(Some((body, merges))) => {
                    own_merges = Some(merges);
                    self.slots[index].xml = Some(xml);
                    body
                }
                Ok(None) => xml,
                Err(e) => {
                    self.slots[index].xml = Some(xml);
                    return Err(e);
                }
            };
            self.parts[part].1 = body;
        }
        #[cfg(test)]
        {
            self.string_reads += usize::from(position(&self.parts, "xl/sharedStrings.xml").is_some());
        }
        let parts = &self.parts;
        let parsed = on_engine_stack(|| xlsx_parse::parse_workbook(parts));
        if let Some(part) = part {
            let body = std::mem::replace(&mut self.parts[part].1, BLANK_WORKSHEET.to_vec());
            if self.slots[index].xml.is_none() {
                self.slots[index].xml = Some(body);
            }
        }
        let mut model = parsed?.map_err(bad)?;
        if model.sheets.len() != self.slots.len() {
            if !self.slots.is_empty() {
                // The workbook part read differently here than in the engine: put every sheet back
                // and let the engine's reading decide, without setting any sheet aside.
                for slot in &mut self.slots {
                    if let (Some(part), Some(xml)) = (slot.part, slot.xml.take()) {
                        self.parts[part].1 = xml;
                    }
                }
                self.slots.clear();
                return self.parse(index);
            }
            self.slots = model.sheets.iter().map(|_| Slot { part: None, xml: None, loaded: None }).collect();
        }
        // The sheet's own part, read again for its shared strings when an earlier parse read them.
        let xml = self.slots.get_mut(index).and_then(|slot| slot.xml.take());
        let first = !self.workbook_read;
        if first {
            self.strings = std::mem::take(&mut model.shared_strings);
            self.styles = std::mem::take(&mut model.styles);
            self.date_system = model.date_system;
            self.drop_workbook_parts();
            self.workbook_read = true;
        }
        let names = model.sheets.iter().map(|sheet| sheet.name.clone()).collect();
        for (at, mut sheet) in model.sheets.into_iter().enumerate() {
            let slot = &mut self.slots[at];
            if slot.loaded.is_some() || (at != index && slot.part.is_some()) {
                continue;
            }
            if at == index {
                if let Some(mut merges) = own_merges.take() {
                    merges.append(&mut sheet.merges);
                    normalize_merges(&mut merges);
                    sheet.merges = merges;
                }
                if !first && let Some(xml) = &xml {
                    add_shared_strings(&mut sheet, xml, &self.strings)?;
                }
            }
            slot.loaded = Some(Loaded::measure(sheet, &self.styles));
        }
        Ok(names)
    }

    /// Empties the shared strings, styles and theme parts, found as the engine finds them.
    fn drop_workbook_parts(&mut self) {
        let rels = find(&self.parts, "xl/_rels/workbook.xml.rels");
        let paths = [("sharedStrings", "xl/sharedStrings.xml"), ("styles", "xl/styles.xml"), ("theme", "xl/theme/theme1.xml")]
            .map(|(kind, fallback)| {
                rels.and_then(|rels| typed_target(rels, kind))
                    .map_or_else(|| fallback.to_string(), |target| resolve_part_path("xl", &target))
            });
        for path in paths {
            if let Some(at) = position(&self.parts, &path)
                && !self.slots.iter().any(|slot| slot.part == Some(at))
            {
                // The engine looks parts up by name and never finds a nameless one.
                self.parts[at] = (String::new(), Vec::new());
            }
        }
    }

    /// The number format code of cell format `xf`, built-ins 14 and 22 in the caller's locale.
    fn format_code(&self, xf: u32) -> &str {
        match self.styles.format_code_for(xf) {
            FormatCode::Custom(code) => code,
            FormatCode::Builtin(14) => self.dates.date,
            FormatCode::Builtin(22) => self.dates.date_time,
            FormatCode::Builtin(id) => builtin_format_code(id).unwrap_or("General"),
        }
    }

    /// The value as Excel shows it in the cell's number format, before it is fitted to the cell.
    fn display(&self, xf: u32, value: &CellValue) -> FormattedValue {
        if matches!(value, CellValue::Empty) {
            return Default::default();
        }
        format_value(value, self.format_code(xf), self.date_system)
    }
}

impl Relationship {
    fn is_worksheet(&self) -> bool {
        self.kind.as_deref().and_then(|kind| kind.rsplit('/').next()).is_none_or(|kind| kind == "worksheet")
    }
}

impl Loaded {
    /// Track sizes at 96 dpi. Columns follow the engine's screen geometry (Excel's width formula in
    /// the normal font's digit width). Rows keep their saved height, as Excel for Windows shows it,
    /// and rows without one take the engine's fitted height when their content is taller than the default.
    fn measure(sheet: Sheet, styles: &Stylesheet) -> Self {
        let geometry = GridGeometry::new(&sheet, styles);
        let used = sheet.used_range();

        let default_row_px =
            if sheet.format.zero_height { 0.0 } else { pt_to_px(sheet.format.default_row_height_pt.unwrap_or(15.0)) };
        let row_end = [
            used.map(|range| range.end.row),
            sheet.row_heights.keys().next_back().copied(),
            sheet.row_styles.keys().next_back().copied(),
        ]
        .into_iter()
        .flatten()
        .max()
        .map_or(0, |row| row + 1);
        let mut rows = Vec::new();
        let mut top = geometry.row_y(0);
        // Limit: row offsets past about 8 million pixels lose sub-pixel precision in the engine's f32 geometry; f64 offsets in the engine lift it.
        for row in 0..row_end {
            let bottom = geometry.row_y(row + 1);
            let px = match sheet.row_heights.get(&row) {
                Some(&pt) => pt_to_px(pt),
                None => round_px(f64::from(bottom - top)).max(default_row_px),
            };
            top = bottom;
            push_run(&mut rows, row, px, default_row_px);
        }

        let col_px = |col: u32| round_px(f64::from(geometry.col_x(col + 1) - geometry.col_x(col)));
        let default_col_px = (0..MAX_COLS).find(|col| !sheet.col_widths.contains_key(col)).map_or(col_px(0), col_px);
        let col_end = [used.map(|range| range.end.col), sheet.col_widths.keys().next_back().copied()]
            .into_iter()
            .flatten()
            .max()
            .map_or(0, |col| col + 1);
        let mut cols = Vec::new();
        for col in 0..col_end {
            push_run(&mut cols, col, col_px(col), default_col_px);
        }
        Self { sheet, cols, rows, default_col_px, default_row_px }
    }

    /// Pixels at 96 dpi of columns `first..=last`.
    fn span_px(&self, first: u32, last: u32) -> f32 {
        let start = self.cols.partition_point(|run| run.last < first);
        let mut px = (last - first + 1) as f32 * self.default_col_px;
        for run in self.cols[start..].iter().take_while(|run| run.first <= last) {
            let overlap = run.last.min(last) - run.first.max(first) + 1;
            px += overlap as f32 * (run.px - self.default_col_px);
        }
        px
    }
}

fn push_run(runs: &mut Vec<SheetSizeRun>, index: u32, px: f32, default: f32) {
    if px == default {
        return;
    }
    match runs.last_mut() {
        Some(run) if run.last + 1 == index && run.px == px => run.last = index,
        _ => runs.push(SheetSizeRun { first: index, last: index, px }),
    }
}

fn pt_to_px(pt: f64) -> f32 {
    round_px(pt * 96.0 / 72.0)
}

/// Pixels to a hundredth, so float noise doesn't split runs of equal tracks.
fn round_px(px: f64) -> f32 {
    ((px * 100.0).round() / 100.0) as f32
}

/// The window as an inclusive range clamped to the sheet, `None` when empty.
fn window(first_row: u32, row_count: u32, first_col: u32, col_count: u32) -> Option<CellRange> {
    if row_count == 0 || col_count == 0 || first_row >= MAX_ROWS || first_col >= MAX_COLS {
        return None;
    }
    let last_row = first_row.saturating_add(row_count - 1).min(MAX_ROWS - 1);
    let last_col = first_col.saturating_add(col_count - 1).min(MAX_COLS - 1);
    Some(CellRange::new(CellRef::new(first_row, first_col), CellRef::new(last_row, last_col)))
}

impl From<CellRange> for SheetRange {
    fn from(range: CellRange) -> Self {
        Self { first_row: range.start.row, first_col: range.start.col, last_row: range.end.row, last_col: range.end.col }
    }
}

/// Turns cells into [`SheetCell`]s, resolving each cell format once per call.
struct Styler<'a> {
    book: &'a Book,
    formats: HashMap<u32, Format>,
    /// One indent level in pixels, measured on first use.
    indent_px: Option<f32>,
}

/// A cell format resolved for drawing.
struct Format {
    style: SheetCellStyle,
    h: Option<HAlign>,
    indent: u32,
    /// Draws something on a blank cell: a fill or a border.
    visible: bool,
    /// The font's faces in [`Fonts`], resolved on first measurement.
    font: Option<usize>,
}

impl<'a> Styler<'a> {
    fn new(book: &'a Book) -> Self {
        Self { book, formats: HashMap::new(), indent_px: None }
    }

    fn format(&mut self, xf: u32) -> &mut Format {
        let styles = &self.book.styles;
        self.formats.entry(xf).or_insert_with(|| resolve_format(styles, xf))
    }

    fn window(&mut self, loaded: &Loaded, window: CellRange) -> Vec<SheetCell> {
        let sheet = &loaded.sheet;
        let merges: Vec<CellRange> = sheet.merges.iter().copied().filter(|&range| intersects(range, window)).collect();
        let covered = |at: CellRef| merges.iter().any(|range| range.contains(at) && range.start != at);
        let mut out = Vec::new();
        for (at, cell) in sheet.cells_in_range(window) {
            let xf = cell.style.unwrap_or(0);
            let value = if covered(at) { &CellValue::Empty } else { &cell.value };
            let shown = self.book.display(xf, value);
            if shown.text.is_empty() && !self.format(xf).visible {
                continue;
            }
            let color = shown.color.as_deref().and_then(argb);
            let fits = !shown.text.is_empty()
                && (matches!(value, CellValue::Number { .. })
                    || shown.layout.iter().any(|part| part.kind == FormatPartKind::Fill));
            let text = if fits {
                let last = merges.iter().find(|range| range.start == at).map_or(at.col, |range| range.end.col);
                let width = self.text_width(xf, loaded.span_px(at.col, last));
                self.fit(xf, value, shown, width)
            } else {
                shown.text
            };
            let format = self.format(xf);
            let mut style = format.style.clone();
            style.h_align = h_align(format.h, value);
            if let Some(color) = color {
                style.font_color = Some(color);
            }
            out.push(SheetCell { row: at.row, col: at.col, text, kind: kind(value), style });
        }
        if !sheet.row_styles.is_empty() || !sheet.col_styles.is_empty() {
            let before = out.len();
            self.formatted_blanks(sheet, window, &mut out);
            if out.len() > before {
                out.sort_unstable_by_key(|cell| (cell.row, cell.col));
            }
        }
        out
    }

    /// Blank cells that take a fill or border from their row's or column's formatting.
    fn formatted_blanks(&mut self, sheet: &Sheet, window: CellRange, out: &mut Vec<SheetCell>) {
        let cols = window.start.col..=window.end.col;
        let col_styles: Vec<Option<u32>> = cols.clone().map(|col| sheet.col_style(col)).collect();
        let any_col = col_styles.iter().flatten().any(|&xf| self.format(xf).visible);
        for row in window.start.row..=window.end.row {
            let row_style = sheet.row_styles.get(&row).copied();
            if !any_col && !row_style.is_some_and(|xf| self.format(xf).visible) {
                continue;
            }
            for (col, col_style) in cols.clone().zip(&col_styles) {
                let at = CellRef::new(row, col);
                if sheet.cell(at).is_some() {
                    continue;
                }
                let Some(xf) = row_style.or(*col_style) else { continue };
                let format = self.format(xf);
                if format.visible {
                    let mut style = format.style.clone();
                    style.h_align = h_align(format.h, &CellValue::Empty);
                    out.push(SheetCell { row, col, text: String::new(), kind: SheetValueKind::Empty, style });
                }
            }
        }
    }

    /// Pixels the engine lays text out in for a cell `cell_px` wide in format `xf`: the cell less
    /// its padding and a left or right indent.
    fn text_width(&mut self, xf: u32, cell_px: f32) -> f32 {
        let format = self.format(xf);
        let indent = match format.h {
            Some(HAlign::Left | HAlign::Right) if format.indent > 0 => format.indent as f32,
            _ => 0.0,
        };
        let indent = if indent > 0.0 { indent * self.indent_px() } else { 0.0 };
        (cell_px - 2.0 * TEXT_PAD_PX - indent).max(0.0)
    }

    /// One indent level: three spaces of the normal font, each on whole pixels, as the engine lays it out.
    fn indent_px(&mut self) -> f32 {
        if let Some(px) = self.indent_px {
            return px;
        }
        let book = self.book;
        let normal = book.styles.font_for(0).or_else(|| book.styles.fonts.first());
        let family = normal.and_then(|font| font.name.as_deref()).unwrap_or(DEFAULT_FAMILY);
        let size = normal.and_then(|font| font.size_pt).filter(|pt| pt.is_finite() && *pt > 0.0).unwrap_or(11.0) as f32;
        let mut fonts = book.fonts.borrow_mut();
        let fonts = fonts.get_or_insert_with(Fonts::new);
        let chain = fonts.chain(family, false, false);
        let px = 3.0 * fonts.width(chain, size, " ").round();
        self.indent_px = Some(px);
        px
    }

    /// The faces format `xf`'s font draws with.
    fn font(&mut self, xf: u32) -> usize {
        let book = self.book;
        let fonts = &book.fonts;
        let format = self.format(xf);
        if let Some(font) = format.font {
            return font;
        }
        let style = &format.style;
        let font = fonts.borrow_mut().get_or_insert_with(Fonts::new).chain(
            style.font_name.as_deref().unwrap_or(DEFAULT_FAMILY),
            style.bold,
            style.italic,
        );
        format.font = Some(font);
        font
    }

    /// The text as the engine draws it `width` pixels wide: a number too wide in fewer digits
    /// (General format) or as `#` repeated, else the `*` fill character repeated across the space left.
    fn fit(&mut self, xf: u32, value: &CellValue, shown: FormattedValue, width: f32) -> String {
        let font = self.font(xf);
        let size = self.format(xf).style.font_size_pt;
        let book = self.book;
        let mut fonts = book.fonts.borrow_mut();
        let fonts = &*fonts.get_or_insert_with(Fonts::new);
        let mut measure = |text: &str| fonts.width(font, size, text);
        let FormattedValue { mut text, layout, .. } = shown;
        let mut fixed = 0.0;
        let mut cursor = 0;
        let mut fill = None;
        for part in &layout {
            fixed += measure(&text[cursor..part.offset]);
            if part.kind == FormatPartKind::Padding {
                fixed += measure(part.character.encode_utf8(&mut [0; 4]));
                cursor = part.offset + 1;
            } else {
                fill = Some(*part);
                cursor = part.offset;
            }
        }
        fixed += measure(&text[cursor..]);
        if let CellValue::Number { value: number } = value
            && fixed > width
        {
            let general = is_general(value, book.format_code(xf));
            return general.then(|| fit_general(*number, width, &mut measure)).flatten().unwrap_or_else(|| {
                let advance = measure("#");
                let count =
                    if advance > 0.0 && width > 0.0 { (width / advance).floor().max(1.0) as usize } else { 0 };
                "#".repeat(count)
            });
        }
        if let Some(part) = fill {
            let character = part.character.encode_utf8(&mut [0; 4]).to_owned();
            let advance = measure(&character);
            let extra = (width - fixed).max(0.0);
            if advance > 0.0 && extra >= advance {
                text.insert_str(part.offset, &character.repeat((extra / advance).floor() as usize));
            }
        }
        text
    }
}

/// Text widths in the bundled faces the engine draws a font with, as its raster backend measures them.
struct Fonts {
    /// `None` when the bundled faces can't load: widths then take Calibri's advances, as the engine
    /// measures without a face.
    manager: Option<FontManager>,
    store: FontStore,
    /// Registered faces by bundled family and style.
    faces: HashMap<(String, bool, bool), FontId>,
    /// Index in `chains` by requested family and style.
    keys: HashMap<(String, bool, bool), usize>,
    /// Faces a font draws with, first face first.
    chains: Vec<Vec<FontId>>,
}

impl Fonts {
    fn new() -> Self {
        Self {
            manager: FontManager::new(&FontSource::default(), FontLoading::OnDemand).ok(),
            store: FontStore::new(),
            faces: HashMap::new(),
            keys: HashMap::new(),
            chains: Vec::new(),
        }
    }

    /// The faces `family` draws with, as an index for [`Fonts::width`].
    fn chain(&mut self, family: &str, bold: bool, italic: bool) -> usize {
        let family = if family.trim().is_empty() { DEFAULT_FAMILY } else { family.trim() };
        let key = (family.to_lowercase(), bold, italic);
        if let Some(&chain) = self.keys.get(&key) {
            return chain;
        }
        let mut chain = Vec::new();
        if let Some(manager) = &self.manager {
            for face in manager.resolve(family, bold, italic, &[], LastResortOffice::Word) {
                let face_key = (face.family.clone(), face.bold, face.italic);
                let id = match self.faces.get(&face_key) {
                    Some(&id) => id,
                    None => {
                        let Some(id) = face.load().ok().and_then(|bytes| self.store.register(bytes).ok()) else {
                            continue;
                        };
                        self.faces.insert(face_key, id);
                        id
                    }
                };
                if !chain.contains(&id) {
                    chain.push(id);
                }
            }
        }
        self.chains.push(chain);
        self.keys.insert(key, self.chains.len() - 1);
        self.chains.len() - 1
    }

    /// Pixels at 96 dpi `text` takes at `size_pt`, each run in the first face of `chain` that has its characters.
    fn width(&self, chain: usize, size_pt: f32, text: &str) -> f32 {
        let chain = &self.chains[chain];
        if chain.is_empty() {
            return xlsx_render::wrap::estimate_text_width(text, f64::from(size_pt));
        }
        let px = size_pt * 96.0 / 72.0;
        let face = |ch: char| {
            chain
                .iter()
                .position(|&id| {
                    ch.is_control()
                        || ch.is_whitespace()
                        || matches!(ch as u32, 0x200b..=0x200f | 0xfe00..=0xfe0f)
                        || self.store.glyph_id(id, ch).ok().flatten().is_some()
                })
                .unwrap_or(0)
        };
        let run = |id: FontId, text: &str| {
            ooxml_text::shape(&self.store, id, text, px, &[])
                .map_or(0.0, |glyphs| glyphs.iter().map(|glyph| glyph.x_advance).sum::<f32>())
        };
        let mut total = 0.0;
        let mut start = 0;
        let mut current = None;
        for (offset, ch) in text.char_indices() {
            let index = face(ch);
            if let Some(previous) = current
                && previous != index
            {
                total += run(chain[previous], &text[start..offset]);
                start = offset;
            }
            current = Some(index);
        }
        if let Some(index) = current {
            total += run(chain[index], &text[start..]);
        }
        total
    }
}

fn resolve_format(styles: &Stylesheet, xf: u32) -> Format {
    let format = styles.resolved_format(Some(xf));
    let normal = styles.font_for(0).or_else(|| styles.fonts.first());
    let font = format.font;
    let alignment = format.alignment;
    let fill = fill_color(styles, format.fill);
    let border = format.border;
    let edge = |edge: &Option<BorderEdge>| edge.as_ref().map(|edge| border_side(styles, edge));
    let style = SheetCellStyle {
        font_name: font.name.clone().or_else(|| normal.and_then(|font| font.name.clone())),
        font_size_pt: font
            .size_pt
            .or_else(|| normal.and_then(|font| font.size_pt))
            .filter(|pt| pt.is_finite() && *pt > 0.0)
            .unwrap_or(11.0) as f32,
        bold: font.bold,
        italic: font.italic,
        underline: font.underline != RunUnderline::None,
        strike: font.strike,
        font_color: font.color.as_ref().and_then(|color| styles.resolve_color(color)).as_deref().and_then(argb),
        fill,
        h_align: SheetHAlign::Left,
        v_align: match alignment.v {
            Some(VAlign::Top) => SheetVAlign::Top,
            Some(VAlign::Center) => SheetVAlign::Center,
            Some(VAlign::Justify) => SheetVAlign::Justify,
            Some(VAlign::Distributed) => SheetVAlign::Distributed,
            Some(VAlign::Bottom) | None => SheetVAlign::Bottom,
        },
        wrap: alignment.wrap_text,
        border_left: edge(&border.left),
        border_right: edge(&border.right),
        border_top: edge(&border.top),
        border_bottom: edge(&border.bottom),
    };
    let visible = style.fill.is_some()
        || style.border_left.is_some()
        || style.border_right.is_some()
        || style.border_top.is_some()
        || style.border_bottom.is_some();
    Format { style, h: alignment.h, indent: alignment.indent.unwrap_or(0), visible, font: None }
}

/// Excel's locale-dependent built-in number formats: 14 (short date) and 22 (short date and time).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ShortDate {
    date: &'static str,
    date_time: &'static str,
}

impl ShortDate {
    const fn new(date: &'static str, date_time: &'static str) -> Self {
        Self { date, date_time }
    }
}

/// The short date Windows sets for the BCP-47 `locale` (en-US when `None`), which Excel uses for
/// built-in formats 14 and 22.
// Limit: a table of common locales; other languages take day/month/year with slashes (month first in the
// United States and its territories). Passing the device's own short date pattern lifts it.
fn short_date(locale: Option<&str>) -> ShortDate {
    const US: ShortDate = ShortDate::new("m/d/yyyy", "m/d/yyyy h:mm");
    const DMY_SLASH: ShortDate = ShortDate::new("dd/mm/yyyy", "dd/mm/yyyy hh:mm");
    const DMY_DOT: ShortDate = ShortDate::new("dd.mm.yyyy", "dd.mm.yyyy hh:mm");
    const DMY_DASH: ShortDate = ShortDate::new("dd-mm-yyyy", "dd-mm-yyyy hh:mm");
    const YMD_DASH: ShortDate = ShortDate::new("yyyy-mm-dd", "yyyy-mm-dd hh:mm");
    const YMD_SLASH: ShortDate = ShortDate::new("yyyy/mm/dd", "yyyy/mm/dd h:mm");

    let Some(tag) = locale.map(str::trim).filter(|tag| !tag.is_empty()) else {
        return US;
    };
    let mut subtags = tag.split(['-', '_']);
    let language = subtags.next().unwrap_or_default().to_ascii_lowercase();
    // The region: the first two-letter or three-digit subtag, before any extension.
    let region = subtags
        .take_while(|subtag| subtag.len() > 1)
        .find(|subtag| {
            (subtag.len() == 2 && subtag.chars().all(|ch| ch.is_ascii_alphabetic()))
                || (subtag.len() == 3 && subtag.chars().all(|ch| ch.is_ascii_digit()))
        })
        .map(str::to_ascii_uppercase);
    match (language.as_str(), region.as_deref()) {
        (_, Some("US" | "PH" | "PR" | "AS" | "GU" | "MP" | "UM" | "VI" | "FM" | "MH" | "PW")) | ("en", None) => US,
        ("en", Some("AU" | "NZ")) | ("nl", Some("BE")) => ShortDate::new("d/mm/yyyy", "d/mm/yyyy h:mm"),
        ("en" | "fr", Some("CA")) | ("sv" | "lt", _) => YMD_DASH,
        ("en", Some("ZA")) | ("ja", _) => YMD_SLASH,
        ("en", Some("IN")) | ("hi" | "da", _) => DMY_DASH,
        ("de" | "ru" | "pl" | "tr" | "uk" | "nb" | "no" | "nn" | "ro" | "cs" | "sk" | "be" | "kk" | "az" | "et" | "lv", _)
        | (_, Some("CH")) => DMY_DOT,
        ("fi", _) => ShortDate::new("d.m.yyyy", "d.m.yyyy h:mm"),
        ("nl", _) => ShortDate::new("d-m-yyyy", "d-m-yyyy hh:mm"),
        ("hu", _) => ShortDate::new("yyyy.mm.dd", "yyyy.mm.dd h:mm"),
        ("zh", _) => ShortDate::new("yyyy/m/d", "yyyy/m/d h:mm"),
        ("ko", _) => ShortDate::new("yyyy-mm-dd", "yyyy-mm-dd h:mm"),
        _ => DMY_SLASH,
    }
}

/// Excel's "General" alignment: numbers right, logical values and errors centred, text left.
fn h_align(h: Option<HAlign>, value: &CellValue) -> SheetHAlign {
    match h {
        Some(HAlign::Left) => SheetHAlign::Left,
        Some(HAlign::Center) => SheetHAlign::Center,
        Some(HAlign::Right) => SheetHAlign::Right,
        Some(HAlign::Fill) => SheetHAlign::Fill,
        Some(HAlign::Justify) => SheetHAlign::Justify,
        Some(HAlign::CenterContinuous) => SheetHAlign::CenterAcross,
        Some(HAlign::Distributed) => SheetHAlign::Distributed,
        Some(HAlign::General) | None => match value {
            CellValue::Number { .. } => SheetHAlign::Right,
            CellValue::Bool { .. } | CellValue::Error { .. } => SheetHAlign::Center,
            CellValue::Text { .. } | CellValue::Empty => SheetHAlign::Left,
        },
    }
}

fn kind(value: &CellValue) -> SheetValueKind {
    match value {
        CellValue::Empty => SheetValueKind::Empty,
        CellValue::Number { .. } => SheetValueKind::Number,
        CellValue::Text { .. } => SheetValueKind::Text,
        CellValue::Bool { .. } => SheetValueKind::Bool,
        CellValue::Error { .. } => SheetValueKind::Error,
    }
}

fn fill_color(styles: &Stylesheet, fill: &Fill) -> Option<u32> {
    let color = |color: &Option<xlsx_model::Color>| {
        color.as_ref().and_then(|color| styles.resolve_color(color)).as_deref().and_then(argb)
    };
    match fill {
        Fill::None => None,
        Fill::Solid(solid) => styles.resolve_color(solid).as_deref().and_then(argb),
        Fill::Pattern { pattern, fg, bg } => {
            let cover = pattern.mask().iter().map(|row| row.count_ones()).sum::<u32>() as f32 / 64.0;
            Some(blend(color(fg).unwrap_or(0xFF00_0000), color(bg).unwrap_or(0xFFFF_FFFF), cover))
        }
    }
}

fn blend(fg: u32, bg: u32, cover: f32) -> u32 {
    let channel = |shift: u32| {
        let (f, b) = (((fg >> shift) & 0xFF) as f32, ((bg >> shift) & 0xFF) as f32);
        ((f * cover + b * (1.0 - cover)).round() as u32) << shift
    };
    0xFF00_0000 | channel(16) | channel(8) | channel(0)
}

fn border_side(styles: &Stylesheet, edge: &BorderEdge) -> SheetBorder {
    SheetBorder {
        style: match edge.style {
            BorderStyle::Thin => SheetLineStyle::Thin,
            BorderStyle::Medium => SheetLineStyle::Medium,
            BorderStyle::Thick => SheetLineStyle::Thick,
            BorderStyle::Dashed => SheetLineStyle::Dashed,
            BorderStyle::Dotted => SheetLineStyle::Dotted,
            BorderStyle::Double => SheetLineStyle::Double,
            BorderStyle::Hair => SheetLineStyle::Hair,
            BorderStyle::MediumDashed => SheetLineStyle::MediumDashed,
            BorderStyle::DashDot => SheetLineStyle::DashDot,
            BorderStyle::MediumDashDot => SheetLineStyle::MediumDashDot,
            BorderStyle::DashDotDot => SheetLineStyle::DashDotDot,
            BorderStyle::MediumDashDotDot => SheetLineStyle::MediumDashDotDot,
            BorderStyle::SlantDashDot => SheetLineStyle::SlantDashDot,
        },
        color: edge
            .color
            .as_ref()
            .and_then(|color| styles.resolve_color(color))
            .as_deref()
            .and_then(argb)
            .unwrap_or(0xFF00_0000),
    }
}

/// `#rrggbb` (or `#aarrggbb`) as opaque ARGB.
fn argb(hex: &str) -> Option<u32> {
    let hex = hex.trim_start_matches('#');
    let rgb = hex.get(hex.len().checked_sub(6)?..)?;
    u32::from_str_radix(rgb, 16).ok().map(|rgb| 0xFF00_0000 | rgb)
}

fn intersects(a: CellRange, b: CellRange) -> bool {
    a.start.row <= b.end.row && a.end.row >= b.start.row && a.start.col <= b.end.col && a.end.col >= b.start.col
}

/// The engine's rule (an overlapping range loses to the earlier one), checked in O(n log n) when
/// nothing overlaps, which is every file Excel writes. The engine checks every pair, which took
/// seconds on a sheet with 50,000 merged ranges.
fn normalize_merges(merges: &mut Vec<CellRange>) {
    let mut order: Vec<&CellRange> = merges.iter().collect();
    order.sort_unstable_by_key(|range| (range.start.row, range.start.col));
    let mut open: Vec<&CellRange> = Vec::new();
    let mut overlap = false;
    for range in order {
        open.retain(|other| other.end.row >= range.start.row);
        if open.iter().any(|other| other.start.col <= range.end.col && other.end.col >= range.start.col) {
            overlap = true;
            break;
        }
        open.push(range);
    }
    if overlap {
        let mut kept: Vec<CellRange> = Vec::with_capacity(merges.len());
        for range in merges.drain(..) {
            if !kept.iter().any(|&other| intersects(other, range)) {
                kept.push(range);
            }
        }
        *merges = kept;
    }
}

/// The worksheet without its `<mergeCells>` elements, and the ranges they held; `None` when it has none.
fn split_merges(xml: &[u8]) -> Result<Option<(Vec<u8>, Vec<CellRange>)>, CoreError> {
    const NAME: &[u8] = b"mergeCells";
    let mut sections = Vec::new();
    let mut from = 0;
    while let Some(found) = find_bytes(&xml[from..], NAME).map(|at| at + from) {
        from = found + NAME.len();
        // An opening tag: `<mergeCells` or `<prefix:mergeCells`, followed by a space, `>` or `/`.
        let mut start = found;
        if start > 0 && xml[start - 1] == b':' {
            start -= 1;
            while start > 0 && (xml[start - 1].is_ascii_alphanumeric() || matches!(xml[start - 1], b'_' | b'-' | b'.')) {
                start -= 1;
            }
        }
        if start == 0
            || xml[start - 1] != b'<'
            || !xml.get(from).is_some_and(|&next| next.is_ascii_whitespace() || next == b'>' || next == b'/')
        {
            continue;
        }
        let open_start = start - 1;
        let qname = &xml[start..from];
        let Some(open_end) = find_bytes(&xml[from..], b">").map(|at| from + at + 1) else { break };
        let end = if xml[open_end - 2] == b'/' {
            open_end
        } else {
            let mut close = b"</".to_vec();
            close.extend_from_slice(qname);
            let Some(close_at) = find_bytes(&xml[open_end..], &close).map(|at| open_end + at) else { break };
            let Some(close_end) = find_bytes(&xml[close_at..], b">").map(|at| close_at + at + 1) else { break };
            close_end
        };
        sections.push(open_start..end);
        from = end;
    }
    if sections.is_empty() {
        return Ok(None);
    }
    let mut merges = Vec::new();
    let mut body = Vec::with_capacity(xml.len());
    let mut kept = 0;
    for section in sections {
        body.extend_from_slice(&xml[kept..section.start]);
        kept = section.end;
        let mut reader = Reader::from_reader(&xml[section]);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_event_into(&mut buf).map_err(bad)? {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"mergeCell" => {
                    if let Some(reference) = attr(&e, b"ref") {
                        let range = CellRange::parse_a1(&reference)
                            .map_err(|_| bad(format!("malformed spreadsheetml: bad merge ref {reference:?}")))?;
                        merges.push(range);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    body.extend_from_slice(&xml[kept..]);
    Ok(Some((body, merges)))
}

/// Puts the shared strings back into a sheet the engine parsed without the shared strings part,
/// from the cells of its worksheet `xml` typed `s`. Cells are addressed and their `<v>` read as
/// the engine reads them.
fn add_shared_strings(sheet: &mut Sheet, xml: &[u8], strings: &[String]) -> Result<(), CoreError> {
    /// A `<c>` element being read.
    struct Open {
        depth: usize,
        at: CellRef,
        shared: bool,
        value: Option<String>,
        /// Depth of an `mc:AlternateContent` directly under the cell.
        alternate: Option<usize>,
        /// Depth of that block's `mc:Fallback`, whose children count as the cell's own.
        fallback: Option<usize>,
    }
    if strings.is_empty() {
        return Ok(());
    }
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut row: Option<u32> = None;
    let mut cursor = 0u32;
    let mut cell: Option<Open> = None;
    loop {
        buf.clear();
        // A `<v>` of the open cell starts: read once the event releases `buf`.
        let mut value_starts = false;
        match reader.read_event_into(&mut buf).map_err(bad)? {
            Event::Start(e) => {
                depth += 1;
                if let Some(open) = cell.as_mut() {
                    let name = e.local_name();
                    let name = name.as_ref();
                    let direct = depth == open.depth + 1;
                    if direct {
                        open.alternate = (name == b"AlternateContent").then_some(depth);
                        open.fallback = None;
                    }
                    if open.alternate.is_some_and(|at| depth == at + 1) {
                        open.fallback = (name == b"Fallback").then_some(depth);
                    } else if open.alternate != Some(depth) {
                        value_starts = name == b"v" && (direct || open.fallback.is_some_and(|at| depth == at + 1));
                    }
                } else {
                    match e.local_name().as_ref() {
                        b"row" => {
                            row = Some(match attr(&e, b"r") {
                                Some(r) => r
                                    .parse::<u32>()
                                    .ok()
                                    .filter(|r| (1..=MAX_ROWS).contains(r))
                                    .ok_or_else(|| bad(format!("bad row {r:?}")))?
                                    - 1,
                                None => row.map_or(0, |row| row + 1),
                            });
                            cursor = 0;
                        }
                        b"c" => {
                            let at = match attr(&e, b"r") {
                                Some(r) => CellRef::parse_a1(&r).map_err(|_| bad(format!("bad cell ref {r:?}")))?,
                                None => CellRef::new(row.unwrap_or(0), cursor),
                            };
                            cursor = at.col;
                            let shared = attr(&e, b"t").as_deref() == Some("s");
                            cell = Some(Open { depth, at, shared, value: None, alternate: None, fallback: None });
                        }
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                if cell.as_ref().is_some_and(|open| depth >= open.depth) {
                    continue;
                }
                match e.local_name().as_ref() {
                    b"c" => {
                        if let Some(open) = cell.take()
                            && open.shared
                            && let Some(text) = open
                                .value
                                .as_deref()
                                .and_then(|value| value.trim().parse::<usize>().ok())
                                .and_then(|index| strings.get(index))
                        {
                            sheet.set_calculated_value(open.at, CellValue::Text { value: text.clone() });
                        }
                        cursor += 1;
                    }
                    b"row" => row = None,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if value_starts && let Some(open) = cell.as_mut() {
            open.value = Some(element_text(&mut reader, &mut buf, &mut depth)?);
        }
    }
    Ok(())
}

/// The text inside the element whose start was just read, nested markup flattened.
fn element_text(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>, depth: &mut usize) -> Result<String, CoreError> {
    let target = *depth;
    let mut out = String::new();
    loop {
        buf.clear();
        match reader.read_event_into(buf).map_err(bad)? {
            Event::Start(_) => *depth += 1,
            Event::End(_) => {
                *depth = depth.saturating_sub(1);
                if *depth < target {
                    break;
                }
            }
            Event::Text(text) => out.push_str(&text.xml10_content().map_err(bad)?),
            Event::CData(text) => out.push_str(&text.xml10_content().map_err(bad)?),
            Event::GeneralRef(reference) => {
                let name = reference.decode().map_err(bad)?;
                out.push_str(&quick_xml::escape::unescape(&format!("&{name};")).map_err(bad)?);
            }
            Event::Eof => return Err(bad("a cell value ends early")),
            _ => {}
        }
    }
    Ok(out)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// The workbook part's `<sheet>` entries in order, and its active tab. `None` when it can't be read;
/// the engine's parser then reports the problem.
fn workbook_entries(xml: &[u8]) -> Option<(Vec<Entry>, usize)> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut in_sheets = false;
    let mut entries = Vec::new();
    let mut active = None;
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).ok()? {
            Event::Start(e) => {
                depth += 1;
                match e.local_name().as_ref() {
                    b"sheets" if depth == 2 => in_sheets = true,
                    b"sheet" if in_sheets && depth == 3 => entries.push(Entry {
                        hidden: matches!(attr(&e, b"state").as_deref(), Some("hidden" | "veryHidden")),
                        rid: attr(&e, b"id"),
                    }),
                    b"workbookView" if active.is_none() => {
                        active = Some(attr(&e, b"activeTab").and_then(|tab| tab.parse().ok()).unwrap_or(0));
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                if depth == 2 && e.local_name().as_ref() == b"sheets" {
                    in_sheets = false;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Some((entries, active.unwrap_or(0)))
}

fn relationships(xml: &[u8]) -> Option<HashMap<String, Relationship>> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut rels = HashMap::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).ok()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    let kind = attr(&e, b"Type");
                    let external = attr(&e, b"TargetMode").is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
                    rels.insert(id, Relationship { target, kind, external });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Some(rels)
}

/// The target of the first relationship whose type ends in `/kind`, as the engine finds a workbook part.
fn typed_target(xml: &[u8], kind: &str) -> Option<String> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).ok()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                if let (Some(kind_uri), Some(target)) = (attr(&e, b"Type"), attr(&e, b"Target"))
                    && kind_uri.strip_suffix(kind).is_some_and(|rest| rest.ends_with('/'))
                {
                    return Some(target);
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// An attribute by local name, prefix ignored, unescaped.
fn attr(e: &BytesStart, local: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == local).and_then(|a| {
        a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|value| value.into_owned())
    })
}

/// A relationship target resolved against its part's folder, `..` collapsed (as the engine does).
fn resolve_part_path(base: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut segments: Vec<&str> = base.split('/').filter(|segment| !segment.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    segments.join("/")
}

fn position(parts: &[(String, Vec<u8>)], name: &str) -> Option<usize> {
    let name = name.trim_start_matches('/');
    parts.iter().position(|(part, _)| part.trim_start_matches('/') == name)
}

fn find<'a>(parts: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    position(parts, name).map(|index| parts[index].1.as_slice())
}

fn keep_part(name: &str) -> bool {
    let name = name.trim_start_matches('/').to_ascii_lowercase();
    (name.ends_with(".xml") || name.ends_with(".rels")) && !SKIPPED_PARTS.iter().any(|skipped| name.starts_with(skipped))
}

/// Runs `work` on a thread with the engine's stack.
fn on_engine_stack<T: Send>(work: impl FnOnce() -> T + Send) -> Result<T, CoreError> {
    thread::scope(|scope| {
        thread::Builder::new()
            .name("sheet-engine".into())
            .stack_size(ENGINE_STACK)
            .spawn_scoped(scope, work)
            .map_err(|e| CoreError::BadFile(format!("Can't start reading this file: {e}")))?
            .join()
            .map_err(|_| stopped())
    })
}

fn bad(e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this spreadsheet: {e}"))
}

fn stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped reading this file because it hit something it can't handle.".into())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::time::Instant;

    use betteroffice_xlsx::{SheetId, Workbook};

    use super::*;

    // Fixtures in tests/fixtures/sheets are Apache POI test-data (Apache-2.0), named per test.
    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/sheets/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn open(name: &str) -> Arc<SheetDoc> {
        SheetDoc::open(fixture(name), OfficeFamily::Spreadsheet, None).unwrap()
    }

    fn cell(doc: &SheetDoc, sheet: u32, a1: &str) -> SheetCell {
        let at = CellRef::parse_a1(a1).unwrap();
        let mut cells = doc.cells(sheet, at.row, 1, at.col, 1).unwrap();
        assert_eq!(cells.len(), 1, "{a1}");
        cells.remove(0)
    }

    fn text(doc: &SheetDoc, sheet: u32, a1: &str) -> String {
        cell(doc, sheet, a1).text
    }

    fn range(a1: &str) -> SheetRange {
        CellRange::parse_a1(a1).unwrap().into()
    }

    // Formatting.xlsx (Apache POI test-data, Apache-2.0): every date is 24 November 2006, every number 10.52.
    #[test]
    fn dates_numbers_and_currency_read_as_excel_shows_them() {
        let doc = open("Formatting.xlsx");
        for (a1, shown) in [
            ("B2", "11/24/2006"),
            ("B3", "2006/11/24"),
            ("B4", "2006-11-24"),
            ("B5", "06/11/24"),
            ("B6", "24/11/06"),
            ("B7", "24-11-06"),
            ("B10", "10.52"),
            ("B11", "10.520"),
            ("B12", "10.5"),
            ("B13", "£10.52"),
        ] {
            assert_eq!(text(&doc, 0, a1), shown, "{a1}");
        }
        let date = cell(&doc, 0, "B2");
        assert_eq!(date.kind, SheetValueKind::Number);
        assert_eq!(date.style.h_align, SheetHAlign::Right);
        let label = cell(&doc, 0, "A1");
        assert_eq!((label.text.as_str(), label.kind), ("Dates, all 24th November 2006", SheetValueKind::Text));
        assert_eq!(label.style.h_align, SheetHAlign::Left);
    }

    // FormatKM.xlsx (Apache POI test-data, Apache-2.0): columns C and E hold the text Excel showed for B and D.
    #[test]
    fn conditional_and_scaled_formats_match_excel() {
        let doc = open("FormatKM.xlsx");
        for row in 2..=12 {
            assert_eq!(text(&doc, 0, &format!("B{row}")), text(&doc, 0, &format!("C{row}")), "B{row}");
            assert_eq!(text(&doc, 0, &format!("D{row}")), text(&doc, 0, &format!("E{row}")), "D{row}");
        }
    }

    // simple-monthly-budget.xlsx (Apache POI test-data, Apache-2.0): an Excel template with merges, fills,
    // borders, a picture and a chart.
    #[test]
    fn template_shows_percent_currency_merges_sizes_and_styles() {
        let doc = open("simple-monthly-budget.xlsx");
        assert_eq!(doc.sheets(), vec![SheetTab { name: "Simple Monthly Budget".into(), hidden: false }]);
        assert_eq!(text(&doc, 0, "H4"), "62%");
        assert_eq!(text(&doc, 0, "C5"), "$2,500.00");
        assert_eq!(text(&doc, 0, "E9"), "$3,750");

        let merges = doc.merges(0).unwrap();
        for merged in ["E4:G5", "H4:H5", "B8:C8", "G8:H8", "G9:H9"] {
            assert!(merges.contains(&range(merged)), "{merged}");
        }
        let merged = cell(&doc, 0, "E4");
        assert_eq!(merged.text, "2336");
        assert!(merged.style.fill.is_some());

        let title = cell(&doc, 0, "A1");
        assert_eq!(title.text, "SIMPLE MONTHLY BUDGET");
        assert_eq!(title.style.font_name.as_deref(), Some("Century Gothic"));
        assert_eq!(title.style.font_size_pt, 25.0);
        assert_eq!(title.style.font_color, Some(0xFF4E_5B6F));
        assert_eq!(title.style.h_align, SheetHAlign::Left);

        let percent = cell(&doc, 0, "H4");
        assert_eq!(percent.style.font_size_pt, 22.0);
        assert_eq!((percent.style.h_align, percent.style.v_align), (SheetHAlign::Right, SheetVAlign::Center));
        assert_eq!(percent.style.border_right.map(|side| side.style), Some(SheetLineStyle::Thick));
        assert_eq!(percent.style.border_top.map(|side| side.style), Some(SheetLineStyle::Thick));
        assert_eq!(percent.style.border_left, None);

        let layout = doc.layout(0).unwrap();
        assert_eq!((layout.row_count, layout.col_count), (23, 8));
        assert!(!layout.show_gridlines);
        assert_eq!(layout.default_row_height_px, 28.0);
        let widths = doc.col_widths(0).unwrap();
        assert_eq!(widths[..2], [SheetSizeRun { first: 0, last: 0, px: 23.0 }, SheetSizeRun { first: 1, last: 1, px: 157.0 }]);
        assert_eq!(doc.row_heights(0).unwrap()[0], SheetSizeRun { first: 0, last: 0, px: 54.0 });
    }

    // 47737.xlsx (Apache POI test-data, Apache-2.0): two sheets with frozen header rows.
    #[test]
    fn frozen_rows_merges_and_column_widths() {
        let doc = open("47737.xlsx");
        let layout = doc.layout(0).unwrap();
        assert_eq!((layout.frozen_rows, layout.frozen_cols), (5, 0));
        assert_eq!((layout.default_col_width_px, layout.default_row_height_px), (64.0, 17.0));
        let widths = doc.col_widths(0).unwrap();
        assert_eq!(widths[0], SheetSizeRun { first: 0, last: 3, px: 41.0 });
        assert!(widths.contains(&SheetSizeRun { first: 8, last: 8, px: 283.0 }), "{widths:?}");
        assert!(doc.merges(0).unwrap().contains(&range("A2:O2")));
        assert_eq!(text(&doc, 0, "A5"), "Heading");
        assert_eq!(doc.layout(1).unwrap().frozen_rows, 4);
        assert!(doc.layout(2).is_err());
    }

    // simple-monthly-budget.xlsx and 47737.xlsx as above.
    #[test]
    fn merged_ranges_match_the_engines_workbook() {
        for name in ["simple-monthly-budget.xlsx", "47737.xlsx"] {
            let bytes = fixture(name);
            let workbook = Workbook::open_for_read(&bytes).unwrap();
            let doc = SheetDoc::open(bytes, OfficeFamily::Spreadsheet, None).unwrap();
            for index in 0..workbook.sheet_count() as u32 {
                let expected: Vec<SheetRange> =
                    workbook.sheet(SheetId(index)).unwrap().merges.iter().map(|&merged| merged.into()).collect();
                assert_eq!(doc.merges(index).unwrap(), expected, "{name} sheet {index}");
            }
        }
    }

    // TwoSheetsOneHidden.xlsx (Apache POI test-data, Apache-2.0).
    #[test]
    fn hidden_sheet_and_active_tab() {
        let doc = open("TwoSheetsOneHidden.xlsx");
        assert_eq!(
            doc.sheets(),
            vec![SheetTab { name: "Sheet1".into(), hidden: true }, SheetTab { name: "Sheet2".into(), hidden: false }]
        );
        assert_eq!(doc.active_sheet(), 1);
        assert_eq!(doc.sheet_text(1).unwrap(), "Sheet2A1");
        assert_eq!(doc.sheet_text(0).unwrap(), "Sheet1A1");
    }

    // WithMoreVariousData.xlsx (Apache POI test-data, Apache-2.0).
    #[test]
    fn hyperlinks_keep_their_targets() {
        let links = open("WithMoreVariousData.xlsx").hyperlinks(0).unwrap();
        assert_eq!(links.len(), 4);
        assert_eq!((links[0].range, links[0].url.as_deref()), (range("C4"), Some("http://poi.apache.org/")));
        assert_eq!((links[1].range, links[1].url.as_deref()), (range("C15"), None));
        assert_eq!(links[1].location.as_deref(), Some("Sheet1!A2"));
        assert_eq!(links[3].url.as_deref(), Some("mailto:dev@poi.apache.org?subject=XSSF%20Hyperlinks"));
    }

    // Formatting.xls (Apache POI test-data, Apache-2.0): the Excel 97-2003 twin of Formatting.xlsx.
    #[test]
    fn legacy_xls_opens_through_the_converter() {
        let doc = open("Formatting.xls");
        assert_eq!(doc.sheets().len(), 3);
        assert!(doc.sheet_text(0).unwrap().contains("Dates, all 24th November 2006"));
    }

    #[test]
    fn other_families_are_refused() {
        assert!(SheetDoc::open(fixture("Formatting.xlsx"), OfficeFamily::Document, None).is_err());
    }

    // Generated workbook: 1,234,567.89 in columns 3 and 30 characters wide, and 5 behind a `-` fill
    // in columns 10 and 30 characters wide.
    #[test]
    fn narrow_numbers_show_hashes_and_fill_characters_repeat() {
        let sheet = r#"<cols><col min="1" max="1" width="3" customWidth="1"/><col min="2" max="2" width="30" customWidth="1"/><col min="3" max="3" width="10" customWidth="1"/><col min="4" max="4" width="30" customWidth="1"/></cols><sheetData><row r="1"><c r="A1" s="2"><v>1234567.891</v></c><c r="B1" s="2"><v>1234567.891</v></c><c r="C1" s="3"><v>5</v></c><c r="D1" s="3"><v>5</v></c></row></sheetData>"#;
        let doc = SheetDoc::open(package(&[sheet], None, Vec::new()), OfficeFamily::Spreadsheet, None).unwrap();
        let narrow = text(&doc, 0, "A1");
        assert!(!narrow.is_empty() && narrow.chars().all(|ch| ch == '#'), "{narrow:?}");
        assert_eq!(text(&doc, 0, "B1"), "1,234,567.89");
        let dashes = |a1: &str| {
            let shown = text(&doc, 0, a1);
            let fill = shown.strip_suffix('5').unwrap_or_else(|| panic!("{a1}: {shown:?}"));
            assert!(fill.chars().all(|ch| ch == '-'), "{a1}: {shown:?}");
            fill.len()
        };
        let (narrow, wide) = (dashes("C1"), dashes("D1"));
        assert!(narrow >= 5 && wide > 2 * narrow, "{narrow} then {wide} dashes");
        // Search text is not fitted to any width.
        assert_eq!(doc.sheet_text(0).unwrap(), "1,234,567.89\t1,234,567.89\t5\t5");
    }

    // Generated workbook: 24 November 2006 at noon in built-in formats 14 and 22.
    #[test]
    fn short_dates_follow_the_locale() {
        let sheet = r#"<cols><col min="1" max="2" width="30" customWidth="1"/></cols><sheetData><row r="1"><c r="A1" s="4"><v>39045.5</v></c><c r="B1" s="5"><v>39045.5</v></c></row></sheetData>"#;
        let bytes = package(&[sheet], None, Vec::new());
        let shown = |locale: Option<&str>| {
            let doc = SheetDoc::open(bytes.clone(), OfficeFamily::Spreadsheet, locale.map(String::from)).unwrap();
            (text(&doc, 0, "A1"), text(&doc, 0, "B1"))
        };
        let pair = |date: &str, date_time: &str| (date.to_string(), date_time.to_string());
        assert_eq!(shown(None), pair("11/24/2006", "11/24/2006 12:00"));
        assert_eq!(shown(Some("en-US")), pair("11/24/2006", "11/24/2006 12:00"));
        assert_eq!(shown(Some("en-GB")), pair("24/11/2006", "24/11/2006 12:00"));
        assert_eq!(shown(Some("de-DE")), pair("24.11.2006", "24.11.2006 12:00"));
        assert_eq!(shown(Some("ja-JP")), pair("2006/11/24", "2006/11/24 12:00"));
    }

    // Generated workbook: two sheets reading one shared strings part, the second with a cell addressed
    // by its position, an index past the table, a rich string and a merged range.
    #[test]
    fn shared_strings_parse_once_per_workbook() {
        let first = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData>"#;
        let second = r#"<sheetData><row r="1"><c r="A1" t="s"><v>2</v></c><c t="s"><v>1</v></c><c r="D1" t="s"><v>7</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="E1:F1"/></mergeCells>"#;
        let strings = "<si><t>alpha</t></si><si><t>beta</t></si><si><r><t>gam</t></r><r><t>ma</t></r></si>";
        let doc = SheetDoc::open(package(&[first, second], Some(strings), Vec::new()), OfficeFamily::Spreadsheet, None)
            .unwrap();
        assert_eq!(text(&doc, 0, "A1"), "alpha");
        assert_eq!(doc.sheet_text(1).unwrap(), "gamma\tbeta");
        let second_cell = cell(&doc, 1, "B1");
        assert_eq!((second_cell.kind, second_cell.style.h_align), (SheetValueKind::Text, SheetHAlign::Left));
        assert_eq!(doc.merges(1).unwrap(), vec![range("E1:F1")]);
        assert_eq!(doc.book.lock().unwrap().string_reads, 1);
    }

    /// Before/after open timings on a generated workbook shaped like the slow spike files (many
    /// cells and merged ranges, a large picture), written past the test harness so CI logs show them.
    #[test]
    fn view_path_times_against_the_editing_workbook() {
        let rows = 10_000;
        let bytes = generated_workbook(rows);

        let started = Instant::now();
        let workbook = Workbook::open(&bytes).unwrap();
        let open = started.elapsed();
        let started = Instant::now();
        let for_read = Workbook::open_for_read(&bytes).unwrap();
        let open_for_read = started.elapsed();

        let started = Instant::now();
        let doc = SheetDoc::open(bytes, OfficeFamily::Spreadsheet, None).unwrap();
        let small = doc.cells(0, 0, 40, 0, 10).unwrap();
        let first_view = started.elapsed();
        let started = Instant::now();
        let layout = doc.layout(1).unwrap();
        let big = doc.cells(1, 0, 40, 0, 10).unwrap();
        let second_sheet = started.elapsed();

        assert_eq!(small.len(), 1);
        assert_eq!(small[0].text, "25.00%");
        assert_eq!((layout.row_count, layout.col_count), (rows, 4));
        assert_eq!(doc.merges(1).unwrap().len(), for_read.sheet(SheetId(1)).unwrap().merges.len());
        assert_eq!(doc.merges(1).unwrap().len(), rows as usize);
        let first_row: Vec<&str> = big.iter().take_while(|cell| cell.row == 0).map(|cell| cell.text.as_str()).collect();
        assert_eq!(first_row, ["1", "row 1", "0.5"]);
        assert_eq!(workbook.sheet_count(), 2);

        writeln!(
            std::io::stderr(),
            "sheet open timings, {rows} rows x 3 cells + {rows} merges + 4 MiB picture, debug build: \
             Workbook::open {open:?}, Workbook::open_for_read {open_for_read:?}, \
             SheetDoc::open + first window {first_view:?}, second sheet layout + window {second_sheet:?}"
        )
        .unwrap();
    }

    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    /// Cell formats of generated workbooks: 0 General, 1 `0.00%`, 2 `#,##0.00`, 3 `*-0` (a `-` fill
    /// before the number), 4 and 5 the built-in short date and short date with time.
    const STYLES: &str = r#"<numFmts count="1"><numFmt numFmtId="164" formatCode="*-0"/></numFmts><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="6"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="10" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="4" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="22" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs>"#;

    /// A workbook with one sheet per worksheet body in `sheets`, the cell formats [`STYLES`], the
    /// shared strings `strings` (`<si>` items) when given, and `extra` parts nothing links to.
    fn package(sheets: &[&str], strings: Option<&str>, extra: Vec<(String, Vec<u8>)>) -> Vec<u8> {
        let sheet_type = "application/vnd.openxmlformats-officedocument.spreadsheetml";
        let mut entries = String::new();
        let mut rels = String::new();
        let mut types = String::new();
        let mut worksheets = Vec::new();
        for (index, body) in sheets.iter().enumerate() {
            let n = index + 1;
            write!(entries, r#"<sheet name="Sheet{n}" sheetId="{n}" r:id="rId{n}"/>"#).unwrap();
            write!(rels, r#"<Relationship Id="rId{n}" Type="{REL}/worksheet" Target="worksheets/sheet{n}.xml"/>"#).unwrap();
            write!(types, r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="{sheet_type}.worksheet+xml"/>"#)
                .unwrap();
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="{MAIN}">{body}</worksheet>"#
            );
            worksheets.push((format!("xl/worksheets/sheet{n}.xml"), xml.into_bytes()));
        }
        let next = sheets.len() + 1;
        write!(rels, r#"<Relationship Id="rId{next}" Type="{REL}/styles" Target="styles.xml"/>"#).unwrap();
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        if let Some(strings) = strings {
            write!(rels, r#"<Relationship Id="rId{}" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/>"#, next + 1)
                .unwrap();
            write!(types, r#"<Override PartName="/xl/sharedStrings.xml" ContentType="{sheet_type}.sharedStrings+xml"/>"#)
                .unwrap();
            parts.push(("xl/sharedStrings.xml".into(), format!(r#"<sst xmlns="{MAIN}">{strings}</sst>"#).into_bytes()));
        }
        let content_types = format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/workbook.xml" ContentType="{sheet_type}.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="{sheet_type}.styles+xml"/>{types}</Types>"#
        );
        let root_rels = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
        );
        let workbook = format!(r#"<workbook xmlns="{MAIN}" xmlns:r="{REL}"><sheets>{entries}</sheets></workbook>"#);
        let workbook_rels =
            format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
        let styles = format!(r#"<styleSheet xmlns="{MAIN}">{STYLES}</styleSheet>"#);
        let mut all: Vec<(String, Vec<u8>)> = vec![
            ("[Content_Types].xml".into(), content_types.into_bytes()),
            ("_rels/.rels".into(), root_rels.into_bytes()),
            ("xl/workbook.xml".into(), workbook.into_bytes()),
            ("xl/_rels/workbook.xml.rels".into(), workbook_rels.into_bytes()),
            ("xl/styles.xml".into(), styles.into_bytes()),
        ];
        all.append(&mut parts);
        all.append(&mut worksheets);
        all.extend(extra);
        ooxml_opc::rezip_parts(&all).unwrap()
    }

    fn generated_workbook(rows: u32) -> Vec<u8> {
        let mut big = String::from("<sheetData>");
        for row in 1..=rows {
            let half = f64::from(row) / 2.0;
            write!(
                big,
                r#"<row r="{row}"><c r="A{row}"><v>{row}</v></c><c r="B{row}" t="inlineStr"><is><t>row {row}</t></is></c><c r="D{row}"><v>{half}</v></c></row>"#
            )
            .unwrap();
        }
        write!(big, r#"</sheetData><mergeCells count="{rows}">"#).unwrap();
        for row in 1..=rows {
            write!(big, r#"<mergeCell ref="B{row}:C{row}"/>"#).unwrap();
        }
        big.push_str("</mergeCells>");
        let small = r#"<sheetData><row r="1"><c r="A1" s="1"><v>0.25</v></c></row></sheetData>"#;
        // Incompressible bytes, like a photo, that a grid never needs to inflate.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let picture: Vec<u8> = (0..4 << 20)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        package(&[small, &big], None, vec![("xl/media/image1.png".into(), picture)])
    }
}
