//! BIFF8 formulas ([MS-XLS] §2.2.2): the reverse-Polish token stream (`Rgce`) of a cell, shared or array
//! formula, defined name, conditional format or data validation, written back as the infix text
//! SpreadsheetML keeps in `<f>`. Excel stores spaces and parentheses as tokens, so the text comes back as
//! it was typed.

use std::collections::HashMap;

use super::{biff_string, col_name, u16_at, u32_at};

const BOUNDSHEET: u16 = 0x0085;
const SUPBOOK: u16 = 0x01AE;
const EXTERNNAME: u16 = 0x0023;
const EXTERNSHEET: u16 = 0x0017;
const NAME: u16 = 0x0018;

/// What a `SUPBOOK` record points at.
#[derive(Clone, Copy, PartialEq)]
enum BookKind {
    Own,
    AddIn,
    External,
}

struct Supbook {
    kind: BookKind,
    /// Its `EXTERNNAME` records: add-in function names for an add-in book.
    names: Vec<String>,
}

/// A `NAME` (`Lbl`) record.
pub(super) struct Name {
    pub(super) name: String,
    /// 1-based sheet of a sheet-level name, 0 for a workbook-level one.
    pub(super) itab: u16,
    pub(super) hidden: bool,
    /// A macro or add-in function name (`fFunc`), which is no defined name.
    function: bool,
    /// The `NameParsedFormula`: tokens and their extra data.
    rgce: Vec<u8>,
    rgcb: Vec<u8>,
}

/// The workbook tables formula tokens point into.
#[derive(Default)]
pub(super) struct Links {
    /// `BOUNDSHEET` names in sheet-tab order, what `itab` counts.
    sheets: Vec<String>,
    books: Vec<Supbook>,
    /// `EXTERNSHEET` bytes until `finish` splits them into `xti`.
    externsheet: Vec<u8>,
    /// `XTI`: supporting book, first and last sheet.
    xti: Vec<(u16, i16, i16)>,
    pub(super) names: Vec<Name>,
}

impl Links {
    /// One globals record; `kind` is the record type, `CONTINUE` reported as the record it continues.
    pub(super) fn record(&mut self, kind: u16, continued: bool, data: &[u8]) {
        match kind {
            EXTERNSHEET => self.externsheet.extend_from_slice(data),
            _ if continued => {},
            BOUNDSHEET => self.sheets.push(biff_string(data, 6, false).map(|s| s.0).unwrap_or_default()),
            SUPBOOK => {
                let kind = match u16_at(data, 2) {
                    Some(0x0401) => BookKind::Own,
                    Some(0x3A01) => BookKind::AddIn,
                    _ => BookKind::External,
                };
                self.books.push(Supbook { kind, names: Vec::new() });
            },
            // ExternDocName, AddinUdf and ExternOleDdeLink all keep the name at offset 6.
            EXTERNNAME => {
                if let Some(book) = self.books.last_mut() {
                    book.names.push(biff_string(data, 6, false).map(|s| s.0).unwrap_or_default());
                }
            },
            // Tokens count names by record, so an unreadable one still takes its place.
            NAME => self.names.push(parse_name(data).unwrap_or(Name {
                name: String::new(),
                itab: 0,
                hidden: false,
                function: true,
                rgce: Vec::new(),
                rgcb: Vec::new(),
            })),
            _ => {},
        }
    }

    /// Splits the `EXTERNSHEET` table once the globals are read.
    pub(super) fn finish(&mut self) {
        let count = u16_at(&self.externsheet, 0).unwrap_or(0) as usize;
        let body = self.externsheet.get(2..).unwrap_or_default();
        self.xti = body
            .chunks_exact(6)
            .take(count)
            .map(|x| (u16::from_le_bytes([x[0], x[1]]), i16::from_le_bytes([x[2], x[3]]), i16::from_le_bytes([x[4], x[5]])))
            .collect();
        self.externsheet = Vec::new();
    }

    /// A defined name's formula text; `None` for function names and formulas that cannot be written.
    pub(super) fn name_formula(&self, name: &Name) -> Option<String> {
        if name.function || name.name.is_empty() || name.rgce.is_empty() {
            return None;
        }
        let sheet = name.itab.checked_sub(1).map(usize::from);
        text(&name.rgce, &name.rgcb, Origin { row: 0, col: 0, sheet, relative: true }, self)
    }

    /// `'Sheet 1'!` or `Sheet1:Sheet3!` for `XTI` `ixti`; `None` when it leaves this workbook.
    fn sheet_prefix(&self, ixti: u16) -> Option<Sheets> {
        let &(book, first, last) = self.xti.get(usize::from(ixti))?;
        if self.books.get(usize::from(book))?.kind != BookKind::Own {
            // Limit: references into other workbooks drop the formula (the cell keeps its value); writing externalLink parts lifts it.
            return None;
        }
        // A deleted sheet.
        let (Ok(first), Ok(last)) = (usize::try_from(first), usize::try_from(last)) else { return Some(Sheets::Deleted) };
        let (a, b) = (self.sheets.get(first)?, self.sheets.get(last)?);
        let names = if first == last { a.clone() } else { format!("{a}:{b}") };
        Some(Sheets::Named(if needs_quotes(a) || (first != last && needs_quotes(b)) {
            format!("'{}'!", names.replace('\'', "''"))
        } else {
            format!("{names}!")
        }))
    }

    fn sheet_name(&self, index: usize) -> Option<String> {
        let name = self.sheets.get(index)?;
        Some(if needs_quotes(name) { format!("'{}'!", name.replace('\'', "''")) } else { format!("{name}!") })
    }
}

enum Sheets {
    Named(String),
    Deleted,
}

/// `NAME` ([MS-XLS] §2.4.150): grbit, chKey, cch, cce, reserved, itab, 4 reserved bytes, then the name
/// (`XLUnicodeStringNoCch`) and the formula.
fn parse_name(data: &[u8]) -> Option<Name> {
    let grbit = u16_at(data, 0)?;
    let cch = usize::from(*data.get(3)?);
    let cce = usize::from(u16_at(data, 4)?);
    let itab = u16_at(data, 8)?;
    let wide = data.get(14)? & 1 != 0;
    let start = 15;
    let end = start + if wide { cch * 2 } else { cch };
    let raw = data.get(start..end)?;
    let name = if grbit & 0x0020 != 0 {
        builtin_name(*raw.first()?)?.to_string()
    } else if wide {
        String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        raw.iter().map(|&b| char::from(b)).collect()
    };
    let rgce = data.get(end..end + cce)?.to_vec();
    let rgcb = data[end + cce..].to_vec();
    Some(Name { name, itab, hidden: grbit & 0x0001 != 0, function: grbit & 0x0002 != 0, rgce, rgcb })
}

/// [MS-XLS] §2.5.28 built-in names as SpreadsheetML spells them.
fn builtin_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x00 => "_xlnm.Consolidate_Area",
        0x01 => "_xlnm.Auto_Open",
        0x02 => "_xlnm.Auto_Close",
        0x03 => "_xlnm.Extract",
        0x04 => "_xlnm.Database",
        0x05 => "_xlnm.Criteria",
        0x06 => "_xlnm.Print_Area",
        0x07 => "_xlnm.Print_Titles",
        0x08 => "_xlnm.Recorder",
        0x09 => "_xlnm.Data_Form",
        0x0A => "_xlnm.Auto_Activate",
        0x0B => "_xlnm.Auto_Deactivate",
        0x0C => "_xlnm.Sheet_Title",
        0x0D => "_xlnm._FilterDatabase",
        _ => return None,
    })
}

/// Whether a sheet name needs quotes in a reference: anything but letters, digits, `_` and `.`, a leading
/// digit, or a name that reads as a cell (`A1`, `R1C1`) or a boolean.
fn needs_quotes(name: &str) -> bool {
    let first = name.chars().next();
    if first.is_none_or(|c| c.is_ascii_digit() || c == '.')
        || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return true;
    }
    let upper = name.to_ascii_uppercase();
    if upper == "TRUE" || upper == "FALSE" {
        return true;
    }
    // A1: a column up to XFD, then a row up to 1048576.
    let letters = upper.bytes().take_while(u8::is_ascii_alphabetic).count();
    let rest = &upper[letters..];
    let col = upper.bytes().take(letters).fold(0u32, |n, b| n.saturating_mul(26).saturating_add(u32::from(b - b'A') + 1));
    if (1..=16_384).contains(&col) && rest.parse::<u32>().is_ok_and(|row| (1..=1_048_576).contains(&row)) {
        return true;
    }
    // R1C1: R, R1, C, C1, R1C, R1C1.
    let r1c1 = |s: &str| {
        let s = s.strip_prefix('R').map_or(s, |s| s.trim_start_matches(|c: char| c.is_ascii_digit()));
        s.is_empty() || s.strip_prefix('C').is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
    };
    (upper.starts_with('R') || upper.starts_with('C')) && r1c1(&upper)
}

/// Where a formula sits.
#[derive(Clone, Copy)]
pub(super) struct Origin {
    pub(super) row: u16,
    pub(super) col: u16,
    /// The 0-based sheet the formula is on, for sheet-level names.
    pub(super) sheet: Option<usize>,
    /// Whether 3-D references count relative rows and columns from the origin (`RgceLocRel`), as in shared
    /// formulas, names, conditional formats and data validations.
    pub(super) relative: bool,
}

/// A `cce`-prefixed parsed formula at `at`: its tokens and the extra data after them.
pub(super) fn split(data: &[u8], at: usize) -> Option<(&[u8], &[u8])> {
    let cce = usize::from(u16_at(data, at)?);
    let rgce = data.get(at + 2..at + 2 + cce)?;
    Some((rgce, &data[at + 2 + cce..]))
}

/// A cell's formula for `<f>`.
pub(super) struct CellFormula {
    pub(super) text: String,
    /// The `ref` of an array formula, on its top-left cell.
    pub(super) array: Option<String>,
}

/// A rectangle of cells: first row, last row, first column, last column.
pub(super) type Rect = (u16, u16, u16, u16);

/// One sheet's formula records.
#[derive(Default)]
pub(super) struct SheetFormulas {
    /// `FORMULA`: row, column and the `CellParsedFormula` (`cce`, tokens, extra data).
    cells: Vec<(u16, u16, Vec<u8>)>,
    /// `SHRFMLA` and `ARRAY`: the cells they cover and the parsed formula.
    shared: Vec<(Rect, Vec<u8>)>,
    arrays: Vec<(Rect, Vec<u8>)>,
}

const FORMULA: u16 = 0x0006;
const SHRFMLA: u16 = 0x04BC;
const ARRAY: u16 = 0x0221;

impl SheetFormulas {
    pub(super) fn record(&mut self, kind: u16, data: &[u8]) {
        // RefU: rwFirst, rwLast, colFirst and colLast as bytes.
        let rect = || Some((u16_at(data, 0)?, u16_at(data, 2)?, u16::from(*data.get(4)?), u16::from(*data.get(5)?)));
        match kind {
            FORMULA => {
                if let (Some(row), Some(col), Some(parsed)) = (u16_at(data, 0), u16_at(data, 2), data.get(20..)) {
                    self.cells.push((row, col, parsed.to_vec()));
                }
            },
            SHRFMLA => {
                if let (Some(rect), Some(parsed)) = (rect(), data.get(8..)) {
                    self.shared.push((rect, parsed.to_vec()));
                }
            },
            ARRAY => {
                if let (Some(rect), Some(parsed)) = (rect(), data.get(12..)) {
                    self.arrays.push((rect, parsed.to_vec()));
                }
            },
            _ => {},
        }
    }

    /// Each formula cell's text, keyed by `(row, col)`. A cell of a shared formula gets the formula as
    /// it reads there; an array formula sits on its top-left cell, the other cells keep values only.
    // Limit: data tables (TABLE records) keep their values without the TABLE() formula; writing <f t="dataTable"> lifts it.
    pub(super) fn texts(&self, sheet: usize, links: &Links) -> HashMap<(u16, u16), CellFormula> {
        let shared: HashMap<(u16, u16), &[u8]> = self.shared.iter().map(|(r, f)| ((r.0, r.2), f.as_slice())).collect();
        let arrays: HashMap<(u16, u16), (Rect, &[u8])> = self.arrays.iter().map(|(r, f)| ((r.0, r.2), (*r, f.as_slice()))).collect();
        let mut out = HashMap::new();
        for (row, col, parsed) in &self.cells {
            let (row, col) = (*row, *col);
            let Some((rgce, rgcb)) = split(parsed, 0) else { continue };
            let at = Origin { row, col, sheet: Some(sheet), relative: false };
            let formula = match rgce {
                // PtgExp: this cell belongs to the shared or array formula anchored at (row, col).
                [0x01, r0, r1, c0, c1, ..] => {
                    let anchor = (u16::from_le_bytes([*r0, *r1]), u16::from_le_bytes([*c0, *c1]));
                    if let Some(&(rect, parsed)) = arrays.get(&anchor) {
                        if anchor != (row, col) {
                            continue;
                        }
                        let Some((rgce, rgcb)) = split(parsed, 0) else { continue };
                        text(rgce, rgcb, at, links).map(|text| CellFormula { text, array: Some(rect_text(rect)) })
                    } else if let Some(parsed) = shared.get(&anchor) {
                        let Some((rgce, rgcb)) = split(parsed, 0) else { continue };
                        text(rgce, rgcb, Origin { relative: true, ..at }, links).map(|text| CellFormula { text, array: None })
                    } else {
                        None
                    }
                },
                _ => text(rgce, rgcb, at, links).map(|text| CellFormula { text, array: None }),
            };
            if let Some(formula) = formula {
                out.insert((row, col), formula);
            }
        }
        out
    }
}

/// `A1` or `A1:B2` for a rectangle.
pub(super) fn rect_text((r1, r2, c1, c2): Rect) -> String {
    let first = format!("{}{}", col_name(c1.into()), u32::from(r1) + 1);
    if (r1, c1) == (r2, c2) { first } else { format!("{first}:{}{}", col_name(c2.into()), u32::from(r2) + 1) }
}

/// Whitespace Excel stored with `PtgAttrSpace`, waiting for the token it belongs before.
#[derive(Default)]
struct Spaces {
    next: String,
    open: String,
    close: String,
}

/// The formula text of tokens `rgce` with extra data `rgcb`; `None` when a token is unknown or points where
/// SpreadsheetML cannot follow (another workbook, a data table, a macro command).
pub(super) fn text(rgce: &[u8], rgcb: &[u8], at: Origin, links: &Links) -> Option<String> {
    let mut stack: Vec<String> = Vec::new();
    let mut extra = rgcb;
    let mut spaces = Spaces::default();
    let mut pos = 0;
    while pos < rgce.len() {
        let ptg = rgce[pos];
        let body = &rgce[pos + 1..];
        // Classed tokens (reference, value, array) share one meaning per low five bits.
        let kind = if ptg >= 0x20 { ptg & 0x1F | 0x20 } else { ptg };
        let lead = std::mem::take(&mut spaces.next);
        let size = match kind {
            0x03..=0x11 => {
                let b = stack.pop()?;
                let a = stack.pop()?;
                let op = ["+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>", " ", ",", ":"][usize::from(kind - 3)];
                stack.push(format!("{a}{lead}{op}{b}"));
                0
            },
            0x12 | 0x13 => {
                let a = stack.pop()?;
                stack.push(format!("{lead}{}{a}", if kind == 0x12 { "+" } else { "-" }));
                0
            },
            0x14 => {
                let a = stack.pop()?;
                stack.push(format!("{a}{lead}%"));
                0
            },
            0x15 => {
                let a = stack.pop()?;
                let (open, close) = (std::mem::take(&mut spaces.open), std::mem::take(&mut spaces.close));
                stack.push(format!("{lead}{open}({a}{close})"));
                0
            },
            0x16 => {
                stack.push(lead);
                0
            },
            0x17 => {
                let (s, end) = biff_string(rgce, pos + 1, false)?;
                stack.push(format!("{lead}\"{}\"", s.replace('"', "\"\"")));
                end - pos - 1
            },
            0x19 => {
                let grbit = *body.first()?;
                if grbit & 0x40 != 0 {
                    // tAttrSpace: kind and count; odd kinds are line breaks.
                    let (place, count) = (*body.get(1)?, usize::from(*body.get(2)?));
                    let ws = if place & 1 == 0 { " " } else { "\n" }.repeat(count);
                    match place {
                        2 | 3 => spaces.open.push_str(&ws),
                        4 | 5 => spaces.close.push_str(&ws),
                        _ => spaces.next.push_str(&ws),
                    }
                    // A space token passes the lead on to the token it precedes.
                    spaces.next.insert_str(0, &lead);
                } else {
                    // The other attributes steer evaluation only (if, goto, choose jumps, volatility);
                    // their lead stays for the next token.
                    spaces.next.insert_str(0, &lead);
                }
                if grbit & 0x10 != 0 {
                    // tAttrSum: SUM of one argument.
                    let a = stack.pop()?;
                    let lead = std::mem::take(&mut spaces.next);
                    let (open, close) = (std::mem::take(&mut spaces.open), std::mem::take(&mut spaces.close));
                    stack.push(format!("{lead}SUM{open}({a}{close})"));
                }
                if grbit & 0x04 != 0 { 3 + 2 * (usize::from(u16_at(body, 1)?) + 1) } else { 3 }
            },
            0x1C => {
                stack.push(format!("{lead}{}", error(*body.first()?)?));
                1
            },
            0x1D => {
                stack.push(format!("{lead}{}", if *body.first()? != 0 { "TRUE" } else { "FALSE" }));
                1
            },
            0x1E => {
                stack.push(format!("{lead}{}", u16_at(body, 0)?));
                2
            },
            0x1F => {
                stack.push(format!("{lead}{}", number(f64::from_le_bytes(body.get(..8)?.try_into().ok()?))));
                8
            },
            // PtgArray: the values sit in the extra data, in token order.
            0x20 => {
                let (array, used) = array(extra)?;
                extra = &extra[used..];
                stack.push(format!("{lead}{array}"));
                7
            },
            0x21 => {
                let (name, argc) = FUNCTIONS.get(usize::from(u16_at(body, 0)?))?;
                if name.is_empty() {
                    return None;
                }
                let args = stack.split_off(stack.len().checked_sub(usize::from(*argc))?);
                call(&mut stack, &mut spaces, lead, name, &args);
                2
            },
            0x22 => {
                let argc = usize::from(*body.first()? & 0x7F);
                let tab = u16_at(body, 1)?;
                // Macro-sheet commands have no worksheet formula text.
                if tab & 0x8000 != 0 {
                    return None;
                }
                let mut args = stack.split_off(stack.len().checked_sub(argc)?);
                if tab == 255 {
                    // A user-defined or add-in function: its name is the first argument.
                    if args.is_empty() {
                        return None;
                    }
                    let name = args.remove(0);
                    call(&mut stack, &mut spaces, lead, name.trim(), &args);
                } else {
                    let name = FUNCTIONS.get(usize::from(tab))?.0;
                    if name.is_empty() {
                        return None;
                    }
                    call(&mut stack, &mut spaces, lead, name, &args);
                }
                3
            },
            0x23 => {
                let name = links.names.get(usize::try_from(u32_at(body, 0)?).ok()?.checked_sub(1)?)?;
                if name.name.is_empty() {
                    return None;
                }
                // A sheet-level name of another sheet needs that sheet in front.
                let prefix = match name.itab.checked_sub(1).map(usize::from) {
                    Some(own) if Some(own) != at.sheet => links.sheet_name(own)?,
                    _ => String::new(),
                };
                stack.push(format!("{lead}{prefix}{}", name.name));
                4
            },
            0x24 | 0x2C => {
                let cell = Ref::loc(at, u16_at(body, 0)?, u16_at(body, 2)?, kind == 0x2C);
                stack.push(format!("{lead}{}", cell.text()));
                4
            },
            0x25 | 0x2D => {
                let offsets = kind == 0x2D;
                let first = Ref::loc(at, u16_at(body, 0)?, u16_at(body, 4)?, offsets);
                let last = Ref::loc(at, u16_at(body, 2)?, u16_at(body, 6)?, offsets);
                stack.push(format!("{lead}{}", area(first, last)));
                8
            },
            // PtgMemArea, PtgMemErr, PtgMemNoMem: a precomputed reference; the tokens that compute it follow.
            0x26..=0x28 => {
                spaces.next.insert_str(0, &lead);
                if kind == 0x26 {
                    // PtgExtraMem: a count of Ref8U rectangles.
                    let count = usize::from(u16_at(extra, 0)?);
                    extra = extra.get(2 + count * 8..)?;
                }
                6
            },
            // PtgMemFunc, PtgMemAreaN, PtgMemNoMemN: the same without the cached reference.
            0x29 | 0x2E | 0x2F => {
                spaces.next.insert_str(0, &lead);
                2
            },
            0x2A => {
                stack.push(format!("{lead}#REF!"));
                4
            },
            0x2B => {
                stack.push(format!("{lead}#REF!"));
                8
            },
            0x39 => {
                let &(book, ..) = links.xti.get(usize::from(u16_at(body, 0)?))?;
                let index = usize::try_from(u32_at(body, 2)?).ok()?.checked_sub(1)?;
                let name = match links.books.get(usize::from(book))?.kind {
                    // In this workbook the index counts NAME records.
                    BookKind::Own => links.names.get(index)?.name.clone(),
                    BookKind::AddIn => links.books[usize::from(book)].names.get(index)?.clone(),
                    BookKind::External => return None,
                };
                if name.is_empty() {
                    return None;
                }
                stack.push(format!("{lead}{name}"));
                6
            },
            0x3A => {
                let cell = Ref::loc(at, u16_at(body, 2)?, u16_at(body, 4)?, at.relative);
                let text = match links.sheet_prefix(u16_at(body, 0)?)? {
                    Sheets::Named(prefix) => format!("{prefix}{}", cell.text()),
                    Sheets::Deleted => "#REF!".into(),
                };
                stack.push(format!("{lead}{text}"));
                6
            },
            0x3B => {
                let first = Ref::loc(at, u16_at(body, 2)?, u16_at(body, 6)?, at.relative);
                let last = Ref::loc(at, u16_at(body, 4)?, u16_at(body, 8)?, at.relative);
                let text = match links.sheet_prefix(u16_at(body, 0)?)? {
                    Sheets::Named(prefix) => format!("{prefix}{}", area(first, last)),
                    Sheets::Deleted => "#REF!".into(),
                };
                stack.push(format!("{lead}{text}"));
                10
            },
            0x3C | 0x3D => {
                let text = match links.sheet_prefix(u16_at(body, 0)?)? {
                    Sheets::Named(prefix) => format!("{prefix}#REF!"),
                    Sheets::Deleted => "#REF!".into(),
                };
                stack.push(format!("{lead}{text}"));
                if kind == 0x3C { 6 } else { 10 }
            },
            // PtgExp and PtgTbl (resolved by the caller), PtgElf and other macro-only tokens.
            _ => return None,
        };
        pos += 1 + size;
        if pos > rgce.len() {
            return None;
        }
    }
    if stack.len() == 1 { stack.pop() } else { None }
}

/// Pushes `name(args)` with the whitespace Excel stored around it.
fn call(stack: &mut Vec<String>, spaces: &mut Spaces, lead: String, name: &str, args: &[String]) {
    let (open, close) = (std::mem::take(&mut spaces.open), std::mem::take(&mut spaces.close));
    stack.push(format!("{lead}{name}{open}({}{close})", args.join(",")));
}

/// A cell reference as tokens store it.
#[derive(Clone, Copy)]
struct Ref {
    row: u32,
    col: u32,
    row_abs: bool,
    col_abs: bool,
}

impl Ref {
    /// `RgceLoc`/`RgceLocRel`: the column word carries the relative flags. With `offsets`, relative parts
    /// count from the origin and wrap around the 65536 x 256 BIFF8 grid.
    fn loc(at: Origin, rw: u16, col: u16, offsets: bool) -> Self {
        // fColRelative is bit 14, fRwRelative bit 15.
        let (col_rel, row_rel) = (col & 0x4000 != 0, col & 0x8000 != 0);
        let row = if offsets && row_rel { at.row.wrapping_add(rw) } else { rw };
        let col = if offsets && col_rel { u16::from((at.col as u8).wrapping_add(col as u8)) } else { col & 0x3FFF };
        Ref { row: row.into(), col: col.into(), row_abs: !row_rel, col_abs: !col_rel }
    }

    fn text(self) -> String {
        format!("{}{}{}", self.col_text(), if self.row_abs { "$" } else { "" }, self.row + 1)
    }

    fn col_text(self) -> String {
        format!("{}{}", if self.col_abs { "$" } else { "" }, col_name(self.col))
    }

    fn row_text(self) -> String {
        format!("{}{}", if self.row_abs { "$" } else { "" }, self.row + 1)
    }
}

/// `A1:B2`; whole rows as `1:2` and whole columns as `A:B`, as Excel shows them.
fn area(first: Ref, last: Ref) -> String {
    if first.col == 0 && last.col == 0xFF {
        format!("{}:{}", first.row_text(), last.row_text())
    } else if first.row == 0 && last.row == 0xFFFF {
        format!("{}:{}", first.col_text(), last.col_text())
    } else {
        format!("{}:{}", first.text(), last.text())
    }
}

/// An array constant (`PtgExtraArray`): `{1,2;3,4}`, and the bytes it used.
fn array(extra: &[u8]) -> Option<(String, usize)> {
    let cols = usize::from(*extra.first()?) + 1;
    let rows = usize::from(u16_at(extra, 1)?) + 1;
    let mut at = 3;
    let mut text = String::from("{");
    for r in 0..rows {
        if r > 0 {
            text.push(';');
        }
        for c in 0..cols {
            if c > 0 {
                text.push(',');
            }
            let kind = *extra.get(at)?;
            match kind {
                0x01 => text.push_str(&number(f64::from_le_bytes(extra.get(at + 1..at + 9)?.try_into().ok()?))),
                0x02 => {
                    let (s, end) = biff_string(extra, at + 1, true)?;
                    text.push_str(&format!("\"{}\"", s.replace('"', "\"\"")));
                    at = end;
                    continue;
                },
                0x04 => text.push_str(if *extra.get(at + 1)? != 0 { "TRUE" } else { "FALSE" }),
                0x10 => text.push_str(error(*extra.get(at + 1)?)?),
                _ => {},
            }
            at += 9;
        }
    }
    text.push('}');
    Some((text, at))
}

/// An error code ([MS-XLS] §2.5.97 BErr) as formula text.
pub(super) fn error(code: u8) -> Option<&'static str> {
    Some(match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0F => "#VALUE!",
        0x17 => "#REF!",
        0x1D => "#NAME?",
        0x24 => "#NUM!",
        0x2A => "#N/A",
        0x2B => "#GETTING_DATA",
        _ => return None,
    })
}

/// A number as formula text: `1.5`, `1E+20`.
fn number(n: f64) -> String {
    let a = n.abs();
    if a != 0.0 && !(1e-5..1e15).contains(&a) {
        let s = format!("{n:E}");
        match s.split_once('E') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}E+{e}"),
            _ => s,
        }
    } else {
        format!("{n}")
    }
}

/// BIFF8 built-in functions by `iftab` ([MS-XLS] §2.5.198.17 Ftab): name and the argument count a fixed-count
/// call (`PtgFunc`) pops; variable-count functions only appear in `PtgFuncVar`, which carries its own count.
/// An empty name is an unused index; 255 is the user-defined call.
const FUNCTIONS: [(&str, u8); 384] = [
    ("COUNT", 0), ("IF", 0), ("ISNA", 1), ("ISERROR", 1), ("SUM", 0), ("AVERAGE", 0), ("MIN", 0), ("MAX", 0),
    ("ROW", 0), ("COLUMN", 0), ("NA", 0), ("NPV", 0), ("STDEV", 0), ("DOLLAR", 0), ("FIXED", 0), ("SIN", 1),
    ("COS", 1), ("TAN", 1), ("ATAN", 1), ("PI", 0), ("SQRT", 1), ("EXP", 1), ("LN", 1), ("LOG10", 1), ("ABS", 1),
    ("INT", 1), ("SIGN", 1), ("ROUND", 2), ("LOOKUP", 0), ("INDEX", 0), ("REPT", 2), ("MID", 3), ("LEN", 1),
    ("VALUE", 1), ("TRUE", 0), ("FALSE", 0), ("AND", 0), ("OR", 0), ("NOT", 1), ("MOD", 2), ("DCOUNT", 3),
    ("DSUM", 3), ("DAVERAGE", 3), ("DMIN", 3), ("DMAX", 3), ("DSTDEV", 3), ("VAR", 0), ("DVAR", 3), ("TEXT", 2),
    ("LINEST", 0), ("TREND", 0), ("LOGEST", 0), ("GROWTH", 0), ("GOTO", 1), ("HALT", 0), ("RETURN", 0), ("PV", 0),
    ("FV", 0), ("NPER", 0), ("PMT", 0), ("RATE", 0), ("MIRR", 3), ("IRR", 0), ("RAND", 0), ("MATCH", 0), ("DATE", 3),
    ("TIME", 3), ("DAY", 1), ("MONTH", 1), ("YEAR", 1), ("WEEKDAY", 1), ("HOUR", 1), ("MINUTE", 1), ("SECOND", 1),
    ("NOW", 0), ("AREAS", 1), ("ROWS", 1), ("COLUMNS", 1), ("OFFSET", 0), ("ABSREF", 2), ("RELREF", 2),
    ("ARGUMENT", 0), ("SEARCH", 0), ("TRANSPOSE", 1), ("ERROR", 0), ("STEP", 0), ("TYPE", 1), ("ECHO", 0),
    ("SET.NAME", 0), ("CALLER", 0), ("DEREF", 1), ("WINDOWS", 0), ("SERIES", 0), ("DOCUMENTS", 0), ("ACTIVE.CELL", 0),
    ("SELECTION", 0), ("RESULT", 0), ("ATAN2", 2), ("ASIN", 1), ("ACOS", 1), ("CHOOSE", 0), ("HLOOKUP", 3),
    ("VLOOKUP", 3), ("LINKS", 0), ("INPUT", 0), ("ISREF", 1), ("GET.FORMULA", 1), ("GET.NAME", 0), ("SET.VALUE", 2),
    ("LOG", 0), ("EXEC", 0), ("CHAR", 1), ("LOWER", 1), ("UPPER", 1), ("PROPER", 1), ("LEFT", 0), ("RIGHT", 0),
    ("EXACT", 2), ("TRIM", 1), ("REPLACE", 4), ("SUBSTITUTE", 0), ("CODE", 1), ("NAMES", 0), ("DIRECTORY", 0),
    ("FIND", 0), ("CELL", 0), ("ISERR", 1), ("ISTEXT", 1), ("ISNUMBER", 1), ("ISBLANK", 1), ("T", 1), ("N", 1),
    ("FOPEN", 0), ("FCLOSE", 1), ("FSIZE", 1), ("FREADLN", 1), ("FREAD", 2), ("FWRITELN", 2), ("FWRITE", 2),
    ("FPOS", 0), ("DATEVALUE", 1), ("TIMEVALUE", 1), ("SLN", 3), ("SYD", 4), ("DDB", 4), ("GET.DEF", 0),
    ("REFTEXT", 0), ("TEXTREF", 0), ("INDIRECT", 0), ("REGISTER", 0), ("CALL", 0), ("ADD.BAR", 0), ("ADD.MENU", 0),
    ("ADD.COMMAND", 0), ("ENABLE.COMMAND", 0), ("CHECK.COMMAND", 0), ("RENAME.COMMAND", 0), ("SHOW.BAR", 0),
    ("DELETE.MENU", 0), ("DELETE.COMMAND", 0), ("GET.CHART.ITEM", 0), ("DIALOG.BOX", 1), ("CLEAN", 1), ("MDETERM", 1),
    ("MINVERSE", 1), ("MMULT", 2), ("FILES", 0), ("IPMT", 0), ("PPMT", 0), ("COUNTA", 0), ("CANCEL.KEY", 0),
    ("FOR", 0), ("WHILE", 1), ("BREAK", 0), ("NEXT", 0), ("INITIATE", 2), ("REQUEST", 2), ("POKE", 3), ("EXECUTE", 2),
    ("TERMINATE", 1), ("RESTART", 0), ("HELP", 0), ("GET.BAR", 0), ("PRODUCT", 0), ("FACT", 1), ("GET.CELL", 0),
    ("GET.WORKSPACE", 1), ("GET.WINDOW", 0), ("GET.DOCUMENT", 0), ("DPRODUCT", 3), ("ISNONTEXT", 1), ("GET.NOTE", 0),
    ("NOTE", 0), ("STDEVP", 0), ("VARP", 0), ("DSTDEVP", 3), ("DVARP", 3), ("TRUNC", 1), ("ISLOGICAL", 1),
    ("DCOUNTA", 3), ("DELETE.BAR", 0), ("UNREGISTER", 1), ("", 0), ("", 0), ("USDOLLAR", 0), ("FINDB", 0),
    ("SEARCHB", 0), ("REPLACEB", 4), ("LEFTB", 0), ("RIGHTB", 0), ("MIDB", 3), ("LENB", 1), ("ROUNDUP", 2),
    ("ROUNDDOWN", 2), ("ASC", 1), ("DBCS", 1), ("RANK", 0), ("", 0), ("", 0), ("ADDRESS", 0), ("DAYS360", 0),
    ("TODAY", 0), ("VDB", 0), ("ELSE", 0), ("ELSE.IF", 0), ("END.IF", 0), ("FOR.CELL", 0), ("MEDIAN", 0),
    ("SUMPRODUCT", 0), ("SINH", 1), ("COSH", 1), ("TANH", 1), ("ASINH", 1), ("ACOSH", 1), ("ATANH", 1), ("DGET", 3),
    ("CREATE.OBJECT", 0), ("VOLATILE", 0), ("LAST.ERROR", 0), ("CUSTOM.UNDO", 0), ("CUSTOM.REPEAT", 0),
    ("FORMULA.CONVERT", 0), ("GET.LINK.INFO", 0), ("TEXT.BOX", 0), ("INFO", 1), ("GROUP", 0), ("GET.OBJECT", 0),
    ("DB", 4), ("PAUSE", 0), ("", 0), ("", 0), ("RESUME", 0), ("FREQUENCY", 2), ("ADD.TOOLBAR", 0),
    ("DELETE.TOOLBAR", 0), ("", 0), ("RESET.TOOLBAR", 0), ("EVALUATE", 1), ("GET.TOOLBAR", 0), ("GET.TOOL", 0),
    ("SPELLING.CHECK", 0), ("ERROR.TYPE", 1), ("APP.TITLE", 0), ("WINDOW.TITLE", 0), ("SAVE.TOOLBAR", 0),
    ("ENABLE.TOOL", 0), ("PRESS.TOOL", 0), ("REGISTER.ID", 0), ("GET.WORKBOOK", 0), ("AVEDEV", 0), ("BETADIST", 0),
    ("GAMMALN", 1), ("BETAINV", 0), ("BINOMDIST", 4), ("CHIDIST", 2), ("CHIINV", 2), ("COMBIN", 2), ("CONFIDENCE", 3),
    ("CRITBINOM", 3), ("EVEN", 1), ("EXPONDIST", 3), ("FDIST", 3), ("FINV", 3), ("FISHER", 1), ("FISHERINV", 1),
    ("FLOOR", 2), ("GAMMADIST", 4), ("GAMMAINV", 3), ("CEILING", 2), ("HYPGEOMDIST", 4), ("LOGNORMDIST", 3),
    ("LOGINV", 3), ("NEGBINOMDIST", 3), ("NORMDIST", 4), ("NORMSDIST", 1), ("NORMINV", 3), ("NORMSINV", 1),
    ("STANDARDIZE", 3), ("ODD", 1), ("PERMUT", 2), ("POISSON", 3), ("TDIST", 3), ("WEIBULL", 4), ("SUMXMY2", 2),
    ("SUMX2MY2", 2), ("SUMX2PY2", 2), ("CHITEST", 2), ("CORREL", 2), ("COVAR", 2), ("FORECAST", 3), ("FTEST", 2),
    ("INTERCEPT", 2), ("PEARSON", 2), ("RSQ", 2), ("STEYX", 2), ("SLOPE", 2), ("TTEST", 4), ("PROB", 0), ("DEVSQ", 0),
    ("GEOMEAN", 0), ("HARMEAN", 0), ("SUMSQ", 0), ("KURT", 0), ("SKEW", 0), ("ZTEST", 0), ("LARGE", 2), ("SMALL", 2),
    ("QUARTILE", 2), ("PERCENTILE", 2), ("PERCENTRANK", 0), ("MODE", 0), ("TRIMMEAN", 2), ("TINV", 2), ("", 0),
    ("MOVIE.COMMAND", 0), ("GET.MOVIE", 0), ("CONCATENATE", 0), ("POWER", 2), ("PIVOT.ADD.DATA", 0),
    ("GET.PIVOT.TABLE", 0), ("GET.PIVOT.FIELD", 0), ("GET.PIVOT.ITEM", 0), ("RADIANS", 1), ("DEGREES", 1),
    ("SUBTOTAL", 0), ("SUMIF", 0), ("COUNTIF", 2), ("COUNTBLANK", 1), ("SCENARIO.GET", 0), ("OPTIONS.LISTS.GET", 0),
    ("ISPMT", 4), ("DATEDIF", 3), ("DATESTRING", 1), ("NUMBERSTRING", 2), ("ROMAN", 0), ("OPEN.DIALOG", 0),
    ("SAVE.DIALOG", 0), ("VIEW.GET", 0), ("GETPIVOTDATA", 0), ("HYPERLINK", 0), ("PHONETIC", 1), ("AVERAGEA", 0),
    ("MAXA", 0), ("MINA", 0), ("STDEVPA", 0), ("VARPA", 0), ("STDEVA", 0), ("VARA", 0), ("BAHTTEXT", 1),
    ("THAIDAYOFWEEK", 1), ("THAIDIGIT", 1), ("THAIMONTHOFYEAR", 1), ("THAINUMSOUND", 1), ("THAINUMSTRING", 1),
    ("THAISTRINGLENGTH", 1), ("ISTHAIDIGIT", 1), ("ROUNDBAHTDOWN", 1), ("ROUNDBAHTUP", 1), ("THAIYEAR", 1),
    ("RTD", 0), ("CUBEVALUE", 0), ("CUBEMEMBER", 0), ("CUBEMEMBERPROPERTY", 3), ("CUBERANKEDMEMBER", 0),
];
