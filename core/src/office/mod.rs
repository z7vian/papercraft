//! Word, Excel and PowerPoint files for viewing. One file per format family owns its engine:
//! `docx.rs`, `pptx.rs`, `xlsx.rs`; `convert.rs` turns the old binary formats into OOXML first.

mod convert;
mod docx;
mod pptx;
mod xlsx;

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use crate::CoreError;

/// Which kind of office file the caller has; the bytes then decide between the ZIP (OOXML) and the old binary container.
#[derive(Clone, Copy, uniffi::Enum)]
pub enum OfficeFamily {
    Document,
    Presentation,
    Spreadsheet,
}

/// What every format family's engine offers the viewer. Runs on the file's own engine thread.
trait Pages {
    /// `None` when only layout reveals it; pages are then drawn until [`Pages::render`] returns `None`.
    fn page_count(&self) -> Option<u32>;
    /// A caption per page (sheet names), empty when pages have none.
    fn page_names(&self) -> Vec<String> {
        Vec::new()
    }
    /// PNG of page `index` at `scale` (1.0 is 96 dpi), or `None` past the last page.
    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError>;
    /// The page's text in reading order, for search; `None` past the last page.
    fn page_text(&self, index: u32) -> Result<Option<String>, CoreError>;
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

type Job = Box<dyn FnOnce(&dyn Pages) + Send>;

// Parsing and layout recurse on nested content (office_oxide grows its own parse stack for the same reason).
const ENGINE_STACK: usize = 32 * 1024 * 1024;

fn bad(what: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this {what}: {e}"))
}

fn engine_stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped reading this file because it hit something it can't handle.".into())
}

fn parse(bytes: Vec<u8>, family: OfficeFamily) -> Result<Box<dyn Pages>, CoreError> {
    let bytes = if bytes.starts_with(b"PK") { bytes } else { convert::to_ooxml(bytes, family)? };
    Ok(match family {
        OfficeFamily::Document => Box::new(docx::Words::open(&bytes)?),
        OfficeFamily::Presentation => Box::new(pptx::Slides::open(&bytes)?),
        OfficeFamily::Spreadsheet => Box::new(xlsx::Sheets::open(&bytes)?),
    })
}

impl OfficeDoc {
    fn ask<T: Send + 'static>(&self, f: impl FnOnce(&dyn Pages) -> Result<T, CoreError> + Send + 'static) -> Result<T, CoreError> {
        let (reply, answer) = channel();
        self.jobs.send(Box::new(move |doc: &dyn Pages| drop(reply.send(f(doc))))).map_err(|_| engine_stopped())?;
        answer.recv().map_err(|_| engine_stopped())?
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
                    job(doc.as_ref());
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
        self.ask(move |doc| doc.render(index, scale))
    }

    /// The page's text in reading order, for search; `None` past the last page.
    pub fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        self.ask(move |doc| doc.page_text(index))
    }
}
