//! PDF files through PDFium (`libpdfium.so`, a pinned bblanchon/pdfium-binaries build bundled next to this library).
//!
//! PDFium is not thread-safe, not even across documents, so one engine thread owns the library and every
//! open document, and runs each call in turn.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, LazyLock};
use std::thread;

use pdfium_render::prelude::*;

use crate::CoreError;
use crate::tiff::RgbaImage;

/// Size of a page as drawn (rotation applied), in points (1/72 inch).
#[derive(Clone, Copy, uniffi::Record)]
pub struct PdfPageSize {
    pub width: f32,
    pub height: f32,
}

/// A box on a page as drawn, in points from the page's top-left corner.
#[derive(uniffi::Record)]
pub struct PdfTextBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// One match of a search: its page and one box per line it spans.
#[derive(uniffi::Record)]
pub struct PdfHit {
    pub page: u32,
    pub boxes: Vec<PdfTextBox>,
}

/// A PDF file, opened once and drawn one page at a time.
#[derive(uniffi::Object)]
pub struct PdfFile {
    /// Key of the document in the engine thread's table.
    id: u64,
    sizes: Vec<PdfPageSize>,
}

struct Engine<'a> {
    pdfium: &'a Pdfium,
    docs: HashMap<u64, PdfDocument<'a>>,
}

type Job = Box<dyn for<'a> FnOnce(&mut Engine<'a>) + Send>;

static ENGINE: LazyLock<Result<Sender<Job>, String>> = LazyLock::new(start);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

// Limit: at most 16 MP (64 MB) per page bitmap, under Android's 100 MB draw limit; tile very tall pages to make them sharper.
const MAX_PIXELS: f64 = 16e6;

// PDFium's parser recurses on nested objects.
const ENGINE_STACK: usize = 8 * 1024 * 1024;

fn bad(e: PdfiumError) -> CoreError {
    CoreError::BadFile(format!("Can't read this PDF: {e}"))
}

fn engine_stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped reading this PDF because it hit something it can't handle.".into())
}

fn start() -> Result<Sender<Job>, String> {
    let (jobs, inbox) = channel::<Job>();
    let (ready_tx, ready) = channel();
    thread::Builder::new()
        .name("pdfium".into())
        .stack_size(ENGINE_STACK)
        .spawn(move || {
            let bindings = match Pdfium::bind_to_system_library() {
                Ok(bindings) => bindings,
                Err(e) => return drop(ready_tx.send(Err(format!("Papercraft can't load its PDF engine: {e}")))),
            };
            let pdfium = Pdfium::new(bindings);
            let _ = ready_tx.send(Ok(()));
            let mut engine = Engine { pdfium: &pdfium, docs: HashMap::new() };
            for job in inbox {
                // A panic in one call must not take every open PDF down with it; its caller gets `engine_stopped`.
                let _ = catch_unwind(AssertUnwindSafe(|| job(&mut engine)));
            }
        })
        .map_err(|e| format!("Papercraft can't start its PDF engine: {e}"))?;
    ready.recv().map_err(|_| "Papercraft's PDF engine stopped while starting.".to_string())??;
    Ok(jobs)
}

/// Runs `f` on the engine thread and waits for its answer.
fn ask<T: Send + 'static>(
    f: impl for<'a> FnOnce(&mut Engine<'a>) -> Result<T, CoreError> + Send + 'static,
) -> Result<T, CoreError> {
    let jobs = ENGINE.as_ref().map_err(|e| CoreError::BadFile(e.clone()))?;
    let (reply, answer) = channel();
    jobs.send(Box::new(move |engine: &mut Engine<'_>| drop(reply.send(f(engine))))).map_err(|_| engine_stopped())?;
    answer.recv().map_err(|_| engine_stopped())?
}

/// Pixel size for drawing a page `width` pixels wide, shrunk to fit [`MAX_PIXELS`].
fn fit(size: PdfPageSize, width: u32) -> (u32, u32) {
    let (pw, ph) = (f64::from(size.width.max(1.0)), f64::from(size.height.max(1.0)));
    let w = f64::from(width).min((MAX_PIXELS * pw / ph).sqrt()).max(1.0).round();
    let h = (w * ph / pw).round().max(1.0);
    (w as u32, h as u32)
}

/// A box from page space (points, origin bottom-left, before rotation) to the page as drawn.
fn drawn_box(page: &PdfPage, rect: PdfRect) -> Result<PdfTextBox, CoreError> {
    // PDFium maps page space to device pixels (rotation and crop box included); 64 pixels per point keeps sub-point precision.
    const PER_POINT: f32 = 64.0;
    let (w, h) = (page.width().value, page.height().value);
    let device = PdfRenderConfig::new().set_fixed_size((w * PER_POINT).round() as Pixels, (h * PER_POINT).round() as Pixels);
    let (x1, y1) = page.points_to_pixels(rect.left(), rect.top(), &device).map_err(bad)?;
    let (x2, y2) = page.points_to_pixels(rect.right(), rect.bottom(), &device).map_err(bad)?;
    let pt = |px: Pixels| px as f32 / PER_POINT;
    Ok(PdfTextBox { left: pt(x1.min(x2)), top: pt(y1.min(y2)), right: pt(x1.max(x2)), bottom: pt(y1.max(y2)) })
}

impl PdfFile {
    /// Runs `f` with this file's document on the engine thread.
    fn with_doc<T: Send + 'static>(
        &self,
        f: impl for<'a> FnOnce(&PdfDocument<'a>) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        let id = self.id;
        ask(move |engine| f(engine.docs.get(&id).ok_or_else(engine_stopped)?))
    }

    fn page<'a>(index: u32, doc: &PdfDocument<'a>) -> Result<PdfPage<'a>, CoreError> {
        doc.pages().get(index as PdfPageIndex).map_err(bad)
    }
}

#[uniffi::export]
impl PdfFile {
    /// Opens a PDF held in memory. A locked file fails with `PasswordRequired` when `password` is `None`
    /// and with `WrongPassword` when it doesn't unlock it.
    // Limit: the whole file stays in memory; open from a file descriptor through `load_pdf_from_reader` to stream huge files.
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, password: Option<String>) -> Result<Arc<Self>, CoreError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let sizes = ask(move |engine| {
            let doc = engine.pdfium.load_pdf_from_byte_vec(bytes, password.as_deref()).map_err(|e| match e {
                PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => match password {
                    None => CoreError::PasswordRequired,
                    Some(_) => CoreError::WrongPassword,
                },
                e => bad(e),
            })?;
            let sizes = doc.pages().page_sizes().map_err(bad)?;
            let sizes = sizes.iter().map(|r| PdfPageSize { width: r.width().value, height: r.height().value }).collect();
            engine.docs.insert(id, doc);
            Ok(sizes)
        })?;
        Ok(Arc::new(Self { id, sizes }))
    }

    pub fn page_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    /// Size of page `index` as drawn, or `None` past the last page.
    pub fn page_size(&self, index: u32) -> Option<PdfPageSize> {
        self.sizes.get(index as usize).copied()
    }

    /// Page `index` drawn `width` pixels wide on white (smaller if it would exceed 16 MP), or `None` past the last page.
    /// Pixels are RGBA and opaque, so already premultiplied: ready for `Bitmap.copyPixelsFromBuffer`.
    // Limit: the pixels cross UniFFI as bytes, copied three more times (RustBuffer, Kotlin ByteArray, Bitmap), up to
    // 64 MB each at the 16 MP cap; drawing straight into the Bitmap through one JNI call (plan.md doors 4-5) lifts it.
    pub fn render(&self, index: u32, width: u32) -> Result<Option<RgbaImage>, CoreError> {
        let Some(&size) = self.sizes.get(index as usize) else { return Ok(None) };
        let (w, h) = fit(size, width);
        self.with_doc(move |doc| {
            let page = Self::page(index, doc)?;
            let mut pixels = vec![0u8; w as usize * h as usize * 4];
            {
                // PDFium draws straight into `pixels`; the default config clears to white and swaps BGRA to RGBA.
                let mut bitmap =
                    PdfBitmap::from_bytes(w as Pixels, h as Pixels, PdfBitmapFormat::BGRA, &mut pixels).map_err(bad)?;
                let config = PdfRenderConfig::new().set_fixed_size(w as Pixels, h as Pixels);
                page.render_into_bitmap_with_config(&mut bitmap, &config).map_err(bad)?;
            }
            Ok(Some(RgbaImage { width: w, height: h, pixels }))
        })
    }

    /// The page's text in content order, or `None` past the last page.
    pub fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        if index as usize >= self.sizes.len() {
            return Ok(None);
        }
        self.with_doc(move |doc| Ok(Some(Self::page(index, doc)?.text().map_err(bad)?.all())))
    }

    /// Every case-insensitive match of `query`, in page order.
    // Limit: searches the whole file in one call with no way to stop it; a per-page search lets the UI show hits as they come and cancel.
    pub fn search(&self, query: String) -> Result<Vec<PdfHit>, CoreError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.with_doc(move |doc| {
            let options = PdfSearchOptions::new();
            let mut hits = Vec::new();
            for (index, page) in doc.pages().iter().enumerate() {
                let text = page.text().map_err(bad)?;
                let search = text.search(&query, &options).map_err(bad)?;
                while let Some(segments) = search.find_next() {
                    let boxes = segments.iter().map(|s| drawn_box(&page, s.bounds())).collect::<Result<_, _>>()?;
                    hits.push(PdfHit { page: index as u32, boxes });
                }
            }
            Ok(hits)
        })
    }
}

impl Drop for PdfFile {
    fn drop(&mut self) {
        let id = self.id;
        // A PdfFile exists only once the engine is running. The document must close on the engine thread, like every other PDFium call.
        if let Ok(jobs) = &*ENGINE {
            let _ = jobs.send(Box::new(move |engine: &mut Engine<'_>| drop(engine.docs.remove(&id))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PdfFile;
    use crate::CoreError;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/pdf/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    // tracemonkey.pdf: pdf.js test file (github.com/mozilla/pdf.js test/pdfs, Apache-2.0).
    #[test]
    fn opens_counts_and_reads_text() {
        let pdf = PdfFile::open(fixture("tracemonkey.pdf"), None).unwrap();
        assert_eq!(pdf.page_count(), 14);
        assert!(pdf.page_text(0).unwrap().unwrap().contains("Trace-based"));
        assert!(pdf.page_text(14).unwrap().is_none());
    }

    #[test]
    fn renders_opaque_rgba_at_the_asked_width() {
        let pdf = PdfFile::open(fixture("tracemonkey.pdf"), None).unwrap();
        let size = pdf.page_size(0).unwrap();
        let img = pdf.render(0, 600).unwrap().unwrap();
        assert_eq!(img.width, 600);
        assert_eq!(img.height, (600.0 * size.height / size.width).round() as u32);
        assert_eq!(img.pixels.len(), 600 * img.height as usize * 4);
        assert!(img.pixels.chunks_exact(4).all(|px| px[3] == 255), "every pixel is opaque");
        assert!(img.pixels.chunks_exact(4).any(|px| px[0] < 128), "some text is drawn");
        assert!(pdf.render(14, 600).unwrap().is_none());
    }

    #[test]
    fn huge_widths_stay_under_16_megapixels() {
        let pdf = PdfFile::open(fixture("tracemonkey.pdf"), None).unwrap();
        let img = pdf.render(0, 100_000).unwrap().unwrap();
        assert!(u64::from(img.width) * u64::from(img.height) <= 16_000_000);
    }

    #[test]
    fn search_returns_boxes_on_the_page() {
        let pdf = PdfFile::open(fixture("tracemonkey.pdf"), None).unwrap();
        let hits = pdf.search("trace-based".into()).unwrap();
        let first = hits.first().expect("a hit");
        assert_eq!(first.page, 0);
        let size = pdf.page_size(0).unwrap();
        let b = first.boxes.first().expect("a box");
        assert!(0.0 <= b.left && b.left < b.right && b.right <= size.width, "{:?}", (b.left, b.right));
        assert!(0.0 <= b.top && b.top < b.bottom && b.bottom <= size.height, "{:?}", (b.top, b.bottom));
        // The title sits in the top third of the first page.
        assert!(b.bottom < size.height / 3.0);
        assert!(pdf.search("  ".into()).unwrap().is_empty());
    }

    // issue6010_1.pdf: pdf.js test file (github.com/mozilla/pdf.js test/pdfs, Apache-2.0), user password "abc".
    #[test]
    fn password_files_need_the_right_password() {
        assert!(matches!(PdfFile::open(fixture("issue6010_1.pdf"), None), Err(CoreError::PasswordRequired)));
        assert!(matches!(PdfFile::open(fixture("issue6010_1.pdf"), Some("nope".into())), Err(CoreError::WrongPassword)));
        let pdf = PdfFile::open(fixture("issue6010_1.pdf"), Some("abc".into())).unwrap();
        assert_eq!(pdf.page_count(), 1);
        assert!(pdf.page_text(0).unwrap().unwrap().contains("Issue 6010"));
    }

    #[test]
    fn junk_is_an_error() {
        assert!(matches!(PdfFile::open(b"not a pdf".to_vec(), None), Err(CoreError::BadFile(_))));
    }
}
