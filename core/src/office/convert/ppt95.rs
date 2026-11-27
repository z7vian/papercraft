//! PowerPoint 95 (PowerPoint 7) files as a .pptx: one slide per slide, with its title, its text where it
//! sits on the slide, the slide size and the speaker notes.
//!
//! The `PowerPoint Document` stream is a tree of records like PowerPoint 97's, with a 16-byte header:
//! type, instance (`0xFFFF` on a container), body length, and a field this reader doesn't need, all u32.
//! The parts read here (record types as PowerPoint 95 writes them):
//! - Document (1000) > DocumentAtom (1001): slide width and height in master units (576 per inch).
//! - Document > List (2000) of Slide (1006). Each slide holds its rectangle (1005, centred on 0,0), its notes
//!   page (1008, laid out like a slide) and its shapes (3000). Shapes nest through groups.
//! - A shape with text holds an anchor (3009: left, top, right, bottom at byte 12) and a text block
//!   4001 > 4002 > {4003: placeholder kind as u32, 4064 > 2030: the text in the ANSI code page, `\r` between
//!   paragraphs}.
//!
//! PowerPoint 4.0 (a `PP40` stream) is older still and refused.
// Limit: text, titles, positions and notes only (no pictures, shapes, fonts or colours); reading the
// picture (4033) and style (2028) records lifts it.

use std::io::Cursor;

use encoding_rs::WINDOWS_1252;
use office_oxide::cfb::CfbReader;
use office_oxide::pptx::write::{PptxWriter, Run};

use crate::CoreError;
use crate::office::bad;

const DOCUMENT: u32 = 1000;
const DOCUMENT_ATOM: u32 = 1001;
const SLIDE: u32 = 1006;
const SLIDE_RECT: u32 = 1005;
const NOTES: u32 = 1008;
const MAIN_MASTER: u32 = 1016;
const LIST: u32 = 2000;
const TEXT_CHARS: u32 = 2030;
const SHAPES: u32 = 3000;
const ANCHOR: u32 = 3009;
const TEXT_BLOCK: u32 = 4001;
const TEXT_BODY: u32 = 4002;
const TEXT_HEADER: u32 = 4003;
const TEXT_RUNS: u32 = 4064;

/// Placeholder kinds in the text header, as in PowerPoint 97's TextHeaderAtom.
const KIND_TITLE: u32 = 0;
const KIND_NOTES: u32 = 2;
const KIND_CENTER_TITLE: u32 = 6;

/// Deeper nesting than this is a damaged file; its shapes are skipped.
const MAX_DEPTH: u32 = 16;

pub(super) fn to_pptx(bytes: &[u8]) -> Result<Vec<u8>, CoreError> {
    let mut cfb = CfbReader::new(Cursor::new(bytes)).map_err(|e| bad("presentation", e))?;
    if cfb.has_root_entry("PP40") || !cfb.has_stream("PowerPoint Document") {
        return Err(CoreError::BadFile(
            "PowerPoint 95 and older files are not supported. Save it as .pptx in PowerPoint, then open that.".into(),
        ));
    }
    let stream = cfb.open_stream("PowerPoint Document").map_err(|e| bad("presentation", e))?;
    let deck = read(&stream).ok_or_else(|| bad("presentation", "no slide list in the PowerPoint 95 document"))?;
    write(&deck)
}

struct Deck {
    /// Width and height in master units.
    size: (i64, i64),
    slides: Vec<SlideText>,
}

#[derive(Default)]
struct SlideText {
    shapes: Vec<Shape>,
    notes: Vec<String>,
}

struct Shape {
    kind: Option<u32>,
    /// Left, top, right, bottom in master units from the slide's top left corner.
    rect: Option<[i64; 4]>,
    text: String,
}

#[derive(Clone, Copy)]
struct Rec<'a> {
    kind: u32,
    container: bool,
    body: &'a [u8],
}

/// The records laid end to end in `buf`; a record running past the end (a cut stream) keeps what is there.
fn records(mut buf: &[u8]) -> impl Iterator<Item = Rec<'_>> {
    std::iter::from_fn(move || {
        let (head, rest) = buf.split_first_chunk::<16>()?;
        let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let (body, next) = rest.split_at((word(8) as usize).min(rest.len()));
        buf = next;
        Some(Rec { kind: word(0), container: word(4) == 0xFFFF, body })
    })
}

fn child(buf: &[u8], kind: u32) -> Option<Rec<'_>> {
    records(buf).find(|r| r.kind == kind)
}

fn i32_at(buf: &[u8], at: usize) -> Option<i64> {
    buf.get(at..at + 4).map(|b| i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

fn read(stream: &[u8]) -> Option<Deck> {
    // The Document sits at the top or inside the stream's one outer container.
    let document = records(stream)
        .flat_map(|r| std::iter::once(r).chain(records(if r.container { r.body } else { &[] })))
        .find(|r| r.kind == DOCUMENT)?;
    let atom = child(document.body, DOCUMENT_ATOM)?;
    let size = (i32_at(atom.body, 0)?, i32_at(atom.body, 4)?);
    // Masters live in a list of their own, which starts with a MainMaster.
    let list = records(document.body).find(|r| {
        r.kind == LIST && records(r.body).any(|c| c.kind == SLIDE) && !records(r.body).any(|c| c.kind == MAIN_MASTER)
    })?;
    let slides = records(list.body).filter(|r| r.kind == SLIDE).map(|slide| read_slide(slide.body, size)).collect();
    Some(Deck { size, slides })
}

fn read_slide(slide: &[u8], size: (i64, i64)) -> SlideText {
    let mut shapes = Vec::new();
    page_shapes(slide, size, &mut shapes);
    let mut notes_shapes = Vec::new();
    if let Some(notes) = child(slide, NOTES) {
        page_shapes(notes.body, size, &mut notes_shapes);
    }
    let notes = notes_shapes.into_iter().filter(|s| s.kind == Some(KIND_NOTES)).map(|s| s.text).collect();
    SlideText { shapes, notes }
}

/// The text shapes of a slide or notes page, anchors moved to its top left corner.
fn page_shapes(page: &[u8], size: (i64, i64), out: &mut Vec<Shape>) {
    let origin = child(page, SLIDE_RECT)
        .and_then(|r| Some((i32_at(r.body, 0)?, i32_at(r.body, 4)?)))
        .unwrap_or((-size.0 / 2, -size.1 / 2));
    let start = out.len();
    if let Some(shapes) = child(page, SHAPES) {
        collect_shapes(shapes.body, 0, out);
    }
    for shape in &mut out[start..] {
        if let Some(rect) = &mut shape.rect {
            rect[0] -= origin.0;
            rect[2] -= origin.0;
            rect[1] -= origin.1;
            rect[3] -= origin.1;
        }
    }
}

fn collect_shapes(group: &[u8], depth: u32, out: &mut Vec<Shape>) {
    if depth > MAX_DEPTH {
        return;
    }
    for rec in records(group).filter(|r| r.container) {
        match child(rec.body, TEXT_BLOCK) {
            Some(block) => out.extend(text_shape(rec.body, block.body)),
            None => collect_shapes(rec.body, depth + 1, out),
        }
    }
}

fn text_shape(shape: &[u8], block: &[u8]) -> Option<Shape> {
    let body = child(block, TEXT_BODY)?.body;
    let chars = child(child(body, TEXT_RUNS)?.body, TEXT_CHARS)?.body;
    let (text, _, _) = WINDOWS_1252.decode(chars);
    let kind = child(body, TEXT_HEADER).and_then(|h| i32_at(h.body, 0)).map(|k| k as u32);
    let rect = child(shape, ANCHOR)
        .and_then(|a| Some([i32_at(a.body, 12)?, i32_at(a.body, 16)?, i32_at(a.body, 20)?, i32_at(a.body, 24)?]));
    Some(Shape { kind, rect, text: text.replace('\x0b', "\r") })
}

/// Master units to EMU: 914400 / 576 = 1587.5.
fn emu(units: i64) -> i64 {
    units * 3175 / 2
}

fn write(deck: &Deck) -> Result<Vec<u8>, CoreError> {
    let mut pptx = PptxWriter::new();
    pptx.set_presentation_size(emu(deck.size.0).max(1) as u64, emu(deck.size.1).max(1) as u64);
    for slide in &deck.slides {
        let out = pptx.add_slide();
        let mut titled = false;
        for shape in &slide.shapes {
            if shape.text.trim().is_empty() {
                continue;
            }
            let is_title = matches!(shape.kind, Some(KIND_TITLE | KIND_CENTER_TITLE));
            if is_title && !titled && !shape.text.contains('\r') {
                out.set_title(&shape.text);
                titled = true;
                continue;
            }
            let [left, top, right, bottom] = shape.rect.unwrap_or([0, 0, deck.size.0, deck.size.1]);
            let mut runs = Vec::new();
            for (i, line) in shape.text.split('\r').enumerate() {
                if i > 0 {
                    runs.push(Run::line_break());
                }
                if !line.is_empty() {
                    runs.push(Run::new(line));
                }
            }
            out.add_rich_text_box(&runs, emu(left), emu(top), emu(right - left).max(1), emu(bottom - top).max(1));
        }
        let notes: Vec<&str> =
            slide.notes.iter().map(|n| n.trim_matches(['\r', ' '])).filter(|n| !n.is_empty()).collect();
        if !notes.is_empty() {
            out.set_notes(&notes.join("\n").replace('\r', "\n"));
        }
    }
    let mut buf = Cursor::new(Vec::new());
    pptx.write_to(&mut buf).map_err(|e| bad("presentation", e))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use office_oxide::core::opc::{OpcReader, PartName};

    use super::{read, to_pptx, write};
    use crate::CoreError;
    use crate::office::{OfficeDoc, OfficeFamily};

    fn fixture(path: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    // ppt95/PPT95.ppt: Apache POI test-data (Apache-2.0); the picture (4033) bodies are emptied to fit the
    // fixture size cap, every other record and stream is the original's.
    #[test]
    fn powerpoint95_opens_with_every_slide_and_its_text() {
        let doc = OfficeDoc::open(fixture("ppt95/PPT95.ppt"), OfficeFamily::Presentation).unwrap();
        assert_eq!(doc.page_count(), Some(42));
        let first = doc.page_text(0).unwrap().unwrap();
        assert!(first.contains("XML, HTML and All That"), "{first}");
        assert!(first.contains("What do they Mean, and Why are they Important?"), "{first}");
        assert!(first.contains("Ian GRAHAM"), "{first}");
        let second = doc.page_text(1).unwrap().unwrap();
        assert!(second.contains("Overview"), "{second}");
        assert!(second.contains("Why HTML was not enough"), "{second}");
        // ANSI text: curly quotes and the ellipsis come through.
        let fourth = doc.page_text(3).unwrap().unwrap();
        assert!(fourth.contains("\u{201c}muddle through\u{201d}"), "{fourth}");
        // A group's text boxes.
        let last = doc.page_text(41).unwrap().unwrap();
        assert!(last.contains("The XML Family Tree") && last.contains("SpeechML"), "{last}");
        assert_eq!(doc.page_text(42).unwrap(), None);
        // This deck's notes pages are empty.
        assert_eq!(doc.notes(0).unwrap(), None);
    }

    #[test]
    fn powerpoint95_keeps_slide_size_title_and_body_place() {
        let pptx = to_pptx(&fixture("ppt95/PPT95.ppt")).unwrap();
        let mut parts = OpcReader::new(Cursor::new(pptx)).unwrap();
        let mut part = |name: &str| String::from_utf8(parts.read_part(&PartName::new(name).unwrap()).unwrap()).unwrap();
        // DocumentAtom: 5760 x 4320 master units, 10 x 7.5 inches.
        let presentation = part("/ppt/presentation.xml");
        assert!(presentation.contains(r#"<p:sldSz cx="9144000" cy="6858000""#), "{presentation}");
        let slide = part("/ppt/slides/slide1.xml");
        assert!(slide.contains(r#"type="title""#) && slide.contains("XML, HTML and All That"), "{slide}");
        // The subtitle's anchor is (-1824, -672)..(2592, 444) around the slide centre: 1056, 1488 from its
        // top left corner, 4416 x 1116 units.
        assert!(slide.contains(r#"<a:off x="1676400" y="2362200"/><a:ext cx="7010400" cy="1771650"/>"#), "{slide}");
        assert!(part("/ppt/slides/slide42.xml").contains("The XML Family Tree"));
    }

    /// A record: type, instance (0xFFFF for a container), length, a zero field, body.
    fn rec(kind: u32, container: bool, body: &[u8]) -> Vec<u8> {
        let instance: u32 = if container { 0xFFFF } else { 0 };
        [&kind.to_le_bytes()[..], &instance.to_le_bytes(), &(body.len() as u32).to_le_bytes(), &[0u8; 4], body].concat()
    }

    fn text_shape(kind: u32, rect: [i32; 4], text: &str) -> Vec<u8> {
        let anchor: Vec<u8> = [[0; 12].to_vec(), rect.iter().flat_map(|v| v.to_le_bytes()).collect(), vec![0; 12]].concat();
        let header = [kind.to_le_bytes().to_vec(), vec![0; 32]].concat();
        let runs = rec(4064, true, &rec(2030, false, text.as_bytes()));
        let body = rec(4002, true, &[rec(4003, false, &header), rec(4021, false, &[0; 4]), runs].concat());
        rec(3010, true, &[rec(3009, false, &anchor), rec(4001, true, &body)].concat())
    }

    #[test]
    fn powerpoint95_notes_and_line_breaks() {
        let page = |shapes: Vec<u8>| [rec(1005, false, &[0xC0, 0xF4, 0xFF, 0xFF, 0x90, 0xF7, 0xFF, 0xFF]), rec(3000, true, &shapes)].concat();
        let notes = rec(1008, true, &page(text_shape(2, [-1584, -144, 1584, 2448], "Say hello\rthen go")));
        let shapes = [text_shape(0, [-2624, -2016, 2272, -1296], "Hello"), text_shape(1, [-2592, -972, 2560, 1656], "One\x0bTwo\rThree")].concat();
        let slide = rec(1006, true, &[notes, page(shapes)].concat());
        let list = rec(2000, true, &[rec(2001, false, &1u32.to_le_bytes()), slide].concat());
        let atom = [5760u32.to_le_bytes(), 4320u32.to_le_bytes()].concat();
        let stream = rec(3, true, &rec(1000, true, &[rec(1001, false, &atom), list].concat()));
        let doc = OfficeDoc::open(write(&read(&stream).unwrap()).unwrap(), OfficeFamily::Presentation).unwrap();
        assert_eq!(doc.page_count(), Some(1));
        assert_eq!(doc.page_text(0).unwrap().unwrap(), "Hello\nOne\nTwo\nThree");
        assert_eq!(doc.notes(0).unwrap().as_deref(), Some("Say hello\nthen go"));
    }

    #[test]
    fn cut_powerpoint95_stream_is_an_error() {
        // pptx/PPT95.ppt keeps only the first 8 KB of its document stream, before the slide list.
        let Err(CoreError::BadFile(reason)) = to_pptx(&fixture("pptx/PPT95.ppt")) else { panic!("accepted") };
        assert!(reason.contains("no slide list"), "{reason}");
    }

    #[test]
    fn powerpoint4_is_refused() {
        let Err(CoreError::BadFile(reason)) = to_pptx(&fixture("pptx/pp40only.ppt")) else { panic!("accepted") };
        assert!(reason.starts_with("PowerPoint 95 and older files are not supported"), "{reason}");
    }
}
