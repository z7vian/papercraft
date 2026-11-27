//! Conditional formats (`CONDFMT` + `CF`, [MS-XLS] §2.4.56/§2.4.42) and data validations (`DV`, §2.4.107)
//! of a BIFF8 sheet as SpreadsheetML, with each rule's look as a `<dxf>` of styles.xml.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::formula::{self, Links, Origin, Rect};
use super::{BORDER_STYLES, Layout, PATTERNS, biff_string, color, esc, points, u16_at, u32_at};

const CONDFMT: u16 = 0x01B0;
const CF: u16 = 0x01B1;
const DV: u16 = 0x01BE;

/// `CONDFMT` and the `CF` rules after it.
struct CondFormat {
    /// The top-left cell of the bounding rectangle, which relative references count from.
    origin: (u16, u16),
    ranges: Vec<Rect>,
    rules: Vec<CfRule>,
}

struct CfRule {
    /// `ct`: 1 compares the cell value, 2 is a formula.
    kind: u8,
    /// `cp`: the comparison, 1 between to 8 less than or equal.
    op: u8,
    /// The `DXFN` the rule applies.
    dxf: Vec<u8>,
    formula1: Vec<u8>,
    formula2: Vec<u8>,
}

struct Validation {
    flags: u32,
    prompt_title: String,
    error_title: String,
    prompt: String,
    error: String,
    formula1: Vec<u8>,
    formula2: Vec<u8>,
    ranges: Vec<Rect>,
}

/// A sheet's conditional formats and data validations.
#[derive(Default)]
pub(super) struct SheetRules {
    formats: Vec<CondFormat>,
    validations: Vec<Validation>,
}

/// A `Ref8U` list: count, then first row, last row, first column, last column.
fn ref8_list(data: &[u8], at: usize) -> Option<Vec<Rect>> {
    let count = usize::from(u16_at(data, at)?);
    let body = data.get(at + 2..at + 2 + count * 8)?;
    Some(
        body.chunks_exact(8)
            .map(|r| {
                let at = |i: usize| u16::from_le_bytes([r[i], r[i + 1]]);
                (at(0), at(2), at(4).min(255), at(6).min(255))
            })
            .collect(),
    )
}

fn sqref(ranges: &[Rect]) -> String {
    ranges.iter().map(|&r| formula::rect_text(r)).collect::<Vec<_>>().join(" ")
}

/// The byte length of a `DXFN`: its flags say which blocks follow.
fn dxfn_len(d: &[u8]) -> Option<usize> {
    let flags = u32_at(d, 0)?;
    let user_format = u16_at(d, 4)? & 1 != 0;
    let mut len = 6;
    if flags & 1 << 25 != 0 {
        len += if user_format { usize::from(u16_at(d, len)?) } else { 2 };
    }
    for (bit, size) in [(26, 118), (27, 8), (28, 8), (29, 4), (30, 2)] {
        if flags & 1 << bit != 0 {
            len += size;
        }
    }
    (len <= d.len()).then_some(len)
}

impl SheetRules {
    pub(super) fn record(&mut self, kind: u16, data: &[u8]) {
        match kind {
            CONDFMT => {
                // ccf, flags, refBound (Ref8U), then the sqref.
                if let (Some(row), Some(col), Some(ranges)) = (u16_at(data, 4), u16_at(data, 8), ref8_list(data, 12)) {
                    self.formats.push(CondFormat { origin: (row, col), ranges, rules: Vec::new() });
                }
            },
            CF => {
                if let (Some(format), Some(rule)) = (self.formats.last_mut(), parse_cf(data)) {
                    format.rules.push(rule);
                }
            },
            DV => {
                if let Some(validation) = parse_dv(data) {
                    self.validations.push(validation);
                }
            },
            _ => {},
        }
    }

    /// `<conditionalFormatting>` elements; each rule's look is added to `dxfs`.
    // Limit: Excel 2007 rules kept in CF12 records (colour scales, data bars, icon sets, top 10, text and date rules) and the theme colours CFEX adds are not written; reading CONDFMT12/CF12/CFEX lifts it.
    pub(super) fn conditional_formats(&self, xml: &mut String, sheet: usize, links: &Links, layout: &Layout, dxfs: &mut Dxfs) {
        let mut priority = 0;
        for format in &self.formats {
            let at = Origin { row: format.origin.0, col: format.origin.1, sheet: Some(sheet), relative: true };
            let mut rules = String::new();
            for rule in &format.rules {
                let formula = |rgce: &[u8]| formula::text(rgce, &[], at, links);
                let formulas = match (rule.kind, rule.op) {
                    (1, 1 | 2) => formula(&rule.formula1).zip(formula(&rule.formula2)).map(|(a, b)| vec![a, b]),
                    (1, 3..=8) | (2, _) => formula(&rule.formula1).map(|a| vec![a]),
                    _ => None,
                };
                let Some(formulas) = formulas else { continue };
                let Some(dxf) = dxf(&rule.dxf, layout, dxfs) else { continue };
                priority += 1;
                if rule.kind == 1 {
                    let op = ["between", "notBetween", "equal", "notEqual", "greaterThan", "lessThan", "greaterThanOrEqual", "lessThanOrEqual"]
                        [usize::from(rule.op - 1)];
                    w!(rules, r#"<cfRule type="cellIs" dxfId="{dxf}" priority="{priority}" operator="{op}">"#);
                } else {
                    w!(rules, r#"<cfRule type="expression" dxfId="{dxf}" priority="{priority}">"#);
                }
                for f in formulas {
                    w!(rules, "<formula>{}</formula>", esc(&f));
                }
                rules.push_str("</cfRule>");
            }
            if !rules.is_empty() {
                w!(xml, r#"<conditionalFormatting sqref="{}">{rules}</conditionalFormatting>"#, sqref(&format.ranges));
            }
        }
    }

    /// `<dataValidations>`, when the sheet has any.
    pub(super) fn data_validations(&self, xml: &mut String, sheet: usize, links: &Links) {
        let mut list = String::new();
        let mut count = 0;
        for v in &self.validations {
            let Some(&(row, _, col, _)) = v.ranges.first() else { continue };
            let at = Origin { row, col, sheet: Some(sheet), relative: true };
            let kind = v.flags & 0xF;
            let formula1 = if v.flags & 0x80 != 0 {
                // An explicit list: one string, the items split by NUL.
                match v.formula1.as_slice() {
                    [0x17, ..] => biff_string(&v.formula1, 1, false).map(|(s, _)| format!("\"{}\"", s.trim_end_matches('\0').replace('\0', ",").replace('"', "\"\""))),
                    _ => None,
                }
            } else {
                formula::text(&v.formula1, &[], at, links)
            };
            let formula2 = formula::text(&v.formula2, &[], at, links);
            if kind != 0 && formula1.is_none() {
                continue;
            }
            count += 1;
            list.push_str("<dataValidation");
            if let Some(name) = ["", "whole", "decimal", "list", "date", "time", "textLength", "custom"].get(kind as usize).filter(|n| !n.is_empty()) {
                w!(list, r#" type="{name}""#);
            }
            match (v.flags >> 4) & 7 {
                1 => list.push_str(r#" errorStyle="warning""#),
                2 => list.push_str(r#" errorStyle="information""#),
                _ => {},
            }
            let op = (v.flags >> 20) & 0xF;
            if matches!(kind, 1 | 2 | 4 | 5 | 6) && op != 0 {
                if let Some(op) = ["", "notBetween", "equal", "notEqual", "greaterThan", "lessThan", "greaterThanOrEqual", "lessThanOrEqual"].get(op as usize) {
                    w!(list, r#" operator="{op}""#);
                }
            }
            for (bit, attr) in [(8, "allowBlank"), (9, "showDropDown"), (18, "showInputMessage"), (19, "showErrorMessage")] {
                if v.flags & 1 << bit != 0 {
                    w!(list, r#" {attr}="1""#);
                }
            }
            for (text, attr) in [(&v.error_title, "errorTitle"), (&v.error, "error"), (&v.prompt_title, "promptTitle"), (&v.prompt, "prompt")] {
                if !text.is_empty() {
                    w!(list, r#" {attr}="{}""#, esc(text));
                }
            }
            w!(list, r#" sqref="{}">"#, sqref(&v.ranges));
            if let Some(f) = formula1.filter(|_| kind != 0) {
                w!(list, "<formula1>{}</formula1>", esc(&f));
                if let Some(f) = formula2.filter(|_| matches!(kind, 1 | 2 | 4 | 5 | 6) && op <= 1) {
                    w!(list, "<formula2>{}</formula2>", esc(&f));
                }
            }
            list.push_str("</dataValidation>");
        }
        if count > 0 {
            w!(xml, r#"<dataValidations count="{count}">{list}</dataValidations>"#);
        }
    }
}

/// `CF`: ct, cp, cce1, cce2, the `DXFN`, then both formulas.
fn parse_cf(data: &[u8]) -> Option<CfRule> {
    let (cce1, cce2) = (usize::from(u16_at(data, 2)?), usize::from(u16_at(data, 4)?));
    let dxf = data.get(6..)?;
    let len = dxfn_len(dxf)?;
    let start = 6 + len;
    Some(CfRule {
        kind: *data.first()?,
        op: *data.get(1)?,
        dxf: dxf[..len].to_vec(),
        formula1: data.get(start..start + cce1)?.to_vec(),
        formula2: data.get(start + cce1..start + cce1 + cce2)?.to_vec(),
    })
}

/// `DV`: flags, four `XLUnicodeString`s (prompt title, error title, prompt, error), two `DVParsedFormula`s
/// (cce, unused, tokens), then the cells.
fn parse_dv(data: &[u8]) -> Option<Validation> {
    let flags = u32_at(data, 0)?;
    let mut at = 4;
    let mut texts: [String; 4] = Default::default();
    for text in &mut texts {
        let (s, end) = biff_string(data, at, true)?;
        // Excel writes an empty text as one NUL.
        *text = if s == "\0" { String::new() } else { s };
        at = end;
    }
    let mut formula = || {
        let cce = usize::from(u16_at(data, at)?);
        let rgce = data.get(at + 4..at + 4 + cce)?.to_vec();
        at += 4 + cce;
        Some(rgce)
    };
    let (formula1, formula2) = (formula()?, formula()?);
    let [prompt_title, error_title, prompt, error] = texts;
    Some(Validation { flags, prompt_title, error_title, prompt, error, formula1, formula2, ranges: ref8_list(data, at)? })
}

/// The `<dxfs>` of styles.xml, each look once, and the number formats they add.
pub(super) struct Dxfs {
    index: HashMap<String, usize>,
    list: Vec<String>,
    next_format: u16,
}

impl Dxfs {
    pub(super) fn new(layout: &Layout) -> Self {
        let next_format = layout.formats.iter().map(|f| f.0 + 1).max().unwrap_or(0).max(164);
        Dxfs { index: HashMap::new(), list: Vec::new(), next_format }
    }

    fn add(&mut self, xml: String) -> usize {
        if let Some(&i) = self.index.get(&xml) {
            return i;
        }
        self.list.push(xml.clone());
        self.index.insert(xml, self.list.len() - 1);
        self.list.len() - 1
    }

    pub(super) fn xml(&self) -> String {
        if self.list.is_empty() {
            return String::new();
        }
        format!(r#"<dxfs count="{}">{}</dxfs>"#, self.list.len(), self.list.concat())
    }
}

/// Built-in number format codes a `DXFN` may name by id.
fn builtin_format(id: u16) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yyyy h:mm",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// A `DXFN` ([MS-XLS] §2.5.53) as a `<dxf>`; its index in `dxfs`.
// Limit: a rule's alignment and protection blocks are not written; Excel only applies font, number format, fill and border in conditional formats.
fn dxf(d: &[u8], layout: &Layout, dxfs: &mut Dxfs) -> Option<usize> {
    let flags = u32_at(d, 0)?;
    let user_format = u16_at(d, 4)? & 1 != 0;
    let mut at = 6;
    let mut xml = String::from("<dxf>");
    let mut format = String::new();
    if flags & 1 << 25 != 0 {
        if user_format {
            let (code, _) = biff_string(d, at + 2, true)?;
            w!(format, r#"<numFmt numFmtId="{}" formatCode="{}"/>"#, dxfs.next_format, esc(&code));
            dxfs.next_format += 1;
            at += usize::from(u16_at(d, at)?);
        } else {
            let id = u16::from(*d.get(at + 1)?);
            let code = layout.formats.iter().find(|f| f.0 == id).map(|f| f.1.as_str()).or_else(|| builtin_format(id));
            if let Some(code) = code {
                w!(format, r#"<numFmt numFmtId="{id}" formatCode="{}"/>"#, esc(code));
            }
            at += 2;
        }
    }
    if flags & 1 << 26 != 0 {
        font(&mut xml, d.get(at..at + 118)?, layout);
        at += 118;
    }
    xml.push_str(&format);
    if flags & 1 << 27 != 0 {
        at += 8;
    }
    let border = if flags & 1 << 28 != 0 {
        let b = d.get(at..at + 8)?;
        at += 8;
        Some((u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u32::from_le_bytes([b[4], b[5], b[6], b[7]])))
    } else {
        None
    };
    if flags & 1 << 29 != 0 {
        let p = d.get(at..at + 4)?;
        let (fls, icv) = (u16::from_le_bytes([p[0], p[1]]) >> 10, u16::from_le_bytes([p[2], p[3]]));
        let (fls_set, fore_set, back_set) = (flags & 1 << 16 == 0, flags & 1 << 17 == 0, flags & 1 << 18 == 0);
        xml.push_str("<fill><patternFill");
        if fls_set {
            w!(xml, r#" patternType="{}""#, PATTERNS.get(usize::from(fls)).unwrap_or(&"solid"));
        }
        xml.push('>');
        // A solid rule fill shows its background colour, as SpreadsheetML's <dxf> does.
        let solid = !fls_set || fls == 1;
        if fore_set && !solid {
            color(&mut xml, "fgColor", icv & 0x7F, layout);
        }
        if back_set {
            color(&mut xml, "bgColor", (icv >> 7) & 0x7F, layout);
        }
        xml.push_str("</patternFill></fill>");
    }
    if let Some((line, edge)) = border {
        xml.push_str("<border>");
        let sides = [
            ("left", line & 0xF, (line >> 16) & 0x7F, 10),
            ("right", (line >> 4) & 0xF, (line >> 23) & 0x7F, 11),
            ("top", (line >> 8) & 0xF, edge & 0x7F, 12),
            ("bottom", (line >> 12) & 0xF, (edge >> 7) & 0x7F, 13),
        ];
        for (side, style, icv, ninch) in sides {
            if flags & 1 << ninch != 0 {
                continue;
            }
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
    xml.push_str("</dxf>");
    Some(dxfs.add(xml))
}

/// `DXFFntD`: only the properties whose "not changed" flag is clear.
fn font(xml: &mut String, f: &[u8], layout: &Layout) {
    let int = |at: usize| i32::from_le_bytes([f[at], f[at + 1], f[at + 2], f[at + 3]]);
    let (height, style, weight, script, underline, icv) =
        (int(64), int(68), u16::from_le_bytes([f[72], f[73]]), u16::from_le_bytes([f[74], f[75]]), f[76], int(80));
    let (style_unset, script_unset, underline_unset, weight_unset) = (int(88), int(92), int(96), int(100));
    let mut font = String::new();
    if weight_unset == 0 {
        font.push_str(if weight >= 700 { "<b/>" } else { r#"<b val="0"/>"# });
    }
    for (bit, tag) in [(0x02, "i"), (0x80, "strike")] {
        if style_unset & bit == 0 {
            if style & bit != 0 {
                w!(font, "<{tag}/>");
            } else {
                w!(font, r#"<{tag} val="0"/>"#);
            }
        }
    }
    if underline_unset == 0 {
        font.push_str(match underline {
            0 => r#"<u val="none"/>"#,
            0x02 => r#"<u val="double"/>"#,
            0x21 => r#"<u val="singleAccounting"/>"#,
            0x22 => r#"<u val="doubleAccounting"/>"#,
            _ => "<u/>",
        });
    }
    if script_unset == 0 {
        font.push_str(match script {
            1 => r#"<vertAlign val="superscript"/>"#,
            2 => r#"<vertAlign val="subscript"/>"#,
            _ => r#"<vertAlign val="baseline"/>"#,
        });
    }
    if height > 0 {
        w!(font, r#"<sz val="{}"/>"#, points(f64::from(height)));
    }
    if let Ok(icv) = u16::try_from(icv) {
        color(&mut font, "color", icv, layout);
    }
    if !font.is_empty() {
        w!(xml, "<font>{font}</font>");
    }
}
