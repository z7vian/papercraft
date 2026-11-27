//! Word documents. The file is laid out once when it opens; every page is then drawn from that
//! retained layout, so drawing page N costs one page, not the whole document.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use betteroffice_docx::fonts::{FontLoading, FontManager, FontSource, LastResortOffice, embedded_font_definitions};
use docx_edit::EngineSession;
use docx_layout::display_list::{DisplayList, PARAGRAPH_MARK_FACE, Primitive, TextRunPrimitive};
use docx_layout::measure_blocks::FontRequirement;
use docx_raster::{FontChains, GlyphCache, ImageCache, ImageMap, RenderResources, RenderedPage};
use ooxml_text::{FontId, FontStore};
use serde_json::{Value, json};

use super::{Pages, bad};
use crate::CoreError;

/// Space between pages in the layout's page coordinates, as the engine's own renderer asks for.
const PAGE_GAP: f64 = 24.0;

pub(super) struct Words {
    engine: EngineSession,
    pages: u32,
    /// Font ids the layout measured with, by chain key; drawing a page shapes its text with them.
    layout_chains: BTreeMap<String, Vec<u32>>,
    fonts: RefCell<Fonts>,
    /// Glyph outlines and decoded pictures, kept so later pages reuse what earlier ones extracted.
    caches: RefCell<(GlyphCache, ImageCache)>,
}

impl Words {
    pub(super) fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        let bytes = resolve_picture_alternates(bytes)?;
        let (envelope, parts) = docx_parse::parse_docx_s9_wire_parts_with_limits(
            &bytes,
            docx_parse::S9ParseOptions { source_ordinals: true, ..Default::default() },
            &docx_parse::xml::ParseLimits::default(),
        )
        .map_err(|e| bad("document", e))?;
        let package = layout_package(&envelope.document.package).map_err(|e| bad("document", e))?;
        let mut fonts = Fonts::new()?;
        fonts.register_embedded(&parts);
        let engine = EngineSession::new(1);
        docx_edit::seed_from_parsed_docx(engine.doc(), envelope, parts, &bytes).map_err(|e| bad("document", e))?;

        let mut request = docx_edit::layout_request::region_layout_request(&package, PAGE_GAP);
        request["measurement"] = docx_edit::layout_request::measurement(&package);
        let requirements = engine
            .layout_font_requirements_json(&request.to_string())
            .map_err(|e| bad("document", e))?;
        let requirements = font_requirements(&requirements)?;
        fonts.resolve(&requirements)?;
        let layout_chains = fonts.layout_chains(&requirements);
        request["measurement"]["fontChains"] = json!(layout_chains);
        let request = request.to_string();
        docx_layout::with_measure_font_store(&mut fonts.store, || engine.layout_document_with_regions_retained(&request))
            .map_err(|e| bad("document", e))?;
        let pages = u32::try_from(engine.stats().retained_pages).map_err(|e| bad("document", e))?;
        Ok(Self {
            engine,
            pages,
            layout_chains,
            fonts: RefCell::new(fonts),
            caches: RefCell::new((GlyphCache::default(), ImageCache::default())),
        })
    }

    /// The retained layout's display list with only `page` built.
    fn page_list(&self, page: usize) -> Result<DisplayList, CoreError> {
        let mut fonts = self.fonts.borrow_mut();
        // Limit: content the display-list conversion can't read is left off the page (the engine's own renderer
        // then lays the whole file out again through a save); reporting it to the viewer lifts it.
        let (list, _unreadable) = docx_layout::with_measure_font_store(&mut fonts.store, || {
            self.engine.retained_page_display_list(&self.layout_chains, page)
        })
        .map_err(|e| bad("document", e))?;
        Ok(list)
    }

    /// Page `index` drawn at `scale`, with what it could not draw; `None` past the last page.
    fn draw(&self, index: u32, scale: f32) -> Result<Option<RenderedPage>, CoreError> {
        if index >= self.pages {
            return Ok(None);
        }
        let page = index as usize;
        let list = self.page_list(page)?;
        let mut fonts = self.fonts.borrow_mut();
        // Runs layout could not shape are painted from their CSS font, so their faces load now.
        fonts.resolve(&display_requirements(&list, page))?;
        let mut caches = self.caches.borrow_mut();
        let (glyphs, images) = &mut *caches;
        // Pictures reach the display list as `data:` URLs, so no relationship needs bytes supplied here.
        let supplied = ImageMap::new();
        let resources = RenderResources::new(&fonts.store, &fonts.chains, &supplied);
        docx_raster::render_page_cached_scaled(&list, page, &resources, glyphs, images, scale)
            .map(Some)
            .map_err(|e| bad("document", e))
    }
}

impl Pages for Words {
    fn page_count(&self) -> Option<u32> {
        Some(self.pages)
    }

    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        Ok(self.draw(index, scale)?.map(|page| page.bytes))
    }

    // Limit: headers, footers, text boxes and notes are not read (the engine's page text covers body paragraphs and
    // tables); collecting the text runs of those display-list regions lifts it.
    fn page_text(&self, index: u32) -> Result<Option<String>, CoreError> {
        if index >= self.pages {
            return Ok(None);
        }
        let texts = self.engine.page_texts_json(&[index as usize]).map_err(|e| bad("document", e))?;
        let texts: Value = serde_json::from_str(&texts).map_err(|e| bad("document", e))?;
        let paragraphs = texts[0]["paragraphs"].as_array().into_iter().flatten();
        Ok(Some(paragraphs.filter_map(|paragraph| paragraph["text"].as_str()).collect::<Vec<_>>().join("\n")))
    }
}

/// The package fields the region layout request reads.
fn layout_package(package: &docx_parse::S9PackageWire) -> Result<Value, serde_json::Error> {
    let notes = |notes: &Option<Vec<_>>| -> Vec<Value> {
        notes.iter().flatten().map(|note: &docx_parse::Note| json!({"id": note.id, "noteType": note.note_type})).collect()
    };
    let mut document = serde_json::Map::new();
    if let Some(sections) = &package.document.sections {
        document.insert("sections".into(), serde_json::to_value(sections)?);
    }
    if let Some(properties) = &package.document.final_section_properties {
        document.insert("finalSectionProperties".into(), serde_json::to_value(properties)?);
    }
    let mut value = json!({
        "document": document,
        "theme": serde_json::to_value(&package.theme)?,
        "settings": serde_json::to_value(&package.settings)?,
        "footnotes": notes(&package.footnotes),
        "endnotes": notes(&package.endnotes),
    });
    if let Some(styles) = &package.styles {
        value["styles"] = serde_json::to_value(styles)?;
    }
    Ok(value)
}

fn font_requirements(json: &str) -> Result<Vec<FontRequirement>, CoreError> {
    let values: Vec<Value> = serde_json::from_str(json).map_err(|e| bad("document", e))?;
    Ok(values
        .iter()
        .map(|value| FontRequirement {
            key: value["key"].as_str().unwrap_or_default().to_owned(),
            family: value["family"].as_str().unwrap_or(DEFAULT_FAMILY).to_owned(),
            bold: value["bold"].as_bool().unwrap_or(false),
            italic: value["italic"].as_bool().unwrap_or(false),
            scripts: value["scripts"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect(),
            characters: Default::default(),
        })
        .collect())
}

/// The faces the text runs of `page` paint with when layout left them unshaped.
fn display_requirements(list: &DisplayList, page: usize) -> Vec<FontRequirement> {
    let Some(page) = list.pages.get(page) else {
        return Vec::new();
    };
    let regions = [&page.header, &page.footer].into_iter().flatten().map(|region| &region.primitives);
    std::iter::once(&page.primitives)
        .chain(regions)
        .chain(page.note_areas.iter().map(|area| &area.primitives))
        .flatten()
        .filter_map(|primitive| match primitive {
            Primitive::Text(run) => text_requirement(run),
            _ => None,
        })
        .collect()
}

/// A run's face from its CSS font shorthand (`"bold italic 14.67px 'Calibri', sans-serif"`).
fn text_requirement(run: &TextRunPrimitive) -> Option<FontRequirement> {
    let (style, family) = run.font.split_once("px")?;
    let family = family.trim().split(',').next().unwrap_or_default().trim_matches(['\'', '"']);
    let bold = style
        .split_whitespace()
        .any(|word| matches!(word, "bold" | "bolder") || word.parse::<u16>().is_ok_and(|weight| weight >= 600));
    let italic = style.split_whitespace().any(|word| matches!(word, "italic" | "oblique"));
    let own = ooxml_text::paragraph_scripts_with_han_fallback([run.text.as_str()], false);
    let scripts = match run.scripts.as_deref().filter(|scripts| !scripts.is_empty()) {
        None => own,
        Some(paragraph) => {
            let regional = paragraph.iter().any(|script| matches!(script.as_str(), "cjk-jp" | "cjk-kr" | "cjk-tc"));
            let mut scripts = paragraph.clone();
            for script in own {
                if !(regional && script == "cjk-sc") && !scripts.contains(&script) {
                    scripts.push(script);
                }
            }
            scripts
        }
    };
    Some(FontRequirement {
        key: chain_key(family, bold, italic),
        family: family.to_owned(),
        bold,
        italic,
        scripts,
        characters: Default::default(),
    })
}

/// The family layout measures a run with when it names none.
const DEFAULT_FAMILY: &str = "Calibri";

fn chain_key(family: &str, bold: bool, italic: bool) -> String {
    format!("{}|{}|{}", family.trim().to_lowercase(), u8::from(bold), u8::from(italic))
}

fn font_error(e: impl std::fmt::Display) -> CoreError {
    CoreError::BadFile(format!("Can't load a font for this document: {e}"))
}

/// The faces layout measures with and pages paint with: the document's embedded fonts first, then the bundled
/// faces the font manager resolves each requested family to. Layout and drawing share this one store, because the
/// display list names faces by their id in it.
struct Fonts {
    manager: FontManager,
    store: FontStore,
    /// Fallback chain per `family|bold|italic` key, first face first.
    chains: FontChains,
    /// How many embedded faces lead each chain.
    embedded: HashMap<String, usize>,
    /// Requests already resolved, by key and scripts.
    resolved: HashSet<(String, Vec<String>)>,
    /// A face measured as another family's metrics, by byte owner and family.
    views: HashMap<(FontId, String), FontId>,
    /// Faces that are a chain's own family rather than a fallback.
    own: HashSet<(String, FontId)>,
}

impl Fonts {
    fn new() -> Result<Self, CoreError> {
        Ok(Self {
            manager: FontManager::new(&FontSource::default(), FontLoading::OnDemand).map_err(font_error)?,
            store: FontStore::default(),
            chains: FontChains::new(),
            embedded: HashMap::new(),
            resolved: HashSet::new(),
            views: HashMap::new(),
            own: HashSet::new(),
        })
    }

    /// Puts the fonts embedded in the package at the head of their family's chain. A face that does not decode is
    /// skipped: the bundled faces stand in for it.
    fn register_embedded(&mut self, parts: &[(String, Vec<u8>)]) {
        let part = |path: &str| parts.iter().find(|(name, _)| name == path).map(|(_, bytes)| bytes.as_slice());
        let fonts = embedded_font_definitions(
            "word/fontTable.xml",
            part("word/fontTable.xml").unwrap_or_default(),
            part("word/_rels/fontTable.xml.rels").unwrap_or_default(),
            part,
        );
        for font in fonts {
            if font.family.trim().is_empty() {
                continue;
            }
            let Ok(id) = self.store.register_slice(&font.data) else {
                continue;
            };
            let key = chain_key(&font.family, font.bold, font.italic);
            self.own.insert((key.clone(), id));
            let embedded = self.embedded.entry(key.clone()).or_default();
            self.chains.entry(key).or_default().insert(*embedded, id);
            *embedded += 1;
        }
    }

    /// Loads the bundled faces each requirement needs, once per key and scripts.
    fn resolve(&mut self, requirements: &[FontRequirement]) -> Result<(), CoreError> {
        for request in requirements {
            let key = chain_key(&request.family, request.bold, request.italic);
            let resolved = (key.clone(), request.scripts.clone());
            if self.resolved.contains(&resolved) {
                continue;
            }
            let family = if request.family.trim().is_empty() { DEFAULT_FAMILY } else { request.family.as_str() };
            let (bold, italic) = (request.bold, request.italic);
            let (faces, glyph) =
                self.manager.resolve_with_glyph_fallback(family, bold, italic, &request.scripts, LastResortOffice::Word);
            let primary = self.manager.resolves_family_face(family, bold, italic);
            // With scripts asked for, faces outside the family's own resolution are script fallbacks: they go last.
            let own = (!request.scripts.is_empty())
                .then(|| self.manager.resolve(family, bold, italic, &[], LastResortOffice::Word));
            let mut deferred = None;
            let mut scripts = Vec::new();
            for (index, face) in faces.into_iter().enumerate() {
                let base = self.store.register_slice(&face.load().map_err(font_error)?).map_err(font_error)?;
                let script = own.as_ref().is_some_and(|own| {
                    !own.iter().any(|known| known.family == face.family && known.bold == face.bold && known.italic == face.italic)
                });
                if script {
                    scripts.push(base);
                    continue;
                }
                let id = self.measured_as(base, face.measure_as.as_deref().unwrap_or(family));
                if primary && index == 0 {
                    self.own.insert((key.clone(), id));
                }
                if glyph == Some(index) {
                    deferred = Some(id);
                } else {
                    self.append(&key, id);
                }
            }
            for id in deferred.into_iter().chain(scripts) {
                self.append(&key, id);
            }
            self.resolved.insert(resolved);
        }
        Ok(())
    }

    fn append(&mut self, key: &str, id: FontId) {
        let chain = self.chains.entry(key.to_owned()).or_default();
        if !chain.contains(&id) {
            chain.push(id);
        }
    }

    /// `base` measured as Word measures `family`; `base` itself when `family` has no metrics.
    fn measured_as(&mut self, base: FontId, family: &str) -> FontId {
        let owner = self.store.byte_owner(base).unwrap_or(base);
        *self
            .views
            .entry((owner, family.trim().to_lowercase()))
            .or_insert_with(|| self.store.register_named_substitute(base, family).unwrap_or(base))
    }

    /// Every chain as layout reads it, plus the face of the paragraph mark when the requirements name the default
    /// family and its chain starts with that family's own face.
    fn layout_chains(&self, requirements: &[FontRequirement]) -> BTreeMap<String, Vec<u32>> {
        let mut chains: BTreeMap<String, Vec<u32>> =
            self.chains.iter().map(|(key, ids)| (key.clone(), ids.iter().map(|id| id.to_u32()).collect())).collect();
        let key = chain_key(DEFAULT_FAMILY, false, false);
        let named = requirements.iter().any(|request| chain_key(&request.family, request.bold, request.italic) == key);
        if let Some(&head) = self.chains.get(&key).and_then(|chain| chain.first())
            && named
            && self.own.contains(&(key, head))
        {
            chains.insert(PARAGRAPH_MARK_FACE.to_owned(), vec![head.to_u32()]);
        }
        chains
    }
}

/// Markup Compatibility (`mc:`) namespace.
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// The package with every `mc:AlternateContent` inside a picture (`pic:pic`) replaced by its `mc:Fallback`, or by
/// its first `mc:Choice` when it has none. The engine resolves alternate content around drawings but reads a
/// picture's fill only as the picture's own child, so a fill wrapped in alternate content (Word 2008 for Mac wraps a
/// PDF choice and a PNG fallback that way) parsed as a picture without bytes and drew as a grey box.
fn resolve_picture_alternates(bytes: &[u8]) -> Result<Cow<'_, [u8]>, CoreError> {
    let story = |path: &str| path.starts_with("word/") && path.ends_with(".xml");
    let stories = ooxml_opc::unzip_parts_where(bytes, u64::MAX, story).map_err(|e| bad("document", e))?;
    let mut resolved: HashMap<String, Vec<u8>> = stories
        .into_iter()
        .filter_map(|(path, xml)| {
            let xml = std::str::from_utf8(&xml).ok()?;
            Some((path, resolve_alternates_in_pictures(xml)?.into_bytes()))
        })
        .collect();
    if resolved.is_empty() {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut parts = ooxml_opc::unzip_parts(bytes).map_err(|e| bad("document", e))?;
    for (path, data) in &mut parts {
        if let Some(xml) = resolved.remove(path.as_str()) {
            *data = xml;
        }
    }
    ooxml_opc::rezip_parts(&parts).map(Cow::Owned).map_err(|e| bad("document", e))
}

/// `xml` with its pictures' alternate content resolved, or `None` when it has none.
fn resolve_alternates_in_pictures(xml: &str) -> Option<String> {
    if !xml.contains("AlternateContent") {
        return None;
    }
    let mut output: Option<String> = None;
    for prefix in mc_prefixes(xml) {
        let source = output.as_deref().unwrap_or(xml);
        let name = format!("{prefix}:AlternateContent");
        let mut resolved = String::new();
        let (mut copied, mut at) = (0, 0);
        while let Some(start) = find_start_tag(source, at, &name) {
            let Some(end) = element_end(source, start, &name) else {
                break;
            };
            if inside_picture(&source[..start])
                && let Some(branch) = chosen_branch(&source[start..end], prefix)
            {
                resolved.push_str(&source[copied..start]);
                resolved.push_str(branch);
                copied = end;
            }
            at = end;
        }
        if copied > 0 {
            resolved.push_str(&source[copied..]);
            output = Some(resolved);
        }
    }
    output
}

/// The prefixes `xml` binds to the Markup Compatibility namespace.
fn mc_prefixes(xml: &str) -> Vec<&str> {
    let mut prefixes = Vec::new();
    for (at, _) in xml.match_indices(MC) {
        let Some(before) = xml[..at].strip_suffix(['"', '\'']).and_then(|before| before.strip_suffix('=')) else {
            continue;
        };
        let Some(declaration) = before.rfind("xmlns:") else {
            continue;
        };
        let prefix = &before[declaration + "xmlns:".len()..];
        if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') && !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    prefixes
}

/// Where the next start tag named exactly `name` begins, from `from` on.
fn find_start_tag(xml: &str, from: usize, name: &str) -> Option<usize> {
    let mut at = from;
    while let Some(found) = xml[at..].find(name) {
        let start = at + found;
        let tag = start.checked_sub(1).filter(|&open| xml.as_bytes()[open] == b'<');
        let after = xml.as_bytes().get(start + name.len());
        if let Some(open) = tag
            && matches!(after, Some(b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/'))
        {
            return Some(open);
        }
        at = start + name.len();
    }
    None
}

/// The end, past its end tag, of the `name` element whose start tag begins at `start`; same-name descendants nest.
fn element_end(xml: &str, start: usize, name: &str) -> Option<usize> {
    let close = format!("</{name}>");
    let mut depth = 0usize;
    let mut at = start;
    loop {
        let next_close = at + xml[at..].find(&close)?;
        match find_start_tag(xml, at, name).filter(|&open| open < next_close) {
            Some(open) => {
                let tag_end = open + xml[open..].find('>')?;
                at = tag_end + 1;
                if xml.as_bytes()[tag_end - 1] == b'/' {
                    if depth == 0 {
                        return Some(at);
                    }
                } else {
                    depth += 1;
                }
            }
            None => {
                at = next_close + close.len();
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at);
                }
            }
        }
    }
}

/// Whether the text before a point ends inside an open `pic:pic` element.
fn inside_picture(before: &str) -> bool {
    let mut at = before.len();
    while let Some(found) = before[..at].rfind("<pic:pic") {
        if matches!(before.as_bytes().get(found + "<pic:pic".len()), Some(b' ' | b'\t' | b'\r' | b'\n' | b'>')) {
            return before[found..].find("</pic:pic>").is_none();
        }
        at = found;
    }
    false
}

/// The content an alternate-content element stands for: its fallback's, else its first choice's.
fn chosen_branch<'a>(alternate: &'a str, prefix: &str) -> Option<&'a str> {
    ["Fallback", "Choice"].into_iter().find_map(|branch| {
        let name = format!("{prefix}:{branch}");
        let start = find_start_tag(alternate, 1, &name)?;
        let end = element_end(alternate, start, &name)?;
        let tag_end = start + alternate[start..].find('>')?;
        if alternate.as_bytes()[tag_end - 1] == b'/' {
            return Some("");
        }
        Some(&alternate[tag_end + 1..end - name.len() - "</>".len()])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `test` on a thread with the engine's stack, as `OfficeDoc` does: layout recurses on nested content.
    fn on_engine_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(super::super::ENGINE_STACK)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    fn open(name: &str) -> Words {
        let path = format!("{}/tests/fixtures/docx/{name}", env!("CARGO_MANIFEST_DIR"));
        Words::open(&std::fs::read(path).unwrap()).unwrap()
    }

    // Bug51170.docx: Apache POI test-data (Apache License 2.0).
    #[test]
    fn bug51170_lays_out_once_and_draws_its_header_picture() {
        on_engine_stack(|| {
            let doc = open("Bug51170.docx");
            assert_eq!(doc.page_count(), Some(7));
            let first = doc.page_text(0).unwrap().unwrap();
            assert!(first.contains("CONSELHO NACIONAL DA INDUSTRIA DO GOLFE"), "{first}");
            assert!(doc.page_text(7).unwrap().is_none());

            let laid_out = doc.engine.stats();
            for page in [6, 0, 3] {
                assert!(doc.render(page, 1.5).unwrap().unwrap().starts_with(b"\x89PNG"));
            }
            assert!(doc.render(7, 1.0).unwrap().is_none());
            let drawn = doc.engine.stats();
            assert_eq!(drawn.pagination_calls, laid_out.pagination_calls, "drawing pages laid the document out again");
            assert_eq!(drawn.layout_epoch, laid_out.layout_epoch, "drawing pages laid the document out again");

            // Page 2 carries the header logo: a PDF choice with a PNG fallback inside the picture.
            let header = serde_json::to_string(&doc.page_list(1).unwrap().pages[1].header).unwrap();
            assert!(header.contains("data:image/png"), "the header picture has no PNG source");
            let second = doc.draw(1, 1.0).unwrap().unwrap();
            assert_eq!(second.skipped_images, 0, "{:?}", second.skipped_pictures);
        });
    }

    // WordWithAttachments.docx: Apache POI test-data (Apache License 2.0). Its embedded objects show EMF and WMF previews.
    #[test]
    fn word_with_attachments_draws_both_pages_and_their_metafiles() {
        on_engine_stack(|| {
            let doc = open("WordWithAttachments.docx");
            assert_eq!(doc.page_count(), Some(2));
            let first = doc.page_text(0).unwrap().unwrap();
            assert!(first.contains("Русский вариант"), "{first}");
            let laid_out = doc.engine.stats().pagination_calls;
            for page in [1, 0] {
                let drawn = doc.draw(page, 1.0).unwrap().unwrap();
                assert_eq!(drawn.skipped_images, 0, "page {page}: {:?}", drawn.skipped_pictures);
            }
            assert_eq!(doc.engine.stats().pagination_calls, laid_out);
        });
    }

    #[test]
    fn picture_alternate_content_resolves_to_its_fallback() {
        let xml = concat!(
            r#"<w:hdr xmlns:ve="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:p><w:r><w:drawing><pic:pic>"#,
            r#"<pic:nvPicPr/><ve:AlternateContent><ve:Choice Requires="ma"><pic:blipFill><a:blip r:embed="rId1"/></pic:blipFill></ve:Choice>"#,
            r#"<ve:Fallback><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill></ve:Fallback></ve:AlternateContent><pic:spPr/>"#,
            r#"</pic:pic></w:drawing></w:r></w:p></w:hdr>"#,
        );
        let resolved = resolve_alternates_in_pictures(xml).unwrap();
        assert!(resolved.contains(r#"<pic:nvPicPr/><pic:blipFill><a:blip r:embed="rId2"/></pic:blipFill><pic:spPr/>"#), "{resolved}");

        // Alternate content around a whole drawing is the engine's to resolve.
        let run = r#"<w:r xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing/></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r>"#;
        assert_eq!(resolve_alternates_in_pictures(run), None);
    }
}
