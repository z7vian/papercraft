use betteroffice_pptx::{PngTarget, Presentation, RenderOptions};

use super::{Pages, bad};
use crate::CoreError;

pub(super) struct Slides(Presentation);

impl Slides {
    pub(super) fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        Presentation::open(bytes).map(Self).map_err(|e| bad("presentation", e))
    }
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

    // Limit: no slide text yet; the slide's text frames supply it.
    fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        Ok((index < self.0.slides().len() as u32).then(String::new))
    }
}
