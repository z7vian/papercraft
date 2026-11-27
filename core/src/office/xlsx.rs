use betteroffice_xlsx::{PngTarget, RenderOptions, SheetId, Workbook};

use super::{Pages, bad};
use crate::CoreError;

pub(super) struct Sheets(Workbook);

impl Sheets {
    pub(super) fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        Workbook::open(bytes).map(Self).map_err(|e| bad("spreadsheet", e))
    }
}

impl Pages for Sheets {
    fn page_count(&self) -> Option<u32> {
        Some(self.0.sheet_count() as u32)
    }

    fn page_names(&self) -> Vec<String> {
        (0..self.0.sheet_count() as u32)
            .map(|i| self.0.sheet(SheetId(i)).map(|s| s.name.clone()).unwrap_or_default())
            .collect()
    }

    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        if index as usize >= self.0.sheet_count() {
            return Ok(None);
        }
        // Limit: a sheet draws as one picture capped at 4096 px a side, so big sheets are cut; a cell grid view lifts it.
        let options = RenderOptions { scale, max_width: Some(4096), max_height: Some(4096), ..Default::default() };
        self.0
            .render_png(PngTarget::UsedRange { sheet: SheetId(index) }, &options)
            .map(|s| Some(s.bytes))
            .map_err(|e| bad("spreadsheet", e))
    }

    // Limit: no cell text yet; the sheet's cells supply it.
    fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        Ok((index < self.0.sheet_count() as u32).then(String::new))
    }
}
