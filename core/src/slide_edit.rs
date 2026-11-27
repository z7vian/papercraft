//! PowerPoint editing: open a deck, read its slides and shapes, change them, undo, draw and save.

use std::fmt::Display;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use betteroffice_pptx::{
    EditCtx, PngTarget, Presentation, RenderOptions, ShapeDraft, ShapeKind, ShapeRect, ShapeSnapshot, SlideLayout,
    SlideSnapshot, StorySnapshot, TextParagraphDraft, TextRunDraft, TextStyle, TextStylePatch, UndoCaptureMode,
};

use crate::CoreError;
use crate::office::OfficeFamily;
use crate::office::convert::to_ooxml;

/// A rectangle on the slide in EMU (914400 per inch), measured from the slide's top left.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct DeckRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// Run formatting. Read back, each field is the run's own setting and `None` means it inherits
/// from the placeholder, layout or master; passed to [`SlideEditor::format_text`], `None` leaves it as it is.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct DeckTextStyle {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub size_pt: Option<f64>,
    /// Six hex digits, `RRGGBB`.
    pub color: Option<String>,
    pub font: Option<String>,
}

/// Text in one formatting. A soft line break inside a paragraph is `\u{b}` (vertical tab), as in PowerPoint.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckRun {
    pub text: String,
    pub style: DeckTextStyle,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckParagraph {
    pub runs: Vec<DeckRun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DeckShapeKind {
    Shape,
    TextBox,
    Picture,
    /// A table, chart or other embedded object.
    Frame,
    Group,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckShape {
    pub id: String,
    pub kind: DeckShapeKind,
    pub name: String,
    /// Placeholder type (`title`, `ctrTitle`, `subTitle`, `body`, `obj`, ...), `None` for a free shape.
    pub placeholder: Option<String>,
    /// Where the shape draws, including the geometry a placeholder takes from its layout.
    // Limit: members of a group report the group's child coordinates, not slide coordinates; mapping through the group transform lifts it.
    pub bounds: DeckRect,
    pub rotation_deg: f64,
    /// The group this shape belongs to; a group is listed right before its members.
    pub group_id: Option<String>,
    /// The shape's text, one entry per paragraph; empty when the shape has no text frame.
    /// Joined with `\n`, it is the text that [`SlideEditor::set_shape_text`] takes and whose
    /// UTF-16 offsets [`SlideEditor::format_text`] takes.
    // Limit: a table shows only its first cell's text (one text frame per cell); a list of text frames per shape lifts it.
    pub paragraphs: Vec<DeckParagraph>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckSlide {
    pub id: String,
    /// The layout's name, `None` when the slide names no layout of this deck.
    pub layout: Option<String>,
    /// Index into [`DeckModel::layouts`].
    pub layout_index: Option<u32>,
    pub hidden: bool,
    /// Speaker notes as plain text.
    pub notes: String,
    pub shapes: Vec<DeckShape>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DeckModel {
    pub width_emu: i64,
    pub height_emu: i64,
    pub slides: Vec<DeckSlide>,
    /// Layout names; [`SlideEditor::add_slide`] takes an index into this list.
    pub layouts: Vec<String>,
}

/// An open PowerPoint deck being edited. Every change is one undo step.
#[derive(uniffi::Object)]
pub struct SlideEditor {
    /// The engine's presentation type can't change threads, so one thread owns the deck and runs every call on it.
    jobs: Sender<Job>,
    converted: bool,
}

type Job = Box<dyn FnOnce(&Presentation) + Send>;

// Parsing and layout recurse on nested content, as in `office`.
const ENGINE_STACK: usize = 32 * 1024 * 1024;

fn cant_read(e: impl Display) -> CoreError {
    CoreError::BadFile(format!("Can't read this presentation: {e}"))
}

fn cant_edit(e: impl Display) -> CoreError {
    CoreError::BadFile(format!("Can't make this change: {e}"))
}

fn engine_stopped() -> CoreError {
    CoreError::BadFile("Papercraft stopped editing this file because it hit something it can't handle.".into())
}

fn load(bytes: Vec<u8>, converted: bool) -> Result<Presentation, CoreError> {
    let bytes = if converted { to_ooxml(bytes, OfficeFamily::Presentation)? } else { bytes };
    let deck = Presentation::open(&bytes).map_err(cant_read)?;
    // Each editor call closes its own undo step (see `SlideEditor::edit`), however quickly the next one follows.
    deck.set_undo_capture_mode(UndoCaptureMode::Manual);
    Ok(deck)
}

/// The engine keys a shape's text frame by the shape id; a table's cells use `story:{id}:table:{row}:{cell}`.
fn story_of(shape_id: &str) -> String {
    format!("story:{shape_id}:0")
}

impl SlideEditor {
    fn ask<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Presentation) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        let (reply, answer) = channel();
        self.jobs.send(Box::new(move |deck: &Presentation| drop(reply.send(f(deck))))).map_err(|_| engine_stopped())?;
        answer.recv().map_err(|_| engine_stopped())?
    }

    /// Runs one change as one undo step.
    fn edit<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Presentation, &EditCtx) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        self.ask(move |deck| {
            let done = f(deck, &EditCtx::default());
            deck.add_undo_barrier();
            done
        })
    }
}

#[uniffi::export]
impl SlideEditor {
    /// Opens a .pptx, or an old .ppt (or WPS .dps), which is converted first and then saves as .pptx.
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, family: OfficeFamily) -> Result<Arc<Self>, CoreError> {
        if !matches!(family, OfficeFamily::Presentation) {
            return Err(CoreError::BadFile("Only presentations open in the slide editor.".into()));
        }
        let converted = !bytes.starts_with(b"PK");
        let (jobs, inbox) = channel::<Job>();
        let (opened_tx, opened) = channel();
        thread::Builder::new()
            .name("slide-editor".into())
            .stack_size(ENGINE_STACK)
            .spawn(move || {
                let deck = match load(bytes, converted) {
                    Ok(deck) => deck,
                    Err(e) => return drop(opened_tx.send(Err(e))),
                };
                let _ = opened_tx.send(Ok(()));
                // Runs until the SlideEditor is dropped, which closes the channel.
                for job in inbox {
                    job(&deck);
                }
            })
            .map_err(|e| CoreError::BadFile(format!("Can't start reading this file: {e}")))?;
        opened.recv().map_err(|_| engine_stopped())??;
        Ok(Arc::new(Self { jobs, converted }))
    }

    /// True when the file was an old binary deck: [`Self::save`] then writes .pptx, so it needs a new name.
    pub fn converted(&self) -> bool {
        self.converted
    }

    /// Every slide with its shapes and text, as edited so far.
    pub fn deck(&self) -> Result<DeckModel, CoreError> {
        self.ask(|deck| {
            let snapshot = deck.snapshot().map_err(cant_read)?;
            let layouts = deck.layouts();
            Ok(DeckModel {
                width_emu: snapshot.width_emu,
                height_emu: snapshot.height_emu,
                slides: snapshot.slides.into_iter().map(|slide| deck_slide(slide, layouts)).collect(),
                layouts: layouts.iter().map(|layout| layout.name.clone().unwrap_or_default()).collect(),
            })
        })
    }

    /// PNG of slide `index` as edited, at `scale` (1.0 is 96 dpi), or `None` past the last slide.
    pub fn render_slide(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        self.ask(move |deck| {
            let slide = index as usize;
            if slide >= deck.slide_ids().map_err(cant_read)?.len() {
                return Ok(None);
            }
            let options = RenderOptions { scale, ..Default::default() };
            deck.render_png(PngTarget { slide }, &options).map(|png| Some(png.bytes)).map_err(cant_read)
        })
    }

    /// The edited deck as .pptx bytes. Editing can go on afterwards.
    pub fn save(&self) -> Result<Vec<u8>, CoreError> {
        self.ask(|deck| deck.save().map_err(|e| CoreError::BadFile(format!("Can't save this presentation: {e}"))))
    }

    /// Takes back the last change; false when there is none.
    pub fn undo(&self) -> Result<bool, CoreError> {
        self.ask(|deck| Ok(deck.undo()))
    }

    /// Makes the last undone change again; false when there is none.
    pub fn redo(&self) -> Result<bool, CoreError> {
        self.ask(|deck| Ok(deck.redo()))
    }

    pub fn can_undo(&self) -> Result<bool, CoreError> {
        self.ask(|deck| Ok(deck.can_undo()))
    }

    pub fn can_redo(&self) -> Result<bool, CoreError> {
        self.ask(|deck| Ok(deck.can_redo()))
    }

    /// Replaces the shape's text: `\n` separates paragraphs, `\u{b}` is a soft line break.
    /// Characters that stay keep their formatting; typed ones take the formatting of the character before them.
    pub fn set_shape_text(&self, shape_id: String, text: String) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| set_text(deck, ctx, &shape_id, &text))
    }

    /// Formats the shape's text from UTF-16 offset `start` up to `end`, counted in the text
    /// [`Self::set_shape_text`] takes. `None` fields stay as they are.
    pub fn format_text(&self, shape_id: String, start: u32, end: u32, style: DeckTextStyle) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| {
            deck.format_text(ctx, &story_of(&shape_id), start, end, &style_patch(style)).map(drop).map_err(cant_edit)
        })
    }

    /// Moves and resizes a shape.
    pub fn set_shape_bounds(&self, slide_id: String, shape_id: String, bounds: DeckRect) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| {
            let rect = ShapeRect { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
            deck.set_shape_rect(ctx, &slide_id, &shape_id, rect).map(drop).map_err(cant_edit)
        })
    }

    /// Adds a text box on top of the slide and returns its shape id.
    pub fn add_text_box(&self, slide_id: String, bounds: DeckRect, text: String) -> Result<String, CoreError> {
        self.edit(move |deck, ctx| {
            let draft = ShapeDraft {
                name: "Text Box".into(),
                rect: ShapeRect { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height },
                text: String::new(),
                style: TextStyle::default(),
            };
            let shape_id = deck.add_text_box(ctx, &slide_id, &draft).map_err(cant_edit)?.shape_id;
            if !text.is_empty() {
                set_text(deck, ctx, &shape_id, &text)?;
            }
            Ok(shape_id)
        })
    }

    pub fn delete_shape(&self, slide_id: String, shape_id: String) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| deck.remove_shape(ctx, &slide_id, &shape_id).map(drop).map_err(cant_edit))
    }

    /// Sets the slide's speaker notes; empty text clears them.
    pub fn set_notes(&self, slide_id: String, text: String) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| deck.set_slide_notes(ctx, &slide_id, &text).map_err(cant_edit))
    }

    /// Inserts a slide at `index` on layout `layout_index` of [`DeckModel::layouts`] and returns its id.
    // Limit: the new slide starts without the layout's placeholders, so its title and body are added as text boxes; an engine call that copies layout placeholders lifts it.
    pub fn add_slide(&self, index: u32, layout_index: u32) -> Result<String, CoreError> {
        self.edit(move |deck, ctx| {
            let layout = deck
                .layouts()
                .get(layout_index as usize)
                .ok_or_else(|| cant_edit(format!("this presentation has no layout {layout_index}")))?
                .part_path
                .clone();
            deck.insert_slide(ctx, index, Some(&layout)).map(|done| done.slide_id).map_err(cant_edit)
        })
    }

    /// Copies a slide to `to_index` and returns the copy's id.
    pub fn duplicate_slide(&self, slide_id: String, to_index: u32) -> Result<String, CoreError> {
        self.edit(move |deck, ctx| {
            deck.duplicate_slide(ctx, &slide_id, to_index).map(|done| done.slide_id).map_err(cant_edit)
        })
    }

    pub fn delete_slide(&self, slide_id: String) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| deck.delete_slide(ctx, &slide_id).map(drop).map_err(cant_edit))
    }

    /// Moves a slide so it ends up at `to_index`.
    pub fn move_slide(&self, slide_id: String, to_index: u32) -> Result<(), CoreError> {
        self.edit(move |deck, ctx| deck.move_slide(ctx, &slide_id, to_index).map(drop).map_err(cant_edit))
    }
}

fn set_text(deck: &Presentation, ctx: &EditCtx, shape_id: &str, text: &str) -> Result<(), CoreError> {
    let story = story_of(shape_id);
    let old = deck.story(&story).map_err(cant_edit)?;
    deck.set_story_paragraphs(ctx, &story, &retyped(&old, text)).map(drop).map_err(cant_edit)
}

/// `text` as paragraph drafts over `old`: paragraphs that stay at either end line up with themselves,
/// the rest pair up in order, and inside each pair the characters that stay keep their formatting.
fn retyped(old: &StorySnapshot, text: &str) -> Vec<TextParagraphDraft> {
    let old: Vec<Vec<(char, &TextStyle)>> = old
        .paragraphs
        .iter()
        .map(|paragraph| paragraph.runs.iter().flat_map(|run| run.text.chars().map(move |c| (c, &run.style))).collect())
        .collect();
    // The engine keeps soft line breaks as `\n` inside a paragraph.
    let new: Vec<Vec<char>> =
        text.split('\n').map(|line| line.chars().map(|c| if c == '\u{b}' { '\n' } else { c }).collect()).collect();
    let same = |o: usize, n: usize| old[o].iter().map(|(c, _)| c).eq(new[n].iter());
    let shared = old.len().min(new.len());
    let head = (0..shared).take_while(|&i| same(i, i)).count();
    let tail = (0..shared - head).take_while(|&i| same(old.len() - 1 - i, new.len() - 1 - i)).count();
    let plain = TextStyle::default();
    new.iter()
        .enumerate()
        .map(|(n, line)| {
            let o = if n < head {
                n
            } else if n >= new.len() - tail {
                n + old.len() - new.len()
            } else if n < old.len() - tail {
                n
            } else {
                (old.len() - tail).saturating_sub(1)
            };
            let before = old.get(o).map_or(&[][..], Vec::as_slice);
            TextParagraphDraft { alignment: None, bullet: None, runs: retyped_line(before, line, &plain) }
        })
        .collect()
}

fn retyped_line(old: &[(char, &TextStyle)], new: &[char], plain: &TextStyle) -> Vec<TextRunDraft> {
    let head = old.iter().zip(new).take_while(|((a, _), b)| a == *b).count();
    let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|((a, _), b)| a == *b).count();
    let typed = head.checked_sub(1).and_then(|i| old.get(i)).or(old.get(head)).map_or(plain, |(_, style)| *style);
    let styled = old[..head]
        .iter()
        .copied()
        .chain(new[head..new.len() - tail].iter().map(|&c| (c, typed)))
        .chain(old[old.len() - tail..].iter().copied());
    let mut runs: Vec<TextRunDraft> = Vec::new();
    for (c, style) in styled {
        match runs.last_mut() {
            Some(run) if run.style == *style => run.text.push(c),
            _ => runs.push(TextRunDraft { text: c.to_string(), style: style.clone() }),
        }
    }
    runs
}

fn style_patch(style: DeckTextStyle) -> TextStylePatch {
    TextStylePatch {
        bold: style.bold,
        italic: style.italic,
        font_size_pt: style.size_pt,
        color: style.color,
        font_family: style.font,
        underline: style.underline.map(|on| if on { "sng" } else { "none" }.into()),
        strike: style.strike.map(|on| if on { "sngStrike" } else { "noStrike" }.into()),
        ..Default::default()
    }
}

fn deck_style(style: &TextStyle) -> DeckTextStyle {
    DeckTextStyle {
        bold: style.bold,
        italic: style.italic,
        underline: style.underline.as_deref().map(|kind| kind != "none"),
        strike: style.strike.as_deref().map(|kind| kind != "noStrike"),
        size_pt: style.font_size_pt,
        color: style.color.clone(),
        font: style.font_family.clone(),
    }
}

fn deck_slide(slide: SlideSnapshot, layouts: &[SlideLayout]) -> DeckSlide {
    let layout_index = slide
        .layout_part_path
        .as_deref()
        .and_then(|path| layouts.iter().position(|layout| layout.part_path == path));
    let mut shapes = Vec::new();
    for shape in slide.shapes {
        flatten(shape, None, &mut shapes);
    }
    DeckSlide {
        id: slide.id,
        layout: layout_index.and_then(|i| layouts[i].name.clone()),
        layout_index: layout_index.map(|i| i as u32),
        hidden: slide.hidden.unwrap_or(false),
        notes: slide.notes,
        shapes,
    }
}

fn flatten(shape: ShapeSnapshot, group_id: Option<&str>, out: &mut Vec<DeckShape>) {
    let (bounds, rotation_deg) = match shape.inherited {
        Some(at) => (DeckRect { x: at.x, y: at.y, width: at.width, height: at.height }, at.rotation_deg),
        None => (DeckRect { x: shape.x, y: shape.y, width: shape.width, height: shape.height }, shape.rotation_deg),
    };
    let kind = match shape.kind {
        ShapeKind::Shape if shape.text_box => DeckShapeKind::TextBox,
        ShapeKind::Shape => DeckShapeKind::Shape,
        ShapeKind::Picture => DeckShapeKind::Picture,
        ShapeKind::GraphicFrame => DeckShapeKind::Frame,
        ShapeKind::Group => DeckShapeKind::Group,
    };
    let paragraphs = shape.text_stories.first().map_or_else(Vec::new, |story| {
        story
            .paragraphs
            .iter()
            .map(|paragraph| DeckParagraph {
                runs: paragraph
                    .runs
                    .iter()
                    .map(|run| DeckRun { text: run.text.replace('\n', "\u{b}"), style: deck_style(&run.style) })
                    .collect(),
            })
            .collect()
    });
    let id = shape.id;
    out.push(DeckShape {
        id: id.clone(),
        kind,
        name: shape.name,
        // A placeholder without a type is a content placeholder.
        placeholder: shape.placeholder.map(|ph| ph.placeholder_type.unwrap_or_else(|| "obj".into())),
        bounds,
        rotation_deg,
        group_id: group_id.map(str::to_owned),
        paragraphs,
    });
    for child in shape.children {
        flatten(child, Some(&id), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Apache POI test-data/slideshow/SampleShow.pptx and SampleShow.ppt (Apache-2.0).
    const PPTX: &[u8] = include_bytes!("../tests/fixtures/slide_edit/SampleShow.pptx");
    const PPT: &[u8] = include_bytes!("../tests/fixtures/slide_edit/SampleShow.ppt");

    fn open(bytes: &[u8]) -> Arc<SlideEditor> {
        SlideEditor::open(bytes.to_vec(), OfficeFamily::Presentation).unwrap()
    }

    fn placeholder<'a>(slide: &'a DeckSlide, kind: &str) -> &'a DeckShape {
        slide.shapes.iter().find(|shape| shape.placeholder.as_deref() == Some(kind)).unwrap()
    }

    fn text(shape: &DeckShape) -> String {
        shape
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.runs.iter().map(|run| run.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn title_new_slide_order_and_notes_survive_save_and_reopen() {
        let editor = open(PPTX);
        let deck = editor.deck().unwrap();
        assert_eq!(deck.slides.len(), 2);
        let first = &deck.slides[0];
        let title = placeholder(first, "ctrTitle");
        assert_eq!(text(title), "Title of the first slide");

        editor.set_shape_text(title.id.clone(), "Quarterly plan".into()).unwrap();
        let title_only = deck.layouts.iter().position(|name| name == "Title Only").unwrap() as u32;
        let added = editor.add_slide(2, title_only).unwrap();
        editor.move_slide(added, 0).unwrap();
        editor.set_notes(first.id.clone(), "Speak slowly".into()).unwrap();

        let reopened = open(&editor.save().unwrap()).deck().unwrap();
        assert_eq!(reopened.slides.len(), 3);
        assert_eq!(reopened.slides[0].layout.as_deref(), Some("Title Only"));
        assert_eq!(text(placeholder(&reopened.slides[1], "ctrTitle")), "Quarterly plan");
        assert_eq!(reopened.slides[1].notes, "Speak slowly");
        assert_eq!(text(placeholder(&reopened.slides[2], "title")), "This is the second slide");
    }

    #[test]
    fn undo_restores_each_change_and_redo_brings_it_back() {
        let editor = open(PPTX);
        let deck = editor.deck().unwrap();
        let title = placeholder(&deck.slides[0], "ctrTitle").id.clone();
        editor.set_shape_text(title.clone(), "Changed".into()).unwrap();
        editor.add_slide(0, 0).unwrap();
        assert_eq!(editor.deck().unwrap().slides.len(), 3);

        assert!(editor.undo().unwrap());
        let now = editor.deck().unwrap();
        assert_eq!(now.slides.len(), 2);
        assert_eq!(text(placeholder(&now.slides[0], "ctrTitle")), "Changed");

        assert!(editor.undo().unwrap());
        assert_eq!(editor.deck().unwrap(), deck);
        assert!(!editor.can_undo().unwrap());

        assert!(editor.redo().unwrap());
        assert_eq!(text(placeholder(&editor.deck().unwrap().slides[0], "ctrTitle")), "Changed");
    }

    #[test]
    fn typing_keeps_run_formatting_and_format_text_sets_it() {
        let editor = open(PPTX);
        let subtitle = placeholder(&editor.deck().unwrap().slides[0], "subTitle").id.clone();
        editor
            .set_shape_text(subtitle.clone(), "Subtitle of the first slide\n\nThis bit is in italic green!".into())
            .unwrap();
        editor
            .format_text(subtitle, 0, 8, DeckTextStyle { bold: Some(true), ..Default::default() })
            .unwrap();

        let deck = editor.deck().unwrap();
        let shape = placeholder(&deck.slides[0], "subTitle");
        let green = &shape.paragraphs[2].runs;
        assert_eq!(green.len(), 1);
        assert_eq!(green[0].text, "This bit is in italic green!");
        assert_eq!(green[0].style.italic, Some(true));
        assert!(green[0].style.color.is_some());
        let first = &shape.paragraphs[0].runs;
        assert_eq!(first[0].text, "Subtitle");
        assert_eq!(first[0].style.bold, Some(true));
        assert_ne!(first[1].style.bold, Some(true));
    }

    #[test]
    fn shapes_and_slides_are_added_moved_copied_and_deleted() {
        let editor = open(PPTX);
        let slide = editor.deck().unwrap().slides[1].id.clone();
        let at = DeckRect { x: 914_400, y: 914_400, width: 2_743_200, height: 457_200 };
        let box_id = editor.add_text_box(slide.clone(), at, "Hello".into()).unwrap();
        let find = |deck: &DeckModel| deck.slides[1].shapes.iter().find(|shape| shape.id == box_id).cloned();

        let added = find(&editor.deck().unwrap()).unwrap();
        assert_eq!((added.kind, added.bounds, text(&added)), (DeckShapeKind::TextBox, at, "Hello".to_string()));

        let moved = DeckRect { x: 0, y: 0, ..at };
        editor.set_shape_bounds(slide.clone(), box_id.clone(), moved).unwrap();
        assert_eq!(find(&editor.deck().unwrap()).unwrap().bounds, moved);

        editor.delete_shape(slide.clone(), box_id.clone()).unwrap();
        assert!(find(&editor.deck().unwrap()).is_none());

        let copy = editor.duplicate_slide(slide, 2).unwrap();
        let deck = editor.deck().unwrap();
        assert_eq!(deck.slides.len(), 3);
        assert_eq!(text(placeholder(&deck.slides[2], "title")), "This is the second slide");
        editor.delete_slide(copy).unwrap();
        assert_eq!(editor.deck().unwrap().slides.len(), 2);
    }

    #[test]
    fn renders_the_edited_deck() {
        let editor = open(PPTX);
        let title = placeholder(&editor.deck().unwrap().slides[0], "ctrTitle").id.clone();
        let before = editor.render_slide(0, 0.5).unwrap().unwrap();
        editor.set_shape_text(title, "Something else entirely".into()).unwrap();
        let after = editor.render_slide(0, 0.5).unwrap().unwrap();
        assert!(after.starts_with(b"\x89PNG"));
        assert_ne!(before, after);
        assert!(editor.render_slide(2, 0.5).unwrap().is_none());
    }

    #[test]
    fn legacy_ppt_opens_and_saves_as_pptx() {
        let editor = open(PPT);
        assert!(editor.converted());
        assert_eq!(editor.deck().unwrap().slides.len(), 2);
        assert!(editor.save().unwrap().starts_with(b"PK"));
    }

    #[test]
    fn other_families_and_junk_are_refused() {
        assert!(SlideEditor::open(PPTX.to_vec(), OfficeFamily::Document).is_err());
        assert!(SlideEditor::open(b"not a deck".to_vec(), OfficeFamily::Presentation).is_err());
    }
}
