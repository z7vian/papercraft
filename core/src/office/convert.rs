//! The old binary formats (Office 97-2003, and WPS which shares them) as the matching OOXML package.
//! Excel workbooks are rebuilt from their records (`xls.rs`) so cell formatting, sizes and pictures
//! survive; Word documents go through office_oxide's model and get their pictures put back at their
//! place and size (`doc.rs`); PowerPoint decks are rebuilt from their records with every shape in place (`ppt.rs`).

mod art;
mod doc;
mod ppt;
mod xls;

use std::borrow::Cow;
use std::io::Cursor;

use office_oxide::{DocumentFormat, DocumentIR};

use super::{OfficeFamily, bad};
use crate::CoreError;

pub(crate) fn to_ooxml(bytes: Vec<u8>, family: OfficeFamily) -> Result<Vec<u8>, CoreError> {
    match family {
        OfficeFamily::Document => doc::to_docx(bytes),
        OfficeFamily::Presentation => {
            super::pptx::reject_powerpoint95(&bytes)?;
            ppt::to_pptx(bytes)
        }
        OfficeFamily::Spreadsheet => xls::to_xlsx(bytes),
    }
}

/// office_oxide's own conversion, through its format-neutral model: text and simple layout only. The
/// fallback for files the record readers can't follow.
fn via_model(bytes: Vec<u8>, from: DocumentFormat) -> Result<Vec<u8>, CoreError> {
    let to = match from {
        DocumentFormat::Doc => DocumentFormat::Docx,
        DocumentFormat::Xls => DocumentFormat::Xlsx,
        _ => DocumentFormat::Pptx,
    };
    let doc = office_oxide::Document::from_reader(Cursor::new(bytes), from).map_err(|e| bad("file", e))?;
    write_model(&doc.to_ir(), to)
}

fn write_model(ir: &DocumentIR, to: DocumentFormat) -> Result<Vec<u8>, CoreError> {
    let mut out = Cursor::new(Vec::new());
    office_oxide::create::create_from_ir_to_writer(ir, to, &mut out).map_err(|e| bad("file", e))?;
    Ok(out.into_inner())
}

/// `s` as XML text or attribute value: markup characters escaped, characters XML 1.0 forbids dropped.
fn esc(s: &str) -> Cow<'_, str> {
    let allowed = |c: char| matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && !matches!(c, '\u{FFFE}' | '\u{FFFF}'));
    if s.chars().all(|c| allowed(c) && !matches!(c, '&' | '<' | '>' | '"' | '\'')) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars().filter(|&c| allowed(c)) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    // Fixtures from Apache POI test-data (Apache-2.0): Formatting.xls, SimpleWithImages.xls, two_images.doc, PngPicture.doc.
    use std::io::Cursor;

    use office_oxide::core::opc::{OpcReader, PartName};

    use super::to_ooxml;
    use crate::office::OfficeFamily;

    fn convert(name: &str, family: OfficeFamily) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/convert/{name}", env!("CARGO_MANIFEST_DIR"));
        to_ooxml(std::fs::read(path).unwrap(), family).unwrap()
    }

    fn parts(package: &[u8]) -> OpcReader<Cursor<Vec<u8>>> {
        OpcReader::new(Cursor::new(package.to_vec())).unwrap()
    }

    fn part(package: &[u8], name: &str) -> String {
        String::from_utf8(parts(package).read_part(&PartName::new(name).unwrap()).unwrap()).unwrap()
    }

    /// Each `<name ...>` start tag in `xml`, up to its `>`.
    fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
        let open = format!("<{name}");
        xml.match_indices(&open)
            .map(|(at, _)| &xml[at..at + xml[at..].find('>').unwrap() + 1])
            .filter(|t| t[open.len()..].starts_with([' ', '>', '/']))
            .collect()
    }

    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let key = format!(" {name}=\"");
        let start = tag.find(&key)? + key.len();
        Some(&tag[start..start + tag[start..].find('"')?])
    }

    /// The `<xf>` of `cellXfs` that cell `cell` of sheet 1 uses.
    fn cell_format<'a>(sheet: &str, styles: &'a str, cell: &str) -> &'a str {
        let tag = tags(sheet, "c").into_iter().find(|t| attr(t, "r") == Some(cell)).unwrap();
        let index: usize = attr(tag, "s").unwrap().parse().unwrap();
        let cell_xfs = &styles[styles.find("<cellXfs").unwrap()..];
        tags(cell_xfs, "xf")[index]
    }

    #[test]
    fn xls_keeps_column_widths_bold_and_number_formats() {
        let xlsx = convert("Formatting.xls", OfficeFamily::Spreadsheet);
        let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
        let styles = part(&xlsx, "/xl/styles.xml");
        // COLINFO: column A is 3913/256 characters wide, column B 3181/256.
        assert!(sheet.contains(r#"<col min="1" max="1" width="15.28515625" customWidth="1""#), "{sheet}");
        assert!(sheet.contains(r#"<col min="2" max="2" width="12.42578125" customWidth="1""#), "{sheet}");
        // A1 is a bold heading.
        let fonts: Vec<&str> = styles.split("<font>").skip(1).collect();
        let font: usize = attr(cell_format(&sheet, &styles, "A1"), "fontId").unwrap().parse().unwrap();
        assert!(fonts[font].starts_with("<b/>"), "A1 font: {}", fonts[font]);
        // B3 holds a date serial under the custom format yyyy/mm/dd, still a number.
        assert_eq!(attr(cell_format(&sheet, &styles, "B3"), "numFmtId"), Some("165"));
        assert!(styles.contains(r#"<numFmt numFmtId="165" formatCode="yyyy/mm/dd"/>"#), "{styles}");
        let b3 = tags(&sheet, "c").into_iter().find(|t| attr(t, "r") == Some("B3")).unwrap();
        assert_eq!(attr(b3, "t"), None, "B3 should stay numeric: {b3}");
    }

    #[test]
    fn xls_keeps_pictures_on_their_sheet() {
        let xlsx = convert("SimpleWithImages.xls", OfficeFamily::Spreadsheet);
        let sheet = part(&xlsx, "/xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<drawing r:id="), "{sheet}");
        let drawing = part(&xlsx, "/xl/drawings/drawing1.xml");
        let anchors = tags(&drawing, "xdr:twoCellAnchor");
        assert!(!anchors.is_empty(), "{drawing}");
        assert_eq!(anchors.len(), tags(&drawing, "a:blip").len());
        // Each picture has a real size and its bytes are in the package.
        for ext in tags(&drawing, "a:ext") {
            assert!(attr(ext, "cx").unwrap().parse::<u64>().unwrap() > 0, "{ext}");
            assert!(attr(ext, "cy").unwrap().parse::<u64>().unwrap() > 0, "{ext}");
        }
        let media: Vec<PartName> =
            parts(&xlsx).part_names().into_iter().filter(|p| p.as_str().starts_with("/xl/media/")).collect();
        assert!(!media.is_empty());
        for name in media {
            assert!(!parts(&xlsx).read_part(&name).unwrap().is_empty());
        }
    }

    #[test]
    fn doc_keeps_inline_pictures_in_place_and_at_size() {
        let docx = convert("two_images.doc", OfficeFamily::Document);
        let body = part(&docx, "/word/document.xml");
        // Two 400x400-twip pictures (254000 EMU), each at the start of its paragraph; office_oxide's
        // default-size copies at the end are gone.
        assert_eq!(body.matches("<w:drawing>").count(), 2, "{body}");
        assert_eq!(body.matches(r#"<wp:extent cx="254000" cy="254000"/>"#).count(), 2, "{body}");
        let first = body.find("<w:drawing>").unwrap();
        let second = body.rfind("<w:drawing>").unwrap();
        let jpg = body.find("the jpg file").unwrap();
        let png = body.find("the png file").unwrap();
        assert!(first < jpg && jpg < second && second < png, "{body}");
    }

    #[test]
    fn doc_keeps_floating_pictures_with_size_and_position() {
        let docx = convert("PngPicture.doc", OfficeFamily::Document);
        let body = part(&docx, "/word/document.xml");
        // FSPA: left 2903, top 0, right 5737, bottom 824 twips from the column and paragraph.
        assert_eq!(body.matches("<wp:anchor").count(), 1, "{body}");
        assert!(body.contains(r#"<wp:extent cx="1799590" cy="523240"/>"#), "{body}");
        assert!(body.contains("<wp:posOffset>1843405</wp:posOffset>"), "{body}");
        assert!(body.find("lazy dog").unwrap() < body.find("<wp:anchor").unwrap(), "{body}");
    }
}
