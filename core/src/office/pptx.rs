use std::io::Cursor;

use betteroffice_pptx::{GraphicFrameData, PngTarget, Presentation, RenderOptions, ShapeNode, Slide, TextBody};
use office_oxide::cfb::CfbReader;

use super::{Pages, bad};
use crate::CoreError;

pub(super) struct Slides(Presentation);

impl Slides {
    pub(super) fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        Presentation::open(bytes).map(Self).map_err(|e| bad("presentation", e))
    }

    fn slide(&self, index: u32) -> Option<&Slide> {
        self.0.slides().get(index as usize)
    }

    /// Text of `shapes` in tree order: a group's shapes at the group's place, table cells row by row,
    /// SmartArt through the drawing PowerPoint saves beside it. Hidden shapes are skipped.
    // Limit: chart titles and labels are left out; reading them from the chart part lifts it.
    fn push_shapes(&self, shapes: &[ShapeNode], out: &mut String) {
        for node in shapes {
            match node {
                ShapeNode::Shape(shape) if !shape.base.hidden => {
                    if let Some(body) = &shape.text {
                        push_body(body, out);
                    }
                }
                ShapeNode::Group(group) if !group.base.hidden => self.push_shapes(&group.children, out),
                ShapeNode::GraphicFrame(frame) if !frame.base.hidden => match &frame.data {
                    GraphicFrameData::Table(table) => {
                        for cell in table.rows.iter().flat_map(|row| &row.cells).filter(|cell| !cell.merged) {
                            push_body(&cell.text, out);
                        }
                    }
                    GraphicFrameData::Diagram { drawing_part_path: Some(path), .. } => {
                        if let Some(drawing) = self.0.package().diagram_drawings.iter().find(|d| &d.part_path == path) {
                            self.push_shapes(&drawing.shapes, out);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

/// One line per paragraph with text; a line break inside a paragraph is already a `\n` run, and breaks or spaces
/// ending a paragraph are dropped.
fn push_body(body: &TextBody, out: &mut String) {
    for paragraph in &body.paragraphs {
        let start = out.len();
        for run in &paragraph.runs {
            out.push_str(&run.text);
        }
        out.truncate(start + out[start..].trim_end().len());
        if out.len() > start {
            out.push('\n');
        }
    }
}

/// PowerPoint 95 and older keep their slides in records office_oxide's converter can't read, so they go to
/// `convert/ppt95.rs` instead. Their mark (as Apache POI reads it): a `PP40` stream, or a `Current User` stream
/// that is only a 4-byte length and a user name. A dual-storage file also holds a PowerPoint 97 copy, which
/// office_oxide converts.
pub(crate) fn is_powerpoint95(bytes: &[u8]) -> bool {
    // Not a compound file: the converter reports what is wrong with it.
    let Ok(mut cfb) = CfbReader::new(Cursor::new(bytes)) else { return false };
    !cfb.has_root_entry("PP97_DUALSTORAGE")
        && (cfb.has_root_entry("PP40") || cfb.open_stream("Current User").is_ok_and(|user| is_pre97_user(&user)))
}

/// A PowerPoint 97 `Current User` stream is a 28-byte-or-longer record; PowerPoint 95 writes a length-prefixed name.
fn is_pre97_user(stream: &[u8]) -> bool {
    stream.len() < 28
        && stream.first_chunk::<4>().is_some_and(|len| u32::from_le_bytes(*len) as usize + 4 == stream.len())
}

impl Pages for Slides {
    fn page_count(&self) -> Option<u32> {
        Some(self.0.slides().len() as u32)
    }

    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        let i = index as usize;
        if i >= self.0.slides().len() {
            return Ok(None);
        }
        let options = RenderOptions { scale, ..Default::default() };
        self.0.render_png(PngTarget { slide: i }, &options).map(|s| Some(s.bytes)).map_err(|e| bad("presentation", e))
    }

    fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        Ok(self.slide(index).map(|slide| {
            let mut text = String::new();
            self.push_shapes(&slide.shapes, &mut text);
            text.truncate(text.trim_end().len());
            text
        }))
    }

    fn notes(&self, index: u32) -> Result<Option<String>, CoreError> {
        Ok(self.slide(index).map(|slide| slide.notes.trim()).filter(|notes| !notes.is_empty()).map(str::to_owned))
    }
}

#[cfg(test)]
mod tests {
    use super::is_powerpoint95;
    use crate::office::{OfficeDoc, OfficeFamily};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/pptx/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    // KEY02.pptx: Apache POI test-data (Apache-2.0), pictures shrunk to 320 px to fit the fixture size cap.
    #[test]
    fn pptx_slide_text_in_reading_order_and_no_notes() {
        let doc = OfficeDoc::open(fixture("KEY02.pptx"), OfficeFamily::Presentation).unwrap();
        assert_eq!(doc.page_count(), Some(1));
        let text = doc.page_text(0).unwrap().unwrap();
        // The title's blank line is an authored second line break.
        assert_eq!(
            text,
            "Welcome to Internet Explorer 9\n\nHTML5, Hardware Accelerated\nDean Hachamovitch\nGeneral Manager\nMicrosoft Corporation"
        );
        assert_eq!(doc.page_text(1).unwrap(), None);
        // The notes page carries a footer and header but an empty notes body.
        assert_eq!(doc.notes(0).unwrap(), None);
    }

    // basic_test_ppt_file.ppt: Apache POI test-data (Apache-2.0), PowerPoint 97-2003.
    #[test]
    fn legacy_ppt_slide_text_and_notes() {
        let bytes = fixture("basic_test_ppt_file.ppt");
        assert!(!is_powerpoint95(&bytes));
        let doc = OfficeDoc::open(bytes, OfficeFamily::Presentation).unwrap();
        assert_eq!(doc.page_count(), Some(2));
        assert!(doc.page_text(0).unwrap().unwrap().contains("This is a test title"));
        assert!(doc.page_text(1).unwrap().unwrap().contains("This is the title on page 2"));
        assert_eq!(doc.notes(0).unwrap().as_deref(), Some("These are the notes for page 1"));
        assert_eq!(doc.notes(1).unwrap().as_deref(), Some("These are the notes on page two, again lacking formatting"));
        assert_eq!(doc.notes(2).unwrap(), None);
    }

    // PPT95.ppt and pp40only.ppt: Apache POI test-data (Apache-2.0), their main stream cut to 8 KB to fit the
    // fixture size cap; every other stream and the directory are the original's.
    #[test]
    fn powerpoint95_and_older_are_detected() {
        assert!(is_powerpoint95(&fixture("PPT95.ppt")));
        assert!(is_powerpoint95(&fixture("pp40only.ppt")));
        assert!(!is_powerpoint95(&fixture("KEY02.pptx")));
    }
}
