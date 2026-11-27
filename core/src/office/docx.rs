use betteroffice_docx::{Document, PngTarget, RenderOptions};

use super::{Pages, bad};
use crate::CoreError;

pub(super) struct Words(Document);

impl Words {
    pub(super) fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        Document::open(bytes).map(Self).map_err(|e| bad("document", e))
    }
}

impl Pages for Words {
    fn page_count(&self) -> Option<u32> {
        None
    }

    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        // Limit: every page lays out the whole document again, so long files are slow; a lay-out-once session lifts it.
        match self.0.render_png(PngTarget { page: index as usize }, &RenderOptions { scale }) {
            Ok(page) => Ok(Some(page.bytes)),
            // The engine has no page count: asking past the last page is an error, the same as a page it can't draw.
            Err(_) if index > 0 => Ok(None),
            Err(e) => Err(bad("document", e)),
        }
    }

    // Limit: no per-page text yet (the engine lays out pages only as pictures here); search needs the lay-out-once session.
    fn page_text(&self, _index: u32) -> Result<Option<String>, CoreError> {
        Ok(None)
    }
}
