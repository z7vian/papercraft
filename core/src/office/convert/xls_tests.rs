// Fixtures from Apache POI test-data/spreadsheet (Apache-2.0): 27349-vlookupAcrossSheets.xls, IfFormulaTest.xls,
// 3dFormulas.xls, 55906-MultiSheetRefs.xls, shared_formulas.xls, testArraysAndTables.xls, Themes2.xls,
// WithConditionalFormatting.xls, 32822.xls.
use std::io::Cursor;

use office_oxide::core::opc::{OpcReader, PartName};

use crate::office::OfficeFamily;
use crate::sheet_edit::SheetEditor;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/xlsformula/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn convert(name: &str) -> Vec<u8> {
    super::to_xlsx(fixture(name)).unwrap()
}

fn part(package: &[u8], name: &str) -> String {
    let mut reader = OpcReader::new(Cursor::new(package.to_vec())).unwrap();
    String::from_utf8(reader.read_part(&PartName::new(name).unwrap()).unwrap()).unwrap()
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// The `<c>` element of cell `name`.
fn cell<'a>(sheet: &'a str, name: &str) -> &'a str {
    let open = format!(r#"<c r="{name}""#);
    let start = sheet
        .match_indices(&open)
        .map(|(at, _)| at)
        .find(|&at| sheet[at + open.len()..].starts_with([' ', '>', '/']))
        .unwrap_or_else(|| panic!("no cell {name}"));
    let rest = &sheet[start..];
    let tag_end = rest.find('>').unwrap() + 1;
    if rest[..tag_end].ends_with("/>") { &rest[..tag_end] } else { &rest[..rest.find("</c>").unwrap() + 4] }
}

/// Cell `name`'s formula text, unescaped; `None` when it has none.
fn formula(sheet: &str, name: &str) -> Option<String> {
    let c = cell(sheet, name);
    let start = c.find("<f")?;
    let text = start + c[start..].find('>')? + 1;
    Some(unescape(&c[text..c.find("</f>")?]))
}

/// The `<f ...>` start tag of cell `name`.
fn formula_tag<'a>(sheet: &'a str, name: &str) -> &'a str {
    let c = cell(sheet, name);
    let start = c.find("<f").unwrap();
    &c[start..start + c[start..].find('>').unwrap() + 1]
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    Some(&tag[start..start + tag[start..].find('"')?])
}

/// The `n`th `<item>` of `<list>` in styles.xml.
fn style_item<'a>(styles: &'a str, list: &str, item: &str, n: usize) -> &'a str {
    let start = styles.find(&format!("<{list} ")).unwrap();
    let section = &styles[start..start + styles[start..].find(&format!("</{list}>")).unwrap()];
    let close = format!("</{item}>");
    section.split(&format!("<{item}>")).skip(1).map(|s| &s[..s.find(&close).unwrap()]).nth(n).unwrap()
}

/// The `<xf>` of `cellXfs` cell `name` uses.
fn cell_xf<'a>(sheet: &str, styles: &'a str, name: &str) -> &'a str {
    let c = cell(sheet, name);
    let index: usize = attr(&c[..c.find('>').unwrap()], "s").unwrap().parse().unwrap();
    let xfs = &styles[styles.find("<cellXfs").unwrap()..];
    xfs.match_indices("<xf ").map(|(at, _)| &xfs[at..at + xfs[at..].find('>').unwrap()]).nth(index).unwrap()
}

#[test]
fn formulas_keep_the_text_excel_shows() {
    let xlsx = convert("27349-vlookupAcrossSheets.xls");
    let test = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert_eq!(formula(&test, "A1").as_deref(), Some("VLOOKUP(1, $A$8:$B$10, 2)"));
    // Stored as two sheet references joined by `:`, which Excel shows as such.
    assert_eq!(formula(&test, "A2").as_deref(), Some("VLOOKUP(1, 'DATA TABLE'!$A$8:'DATA TABLE'!$B$10, 2)"));
    assert!(cell(&test, "A1").ends_with("<v>3</v></c>"), "{}", cell(&test, "A1"));

    let xlsx = convert("IfFormulaTest.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert_eq!(formula(&sheet, "A3").as_deref(), Some("MAX(A1:A2)"));
    assert_eq!(formula(&sheet, "A4").as_deref(), Some(r#"IF(A3=A1,"A1","A2")"#));
    // A text result stays in the cell.
    assert!(cell(&sheet, "A4").contains(r#" t="str">"#) && cell(&sheet, "A4").ends_with("<v>A2</v></c>"), "{}", cell(&sheet, "A4"));
    assert_eq!(formula(&sheet, "A1"), None);
}

#[test]
fn formulas_reach_other_sheets_and_sheet_ranges() {
    let xlsx = convert("3dFormulas.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert_eq!(formula(&sheet, "A2").as_deref(), Some("Sheet1!A1"));
    // Sheet names that read as cells (column S row 2, column SH row 3) are quoted.
    assert_eq!(formula(&sheet, "A5").as_deref(), Some("'S2'!A1"));
    assert_eq!(formula(&sheet, "A7").as_deref(), Some("SUM('Sh3'!A1:A4)"));
    let book = part(&xlsx, "/xl/workbook.xml");
    assert!(book.contains(r#"<definedName name="_xlnm.Print_Area" localSheetId="0">Sheet1!$1:$65536</definedName>"#), "{book}");

    let xlsx = convert("55906-MultiSheetRefs.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert_eq!(formula(&sheet, "A3").as_deref(), Some("SUM(Sheet1:Sheet3!A1)"));
    assert_eq!(formula(&sheet, "H3").as_deref(), Some("SUM(Sheet1:Sheet3!A1:B2)"));
    assert_eq!(formula(&sheet, "B4").as_deref(), Some("MIN(Sheet1:Sheet3!A$1)"));
}

#[test]
fn shared_and_array_formulas_are_spelled_out() {
    // One SHRFMLA covers A2:A41, each cell pointing at its neighbour in column B.
    let xlsx = convert("shared_formulas.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    for (at, target) in [("A2", "B2"), ("A20", "B20"), ("A41", "B41")] {
        assert_eq!(formula(&sheet, at).as_deref(), Some(target));
    }

    let xlsx = convert("testArraysAndTables.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert_eq!(formula(&sheet, "C2").as_deref(), Some("{1.5,2.7,3.3,4.9,6.2}"));
    assert_eq!(attr(formula_tag(&sheet, "C2"), "t"), Some("array"));
    assert_eq!(attr(formula_tag(&sheet, "C2"), "ref"), Some("C2:G2"));
    assert_eq!(formula(&sheet, "D2"), None, "only the array's first cell holds it");
    assert_eq!(formula(&sheet, "C11").as_deref(), Some(r#"{"1A","1B","1C";"2A","2B","2C"}"#));
    assert_eq!(formula(&sheet, "B16").as_deref(), Some("{1,2,3,4} * {5,6,7,8}"));
    assert_eq!(attr(formula_tag(&sheet, "B16"), "ref"), Some("B16:E16"));
    assert_eq!(formula(&sheet, "B27").as_deref(), Some(r#"B26+100&"a""#));
    // Data table cells keep their values.
    assert_eq!(formula(&sheet, "C27"), None);
}

#[test]
fn sheet_editor_recalculates_converted_formulas() {
    let editor = SheetEditor::open(fixture("3dFormulas.xls"), OfficeFamily::Spreadsheet).unwrap();
    let total = editor.get_cell(0, 6, 0).unwrap();
    assert!(total.is_formula && total.input.starts_with("=SUM(") && total.display == "165", "{total:?}");
    // Sh3!A1 44 -> 100.
    let changed = editor.set_cell(2, 0, 0, "100".into()).unwrap();
    assert!(changed.iter().any(|c| (c.sheet, c.row, c.col) == (0, 6, 0) && c.display == "221"), "{changed:?}");

    let editor = SheetEditor::open(fixture("27349-vlookupAcrossSheets.xls"), OfficeFamily::Spreadsheet).unwrap();
    assert_eq!(editor.get_cell(0, 1, 0).unwrap().display, "3");
    // 'DATA TABLE'!B8, the value VLOOKUP finds for key 1.
    let changed = editor.set_cell(1, 7, 1, "30".into()).unwrap();
    assert!(changed.iter().any(|c| (c.sheet, c.row, c.col) == (0, 1, 0) && c.display == "30"), "{changed:?}");

    let editor = SheetEditor::open(fixture("IfFormulaTest.xls"), OfficeFamily::Spreadsheet).unwrap();
    editor.set_cell(0, 0, 0, "5".into()).unwrap();
    assert_eq!(editor.get_cell(0, 2, 0).unwrap().display, "5");
    assert_eq!(editor.get_cell(0, 3, 0).unwrap().display, "A1");
}

#[test]
fn excel_2007_colours_replace_palette_colours() {
    let xlsx = convert("Themes2.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    let styles = part(&xlsx, "/xl/styles.xml");
    let font = |name: &str| {
        let id: usize = attr(cell_xf(&sheet, &styles, name), "fontId").unwrap().parse().unwrap();
        style_item(&styles, "fonts", "font", id)
    };
    // C8 "green": RGB text colour 00B050.
    assert!(font("C8").contains(r#"<color rgb="FF00B050"/>"#), "{}", font("C8"));
    // B4 "grey": theme text colour 1 (black) lightened by half.
    assert!(font("B4").contains(r#"<color rgb="FF7F7F7F"/>"#), "{}", font("B4"));
    // E5 "Dark Blue": RGB fill 002060.
    let fill: usize = attr(cell_xf(&sheet, &styles, "E5"), "fillId").unwrap().parse().unwrap();
    let fill = style_item(&styles, "fills", "fill", fill);
    assert!(fill.contains(r#"<fgColor rgb="FF002060"/>"#), "{fill}");
}

#[test]
fn conditional_formats_survive() {
    let xlsx = convert("WithConditionalFormatting.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    let styles = part(&xlsx, "/xl/styles.xml");
    assert!(
        sheet.contains(r#"<conditionalFormatting sqref="A1:A8"><cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan"><formula>3</formula></cfRule><cfRule type="cellIs" dxfId="1" priority="2" operator="lessThan"><formula>-3</formula></cfRule></conditionalFormatting>"#),
        "{sheet}"
    );
    assert!(
        sheet.contains(r#"<conditionalFormatting sqref="B9"><cfRule type="expression" dxfId="2" priority="3"><formula>$A$8&gt;5</formula></cfRule></conditionalFormatting>"#),
        "{sheet}"
    );
    assert!(sheet.contains(r#"operator="between"><formula>&quot;A&quot;</formula><formula>&quot;AAA&quot;</formula>"#), "{sheet}");
    // The first rule turns the text bold; the formula rule fills the cell.
    assert!(style_item(&styles, "dxfs", "dxf", 0).starts_with("<font><b/>"), "{styles}");
    assert!(style_item(&styles, "dxfs", "dxf", 2).contains("<fill><patternFill><bgColor rgb="), "{styles}");
    // The sheet editor reads the converted rules.
    assert!(SheetEditor::open(fixture("WithConditionalFormatting.xls"), OfficeFamily::Spreadsheet).is_ok());
}

#[test]
fn data_validations_survive() {
    let xlsx = convert("32822.xls");
    let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<dataValidations count="4">"#), "{sheet}");
    assert!(
        sheet.contains(r#"<dataValidation type="list" allowBlank="1" showInputMessage="1" showErrorMessage="1" error="You cannot enter a name. Please select from the dropdown" prompt="Please select your name" sqref="D2:D50 D52:D65536"><formula1>&quot;Karen,Donna,Vito,Other&quot;</formula1></dataValidation>"#),
        "{sheet}"
    );
    assert!(sheet.contains(r#"sqref="D1"></dataValidation>"#), "{sheet}");
}
