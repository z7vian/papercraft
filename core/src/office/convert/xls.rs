//! Excel 97-2003 (BIFF8) workbooks as SpreadsheetML. office_oxide reads the cell values, comments, links
//! and names; this module reads the formatting records its model drops (fonts, number formats, cell
//! formats, column widths, row heights, merged cells, panes, pictures) and writes the package itself.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Cursor;

use office_oxide::DocumentFormat;
use office_oxide::cfb::CfbReader;
use office_oxide::core::opc::{OpcWriter, PartName};
use office_oxide::core::relationships::{TargetMode, rel_types};
use office_oxide::xls::{CellValue, Sheet, XlsDocument};

use super::art::{self, Picture};
use super::{esc, via_model};
use crate::CoreError;
use crate::office::bad;

pub(super) fn to_xlsx(bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    // Limit: Excel 5.0/95 (BIFF5) and encrypted workbooks keep values only (office_oxide's model); reading their FONT/XF layouts lifts it.
    let Some(layout) = Layout::read(&bytes) else {
        return via_model(bytes, DocumentFormat::Xls);
    };
    let book = XlsDocument::from_reader(Cursor::new(&bytes[..])).map_err(|e| bad("file", e))?;
    // office_oxide stops at its record budget; a workbook it cut short keeps its own reading.
    if book.sheets.len() != layout.sheets.len() {
        return via_model(bytes, DocumentFormat::Xls);
    }
    write(&book, &layout).map_err(|e| bad("file", e))
}

// BIFF8 record types ([MS-XLS] §2.3).
const BOF: u16 = 0x0809;
const EOF: u16 = 0x000A;
const CONTINUE: u16 = 0x003C;
const FILEPASS: u16 = 0x002F;
const FONT: u16 = 0x0031;
const FORMAT: u16 = 0x041E;
const XF: u16 = 0x00E0;
const PALETTE: u16 = 0x0092;
const DATEMODE: u16 = 0x0022;
const MSODRAWINGGROUP: u16 = 0x00EB;
const MSODRAWING: u16 = 0x00EC;
const COLINFO: u16 = 0x007D;
const DEFCOLWIDTH: u16 = 0x0055;
const STANDARDWIDTH: u16 = 0x0099;
const DEFAULTROWHEIGHT: u16 = 0x0225;
const ROW: u16 = 0x0208;
const MERGEDCELLS: u16 = 0x00E5;
const WINDOW2: u16 = 0x023E;
const PANE: u16 = 0x0041;
const MULRK: u16 = 0x00BD;
const MULBLANK: u16 = 0x00BE;
/// Records that hold one cell and start with `rw`, `col`, `ixfe`.
const ONE_CELL: [u16; 8] = [0x0006, 0x0201, 0x0203, 0x0204, 0x0205, 0x00FD, 0x027E, 0x00D6];

/// `BOF.dt` of the substreams office_oxide turns into sheets: worksheet (dialog sheets included),
/// macro sheet, chart sheet.
const DT_WORKSHEET: u16 = 0x0010;
const DT_CHART: u16 = 0x0020;
const DT_MACRO: u16 = 0x0040;

/// Excel's default colour table for indices 8-63, what `PALETTE` overrides.
const DEFAULT_PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080,
    0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080,
    0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF,
    0xCCFFFF, 0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00,
    0xFF9900, 0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399,
    0x333333,
];
/// Indices 0-7 never change.
const FIXED_COLORS: [u32; 8] = [0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF];

const BORDER_STYLES: [&str; 14] = [
    "none",
    "thin",
    "medium",
    "dashed",
    "dotted",
    "thick",
    "double",
    "hair",
    "mediumDashed",
    "dashDot",
    "mediumDashDot",
    "dashDotDot",
    "mediumDashDotDot",
    "slantDashDot",
];
const PATTERNS: [&str; 19] = [
    "none",
    "solid",
    "mediumGray",
    "darkGray",
    "lightGray",
    "darkHorizontal",
    "darkVertical",
    "darkDown",
    "darkUp",
    "darkGrid",
    "darkTrellis",
    "lightHorizontal",
    "lightVertical",
    "lightDown",
    "lightUp",
    "lightGrid",
    "lightTrellis",
    "gray125",
    "gray0625",
];
const H_ALIGN: [&str; 8] = ["general", "left", "center", "right", "fill", "justify", "centerContinuous", "distributed"];
const V_ALIGN: [&str; 5] = ["top", "center", "bottom", "justify", "distributed"];

/// 96 dpi pixels to EMU.
const EMU_PER_PX: f64 = 9525.0;
/// Twips (1/20 pt) to EMU.
const EMU_PER_TWIP: f64 = 635.0;
/// Width in pixels of the default font's widest digit, the unit of column widths.
const DIGIT_PX: f64 = 7.0;

/// The formatting a BIFF8 workbook stream carries next to its values.
#[derive(Default)]
struct Layout {
    fonts: Vec<Font>,
    formats: Vec<(u16, String)>,
    xfs: Vec<[u8; 20]>,
    palette: Vec<u32>,
    date1904: bool,
    pictures: Vec<Option<Picture>>,
    sheets: Vec<SheetLayout>,
}

struct Font {
    twips: u16,
    italic: bool,
    strike: bool,
    outline: bool,
    shadow: bool,
    color: u16,
    bold: bool,
    script: u16,
    underline: u8,
    family: u8,
    charset: u8,
    name: String,
}

#[derive(Default)]
struct SheetLayout {
    /// `(first, last, width in 1/256 character, xf, flags)`.
    cols: Vec<(u16, u16, u16, u16, u16)>,
    default_chars: Option<u16>,
    standard_width: Option<u16>,
    /// Default row height in twips, and whether rows are hidden by default.
    default_row: Option<(u16, bool)>,
    /// Row -> (height in twips, flags of the ROW record).
    rows: BTreeMap<u16, (u16, u32)>,
    merges: Vec<(u16, u16, u16, u16)>,
    /// Every cell record's `(row, col, xf)`, blank cells included.
    cells: Vec<(u16, u16, u16)>,
    window: Option<u16>,
    pane: Option<(u16, u16)>,
    drawing: Vec<u8>,
}

enum State {
    Start,
    Globals,
    Between,
    Sheet(u32),
    Skip(u32),
}

impl Layout {
    fn read(bytes: &[u8]) -> Option<Self> {
        let mut cfb = CfbReader::new(Cursor::new(bytes)).ok()?;
        // BIFF5 names the stream "Book".
        let stream = cfb.open_stream("Workbook").ok()?;
        Self::parse(&stream)
    }

    /// `None` when the stream is not a readable BIFF8 workbook.
    fn parse(stream: &[u8]) -> Option<Self> {
        let mut layout = Layout { palette: DEFAULT_PALETTE.to_vec(), ..Default::default() };
        let mut group = Vec::new();
        let mut state = State::Start;
        let mut last = 0u16;
        for (kind, data) in biff_records(stream) {
            let kind_of_data = if kind == CONTINUE { last } else { kind };
            if kind != CONTINUE {
                last = kind;
            }
            state = match state {
                State::Start => {
                    // A BIFF8 globals substream: version 0x0600, type 0x0005.
                    if kind != BOF || u16_at(data, 0)? != 0x0600 || u16_at(data, 2)? != 0x0005 {
                        return None;
                    }
                    State::Globals
                },
                State::Globals => match kind_of_data {
                    FILEPASS => return None,
                    EOF => State::Between,
                    _ => {
                        layout.globals_record(kind, kind_of_data, data, &mut group);
                        State::Globals
                    },
                },
                State::Between if kind == BOF => match u16_at(data, 2) {
                    None | Some(DT_WORKSHEET | DT_MACRO) => {
                        layout.sheets.push(SheetLayout::default());
                        State::Sheet(0)
                    },
                    Some(DT_CHART) => {
                        layout.sheets.push(SheetLayout::default());
                        State::Skip(0)
                    },
                    Some(_) => State::Skip(0),
                },
                State::Between => State::Between,
                // Embedded charts are BOF..EOF substreams nested in their sheet.
                State::Sheet(depth) if kind == BOF => State::Sheet(depth + 1),
                State::Skip(depth) if kind == BOF => State::Skip(depth + 1),
                State::Sheet(0) | State::Skip(0) if kind == EOF => State::Between,
                State::Sheet(depth) if kind == EOF => State::Sheet(depth - 1),
                State::Skip(depth) if kind == EOF => State::Skip(depth - 1),
                State::Sheet(0) => {
                    layout.sheets.last_mut()?.record(kind, kind_of_data, data);
                    State::Sheet(0)
                },
                other => other,
            };
        }
        layout.pictures = art::picture_store(&group, &[]);
        Some(layout)
    }

    fn globals_record(&mut self, kind: u16, kind_of_data: u16, data: &[u8], group: &mut Vec<u8>) {
        match kind_of_data {
            FONT if kind == FONT => {
                if let Some(font) = parse_font(data) {
                    self.fonts.push(font);
                }
            },
            FORMAT if kind == FORMAT => {
                if let (Some(id), Some((code, _))) = (u16_at(data, 0), biff_string(data, 2, true)) {
                    self.formats.push((id, code));
                }
            },
            XF if kind == XF => {
                let mut xf = [0u8; 20];
                let n = data.len().min(20);
                xf[..n].copy_from_slice(&data[..n]);
                self.xfs.push(xf);
            },
            PALETTE if kind == PALETTE => {
                let count = u16_at(data, 0).unwrap_or(0) as usize;
                for (i, rgb) in data.get(2..).unwrap_or_default().chunks_exact(4).take(count.min(56)).enumerate() {
                    self.palette[i] = u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
                }
            },
            DATEMODE if kind == DATEMODE => self.date1904 = u16_at(data, 0).is_some_and(|v| v != 0),
            MSODRAWINGGROUP => group.extend_from_slice(data),
            _ => {},
        }
    }

    /// The RGB of colour index `icv`; `None` for the automatic and system colours.
    fn rgb(&self, icv: u16) -> Option<u32> {
        match icv {
            0..=7 => Some(FIXED_COLORS[icv as usize]),
            8..=63 => self.palette.get(icv as usize - 8).copied(),
            _ => None,
        }
    }
}

impl SheetLayout {
    fn record(&mut self, kind: u16, kind_of_data: u16, data: &[u8]) {
        if kind_of_data == MSODRAWING {
            self.drawing.extend_from_slice(data);
            return;
        }
        if kind == CONTINUE {
            return;
        }
        let one = |at| Some((u16_at(data, at)?, u16_at(data, at + 2)?, u16_at(data, at + 4)?));
        match kind {
            COLINFO => {
                if let (Some(first), Some(last), Some(width), Some(xf), Some(flags)) =
                    (u16_at(data, 0), u16_at(data, 2), u16_at(data, 4), u16_at(data, 6), u16_at(data, 8))
                {
                    // Excel writes 256 for "to the last column"; BIFF8 has 256 columns.
                    self.cols.push((first.min(255), last.min(255), width, xf, flags));
                }
            },
            DEFCOLWIDTH => self.default_chars = u16_at(data, 0),
            STANDARDWIDTH => self.standard_width = u16_at(data, 0),
            DEFAULTROWHEIGHT => {
                if let (Some(flags), Some(twips)) = (u16_at(data, 0), u16_at(data, 2)) {
                    self.default_row = Some((twips, flags & 0x0002 != 0));
                }
            },
            ROW => {
                if let (Some(row), Some(twips), Some(flags)) = (u16_at(data, 0), u16_at(data, 6), u32_at(data, 12)) {
                    self.rows.insert(row, (twips & 0x7FFF, flags));
                }
            },
            MERGEDCELLS => {
                let count = u16_at(data, 0).unwrap_or(0) as usize;
                for r in data.get(2..).unwrap_or_default().chunks_exact(8).take(count) {
                    let at = |i: usize| u16::from_le_bytes([r[i], r[i + 1]]);
                    self.merges.push((at(0), at(2), at(4), at(6)));
                }
            },
            WINDOW2 => self.window = u16_at(data, 0),
            PANE => self.pane = u16_at(data, 0).zip(u16_at(data, 2)),
            MULRK | MULBLANK => {
                let (Some(row), Some(first)) = (u16_at(data, 0), u16_at(data, 2)) else { return };
                let step = if kind == MULRK { 6 } else { 2 };
                let body = data.get(4..data.len().saturating_sub(2)).unwrap_or_default();
                for (i, entry) in body.chunks_exact(step).enumerate() {
                    self.cells.push((row, first.saturating_add(i as u16), u16::from_le_bytes([entry[0], entry[1]])));
                }
            },
            kind if ONE_CELL.contains(&kind) => {
                if let Some(c) = one(0) {
                    self.cells.push(c);
                }
            },
            _ => {},
        }
    }

    /// Column `col`'s width in characters, as SpreadsheetML's `width` counts it (padding included).
    fn col_width(&self, col: u16) -> f64 {
        match self.cols.iter().rev().find(|c| c.0 <= col && col <= c.1) {
            Some(&(.., flags)) if flags & 1 != 0 => 0.0,
            Some(&(_, _, width, ..)) => f64::from(width) / 256.0,
            None => self.default_width(),
        }
    }

    fn default_width(&self) -> f64 {
        match (self.standard_width, self.default_chars) {
            (Some(w), _) => f64::from(w) / 256.0,
            (None, chars) => (f64::from(chars.unwrap_or(8)) * DIGIT_PX + 5.0) / DIGIT_PX,
        }
    }

    fn col_px(&self, col: u16) -> f64 {
        (self.col_width(col) * DIGIT_PX + 0.5).trunc()
    }

    /// Row `row`'s height in twips.
    fn row_twips(&self, row: u16) -> f64 {
        let (default, hidden) = self.default_row.unwrap_or((255, false));
        match self.rows.get(&row) {
            Some(&(_, flags)) if flags & 0x20 != 0 => 0.0,
            Some(&(twips, _)) => f64::from(twips),
            None if hidden => 0.0,
            None => f64::from(default),
        }
    }

    /// The EMU offset of `(col, fraction of col in 1/1024)` from the sheet's left edge.
    fn x_emu(&self, col: u16, dx: u16) -> f64 {
        let left: f64 = (0..col).map(|c| self.col_px(c)).sum();
        (left + self.col_px(col) * f64::from(dx.min(1024)) / 1024.0) * EMU_PER_PX
    }

    /// The EMU offset of `(row, fraction of row in 1/256)` from the sheet's top edge.
    fn y_emu(&self, row: u16, dy: u16) -> f64 {
        let top: f64 = (0..row).map(|r| self.row_twips(r)).sum();
        (top + self.row_twips(row) * f64::from(dy.min(256)) / 256.0) * EMU_PER_TWIP
    }
}

/// The records of a BIFF stream: `(type, body)`.
fn biff_records(data: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let kind = u16_at(data, pos)?;
        let len = u16_at(data, pos + 2)? as usize;
        let body = data.get(pos + 4..pos + 4 + len)?;
        pos += 4 + len;
        Some((kind, body))
    })
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// A BIFF8 `XLUnicodeString` (16-bit length) or `ShortXLUnicodeString` (8-bit length) at `at`, and
/// the offset after it.
fn biff_string(data: &[u8], at: usize, long: bool) -> Option<(String, usize)> {
    let (count, flags_at) = if long { (u16_at(data, at)? as usize, at + 2) } else { (*data.get(at)? as usize, at + 1) };
    let wide = data.get(flags_at)? & 1 != 0;
    let start = flags_at + 1;
    if wide {
        let raw = data.get(start..start + count * 2)?;
        let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some((String::from_utf16_lossy(&units), start + count * 2))
    } else {
        let raw = data.get(start..start + count)?;
        Some((raw.iter().map(|&b| char::from(b)).collect(), start + count))
    }
}

/// `FONT` ([MS-XLS] §2.4.122).
fn parse_font(data: &[u8]) -> Option<Font> {
    let flags = u16_at(data, 2)?;
    Some(Font {
        twips: u16_at(data, 0)?,
        italic: flags & 0x02 != 0,
        strike: flags & 0x08 != 0,
        outline: flags & 0x10 != 0,
        shadow: flags & 0x20 != 0,
        color: u16_at(data, 4)?,
        bold: u16_at(data, 6)? >= 700,
        script: u16_at(data, 8)?,
        underline: *data.get(10)?,
        family: *data.get(11)?,
        charset: *data.get(12)?,
        name: biff_string(data, 14, false)?.0,
    })
}

macro_rules! w {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
    }};
}

const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT_WORKBOOK: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const CT_WORKSHEET: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const CT_STYLES: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";
const CT_STRINGS: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
const CT_DRAWING: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
const CT_COMMENTS: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml";
const CT_VML: &str = "application/vnd.openxmlformats-officedocument.vmlDrawing";

type Opc = OpcWriter<Cursor<Vec<u8>>>;

fn part(name: &str) -> office_oxide::core::Result<PartName> {
    PartName::new(name)
}

fn write(book: &XlsDocument, layout: &Layout) -> office_oxide::core::Result<Vec<u8>> {
    let mut opc = OpcWriter::new(Cursor::new(Vec::new()))?;
    let workbook = part("/xl/workbook.xml")?;
    opc.add_package_rel(rel_types::OFFICE_DOCUMENT, "xl/workbook.xml");
    let mut strings = Strings::default();
    let mut media: HashMap<u32, String> = HashMap::new();

    let mut sheets_xml = String::new();
    for (i, (sheet, sheet_layout)) in book.sheets.iter().zip(&layout.sheets).enumerate() {
        let n = i + 1;
        let target = format!("worksheets/sheet{n}.xml");
        let rid = opc.add_part_rel(&workbook, rel_types::WORKSHEET, &target);
        let name = part(&format!("/xl/{target}"))?;
        let xml = worksheet(&mut opc, &name, n, sheet, sheet_layout, layout, &mut strings, &mut media)?;
        opc.add_part(&name, CT_WORKSHEET, xml.as_bytes())?;
        w!(sheets_xml, r#"<sheet name="{}" sheetId="{n}" r:id="{rid}""#, esc(&sheet.name));
        if sheet.hidden {
            sheets_xml.push_str(r#" state="hidden""#);
        }
        sheets_xml.push_str("/>");
    }
    if book.sheets.is_empty() {
        // A workbook needs a sheet.
        let rid = opc.add_part_rel(&workbook, rel_types::WORKSHEET, "worksheets/sheet1.xml");
        let xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="{NS_MAIN}"><sheetData/></worksheet>"#);
        opc.add_part(&part("/xl/worksheets/sheet1.xml")?, CT_WORKSHEET, xml.as_bytes())?;
        w!(sheets_xml, r#"<sheet name="Sheet1" sheetId="1" r:id="{rid}"/>"#);
    }

    let mut xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_REL}">"#);
    if layout.date1904 {
        xml.push_str(r#"<workbookPr date1904="1"/>"#);
    }
    w!(xml, r#"<bookViews><workbookView/></bookViews><sheets>{sheets_xml}</sheets>"#);
    let names: Vec<_> = book.defined_names.iter().filter(|n| !n.value.is_empty()).collect();
    if !names.is_empty() {
        xml.push_str("<definedNames>");
        for name in names {
            w!(xml, r#"<definedName name="{}""#, esc(&name.name));
            if let Some(sheet) = name.local_sheet_id {
                w!(xml, r#" localSheetId="{sheet}""#);
            }
            if name.hidden {
                xml.push_str(r#" hidden="1""#);
            }
            w!(xml, ">{}</definedName>", esc(&name.value));
        }
        xml.push_str("</definedNames>");
    }
    xml.push_str("</workbook>");
    opc.add_part(&workbook, CT_WORKBOOK, xml.as_bytes())?;

    opc.add_part_rel(&workbook, rel_types::STYLES, "styles.xml");
    opc.add_part(&part("/xl/styles.xml")?, CT_STYLES, styles(layout).as_bytes())?;
    opc.add_part_rel(&workbook, rel_types::SHARED_STRINGS, "sharedStrings.xml");
    opc.add_part(&part("/xl/sharedStrings.xml")?, CT_STRINGS, strings.xml().as_bytes())?;
    Ok(opc.finish()?.into_inner())
}

/// The shared string table, each text once.
#[derive(Default)]
struct Strings {
    index: HashMap<String, usize>,
    list: Vec<String>,
    uses: usize,
}

impl Strings {
    fn add(&mut self, s: &str) -> usize {
        self.uses += 1;
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        self.list.push(s.to_string());
        self.index.insert(s.to_string(), self.list.len() - 1);
        self.list.len() - 1
    }

    fn xml(&self) -> String {
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="{NS_MAIN}" count="{}" uniqueCount="{}">"#,
            self.uses,
            self.list.len()
        );
        for s in &self.list {
            w!(xml, r#"<si><t xml:space="preserve">{}</t></si>"#, esc(s));
        }
        xml.push_str("</sst>");
        xml
    }
}

/// `A`, `B`, ..., `AA` for 0-based column `col`.
fn col_name(col: u32) -> String {
    let mut name = Vec::new();
    let mut n = col + 1;
    while n > 0 {
        name.push(b'A' + ((n - 1) % 26) as u8);
        n = (n - 1) / 26;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

fn cell_ref(row: u32, col: u32) -> String {
    format!("{}{}", col_name(col), row + 1)
}

/// A number as SpreadsheetML writes it.
fn number(n: f64) -> String {
    let a = n.abs();
    if a != 0.0 && !(1e-5..1e15).contains(&a) { format!("{n:e}") } else { format!("{n}") }
}

/// Points from twips, without trailing zeros.
fn points(twips: f64) -> String {
    number((twips / 20.0 * 100.0).round() / 100.0)
}

#[allow(clippy::too_many_arguments)]
fn worksheet(
    opc: &mut Opc,
    name: &PartName,
    n: usize,
    sheet: &Sheet,
    sl: &SheetLayout,
    layout: &Layout,
    strings: &mut Strings,
    media: &mut HashMap<u32, String>,
) -> office_oxide::core::Result<String> {
    let mut xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_REL}">"#);

    // View: gridlines, right-to-left, frozen panes.
    let window = sl.window.unwrap_or(0x02B6);
    xml.push_str(r#"<sheetViews><sheetView workbookViewId="0""#);
    if window & 0x0002 == 0 {
        xml.push_str(r#" showGridLines="0""#);
    }
    if window & 0x0040 != 0 {
        xml.push_str(r#" rightToLeft="1""#);
    }
    match sl.pane.filter(|&(x, y)| window & 0x0008 != 0 && (x > 0 || y > 0)) {
        Some((x, y)) => {
            let active = match (x > 0, y > 0) {
                (true, true) => "bottomRight",
                (true, false) => "topRight",
                _ => "bottomLeft",
            };
            xml.push('>');
            if x > 0 {
                w!(xml, r#"<pane xSplit="{x}""#);
            } else {
                xml.push_str("<pane");
            }
            if y > 0 {
                w!(xml, r#" ySplit="{y}""#);
            }
            w!(xml, r#" topLeftCell="{}" activePane="{active}" state="frozen"/></sheetView></sheetViews>"#, cell_ref(y.into(), x.into()));
        },
        None => xml.push_str("/></sheetViews>"),
    }

    // Default sizes.
    let (default_twips, default_hidden) = sl.default_row.unwrap_or((255, false));
    w!(xml, r#"<sheetFormatPr baseColWidth="{}""#, sl.default_chars.unwrap_or(8));
    if let Some(width) = sl.standard_width {
        w!(xml, r#" defaultColWidth="{}""#, number(f64::from(width) / 256.0));
    }
    w!(xml, r#" defaultRowHeight="{}""#, points(f64::from(default_twips)));
    if default_hidden {
        xml.push_str(r#" zeroHeight="1""#);
    }
    xml.push_str("/>");

    // Column widths and formats.
    if !sl.cols.is_empty() {
        xml.push_str("<cols>");
        let mut cols = sl.cols.clone();
        cols.sort_by_key(|c| c.0);
        let mut next = 0u16;
        for (first, last, width, xf, flags) in cols {
            // Ranges must not overlap; a later duplicate loses its covered start.
            let first = first.max(next);
            if first > last {
                continue;
            }
            next = last.saturating_add(1);
            w!(xml, r#"<col min="{}" max="{}" width="{}" customWidth="1""#, first + 1, last + 1, number(f64::from(width) / 256.0));
            if xf != 0 && (xf as usize) < layout.xfs.len() {
                w!(xml, r#" style="{xf}""#);
            }
            if flags & 0x0001 != 0 {
                xml.push_str(r#" hidden="1""#);
            }
            let level = (flags >> 8) & 7;
            if level > 0 {
                w!(xml, r#" outlineLevel="{level}""#);
            }
            if flags & 0x1000 != 0 {
                xml.push_str(r#" collapsed="1""#);
            }
            xml.push_str("/>");
        }
        xml.push_str("</cols>");
    }

    // Cells, row by row, with each row's height and format.
    let mut cells = sl.cells.clone();
    cells.sort_by_key(|&(r, c, _)| (r, c));
    cells.dedup_by(|later, earlier| {
        let same = later.0 == earlier.0 && later.1 == earlier.1;
        if same {
            earlier.2 = later.2;
        }
        same
    });
    let mut rows: BTreeMap<u16, (usize, usize)> = BTreeMap::new();
    let mut start = 0;
    for (i, cell) in cells.iter().enumerate() {
        if i + 1 == cells.len() || cells[i + 1].0 != cell.0 {
            rows.insert(cell.0, (start, i + 1));
            start = i + 1;
        }
    }
    for &row in sl.rows.keys() {
        rows.entry(row).or_insert((0, 0));
    }
    let chart_rows: Vec<&String> = sheet.chart_text.iter().collect();
    xml.push_str("<sheetData>");
    for (&row, &(from, to)) in &rows {
        w!(xml, r#"<row r="{}""#, u32::from(row) + 1);
        if let Some(&(twips, flags)) = sl.rows.get(&row) {
            w!(xml, r#" ht="{}""#, points(f64::from(twips)));
            if flags & 0x40 != 0 {
                xml.push_str(r#" customHeight="1""#);
            }
            if flags & 0x20 != 0 {
                xml.push_str(r#" hidden="1""#);
            }
            let level = flags & 7;
            if level > 0 {
                w!(xml, r#" outlineLevel="{level}""#);
            }
            if flags & 0x10 != 0 {
                xml.push_str(r#" collapsed="1""#);
            }
            let xf = (flags >> 16) & 0x0FFF;
            if flags & 0x80 != 0 && (xf as usize) < layout.xfs.len() {
                w!(xml, r#" s="{xf}" customFormat="1""#);
            }
        }
        xml.push('>');
        for &(_, col, xf) in &cells[from..to] {
            let value = sheet.rows.get(row as usize).and_then(|r| r.get(col as usize));
            cell(&mut xml, row, col, xf, value, layout, strings);
        }
        xml.push_str("</row>");
    }
    // A chart sheet has no cells; its text (titles, series names) stays readable in column A.
    for (i, text) in chart_rows.iter().enumerate() {
        let r = i as u32 + 1;
        w!(xml, r#"<row r="{r}"><c r="A{r}" t="s"><v>{}</v></c></row>"#, strings.add(text.trim()));
    }
    xml.push_str("</sheetData>");

    if !sl.merges.is_empty() {
        w!(xml, r#"<mergeCells count="{}">"#, sl.merges.len());
        for &(r1, r2, c1, c2) in &sl.merges {
            w!(xml, r#"<mergeCell ref="{}:{}"/>"#, cell_ref(r1.into(), c1.into()), cell_ref(r2.into(), c2.into()));
        }
        xml.push_str("</mergeCells>");
    }

    if !sheet.hyperlinks.is_empty() {
        xml.push_str("<hyperlinks>");
        for link in &sheet.hyperlinks {
            let range = if (link.row_first, link.col_first) == (link.row_last, link.col_last) {
                cell_ref(link.row_first.into(), link.col_first.into())
            } else {
                format!(
                    "{}:{}",
                    cell_ref(link.row_first.into(), link.col_first.into()),
                    cell_ref(link.row_last.into(), link.col_last.into())
                )
            };
            match link.target.strip_prefix('#') {
                Some(place) => w!(xml, r#"<hyperlink ref="{range}" location="{}"/>"#, esc(place)),
                None => {
                    let rid = opc.add_part_rel_with_mode(name, rel_types::HYPERLINK, &link.target, TargetMode::External);
                    w!(xml, r#"<hyperlink ref="{range}" r:id="{rid}"/>"#);
                },
            }
        }
        xml.push_str("</hyperlinks>");
    }

    let drawing = pictures(opc, name, n, sl, layout, media)?;
    if let Some(rid) = drawing {
        w!(xml, r#"<drawing r:id="{rid}"/>"#);
    }
    if !sheet.comments.is_empty() {
        let rid = comments(opc, name, n, sheet)?;
        w!(xml, r#"<legacyDrawing r:id="{rid}"/>"#);
    }
    xml.push_str("</worksheet>");
    Ok(xml)
}

fn cell(xml: &mut String, row: u16, col: u16, xf: u16, value: Option<&CellValue>, layout: &Layout, strings: &mut Strings) {
    w!(xml, r#"<c r="{}""#, cell_ref(row.into(), col.into()));
    if xf != 0 && (xf as usize) < layout.xfs.len() {
        w!(xml, r#" s="{xf}""#);
    }
    match value {
        Some(CellValue::Number(v)) if v.is_finite() => w!(xml, "><v>{}</v></c>", number(*v)),
        Some(CellValue::String(s)) => w!(xml, r#" t="s"><v>{}</v></c>"#, strings.add(s)),
        Some(CellValue::Bool(b)) => w!(xml, r#" t="b"><v>{}</v></c>"#, u8::from(*b)),
        Some(err @ CellValue::Error(_)) => w!(xml, r#" t="e"><v>{}</v></c>"#, esc(&err.as_text_cow())),
        _ => xml.push_str("/>"),
    }
}

/// The sheet's pictures as a drawing part; the relationship id for `<drawing>`, or `None` when it has none.
fn pictures(
    opc: &mut Opc,
    sheet: &PartName,
    n: usize,
    sl: &SheetLayout,
    layout: &Layout,
    media: &mut HashMap<u32, String>,
) -> office_oxide::core::Result<Option<String>> {
    let shapes = art::picture_shapes(&sl.drawing);
    let drawing = part(&format!("/xl/drawings/drawing{n}.xml"))?;
    let mut anchors = String::new();
    let mut count = 0u32;
    for shape in shapes {
        // OfficeArtClientAnchorSheet: flags, then col/dx/row/dy of the top-left and bottom-right corners.
        let Some(a) = shape.anchor.filter(|a| a.len() >= 18) else { continue };
        let Some(Some(picture)) = layout.pictures.get((shape.pib as usize).wrapping_sub(1)) else { continue };
        let at = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]);
        let (flags, c1, dx1, r1, dy1, c2, dx2, r2, dy2) = (at(0), at(2), at(4), at(6), at(8), at(10), at(12), at(14), at(16));
        let target = match media.get(&shape.pib) {
            Some(t) => t.clone(),
            None => {
                let (ext, content_type) = picture.media();
                let file = format!("media/image{}.{ext}", media.len() + 1);
                opc.register_default_content_type(ext, content_type);
                opc.add_part(&part(&format!("/xl/{file}"))?, content_type, &picture.data)?;
                media.insert(shape.pib, file.clone());
                file
            },
        };
        let rid = opc.add_part_rel(&drawing, rel_types::IMAGE, &format!("../{target}"));
        let (x1, y1) = (sl.x_emu(c1, dx1), sl.y_emu(r1, dy1));
        let (x2, y2) = (sl.x_emu(c2, dx2), sl.y_emu(r2, dy2));
        let col_off = |c: u16, dx: u16| (sl.col_px(c) * f64::from(dx.min(1024)) / 1024.0 * EMU_PER_PX) as i64;
        let row_off = |r: u16, dy: u16| (sl.row_twips(r) * f64::from(dy.min(256)) / 256.0 * EMU_PER_TWIP) as i64;
        let edit = match flags & 3 {
            3 | 2 => "twoCell",
            1 => "oneCell",
            _ => "absolute",
        };
        count += 1;
        w!(
            anchors,
            r#"<xdr:twoCellAnchor editAs="{edit}"><xdr:from><xdr:col>{c1}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{r1}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:from><xdr:to><xdr:col>{c2}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{r2}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:to><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{}" name="Picture {count}"/><xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/></xdr:twoCellAnchor>"#,
            col_off(c1, dx1),
            row_off(r1, dy1),
            col_off(c2, dx2),
            row_off(r2, dy2),
            count + 1,
            x1 as i64,
            y1 as i64,
            (x2 - x1).max(0.0) as i64,
            (y2 - y1).max(0.0) as i64,
        );
    }
    if count == 0 {
        return Ok(None);
    }
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="{NS_REL}">{anchors}</xdr:wsDr>"#
    );
    opc.add_part(&drawing, CT_DRAWING, xml.as_bytes())?;
    Ok(Some(opc.add_part_rel(sheet, rel_types::DRAWING, &format!("../drawings/drawing{n}.xml"))))
}

/// The sheet's comments part and the VML shapes Excel shows them with; the relationship id for `<legacyDrawing>`.
fn comments(opc: &mut Opc, sheet_part: &PartName, n: usize, sheet: &Sheet) -> office_oxide::core::Result<String> {
    let mut authors: Vec<&str> = Vec::new();
    let mut list = String::new();
    let mut shapes = String::new();
    for (i, c) in sheet.comments.iter().enumerate() {
        let author = c.author.as_deref().unwrap_or("");
        let id = authors.iter().position(|a| *a == author).unwrap_or_else(|| {
            authors.push(author);
            authors.len() - 1
        });
        w!(
            list,
            r#"<comment ref="{}" authorId="{id}"><text><r><t xml:space="preserve">{}</t></r></text></comment>"#,
            cell_ref(c.row.into(), c.col.into()),
            esc(&c.text)
        );
        let (row, col) = (u32::from(c.row), u32::from(c.col));
        w!(
            shapes,
            r##"<v:shape id="_x0000_s{}" type="#_x0000_t202" style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:{};visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>{}, 15, {}, 10, {}, 15, {}, 4</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>{row}</x:Row><x:Column>{col}</x:Column></x:ClientData></v:shape>"##,
            n * 1024 + i + 1,
            i + 1,
            col + 1,
            row,
            col + 3,
            row + 4
        );
    }
    let mut xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><comments xmlns="{NS_MAIN}"><authors>"#);
    for author in authors {
        w!(xml, "<author>{}</author>", esc(author));
    }
    w!(xml, "</authors><commentList>{list}</commentList></comments>");
    opc.add_part_rel(sheet_part, rel_types::COMMENTS, &format!("../comments{n}.xml"));
    opc.add_part(&part(&format!("/xl/comments{n}.xml"))?, CT_COMMENTS, xml.as_bytes())?;

    let vml = format!(
        r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="{n}"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>{shapes}</xml>"#
    );
    opc.register_default_content_type("vml", CT_VML);
    opc.add_part(&part(&format!("/xl/drawings/vmlDrawing{n}.vml"))?, CT_VML, vml.as_bytes())?;
    Ok(opc.add_part_rel(sheet_part, rel_types::VML_DRAWING, &format!("../drawings/vmlDrawing{n}.vml")))
}

/// `xl/styles.xml`: one cell format per BIFF `XF`, in the same order, so cells keep their `XF` index.
fn styles(layout: &Layout) -> String {
    let mut xml = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="{NS_MAIN}">"#);

    if !layout.formats.is_empty() {
        w!(xml, r#"<numFmts count="{}">"#, layout.formats.len());
        for (id, code) in &layout.formats {
            w!(xml, r#"<numFmt numFmtId="{id}" formatCode="{}"/>"#, esc(code));
        }
        xml.push_str("</numFmts>");
    }

    w!(xml, r#"<fonts count="{}">"#, layout.fonts.len().max(1));
    if layout.fonts.is_empty() {
        xml.push_str(r#"<font><sz val="10"/><name val="Arial"/></font>"#);
    }
    for f in &layout.fonts {
        xml.push_str("<font>");
        for (on, tag) in [(f.bold, "<b/>"), (f.italic, "<i/>"), (f.strike, "<strike/>"), (f.outline, "<outline/>"), (f.shadow, "<shadow/>")] {
            if on {
                xml.push_str(tag);
            }
        }
        match f.underline {
            0 => {},
            0x02 => xml.push_str(r#"<u val="double"/>"#),
            0x21 => xml.push_str(r#"<u val="singleAccounting"/>"#),
            0x22 => xml.push_str(r#"<u val="doubleAccounting"/>"#),
            _ => xml.push_str("<u/>"),
        }
        match f.script {
            1 => xml.push_str(r#"<vertAlign val="superscript"/>"#),
            2 => xml.push_str(r#"<vertAlign val="subscript"/>"#),
            _ => {},
        }
        w!(xml, r#"<sz val="{}"/>"#, points(f64::from(f.twips)));
        if let Some(rgb) = layout.rgb(f.color) {
            w!(xml, r#"<color rgb="FF{rgb:06X}"/>"#);
        }
        w!(xml, r#"<name val="{}"/>"#, esc(&f.name));
        if f.family > 0 {
            w!(xml, r#"<family val="{}"/>"#, f.family);
        }
        if f.charset > 0 {
            w!(xml, r#"<charset val="{}"/>"#, f.charset);
        }
        xml.push_str("</font>");
    }
    xml.push_str("</fonts>");

    // Fills and borders, each distinct one once; SpreadsheetML reserves fills 0 and 1.
    let mut fills: Vec<(u32, u16, u16)> = Vec::new();
    let mut borders: Vec<(u32, u32)> = Vec::new();
    let mut xfs = String::new();
    for xf in &layout.xfs {
        let at16 = |i: usize| u16::from_le_bytes([xf[i], xf[i + 1]]);
        let at32 = |i: usize| u32::from_le_bytes([xf[i], xf[i + 1], xf[i + 2], xf[i + 3]]);
        let (font, numfmt, protect) = (at16(0), at16(2), at16(4));
        let (line, edge, colors) = (at32(10), at32(14), at16(18));
        let font = match font {
            f if f >= 4 => f - 1, // BIFF skips font index 4
            f => f,
        };
        let font = if (font as usize) < layout.fonts.len() { font } else { 0 };
        let pattern = edge >> 26;
        let fill = if pattern == 0 {
            0
        } else {
            let key = (pattern, colors & 0x7F, (colors >> 7) & 0x7F);
            2 + fills.iter().position(|f| *f == key).unwrap_or_else(|| {
                fills.push(key);
                fills.len() - 1
            })
        };
        // Border: line styles and colours, minus the fill pattern bits.
        let key = (line, edge & 0x01FF_FFFF);
        let border = if key.0 & 0xFFFF == 0 && (key.1 >> 21) & 0xF == 0 {
            0
        } else {
            1 + borders.iter().position(|b| *b == key).unwrap_or_else(|| {
                borders.push(key);
                borders.len() - 1
            })
        };
        w!(
            xfs,
            r#"<xf numFmtId="{numfmt}" fontId="{font}" fillId="{fill}" borderId="{border}" xfId="0" applyNumberFormat="1" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1" applyProtection="1">"#
        );
        let (align, rotation, indent) = (xf[6], xf[7], xf[8]);
        let mut attrs = String::new();
        let h = (align & 7) as usize;
        if h != 0 {
            w!(attrs, r#" horizontal="{}""#, H_ALIGN[h]);
        }
        let v = ((align >> 4) & 7) as usize;
        if v != 2 {
            w!(attrs, r#" vertical="{}""#, V_ALIGN.get(v).unwrap_or(&"bottom"));
        }
        if rotation != 0 {
            w!(attrs, r#" textRotation="{rotation}""#);
        }
        if align & 0x08 != 0 {
            attrs.push_str(r#" wrapText="1""#);
        }
        if indent & 0x0F != 0 {
            w!(attrs, r#" indent="{}""#, indent & 0x0F);
        }
        if indent & 0x10 != 0 {
            attrs.push_str(r#" shrinkToFit="1""#);
        }
        if (indent >> 6) & 3 != 0 {
            w!(attrs, r#" readingOrder="{}""#, (indent >> 6) & 3);
        }
        if !attrs.is_empty() {
            w!(xfs, "<alignment{attrs}/>");
        }
        if protect & 1 == 0 || protect & 2 != 0 {
            w!(xfs, r#"<protection locked="{}" hidden="{}"/>"#, protect & 1, (protect >> 1) & 1);
        }
        xfs.push_str("</xf>");
    }

    w!(xml, r#"<fills count="{}"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>"#, fills.len() + 2);
    for &(pattern, fore, back) in &fills {
        w!(xml, r#"<fill><patternFill patternType="{}">"#, PATTERNS.get(pattern as usize).unwrap_or(&"solid"));
        color(&mut xml, "fgColor", fore, layout);
        color(&mut xml, "bgColor", back, layout);
        xml.push_str("</patternFill></fill>");
    }
    w!(xml, r#"</fills><borders count="{}"><border><left/><right/><top/><bottom/><diagonal/></border>"#, borders.len() + 1);
    for &(line, edge) in &borders {
        let diagonal = line >> 30;
        xml.push_str("<border");
        if diagonal & 2 != 0 {
            xml.push_str(r#" diagonalUp="1""#);
        }
        if diagonal & 1 != 0 {
            xml.push_str(r#" diagonalDown="1""#);
        }
        xml.push('>');
        let sides = [
            ("left", line & 0xF, (line >> 16) & 0x7F),
            ("right", (line >> 4) & 0xF, (line >> 23) & 0x7F),
            ("top", (line >> 8) & 0xF, edge & 0x7F),
            ("bottom", (line >> 12) & 0xF, (edge >> 7) & 0x7F),
            ("diagonal", (edge >> 21) & 0xF, (edge >> 14) & 0x7F),
        ];
        for (side, style, icv) in sides {
            match BORDER_STYLES.get(style as usize) {
                Some(&name) if style != 0 => {
                    w!(xml, r#"<{side} style="{name}">"#);
                    color(&mut xml, "color", icv as u16, layout);
                    w!(xml, "</{side}>");
                },
                _ => w!(xml, "<{side}/>"),
            }
        }
        xml.push_str("</border>");
    }
    xml.push_str(r#"</borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>"#);
    w!(xml, r#"<cellXfs count="{}">"#, layout.xfs.len().max(1));
    if layout.xfs.is_empty() {
        xml.push_str(r#"<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>"#);
    }
    xml.push_str(&xfs);
    xml.push_str(r#"</cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#);
    xml
}

/// `<tag rgb=.../>` for a palette colour; system colours keep their SpreadsheetML index (64 window
/// text, 65 window background).
fn color(xml: &mut String, tag: &str, icv: u16, layout: &Layout) {
    match layout.rgb(icv) {
        Some(rgb) => w!(xml, r#"<{tag} rgb="FF{rgb:06X}"/>"#),
        None if icv == 64 || icv == 65 => w!(xml, r#"<{tag} indexed="{icv}"/>"#),
        None => w!(xml, r#"<{tag} auto="1"/>"#),
    }
}
