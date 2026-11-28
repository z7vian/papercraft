//! Pictures and charts a sheet's drawings (`xl/drawings/drawingN.xml`) anchor, for the cell grid to
//! paint over its cells. A child of `sheet`: it reads the package `SheetDoc` keeps.
//!
//! Pictures come back as their stored image files for the caller to decode. Charts are drawn here by
//! the engine's chart renderer from the values cached in the chart part; a chart family it can't draw
//! comes back as the engine's grey placeholder frame.

use std::collections::{HashMap, HashSet};
use std::iter;
use std::sync::{Arc, Mutex};

use betteroffice_xlsx::fonts::{FontLoading, FontManager, FontSource, LastResortOffice};
use quick_xml::Reader;
use quick_xml::events::Event;
use xlsx_model::styles::Theme;
use xlsx_model::{AnchorCell, AnchorEditAs, AnchorExtent, AnchorPos, ChartAnchor, MAX_COLS, MAX_ROWS, SheetChart, SheetId};
use xlsx_render::{DisplayList, DrawCmd, RenderError, Viewport};

use super::{Book, Loaded, SheetDoc, SheetSizeRun, attr, bad, relationships, resolve_part_path, stopped};
use crate::CoreError;

/// EMU per pixel at 96 dpi.
const EMU_PER_PX: f64 = 9525.0;

/// The longest side of a drawn chart, in image pixels.
const MAX_CHART_PX: f32 = 2048.0;

/// Drawings read per sheet, as the engine bounds chart anchors.
const MAX_DRAWINGS: usize = 1024;

const CHART_DATA_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// A corner of a drawing: a cell and how far into it, in pixels at 96 dpi.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct SheetAnchor {
    pub row: u32,
    pub row_off_px: f32,
    pub col: u32,
    pub col_off_px: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SheetDrawingKind {
    /// [`SheetDoc::drawing_image`] returns the stored image file (PNG, JPEG, GIF, BMP or WebP as saved).
    Picture,
    /// [`SheetDoc::drawing_image`] returns the chart drawn as a PNG.
    Chart,
}

/// What a picture shows of its image: the share of its width or height cut from each side
/// (negative pads it). All 0 for the whole image and for charts.
#[derive(Debug, Clone, Copy, PartialEq, Default, uniffi::Record)]
pub struct SheetCrop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// A picture or chart over the cells, from its top-left corner `from` to its bottom-right `to`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SheetDrawing {
    pub kind: SheetDrawingKind,
    pub from: SheetAnchor,
    pub to: SheetAnchor,
    pub crop: SheetCrop,
    /// The description, title or name the file gives it, for screen readers.
    pub label: Option<String>,
}

#[uniffi::export]
impl SheetDoc {
    /// The pictures and charts the sheet's drawings anchor, bottom one first. Hidden ones, shapes,
    /// text boxes and grouped pictures are left out.
    // Limit: shapes, text boxes, grouped drawings, form controls and anchors wrapped whole in mc:AlternateContent are not listed; drawing them through the engine's DrawingML shapes lifts it.
    pub fn drawings(&self, sheet: u32) -> Result<Vec<SheetDrawing>, CoreError> {
        self.with(sheet, |book, loaded| {
            book.drawings(sheet as usize, loaded).into_iter().map(|placed| placed.record()).collect()
        })
    }

    /// Drawing `index` of [`SheetDoc::drawings`] as an image file: a picture's stored bytes, or a
    /// chart drawn as a PNG at `scale` image pixels per sheet pixel (capped at 2048 pixels a side).
    /// `None` when the drawing or its part is missing.
    pub fn drawing_image(&self, sheet: u32, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        let source = self.with(sheet, |book, loaded| {
            let placed = book.drawings(sheet as usize, loaded).into_iter().nth(index as usize)?;
            let bytes = book.package_part(placed.part())?;
            Some(match placed.content {
                Content::Picture { .. } => Source::Picture(bytes),
                Content::Chart { .. } => Source::Chart {
                    bytes,
                    theme: book.styles.theme.clone(),
                    width: placed.width,
                    height: placed.height,
                },
            })
        })?;
        // Drawn without the workbook's lock, so the grid keeps scrolling meanwhile.
        match source {
            None => Ok(None),
            Some(Source::Picture(bytes)) => Ok(Some(bytes)),
            Some(Source::Chart { bytes, theme, width, height }) => {
                render_chart(&bytes, &theme, width, height, scale, &self.chart_fonts).map(Some)
            }
        }
    }
}

enum Source {
    Picture(Vec<u8>),
    Chart { bytes: Vec<u8>, theme: Theme, width: f32, height: f32 },
}

#[derive(Debug, Clone, PartialEq)]
enum Content {
    /// The image part's path, and its crop in thousandths of a percent.
    Picture { media: String, crop: [i64; 4] },
    /// The chart part's path.
    Chart { part: String },
}

/// A drawing resolved against the sheet's tracks, in pixels at 96 dpi.
struct Placed {
    content: Content,
    from: SheetAnchor,
    to: SheetAnchor,
    width: f32,
    height: f32,
    label: Option<String>,
}

impl Placed {
    fn part(&self) -> &str {
        match &self.content {
            Content::Picture { media, .. } => media,
            Content::Chart { part } => part,
        }
    }

    fn record(self) -> SheetDrawing {
        let (kind, crop) = match self.content {
            Content::Picture { crop, .. } => {
                let share = |inset: i64| (inset as f64 / 100_000.0) as f32;
                let crop = SheetCrop { left: share(crop[0]), top: share(crop[1]), right: share(crop[2]), bottom: share(crop[3]) };
                (SheetDrawingKind::Picture, crop)
            }
            Content::Chart { .. } => (SheetDrawingKind::Chart, SheetCrop::default()),
        };
        SheetDrawing { kind, from: self.from, to: self.to, crop, label: self.label }
    }
}

impl Book {
    /// Sheet `index`'s pictures and charts in paint order, placed on its tracks.
    fn drawings(&self, index: usize, loaded: &Loaded) -> Vec<Placed> {
        let Some(sheet_path) = self.slots.get(index).and_then(|slot| slot.path.as_deref()) else {
            return Vec::new();
        };
        if self.package.is_none() {
            return Vec::new();
        }
        let Some(rels) = super::find(&self.parts, &rels_path(sheet_path)).and_then(relationships) else {
            return Vec::new();
        };
        let sheet_dir = parent(sheet_path);
        let mut targets: Vec<String> = rels
            .values()
            .filter(|rel| !rel.external && rel.kind.as_deref().and_then(|kind| kind.rsplit('/').next()) == Some("drawing"))
            .map(|rel| resolve_part_path(sheet_dir, &rel.target))
            .collect();
        targets.sort();
        targets.dedup();
        let mut placed = Vec::new();
        for drawing in targets {
            let Some(xml) = self.package_part(&drawing) else {
                continue;
            };
            let rels = self.package_part(&rels_path(&drawing)).and_then(|xml| relationships(&xml)).unwrap_or_default();
            let dir = parent(&drawing);
            for anchored in read_drawing(&xml) {
                let Some(rel) = rels.get(anchored.rid()).filter(|rel| !rel.external) else {
                    continue;
                };
                let part = resolve_part_path(dir, &rel.target);
                let content = match anchored.content {
                    Found::Picture { crop, .. } => Content::Picture { media: part, crop },
                    Found::Chart { .. } => Content::Chart { part },
                };
                if let Some(drawing) = place(loaded, anchored.anchor, content, anchored.label) {
                    placed.push(drawing);
                    if placed.len() >= MAX_DRAWINGS {
                        return placed;
                    }
                }
            }
        }
        placed
    }

    /// A part of the file the grid left compressed, inflated.
    fn package_part(&self, name: &str) -> Option<Vec<u8>> {
        let package = self.package.as_ref()?;
        let name = name.trim_start_matches('/');
        let at = package.parts().position(|(part, _)| part.trim_start_matches('/').eq_ignore_ascii_case(name))?;
        package.read(at).ok()
    }
}

/// `dir/_rels/name.rels` for the part `dir/name`.
fn rels_path(part: &str) -> String {
    let (dir, name) = part.rsplit_once('/').unwrap_or(("", part));
    if dir.is_empty() { format!("_rels/{name}.rels") } else { format!("{dir}/_rels/{name}.rels") }
}

fn parent(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// What an anchor in a drawing part shows, by relationship id.
#[derive(Debug, Clone, PartialEq)]
enum Found {
    Picture { rid: String, crop: [i64; 4] },
    Chart { rid: String },
}

#[derive(Debug, Clone, PartialEq)]
struct Anchored {
    anchor: ChartAnchor,
    content: Found,
    label: Option<String>,
}

impl Anchored {
    fn rid(&self) -> &str {
        match &self.content {
            Found::Picture { rid, .. } | Found::Chart { rid } => rid,
        }
    }
}

/// One `xdr:*Anchor` being read.
#[derive(Default)]
struct AnchorState {
    kind: Vec<u8>,
    from: AnchorCell,
    to: AnchorCell,
    ext: AnchorExtent,
    pos: AnchorPos,
    content: Option<Found>,
    hidden: bool,
    label: Option<String>,
    /// The last `cNvPr` before the content: whether it hides it, and its label.
    pending: (bool, Option<String>),
    /// Inside `xdr:grpSp`, whose children sit in the group's own coordinates.
    groups: usize,
    in_chart_data: bool,
}

/// The visible pictures and charts a drawing part anchors, in document (paint) order. Content
/// inside `mc:AlternateContent` counts, the first readable choice winning.
fn read_drawing(xml: &[u8]) -> Vec<Anchored> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut state: Option<AnchorState> = None;
    let mut field: Option<(bool, Vec<u8>)> = None;
    let mut found = Vec::new();
    loop {
        buf.clear();
        let Ok(event) = reader.read_event_into(&mut buf) else {
            break;
        };
        match event {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_vec();
                path.push(name.clone());
                let depth = path.len();
                if depth == 2 && matches!(name.as_slice(), b"twoCellAnchor" | b"oneCellAnchor" | b"absoluteAnchor") {
                    state = Some(AnchorState { kind: name, ..AnchorState::default() });
                    continue;
                }
                let Some(anchor) = state.as_mut() else {
                    continue;
                };
                if anchor.groups > 0 || name == b"grpSp" {
                    anchor.groups += usize::from(name == b"grpSp");
                    continue;
                }
                let emu = |key: &[u8]| attr(&e, key).and_then(|value| value.trim().parse::<i64>().ok()).unwrap_or(0);
                match (depth, name.as_slice()) {
                    (3, b"ext") => anchor.ext = AnchorExtent { cx: emu(b"cx"), cy: emu(b"cy") },
                    (3, b"pos") => anchor.pos = AnchorPos { x: emu(b"x"), y: emu(b"y") },
                    (4, b"col" | b"colOff" | b"row" | b"rowOff") if matches!(path[2].as_slice(), b"from" | b"to") => {
                        field = Some((path[2] == b"from", name.clone()));
                    }
                    (_, b"cNvPr") if anchor.content.is_none() => {
                        let label = ["descr", "title", "name"]
                            .into_iter()
                            .find_map(|key| attr(&e, key.as_bytes()).filter(|text| !text.trim().is_empty()));
                        anchor.pending = (matches!(attr(&e, b"hidden").as_deref(), Some("1" | "true")), label);
                    }
                    (_, b"blip") if anchor.content.is_none() => {
                        if let Some(rid) = attr(&e, b"embed") {
                            anchor.content = Some(Found::Picture { rid, crop: [0; 4] });
                            (anchor.hidden, anchor.label) = std::mem::take(&mut anchor.pending);
                        }
                    }
                    (_, b"srcRect") => {
                        if let Some(Found::Picture { crop, .. }) = &mut anchor.content
                            && *crop == [0; 4]
                        {
                            *crop = [emu(b"l"), emu(b"t"), emu(b"r"), emu(b"b")];
                        }
                    }
                    (_, b"graphicData") => anchor.in_chart_data = attr(&e, b"uri").as_deref() == Some(CHART_DATA_URI),
                    (_, b"chart") if anchor.in_chart_data && anchor.content.is_none() => {
                        if let Some(rid) = attr(&e, b"id") {
                            anchor.content = Some(Found::Chart { rid });
                            (anchor.hidden, anchor.label) = std::mem::take(&mut anchor.pending);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if let (Some((from, name)), Some(anchor)) = (&field, state.as_mut()) {
                    let Some(value) = t.xml10_content().ok().and_then(|text| text.trim().parse::<i64>().ok()) else {
                        continue;
                    };
                    let cell = if *from { &mut anchor.from } else { &mut anchor.to };
                    let index = u32::try_from(value.max(0)).unwrap_or(u32::MAX);
                    match name.as_slice() {
                        b"col" => cell.col = index,
                        b"row" => cell.row = index,
                        b"colOff" => cell.col_off = value,
                        _ => cell.row_off = value,
                    }
                }
            }
            Event::End(_) => {
                field = None;
                let name = path.pop().unwrap_or_default();
                let Some(anchor) = state.as_mut() else {
                    continue;
                };
                if path.len() == 1 {
                    let anchor = state.take().unwrap_or_default();
                    if let (Some(content), false) = (anchor.content, anchor.hidden) {
                        let geometry = match anchor.kind.as_slice() {
                            b"twoCellAnchor" => ChartAnchor::TwoCell { from: anchor.from, to: anchor.to, edit_as: AnchorEditAs::default() },
                            b"oneCellAnchor" => ChartAnchor::OneCell { from: anchor.from, extent: anchor.ext },
                            _ => ChartAnchor::Absolute { pos: anchor.pos, extent: anchor.ext },
                        };
                        found.push(Anchored { anchor: geometry, content, label: anchor.label });
                    }
                } else if name == b"grpSp" {
                    anchor.groups = anchor.groups.saturating_sub(1);
                } else if name == b"graphicData" {
                    anchor.in_chart_data = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    found
}

/// The drawing's corners on the sheet's tracks; `None` when it has no area.
fn place(loaded: &Loaded, anchor: ChartAnchor, content: Content, label: Option<String>) -> Option<Placed> {
    let px = |emu: i64| emu as f64 / EMU_PER_PX;
    let col_x = |col: u32| track_start(&loaded.cols, loaded.default_col_px, col);
    let row_y = |row: u32| track_start(&loaded.rows, loaded.default_row_px, row);
    let (x0, y0, x1, y1) = match anchor {
        ChartAnchor::TwoCell { from, to, .. } => (
            col_x(from.col) + px(from.col_off),
            row_y(from.row) + px(from.row_off),
            col_x(to.col) + px(to.col_off),
            row_y(to.row) + px(to.row_off),
        ),
        ChartAnchor::OneCell { from, extent } => {
            let x = col_x(from.col) + px(from.col_off);
            let y = row_y(from.row) + px(from.row_off);
            (x, y, x + px(extent.cx), y + px(extent.cy))
        }
        ChartAnchor::Absolute { pos, extent } => (px(pos.x), px(pos.y), px(pos.x) + px(extent.cx), px(pos.y) + px(extent.cy)),
    };
    if !(x1 > x0 && y1 > y0) {
        return None;
    }
    let corner = |x: f64, y: f64| {
        let (col, col_off_px) = track_at(&loaded.cols, loaded.default_col_px, x, MAX_COLS);
        let (row, row_off_px) = track_at(&loaded.rows, loaded.default_row_px, y, MAX_ROWS);
        SheetAnchor { row, row_off_px, col, col_off_px }
    };
    Some(Placed {
        content,
        from: corner(x0, y0),
        to: corner(x1, y1),
        width: (x1 - x0) as f32,
        height: (y1 - y0) as f32,
        label,
    })
}

/// Pixels from the sheet's edge to where track `index` starts.
fn track_start(runs: &[SheetSizeRun], default: f32, index: u32) -> f64 {
    let mut px = f64::from(index) * f64::from(default);
    for run in runs.iter().take_while(|run| run.first < index) {
        let count = run.last.min(index - 1) - run.first + 1;
        px += f64::from(count) * f64::from(run.px - default);
    }
    px
}

/// The track (of `limit`) holding the point `px` from the sheet's edge, and how far into it the point is.
fn track_at(runs: &[SheetSizeRun], default: f32, px: f64, limit: u32) -> (u32, f32) {
    let px = px.max(0.0);
    let default = f64::from(default);
    let mut start = 0.0;
    let mut next = 0u32;
    let within = |start: f64, next: u32, size: f64, count: u32| -> Option<(u32, f32)> {
        if size <= 0.0 || px >= start + size * f64::from(count) {
            return None;
        }
        let k = ((px - start) / size).floor().min(f64::from(count - 1)) as u32;
        Some((next + k, (px - start - f64::from(k) * size) as f32))
    };
    for run in runs {
        if run.first > next {
            if let Some(hit) = within(start, next, default, run.first - next) {
                return hit;
            }
            start += default * f64::from(run.first - next);
        }
        let count = run.last - run.first + 1;
        if let Some(hit) = within(start, run.first, f64::from(run.px), count) {
            return hit;
        }
        start += f64::from(run.px) * f64::from(count);
        next = run.last + 1;
    }
    if default <= 0.0 {
        return (next.min(limit - 1), (px - start) as f32);
    }
    let k = ((px - start) / default).floor();
    let col = (f64::from(next) + k).min(f64::from(limit - 1)) as u32;
    (col, (px - start - (f64::from(col - next)) * default) as f32)
}

/// Fonts the chart renderer draws text with, resolved as the engine resolves them for its own PNGs.
#[derive(Default)]
pub(super) struct ChartFonts(Mutex<Option<FontCache>>);

struct FontCache {
    manager: Option<FontManager>,
    raster: xlsx_raster::RasterFonts,
    data: HashMap<(String, bool, bool), Arc<[u8]>>,
    resolved: HashSet<(String, bool, bool, Vec<String>)>,
}

impl FontCache {
    fn new() -> Self {
        Self {
            manager: FontManager::new(&FontSource::default(), FontLoading::OnDemand).ok(),
            raster: xlsx_raster::RasterFonts::default(),
            data: HashMap::new(),
            resolved: HashSet::new(),
        }
    }

    /// Registers the faces every text of `list` draws with.
    fn cover(&mut self, list: &DisplayList) {
        let Some(manager) = &self.manager else {
            return;
        };
        for command in &list.commands {
            let DrawCmd::Text { font_family, bold, italic, text, .. } = command else {
                continue;
            };
            let pieces = xlsx_render::run_pieces(command).unwrap_or_default();
            let own = (font_family.as_deref(), *bold, *italic, &**text);
            let runs = pieces.iter().map(|piece| (piece.font_family.as_deref(), piece.bold, piece.italic, &text[piece.range.clone()]));
            for (family, bold, italic, text) in iter::once(own).chain(runs) {
                let family = family.unwrap_or("Calibri");
                let scripts = ooxml_text::paragraph_scripts_with_han_fallback([text], false);
                let key = (family.trim().to_lowercase(), bold, italic, scripts);
                if self.resolved.contains(&key) {
                    continue;
                }
                for face in manager.resolve(primary_family(family), bold, italic, &key.3, LastResortOffice::Word) {
                    let face_key = (face.family.clone(), face.bold, face.italic);
                    let data = match self.data.get(&face_key) {
                        Some(data) => Arc::clone(data),
                        None => {
                            let Ok(bytes) = face.load() else {
                                continue;
                            };
                            let data: Arc<[u8]> = bytes.into();
                            self.data.insert(face_key, Arc::clone(&data));
                            data
                        }
                    };
                    let _ = self.raster.register_shared_font(family, bold, italic, data, face.bold, face.italic);
                }
                self.resolved.insert(key);
            }
        }
    }
}

/// The family a list such as a chart's `"Calibri", sans-serif` asks for first.
fn primary_family(family: &str) -> &str {
    let head = family.split(',').next().unwrap_or(family).trim();
    let head = head.trim_matches(|c| c == '"' || c == '\'');
    if head.is_empty() { family } else { head }
}

/// The chart part drawn alone into a `width` x `height` (sheet pixels) PNG at `scale`.
fn render_chart(bytes: &[u8], theme: &Theme, width: f32, height: f32, scale: f32, fonts: &ChartFonts) -> Result<Vec<u8>, CoreError> {
    let list = chart_list(bytes, theme, width, height, scale)?;
    let mut cache = fonts.0.lock().map_err(|_| stopped())?;
    let cache = cache.get_or_insert_with(FontCache::new);
    cache.cover(&list);
    xlsx_raster::render_png_with_fonts(&list, &cache.raster).map_err(bad)
}

/// What the engine paints for the chart part alone in a `width` x `height` sheet-pixel frame, scaled
/// by `scale`. A chart it can't read or draw paints its placeholder frame.
fn chart_list(bytes: &[u8], theme: &Theme, width: f32, height: f32, scale: f32) -> Result<DisplayList, CoreError> {
    let space = xlsx_parse::chart_space(bytes, theme).map(Arc::new);
    let mut sheet = xlsx_model::Sheet::new("Chart");
    sheet.format.hide_gridlines = true;
    sheet.charts.push(SheetChart {
        part: "chart".into(),
        drawing: String::new(),
        anchor_index: 0,
        anchor: ChartAnchor::Absolute {
            pos: AnchorPos::default(),
            extent: AnchorExtent {
                cx: (f64::from(width) * EMU_PER_PX).round() as i64,
                cy: (f64::from(height) * EMU_PER_PX).round() as i64,
            },
        },
        refs: Vec::new(),
        raw_xml: None,
    });
    let workbook = xlsx_model::Workbook { sheets: vec![sheet], ..xlsx_model::Workbook::default() };
    let viewport = Viewport { x: 0.0, y: 0.0, width, height };
    let list = xlsx_render::build_display_list_with_charts(&workbook, SheetId(0), &viewport, |chart: &SheetChart| {
        space.clone().ok_or_else(|| RenderError::ChartParseFailed { part: chart.part.clone() })
    })
    .map_err(bad)?;
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    Ok(xlsx_render::display_list::scaled(list, scale.min(MAX_CHART_PX / width.max(height))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::OfficeFamily;

    // PictureAndChart.xlsx: simple-monthly-budget.xlsx (Apache POI test-data, Apache-2.0) with an 8x6
    // checker PNG added at K3 (oneCellAnchor, 80x60 px, a quarter cut from each side), next to its
    // column chart E11 (+6 px, +17.5 px) to I24 (+8 px, +8 px), and a text box the grid leaves out.
    #[test]
    fn reads_anchored_pictures_and_draws_charts() {
        let bytes = std::fs::read(format!("{}/tests/fixtures/sheets/PictureAndChart.xlsx", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let doc = SheetDoc::open(bytes, OfficeFamily::Spreadsheet, None).unwrap();
        let drawings = doc.drawings(0).unwrap();
        assert_eq!(drawings.len(), 2, "{drawings:?}");

        let chart = &drawings[0];
        assert_eq!(chart.kind, SheetDrawingKind::Chart);
        assert_eq!((chart.from.col, chart.from.row, chart.to.col, chart.to.row), (4, 10, 8, 23));
        assert!((chart.from.col_off_px - 6.0).abs() < 0.01 && (chart.from.row_off_px - 17.5).abs() < 0.01, "{chart:?}");
        assert!((chart.to.col_off_px - 8.0).abs() < 0.01 && (chart.to.row_off_px - 8.0).abs() < 0.01, "{chart:?}");
        assert_eq!(chart.label.as_deref(), Some("Column chart comparing Total Monthly Income to Total Montly Expenses."));

        let picture = &drawings[1];
        assert_eq!(picture.kind, SheetDrawingKind::Picture);
        assert_eq!((picture.from.col, picture.from.row), (10, 2));
        assert!((picture.from.col_off_px - 10.0).abs() < 0.01 && picture.from.row_off_px == 0.0, "{picture:?}");
        assert_eq!(picture.crop, SheetCrop { left: 0.25, top: 0.0, right: 0.25, bottom: 0.0 });
        // 80x60 px from 10 px into K3, wherever the sheet's track sizes put its far corner.
        let layout = doc.layout(0).unwrap();
        let (k, cols, rows) = (layout.default_col_width_px, doc.col_widths(0).unwrap(), doc.row_heights(0).unwrap());
        let (col, col_off) = track_at(&cols, k, track_start(&cols, k, 10) + 10.0 + 80.0, MAX_COLS);
        let (row, row_off) = track_at(&rows, layout.default_row_height_px, track_start(&rows, layout.default_row_height_px, 2) + 60.0, MAX_ROWS);
        assert_eq!((picture.to.col, picture.to.row), (col, row), "{picture:?}");
        assert!((picture.to.col_off_px - col_off).abs() < 0.01 && (picture.to.row_off_px - row_off).abs() < 0.01);

        let png = doc.drawing_image(0, 1, 1.0).unwrap().unwrap();
        assert!(png.starts_with(b"\x89PNG"), "the stored picture comes back as saved");

        let chart_png = doc.drawing_image(0, 0, 2.0).unwrap().unwrap();
        assert!(chart_png.starts_with(b"\x89PNG"));
        let (w, h) = png_size(&chart_png);
        let width = (track_start(&cols, k, 8) + 8.0) - (track_start(&cols, k, 4) + 6.0);
        assert!((f64::from(w) - width * 2.0).abs() <= 2.0, "chart drawn at twice its {width} px width, got {w}x{h}");

        // The engine draws this column chart itself rather than its placeholder frame.
        let (part, theme) = {
            let book = doc.book.read().unwrap();
            (book.package_part("xl/charts/chart1.xml").unwrap(), book.styles.theme.clone())
        };
        let list = chart_list(&part, &theme, 300.0, 200.0, 1.0).unwrap();
        assert_eq!(list.charts.len(), 1);
        assert!(!list.charts[0].placeholder, "{:?}", list.charts[0]);
        assert!(list.commands.iter().any(|command| matches!(command, DrawCmd::Text { text, .. } if &**text == "Income")), "series and axis labels are drawn");

        assert_eq!(doc.drawing_image(0, 2, 1.0).unwrap(), None);
    }

    #[test]
    fn a_sheet_without_drawings_has_none() {
        let bytes = std::fs::read(format!("{}/tests/fixtures/sheets/47737.xlsx", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let doc = SheetDoc::open(bytes, OfficeFamily::Spreadsheet, None).unwrap();
        assert!(doc.drawings(0).unwrap().is_empty());
    }

    #[test]
    fn hidden_and_grouped_content_is_skipped() {
        let xml = br#"<xdr:wsDr xmlns:xdr="x" xmlns:a="a" xmlns:r="r">
            <xdr:absoluteAnchor><xdr:pos x="9525" y="19050"/><xdr:ext cx="95250" cy="47625"/>
              <xdr:pic><xdr:nvPicPr><xdr:cNvPr id="1" name="Shown"/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill></xdr:pic></xdr:absoluteAnchor>
            <xdr:oneCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx="9525" cy="9525"/>
              <xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Gone" hidden="1"/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId2"/></xdr:blipFill></xdr:pic></xdr:oneCellAnchor>
            <xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
              <xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>
              <xdr:grpSp><xdr:pic><xdr:blipFill><a:blip r:embed="rId3"/></xdr:blipFill></xdr:pic></xdr:grpSp></xdr:twoCellAnchor>
        </xdr:wsDr>"#;
        let found = read_drawing(xml);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].content, Found::Picture { rid: "rId1".into(), crop: [0; 4] });
        assert_eq!(found[0].label.as_deref(), Some("Shown"));
        assert_eq!(found[0].anchor, ChartAnchor::Absolute { pos: AnchorPos { x: 9525, y: 19050 }, extent: AnchorExtent { cx: 95250, cy: 47625 } });
    }

    #[test]
    fn tracks_map_points_to_cells_both_ways() {
        let runs = [SheetSizeRun { first: 1, last: 2, px: 100.0 }, SheetSizeRun { first: 3, last: 3, px: 0.0 }];
        assert_eq!(track_start(&runs, 64.0, 0), 0.0);
        assert_eq!(track_start(&runs, 64.0, 1), 64.0);
        assert_eq!(track_start(&runs, 64.0, 3), 264.0);
        assert_eq!(track_start(&runs, 64.0, 5), 328.0);
        assert_eq!(track_at(&runs, 64.0, 10.0, MAX_COLS), (0, 10.0));
        assert_eq!(track_at(&runs, 64.0, 170.0, MAX_COLS), (2, 6.0));
        // The hidden column D takes no room: 264 px is where E starts.
        assert_eq!(track_at(&runs, 64.0, 264.0, MAX_COLS), (4, 0.0));
        assert_eq!(track_at(&runs, 64.0, 400.0, MAX_COLS), (6, 8.0));
    }

    fn png_size(png: &[u8]) -> (u32, u32) {
        let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        (be(16), be(20))
    }
}
