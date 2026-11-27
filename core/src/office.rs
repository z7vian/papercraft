use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use crate::CoreError;
use betteroffice_docx::Document;
use betteroffice_pptx::Presentation;
use betteroffice_xlsx::{SheetId, Workbook};
use office_oxide::DocumentFormat;

/// Which kind of office file the caller has; the bytes then decide between the ZIP (OOXML) and the old binary container.
#[derive(Clone, Copy, uniffi::Enum)]
pub enum OfficeFamily {
    Document,
    Presentation,
    Spreadsheet,
}

/// A Word, Excel or PowerPoint file, opened once and drawn one page at a time.
/// Its pages are document pages, slides, or sheets.
#[derive(uniffi::Object)]
pub struct OfficeDoc {
    /// The engine's presentation type can't change threads, so one thread owns the file and runs every call on it.
    jobs: Sender<Job>,
    page_count: Option<u32>,
    page_names: Vec<String>,
}

type Job = Box<dyn FnOnce(&Doc) + Send>;

enum Doc {
    Words(Document),
    Slides(Presentation),
    Sheets(Workbook),
}

// Parsing and layout recurse on nested content (office_oxide grows its own parse stack for the same reason).
const ENGINE_STACK: usize = 32 * 1024 * 1024;

fn bad(what: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this {what}: {e}"))
}

fn engine_stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped reading this file because it hit something it can't handle.".into())
}

/// The old binary formats (Office 97-2003, and WPS which shares them) as the matching OOXML package.
// Limit: the conversion keeps text and simple layout only (images move, sheet styles drop); a faithful converter lifts it.
fn to_ooxml(bytes: Vec<u8>, family: OfficeFamily) -> Result<Vec<u8>, CoreError> {
    let (from, to) = match family {
        OfficeFamily::Document => (DocumentFormat::Doc, DocumentFormat::Docx),
        OfficeFamily::Presentation => (DocumentFormat::Ppt, DocumentFormat::Pptx),
        OfficeFamily::Spreadsheet => (DocumentFormat::Xls, DocumentFormat::Xlsx),
    };
    let doc = office_oxide::Document::from_reader(Cursor::new(bytes), from).map_err(|e| bad("file", e))?;
    let mut out = Cursor::new(Vec::new());
    office_oxide::create::create_from_ir_to_writer(&doc.to_ir(), to, &mut out).map_err(|e| bad("file", e))?;
    Ok(out.into_inner())
}

fn parse(bytes: Vec<u8>, family: OfficeFamily) -> Result<Doc, CoreError> {
    let bytes = if bytes.starts_with(b"PK") { bytes } else { to_ooxml(bytes, family)? };
    Ok(match family {
        OfficeFamily::Document => Doc::Words(Document::open(&bytes).map_err(|e| bad("document", e))?),
        OfficeFamily::Presentation => Doc::Slides(Presentation::open(&bytes).map_err(|e| bad("presentation", e))?),
        OfficeFamily::Spreadsheet => Doc::Sheets(Workbook::open(&bytes).map_err(|e| bad("spreadsheet", e))?),
    })
}

impl Doc {
    fn page_count(&self) -> Option<u32> {
        match self {
            Doc::Words(_) => None,
            Doc::Slides(deck) => Some(deck.slides().len() as u32),
            Doc::Sheets(book) => Some(book.sheet_count() as u32),
        }
    }

    fn page_names(&self) -> Vec<String> {
        match self {
            Doc::Sheets(book) => (0..book.sheet_count() as u32)
                .map(|i| book.sheet(SheetId(i)).map(|s| s.name.clone()).unwrap_or_default())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        let i = index as usize;
        match self {
            Doc::Words(doc) => {
                use betteroffice_docx::{PngTarget, RenderOptions};
                // Limit: every page lays out the whole document again, so long files are slow; a lay-out-once session lifts it.
                match doc.render_png(PngTarget { page: i }, &RenderOptions { scale }) {
                    Ok(page) => Ok(Some(page.bytes)),
                    // The engine has no page count: asking past the last page is an error, the same as a page it can't draw.
                    Err(_) if i > 0 => Ok(None),
                    Err(e) => Err(bad("document", e)),
                }
            }
            Doc::Slides(deck) => {
                use betteroffice_pptx::{PngTarget, RenderOptions};
                if i >= deck.slides().len() {
                    return Ok(None);
                }
                let options = RenderOptions { scale, ..Default::default() };
                deck.render_png(PngTarget { slide: i }, &options).map(|s| Some(s.bytes)).map_err(|e| bad("presentation", e))
            }
            Doc::Sheets(book) => {
                use betteroffice_xlsx::{PngTarget, RenderOptions};
                if i >= book.sheet_count() {
                    return Ok(None);
                }
                // Limit: a sheet draws as one picture capped at 4096 px a side, so big sheets are cut; a cell grid view lifts it.
                let options = RenderOptions { scale, max_width: Some(4096), max_height: Some(4096), ..Default::default() };
                book.render_png(PngTarget::UsedRange { sheet: SheetId(index) }, &options)
                    .map(|s| Some(s.bytes))
                    .map_err(|e| bad("spreadsheet", e))
            }
        }
    }
}

#[uniffi::export]
impl OfficeDoc {
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, family: OfficeFamily) -> Result<Arc<Self>, CoreError> {
        let (jobs, inbox) = channel::<Job>();
        let (opened_tx, opened) = channel();
        thread::Builder::new()
            .name("office-engine".into())
            .stack_size(ENGINE_STACK)
            .spawn(move || {
                let doc = match parse(bytes, family) {
                    Ok(doc) => doc,
                    Err(e) => return drop(opened_tx.send(Err(e))),
                };
                let _ = opened_tx.send(Ok((doc.page_count(), doc.page_names())));
                // Runs until the OfficeDoc is dropped, which closes the channel.
                for job in inbox {
                    job(&doc);
                }
            })
            .map_err(|e| CoreError::BadFile(format!("Can't start reading this file: {e}")))?;
        let (page_count, page_names) = opened.recv().map_err(|_| engine_stopped())??;
        Ok(Arc::new(Self { jobs, page_count, page_names }))
    }

    /// `None` for documents: their page count is known only after layout, so draw pages until
    /// [`Self::render_page`] returns `None`.
    pub fn page_count(&self) -> Option<u32> {
        self.page_count
    }

    /// Sheet names for a workbook, empty otherwise.
    pub fn page_names(&self) -> Vec<String> {
        self.page_names.clone()
    }

    /// PNG of page `index` at `scale` (1.0 is 96 dpi), or `None` past the last page.
    pub fn render_page(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        let (reply, answer) = channel();
        self.jobs
            .send(Box::new(move |doc: &Doc| drop(reply.send(doc.render(index, scale)))))
            .map_err(|_| engine_stopped())?;
        answer.recv().map_err(|_| engine_stopped())?
    }
}
