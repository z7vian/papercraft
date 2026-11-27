use crate::CoreError;
use crate::tiff::RgbaImage;

fn bad(what: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this {what}: {e}"))
}

/// Renders one page (docx) or slide (pptx), or the used range of the first sheet (xlsx), as PNG bytes.
// Limit: lays the whole file out on every call; a lay-out-once session replaces this before real use.
#[uniffi::export]
pub fn render_office_page(bytes: Vec<u8>, ext: String, index: u32) -> Result<Vec<u8>, CoreError> {
    let index = index as usize;
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "dotm" => {
            use betteroffice_docx::{Document, PngTarget, RenderOptions};
            let doc = Document::open(&bytes).map_err(|e| bad("document", e))?;
            let page = doc.render_png(PngTarget { page: index }, &RenderOptions::default()).map_err(|e| bad("document", e))?;
            Ok(page.bytes)
        }
        "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => {
            use betteroffice_pptx::{PngTarget, Presentation, RenderOptions};
            let deck = Presentation::open(&bytes).map_err(|e| bad("presentation", e))?;
            let slide = deck.render_png(PngTarget { slide: index }, &RenderOptions::default()).map_err(|e| bad("presentation", e))?;
            Ok(slide.bytes)
        }
        "xlsx" | "xlsm" | "xltx" | "xltm" => {
            use betteroffice_xlsx::{PngTarget, RenderOptions, Workbook};
            let book = Workbook::open(&bytes).map_err(|e| bad("spreadsheet", e))?;
            let sheet = book.active_sheet();
            let options = RenderOptions { max_width: Some(2048), max_height: Some(4096), ..Default::default() };
            let png = book.render_png(PngTarget::UsedRange { sheet }, &options).map_err(|e| bad("spreadsheet", e))?;
            Ok(png.bytes)
        }
        _ => Err(CoreError::BadFile(format!("Can't render .{ext} files yet."))),
    }
}

/// Renders one PDF page `width` pixels wide, premultiplied RGBA like [`crate::tiff::decode_tiff`].
#[uniffi::export]
pub fn render_pdf_page(bytes: Vec<u8>, index: u32, width: u32) -> Result<RgbaImage, CoreError> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::{PixmapSettings, RenderCache, RenderSettings};
    let pdf = Pdf::new(bytes).map_err(|e| bad("PDF", format!("{e:?}")))?;
    let pages = pdf.pages();
    let page = pages.get(index as usize).ok_or_else(|| CoreError::BadFile(format!("This PDF has no page {}.", index + 1)))?;
    let (w, _) = page.render_dimensions();
    let scale = width as f32 / w;
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: hayro::vello_cpu::color::palette::css::WHITE };
    let pixmap = hayro::render(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    Ok(RgbaImage { width: pixmap.width() as u32, height: pixmap.height() as u32, pixels: pixmap.data_as_u8_slice().to_vec() })
}
