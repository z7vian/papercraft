//! Word editing: one document opened for changes, edited through the engine's parsed model and
//! saved back as .docx.
//!
//! Paragraphs are numbered in document order, table cells included. Positions inside a paragraph
//! are UTF-16 offsets (Kotlin `String` indices) into its text as [`DocParagraph::text`] shows it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use betteroffice_docx::{Document, PngTarget, RenderOptions};
use docx_parse::BlockContent;
use docx_parse::formatting::{NumberingProperties, TextFormatting};
use docx_parse::inline::{DirectionScope, InlineNode, Run, RunContent, RunType};
use docx_parse::paragraph::{Paragraph, ParagraphContent, TrackedInline};
use docx_parse::scalars::{ColorValue, UnderlineValue};
use docx_parse::styles::{DocDefaults, Style, StyleDefinitions};

use crate::CoreError;
use crate::office::{OfficeFamily, convert};

/// Undo steps kept; the oldest drops first.
const HISTORY: usize = 100;
// Parsing and layout recurse on nested content, like the viewer's engine thread.
const ENGINE_STACK: usize = 32 * 1024 * 1024;
/// Scale of the throwaway renders that count pages.
const PROBE_SCALE: f32 = 0.05;
/// What a picture, symbol or note mark reads as in a paragraph's text.
const OBJECT: &str = "\u{fffc}";
const PAGE_BREAK: &str = "\u{c}";

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DocAlign {
    Left,
    Center,
    Right,
    Justify,
}

/// A stretch of text with one look. `editable` is false for text inside links, fields,
/// content controls and tracked changes.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DocRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub size_pt: Option<f32>,
    /// `RRGGBB`, or `None` for automatic and theme colours.
    pub color: Option<String>,
    pub font: Option<String>,
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DocParagraph {
    pub index: u32,
    /// The runs' text joined: tab `\t`, line break `\n`, page break `\u{c}`, picture `\u{fffc}`.
    pub text: String,
    pub style_id: Option<String>,
    pub style_name: Option<String>,
    pub alignment: DocAlign,
    /// Level in a numbered or bulleted list, from 0; `None` outside lists.
    pub list_level: Option<u32>,
    pub runs: Vec<DocRun>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DocTableCell {
    // Limit: a table nested in a cell is flattened into the cell's paragraphs; a recursive cell model lifts it.
    pub paragraphs: Vec<DocParagraph>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct DocTableRow {
    pub cells: Vec<DocTableCell>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum DocBlock {
    Paragraph { paragraph: DocParagraph },
    Table { rows: Vec<DocTableRow> },
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DocStyle {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, uniffi::Record)]
pub struct DocPosition {
    pub paragraph: u32,
    /// UTF-16 offset into the paragraph's text.
    pub offset: u32,
}

/// From `start` to `end`; either order works.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DocRange {
    pub start: DocPosition,
    pub end: DocPosition,
}

/// Character formatting to set; `None` leaves that property as it is.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct DocRunFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// 1 to 1638 points, in half-point steps.
    pub size_pt: Option<f32>,
    /// `RRGGBB` (a leading `#` is fine) or `auto`.
    pub color: Option<String>,
    pub font: Option<String>,
}

/// A Word document open for editing. One thread owns the engine and runs every call on it.
#[derive(uniffi::Object)]
pub struct DocEditor {
    jobs: Sender<Job>,
    converted: bool,
}

type Job = Box<dyn FnOnce(&mut Editor) + Send>;

fn refuse(why: &str) -> CoreError {
    CoreError::BadFile(why.to_owned())
}

fn stopped() -> CoreError {
    refuse("Papercraft stopped editing this file because it hit something it can't handle.")
}

/// The model no longer has what a position named a moment ago.
fn lost() -> CoreError {
    refuse("That part of the document changed while it was being edited.")
}

const SECTION_END: &str = "That paragraph ends a section, so it can't be removed yet.";

impl DocEditor {
    fn ask<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Editor) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, CoreError> {
        let (reply, answer) = channel();
        self.jobs.send(Box::new(move |editor: &mut Editor| drop(reply.send(f(editor))))).map_err(|_| stopped())?;
        answer.recv().map_err(|_| stopped())?
    }

    fn edit(
        &self,
        change: impl FnOnce(&mut Vec<BlockContent>) -> Result<(), CoreError> + Send + 'static,
    ) -> Result<(), CoreError> {
        self.ask(move |editor| editor.edit(change))
    }
}

#[uniffi::export]
impl DocEditor {
    /// Opens a Word file. An old binary .doc is converted first and saves as .docx.
    #[uniffi::constructor]
    pub fn open(bytes: Vec<u8>, family: OfficeFamily) -> Result<Arc<Self>, CoreError> {
        if !matches!(family, OfficeFamily::Document) {
            return Err(refuse("Only Word documents can be edited here."));
        }
        let (jobs, inbox) = channel::<Job>();
        let (opened_tx, opened) = channel();
        thread::Builder::new()
            .name("word-editor".into())
            .stack_size(ENGINE_STACK)
            .spawn(move || {
                let mut editor = match Editor::open(bytes) {
                    Ok(editor) => editor,
                    Err(e) => return drop(opened_tx.send(Err(e))),
                };
                let _ = opened_tx.send(Ok(editor.converted));
                // Runs until the DocEditor is dropped, which closes the channel.
                for job in inbox {
                    job(&mut editor);
                }
            })
            .map_err(|e| CoreError::BadFile(format!("Can't start editing this file: {e}")))?;
        let converted = opened.recv().map_err(|_| stopped())??;
        Ok(Arc::new(Self { jobs, converted }))
    }

    /// True when the file was an old binary .doc: [`Self::save`] returns .docx bytes, so save under a .docx name.
    pub fn converted(&self) -> bool {
        self.converted
    }

    pub fn paragraph_count(&self) -> Result<u32, CoreError> {
        self.ask(|editor| Ok(editor.doc.paragraphs().len() as u32))
    }

    pub fn paragraph(&self, index: u32) -> Result<DocParagraph, CoreError> {
        self.ask(move |editor| {
            let book = StyleBook::new(editor.doc.model().styles.as_ref());
            let paragraphs = editor.doc.paragraphs();
            let paragraph = paragraphs.get(index as usize).ok_or_else(|| no_paragraph(index))?;
            Ok(read_paragraph(paragraph, index, &book))
        })
    }

    /// The whole body: paragraphs and tables in order.
    pub fn blocks(&self) -> Result<Vec<DocBlock>, CoreError> {
        self.ask(|editor| {
            let book = StyleBook::new(editor.doc.model().styles.as_ref());
            let mut blocks = Vec::new();
            read_blocks(&editor.doc.body().content, &book, &mut 0, &mut blocks);
            Ok(blocks)
        })
    }

    /// Paragraph styles a user can pick, in the file's order.
    pub fn styles(&self) -> Result<Vec<DocStyle>, CoreError> {
        self.ask(|editor| {
            let styles = editor.doc.model().styles.as_ref().map(|defs| defs.styles.as_slice()).unwrap_or_default();
            Ok(styles
                .iter()
                .filter(|s| s.style_type == "paragraph" && s.hidden != Some(true) && s.semi_hidden != Some(true))
                .map(|s| DocStyle { id: s.style_id.clone(), name: s.name.clone().unwrap_or_else(|| s.style_id.clone()) })
                .collect())
        })
    }

    /// Replaces the text in `range` with `text`, which takes the look of the first replaced character
    /// (or of the character before an empty range). `\t`, `\n` and `\u{c}` become a tab, a line break and a page break.
    pub fn replace_text(&self, range: DocRange, text: String) -> Result<(), CoreError> {
        self.edit(move |content| replace(content, range, &text))
    }

    /// Inserts a paragraph so it becomes paragraph `index`, styled like the paragraph it goes before
    /// (or after, when `index` is the paragraph count).
    pub fn insert_paragraph(&self, index: u32, text: String) -> Result<(), CoreError> {
        self.edit(move |content| insert_paragraph(content, index, &text))
    }

    pub fn delete_paragraph(&self, index: u32) -> Result<(), CoreError> {
        self.edit(move |content| delete_paragraph(content, index))
    }

    /// Splits a paragraph at `at`, as Enter does; the second half becomes paragraph `at.paragraph + 1`.
    pub fn split_paragraph(&self, at: DocPosition) -> Result<(), CoreError> {
        self.edit(move |content| split_paragraph(content, at))
    }

    /// Joins paragraph `index` with the paragraph after it, keeping the first one's look.
    pub fn merge_paragraph(&self, index: u32) -> Result<(), CoreError> {
        self.edit(move |content| merge_paragraph(content, index))
    }

    pub fn set_run_format(&self, range: DocRange, format: DocRunFormat) -> Result<(), CoreError> {
        let patch = Patch::new(format)?;
        self.edit(move |content| set_run_format(content, range, &patch))
    }

    pub fn set_alignment(&self, index: u32, alignment: DocAlign) -> Result<(), CoreError> {
        self.edit(move |content| {
            let at = spot(content, index)?;
            let formatting = paragraph_mut(content, &at)?.formatting.get_or_insert_with(Default::default);
            formatting.alignment = Some(
                match alignment {
                    DocAlign::Left => "left",
                    DocAlign::Center => "center",
                    DocAlign::Right => "right",
                    DocAlign::Justify => "both",
                }
                .to_owned(),
            );
            Ok(())
        })
    }

    /// Sets paragraph `index`'s style to `style_id`, one of [`Self::styles`].
    pub fn set_style(&self, index: u32, style_id: String) -> Result<(), CoreError> {
        self.ask(move |editor| {
            let known = editor.doc.model().styles.as_ref().is_some_and(|defs| {
                defs.styles.iter().any(|s| s.style_type == "paragraph" && s.style_id == style_id)
            });
            if !known {
                return Err(refuse("This document has no such paragraph style."));
            }
            editor.edit(move |content| {
                let at = spot(content, index)?;
                paragraph_mut(content, &at)?.formatting.get_or_insert_with(Default::default).style_id = Some(style_id);
                Ok(())
            })
        })
    }

    /// Inserts a page break at `at`, as Ctrl+Enter does.
    pub fn insert_page_break(&self, at: DocPosition) -> Result<(), CoreError> {
        self.replace_text(DocRange { start: at, end: at }, PAGE_BREAK.to_owned())
    }

    /// Takes back the last edit; false when there is none.
    pub fn undo(&self) -> Result<bool, CoreError> {
        self.ask(|editor| Ok(editor.undo()))
    }

    /// Redoes the last undone edit; false when there is none.
    pub fn redo(&self) -> Result<bool, CoreError> {
        self.ask(|editor| Ok(editor.redo()))
    }

    pub fn can_undo(&self) -> Result<bool, CoreError> {
        self.ask(|editor| Ok(!editor.undo.is_empty()))
    }

    pub fn can_redo(&self) -> Result<bool, CoreError> {
        self.ask(|editor| Ok(!editor.redo.is_empty()))
    }

    /// Whether the document differs from the file as opened.
    pub fn is_modified(&self) -> Result<bool, CoreError> {
        self.ask(|editor| Ok(editor.state != 0))
    }

    /// Pages of the edited document.
    pub fn page_count(&self) -> Result<u32, CoreError> {
        self.ask(|editor| editor.page_count())
    }

    /// PNG of page `index` of the edited document at `scale` (1.0 is 96 dpi), or `None` past the last page.
    pub fn render_page(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        self.ask(move |editor| editor.render(index, scale))
    }

    /// The document as .docx bytes. Without net edits these are the opened bytes unchanged.
    pub fn save(&self) -> Result<Vec<u8>, CoreError> {
        self.ask(|editor| editor.save())
    }
}

struct Snapshot {
    content: Vec<BlockContent>,
    state: u64,
}

/// The engine's document plus the edit history. Lives on the editor thread.
struct Editor {
    doc: Document,
    /// The .docx as opened (converted for a .doc), saved as is while no edit is in effect.
    origin: Vec<u8>,
    converted: bool,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    /// Names the body now in effect; 0 is the body as opened.
    state: u64,
    last_state: u64,
    pages: Option<u32>,
}

impl Editor {
    fn open(bytes: Vec<u8>) -> Result<Self, CoreError> {
        let converted = !bytes.starts_with(b"PK");
        let bytes = if converted { convert::to_ooxml(bytes, OfficeFamily::Document)? } else { bytes };
        let doc = Document::open(&bytes).map_err(|e| CoreError::BadFile(format!("Can't read this document: {e}")))?;
        Ok(Self {
            doc,
            origin: bytes,
            converted,
            undo: VecDeque::new(),
            redo: Vec::new(),
            state: 0,
            last_state: 0,
            pages: None,
        })
    }

    /// Runs `change` on a copy of the body (paragraphs are shared until changed) and keeps it only if it succeeds.
    // Limit: an edited save rewrites the body, notes, headers and footers from the model (the engine's
    // whole-model write), so markup the model does not carry can drop; saving through the engine's
    // paragraph-patch session lifts it.
    fn edit(&mut self, change: impl FnOnce(&mut Vec<BlockContent>) -> Result<(), CoreError>) -> Result<(), CoreError> {
        let mut content = self.doc.body().content.clone();
        change(&mut content)?;
        let before = std::mem::replace(&mut self.doc.model_mut().body.content, content);
        self.remember(Snapshot { content: before, state: self.state });
        self.redo.clear();
        self.last_state += 1;
        self.state = self.last_state;
        self.pages = None;
        Ok(())
    }

    fn remember(&mut self, snapshot: Snapshot) {
        if self.undo.len() == HISTORY {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }

    fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop_back() else { return false };
        let current = std::mem::replace(&mut self.doc.model_mut().body.content, snapshot.content);
        self.redo.push(Snapshot { content: current, state: self.state });
        self.state = snapshot.state;
        self.pages = None;
        true
    }

    fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else { return false };
        let current = std::mem::replace(&mut self.doc.model_mut().body.content, snapshot.content);
        self.remember(Snapshot { content: current, state: self.state });
        self.state = snapshot.state;
        self.pages = None;
        true
    }

    fn save(&self) -> Result<Vec<u8>, CoreError> {
        if self.state == 0 {
            return Ok(self.origin.clone());
        }
        self.doc.save().map_err(|e| CoreError::BadFile(format!("Can't save this document: {e}")))
    }

    // Limit: every render lays the whole edited document out again; the engine's retained-layout session lifts it.
    fn render(&self, index: u32, scale: f32) -> Result<Option<Vec<u8>>, CoreError> {
        match self.doc.render_png(PngTarget { page: index as usize }, &RenderOptions { scale }) {
            Ok(page) => Ok(Some(page.bytes)),
            Err(e) if e.to_string() == format!("page ordinal {index} is out of range") => Ok(None),
            Err(e) => Err(CoreError::BadFile(format!("Can't draw this document: {e}"))),
        }
    }

    // Limit: the engine has no page count, so this renders tiny pages about 2·log2(pages) times;
    // the engine's retained-layout session lifts it.
    fn page_count(&mut self) -> Result<u32, CoreError> {
        if let Some(pages) = self.pages {
            return Ok(pages);
        }
        let has = |page: u32| self.render(page, PROBE_SCALE).map(|png| png.is_some());
        let pages = if has(0)? {
            // Pages 0..low exist and page high - 1 does not.
            let (mut low, mut high) = (1u32, 2u32);
            while has(high - 1)? {
                low = high;
                high = high.checked_mul(2).ok_or_else(|| refuse("This document has too many pages."))?;
            }
            while high - low > 1 {
                let mid = low + (high - low) / 2;
                if has(mid - 1)? { low = mid } else { high = mid }
            }
            low
        } else {
            0
        };
        self.pages = Some(pages);
        Ok(pages)
    }
}

fn no_paragraph(index: u32) -> CoreError {
    CoreError::BadFile(format!("There is no paragraph {index}."))
}

// ---- Finding paragraphs ----

/// One step from a block list into a nested one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hop {
    Cell { block: usize, row: usize, cell: usize },
    Control { block: usize },
}

/// Where a paragraph sits: the block list it is in, and its place there.
#[derive(Debug, PartialEq, Eq)]
struct Spot {
    hops: Vec<Hop>,
    block: usize,
}

fn find(blocks: &[BlockContent], target: usize, seen: &mut usize, hops: &mut Vec<Hop>) -> Option<usize> {
    for (block, item) in blocks.iter().enumerate() {
        match item {
            BlockContent::Paragraph(_) => {
                if *seen == target {
                    return Some(block);
                }
                *seen += 1;
            }
            BlockContent::Table(table) => {
                for (row, cells) in table.rows.iter().enumerate() {
                    for (cell, content) in cells.cells.iter().enumerate() {
                        hops.push(Hop::Cell { block, row, cell });
                        if let Some(found) = find(&content.content, target, seen, hops) {
                            return Some(found);
                        }
                        hops.pop();
                    }
                }
            }
            BlockContent::BlockSdt(sdt) => {
                hops.push(Hop::Control { block });
                if let Some(found) = find(&sdt.content, target, seen, hops) {
                    return Some(found);
                }
                hops.pop();
            }
            BlockContent::RawXml(_) => {}
        }
    }
    None
}

fn spot(blocks: &[BlockContent], index: u32) -> Result<Spot, CoreError> {
    let mut hops = Vec::new();
    let block = find(blocks, index as usize, &mut 0, &mut hops).ok_or_else(|| no_paragraph(index))?;
    Ok(Spot { hops, block })
}

fn container<'a>(mut blocks: &'a mut Vec<BlockContent>, hops: &[Hop]) -> Result<&'a mut Vec<BlockContent>, CoreError> {
    for &hop in hops {
        let here = blocks;
        blocks = match hop {
            Hop::Cell { block, row, cell } => match here.get_mut(block) {
                Some(BlockContent::Table(table)) => Arc::make_mut(table)
                    .rows
                    .get_mut(row)
                    .and_then(|row| row.cells.get_mut(cell))
                    .map(|cell| &mut cell.content)
                    .ok_or_else(lost)?,
                _ => return Err(lost()),
            },
            Hop::Control { block } => match here.get_mut(block) {
                Some(BlockContent::BlockSdt(sdt)) => &mut Arc::make_mut(sdt).content,
                _ => return Err(lost()),
            },
        };
    }
    Ok(blocks)
}

fn paragraph_mut<'a>(content: &'a mut Vec<BlockContent>, at: &Spot) -> Result<&'a mut Paragraph, CoreError> {
    match container(content, &at.hops)?.get_mut(at.block) {
        Some(BlockContent::Paragraph(paragraph)) => Ok(Arc::make_mut(paragraph)),
        _ => Err(lost()),
    }
}

fn each_paragraph<'a>(blocks: &'a [BlockContent], visit: &mut impl FnMut(&'a Paragraph)) {
    for block in blocks {
        match block {
            BlockContent::Paragraph(paragraph) => visit(paragraph),
            BlockContent::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        each_paragraph(&cell.content, visit);
                    }
                }
            }
            BlockContent::BlockSdt(sdt) => each_paragraph(&sdt.content, visit),
            BlockContent::RawXml(_) => {}
        }
    }
}

// ---- A paragraph's text ----

/// What one piece of a run reads as.
fn piece_text(piece: &RunContent) -> &str {
    match piece {
        RunContent::Text { text, .. } => text,
        RunContent::Tab => "\t",
        RunContent::Break { break_type, .. } if break_type.as_deref() == Some("page") => PAGE_BREAK,
        RunContent::Break { .. } => "\n",
        RunContent::NoBreakHyphen => "\u{2011}",
        RunContent::SoftHyphen => "\u{ad}",
        RunContent::Symbol { .. }
        | RunContent::FootnoteRef { .. }
        | RunContent::EndnoteRef { .. }
        | RunContent::Drawing { .. }
        | RunContent::Shape { .. }
        | RunContent::HorizontalRule { .. }
        | RunContent::Chart { .. }
        | RunContent::OpaqueDrawing { .. } => OBJECT,
        _ => "",
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn piece_len(piece: &RunContent) -> usize {
    utf16_len(piece_text(piece))
}

fn run_len(run: &Run) -> usize {
    run.content.iter().map(piece_len).sum()
}

fn is_run(item: &ParagraphContent) -> bool {
    matches!(item, ParagraphContent::Inline(InlineNode::Run(_)))
}

/// The runs that show text for one paragraph item: the run itself, or the visible runs inside a link,
/// field, content control or tracked insertion.
// Limit: text inside links, fields, content controls and tracked changes is shown but can't be edited
// or formatted; editing their child runs lifts it.
fn item_runs<'a>(item: &'a ParagraphContent, out: &mut Vec<&'a Run>) {
    match item {
        ParagraphContent::Inline(node) => node_runs(node, out),
        ParagraphContent::Tracked(change) => tracked_runs(change, out),
        _ => {}
    }
}

fn node_runs<'a>(node: &'a InlineNode, out: &mut Vec<&'a Run>) {
    match node {
        InlineNode::Run(run) => out.push(run),
        InlineNode::Hyperlink(link) => link.children.iter().for_each(|child| node_runs(child, out)),
        InlineNode::SimpleField(field) => out.extend(&field.content),
        InlineNode::ComplexField(field) => out.extend(&field.field_result),
        InlineNode::InlineSdt(sdt) => sdt.content.iter().for_each(|child| node_runs(child, out)),
        InlineNode::Tracked(change) => tracked_runs(change, out),
        _ => {}
    }
}

fn tracked_runs<'a>(change: &'a TrackedInline, out: &mut Vec<&'a Run>) {
    if matches!(change.node_type.as_str(), "insertion" | "moveTo") {
        change.content.iter().for_each(|child| node_runs(child, out));
    }
}

fn item_len(item: &ParagraphContent) -> usize {
    match item {
        ParagraphContent::Inline(InlineNode::Run(run)) => run_len(run),
        _ => {
            let mut runs = Vec::new();
            item_runs(item, &mut runs);
            runs.into_iter().map(run_len).sum()
        }
    }
}

fn paragraph_len(paragraph: &Paragraph) -> usize {
    paragraph.content.iter().map(item_len).sum()
}

// ---- Reading ----

/// The file's styles, for the look a run or paragraph ends up with.
struct StyleBook<'a> {
    by_id: HashMap<&'a str, &'a Style>,
    defaults: Option<&'a DocDefaults>,
    normal: Option<&'a Style>,
}

impl<'a> StyleBook<'a> {
    fn new(definitions: Option<&'a StyleDefinitions>) -> Self {
        let Some(definitions) = definitions else {
            return Self { by_id: HashMap::new(), defaults: None, normal: None };
        };
        Self {
            by_id: definitions.styles.iter().map(|style| (style.style_id.as_str(), style)).collect(),
            defaults: definitions.doc_defaults.as_ref(),
            normal: definitions.styles.iter().find(|s| s.style_type == "paragraph" && s.default == Some(true)),
        }
    }

    /// Style `id` and the styles it is based on, base first.
    fn chain(&self, id: Option<&str>) -> Vec<&'a Style> {
        let mut chain = Vec::new();
        let mut next = id;
        // A loop of based-on styles stops at the cap.
        while let Some(id) = next
            && chain.len() < 16
        {
            let Some(style) = self.by_id.get(id).copied() else { break };
            chain.push(style);
            next = style.based_on.as_deref();
        }
        chain.reverse();
        chain
    }

    fn paragraph_style(&self, paragraph: &Paragraph) -> Option<&'a Style> {
        match paragraph.formatting.as_ref().and_then(|f| f.style_id.as_deref()) {
            Some(id) => self.by_id.get(id).copied(),
            None => self.normal,
        }
    }
}

/// A run's look, built up from defaults to direct formatting.
#[derive(Default)]
struct Look {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    half_points: Option<f64>,
    color: Option<String>,
    font: Option<String>,
}

impl Look {
    fn over(&mut self, formatting: Option<&TextFormatting>) {
        let Some(f) = formatting else { return };
        if f.bold.is_some() {
            self.bold = f.bold;
        }
        if f.italic.is_some() {
            self.italic = f.italic;
        }
        if let Some(underline) = &f.underline {
            self.underline = Some(underline.style != "none");
        }
        if f.strike.is_some() {
            self.strike = f.strike;
        }
        if f.font_size.is_some() {
            self.half_points = f.font_size;
        }
        // Limit: theme colours read as None; resolving them against the theme part lifts it.
        if let Some(color) = &f.color {
            self.color = color.rgb.clone().filter(|rgb| !rgb.eq_ignore_ascii_case("auto"));
        }
        if let Some(name) = f.font_family.as_ref().and_then(|family| family.ascii.clone().or_else(|| family.h_ansi.clone())) {
            self.font = Some(name);
        }
    }
}

fn read_run(book: &StyleBook, paragraph_chain: &[&Style], run: &Run, text: String, editable: bool) -> DocRun {
    let mut look = Look::default();
    look.over(book.defaults.and_then(|defaults| defaults.r_pr.as_ref()));
    for style in paragraph_chain {
        look.over(style.r_pr.as_ref());
    }
    for style in book.chain(run.formatting.as_ref().and_then(|f| f.style_id.as_deref())) {
        look.over(style.r_pr.as_ref());
    }
    look.over(run.formatting.as_ref());
    DocRun {
        text,
        bold: look.bold.unwrap_or(false),
        italic: look.italic.unwrap_or(false),
        underline: look.underline.unwrap_or(false),
        strike: look.strike.unwrap_or(false),
        size_pt: look.half_points.map(|half| (half / 2.0) as f32),
        color: look.color,
        font: look.font,
        editable,
    }
}

fn read_paragraph(paragraph: &Paragraph, index: u32, book: &StyleBook) -> DocParagraph {
    let style = book.paragraph_style(paragraph);
    let chain = book.chain(style.map(|s| s.style_id.as_str()));
    let direct = paragraph.formatting.as_ref();
    let mut alignment = None;
    let mut numbering: Option<&NumberingProperties> = None;
    let layers = book
        .defaults
        .and_then(|defaults| defaults.p_pr.as_ref())
        .into_iter()
        .chain(chain.iter().filter_map(|style| style.p_pr.as_ref()))
        .chain(direct);
    for layer in layers {
        if layer.alignment.is_some() {
            alignment = layer.alignment.as_deref();
        }
        if let Some(found) = layer.num_pr_from_style.as_ref() {
            numbering = Some(found);
        }
        if let Some(found) = layer.num_pr.as_ref() {
            numbering = Some(found);
        }
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    for item in &paragraph.content {
        let mut shown = Vec::new();
        item_runs(item, &mut shown);
        for run in shown {
            let run_text: String = run.content.iter().map(piece_text).collect();
            if run_text.is_empty() {
                continue;
            }
            text.push_str(&run_text);
            runs.push(read_run(book, &chain, run, run_text, is_run(item)));
        }
    }
    DocParagraph {
        index,
        text,
        style_id: style.map(|s| s.style_id.clone()).or_else(|| direct.and_then(|f| f.style_id.clone())),
        style_name: style.and_then(|s| s.name.clone()),
        alignment: match alignment {
            Some("center") => DocAlign::Center,
            Some("right" | "end") => DocAlign::Right,
            Some(
                "both" | "justify" | "distribute" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute",
            ) => DocAlign::Justify,
            _ => DocAlign::Left,
        },
        list_level: numbering
            .filter(|n| n.num_id.is_some_and(|id| id > 0.0))
            .map(|n| n.ilvl.unwrap_or(0.0).max(0.0) as u32),
        runs,
    }
}

fn read_blocks(blocks: &[BlockContent], book: &StyleBook, next: &mut u32, out: &mut Vec<DocBlock>) {
    for block in blocks {
        match block {
            BlockContent::Paragraph(paragraph) => {
                out.push(DocBlock::Paragraph { paragraph: read_paragraph(paragraph, *next, book) });
                *next += 1;
            }
            BlockContent::Table(table) => {
                let rows = table
                    .rows
                    .iter()
                    .map(|row| DocTableRow {
                        cells: row
                            .cells
                            .iter()
                            .map(|cell| {
                                let mut paragraphs = Vec::new();
                                each_paragraph(&cell.content, &mut |paragraph| {
                                    paragraphs.push(read_paragraph(paragraph, *next, book));
                                    *next += 1;
                                });
                                DocTableCell { paragraphs }
                            })
                            .collect(),
                    })
                    .collect();
                out.push(DocBlock::Table { rows });
            }
            BlockContent::BlockSdt(sdt) => read_blocks(&sdt.content, book, next, out),
            BlockContent::RawXml(_) => {}
        }
    }
}

// ---- Editing one paragraph's runs ----

fn text_piece(text: String) -> RunContent {
    RunContent::Text { preserve_space: (text.trim() != text).then_some(true), text }
}

/// The byte index `units` UTF-16 units into `text`, unless that falls inside a character.
fn byte_at(text: &str, units: usize) -> Option<usize> {
    let mut seen = 0;
    for (byte, c) in text.char_indices() {
        if seen == units {
            return Some(byte);
        }
        if seen > units {
            return None;
        }
        seen += c.len_utf16();
    }
    (seen == units).then_some(text.len())
}

fn mid_character() -> CoreError {
    refuse("That position falls inside a character.")
}

/// Cuts `run` at `at` units into its text, returning the part after the cut.
fn split_run(run: &mut Run, at: usize) -> Result<Run, CoreError> {
    let mut pos = 0;
    let mut cut = run.content.len();
    let mut carried = None;
    for (index, piece) in run.content.iter_mut().enumerate() {
        if pos == at {
            cut = index;
            break;
        }
        let len = piece_len(piece);
        if at < pos + len {
            let RunContent::Text { text, .. } = piece else { return Err(mid_character()) };
            let byte = byte_at(text, at - pos).ok_or_else(mid_character)?;
            let right = text.split_off(byte);
            let left = std::mem::take(text);
            *piece = text_piece(left);
            carried = Some(text_piece(right));
            cut = index + 1;
            break;
        }
        pos += len;
    }
    let mut tail: Vec<RunContent> = carried.into_iter().collect();
    tail.extend(run.content.drain(cut..));
    Ok(Run {
        node_type: RunType::Run,
        formatting: run.formatting.clone(),
        property_changes: run.property_changes.clone(),
        content: tail,
        direction_scopes: run.direction_scopes.clone(),
    })
}

/// Makes `offset` an item boundary in `paragraph`, splitting a run if needed, and returns the
/// index of the first item at or after it.
fn cut(paragraph: &mut Paragraph, offset: usize) -> Result<usize, CoreError> {
    let mut pos = 0;
    for index in 0..paragraph.content.len() {
        if pos == offset {
            return Ok(index);
        }
        let len = item_len(&paragraph.content[index]);
        if offset < pos + len {
            let ParagraphContent::Inline(InlineNode::Run(run)) = &mut paragraph.content[index] else {
                return Err(refuse(
                    "That text is inside a link, field or tracked change, which can't be edited yet.",
                ));
            };
            let tail = split_run(run, offset - pos)?;
            paragraph.content.insert(index + 1, ParagraphContent::Inline(InlineNode::Run(tail)));
            return Ok(index + 1);
        }
        pos += len;
    }
    if pos == offset {
        Ok(paragraph.content.len())
    } else {
        Err(refuse("That position is past the end of the paragraph."))
    }
}

/// Deletes the text in `from..to`. Zero-width marks (bookmarks, comment anchors, field characters) stay.
fn delete(paragraph: &mut Paragraph, from: usize, to: usize) -> Result<(), CoreError> {
    if from == to {
        return Ok(());
    }
    let start = cut(paragraph, from)?;
    let end = cut(paragraph, to)?;
    let kept: Vec<ParagraphContent> = paragraph
        .content
        .drain(start..end)
        .filter_map(|mut item| {
            if let ParagraphContent::Inline(InlineNode::Run(run)) = &mut item {
                run.content.retain(|piece| piece_len(piece) == 0);
                return (!run.content.is_empty()).then_some(item);
            }
            (item_len(&item) == 0).then_some(item)
        })
        .collect();
    paragraph.content.splice(start..start, kept);
    Ok(())
}

/// Formatting and text direction for new text.
type RunLook = (Option<TextFormatting>, Vec<DirectionScope>);

/// The look of the character after `at` (`forward`) or before it, else of any run, else of the paragraph mark.
fn look_at(paragraph: &Paragraph, at: usize, forward: bool) -> RunLook {
    let (mut before, mut after, mut first) = (None, None, None);
    let mut pos = 0;
    for item in &paragraph.content {
        let len = item_len(item);
        if let ParagraphContent::Inline(InlineNode::Run(run)) = item
            && len > 0
        {
            first.get_or_insert(run);
            if pos < at && at <= pos + len {
                before = Some(run);
            }
            if pos <= at && at < pos + len {
                after = Some(run);
            }
        }
        pos += len;
    }
    let pick = if forward { after.or(before) } else { before.or(after) }.or(first);
    match pick {
        Some(run) => (run.formatting.clone(), run.direction_scopes.clone()),
        None => (paragraph.formatting.as_ref().and_then(|f| f.run_properties.clone()), Vec::new()),
    }
}

/// `text` as run pieces: tabs, line and page breaks become their elements, other control characters drop.
fn pieces(text: &str) -> Vec<RunContent> {
    let mut out = Vec::new();
    let mut plain = String::new();
    for c in text.chars() {
        let special = match c {
            '\t' => RunContent::Tab,
            '\n' | '\u{b}' => RunContent::Break { break_type: None, clear: None },
            '\u{c}' => RunContent::Break { break_type: Some("page".to_owned()), clear: None },
            '\u{fffc}' => continue,
            c if c.is_control() => continue,
            c => {
                plain.push(c);
                continue;
            }
        };
        if !plain.is_empty() {
            out.push(text_piece(std::mem::take(&mut plain)));
        }
        out.push(special);
    }
    if !plain.is_empty() {
        out.push(text_piece(plain));
    }
    out
}

fn insert(paragraph: &mut Paragraph, at: usize, text: &str, look: RunLook) -> Result<(), CoreError> {
    let content = pieces(text);
    let index = cut(paragraph, at)?;
    if content.is_empty() {
        return Ok(());
    }
    let (formatting, direction_scopes) = look;
    let run = Run { node_type: RunType::Run, formatting, property_changes: None, content, direction_scopes };
    paragraph.content.insert(index, ParagraphContent::Inline(InlineNode::Run(run)));
    Ok(())
}

fn has_field(run: &Run) -> bool {
    run.content.iter().any(|piece| matches!(piece, RunContent::FieldChar { .. } | RunContent::InstrText { .. }))
}

fn same_look(a: &Run, b: &Run) -> bool {
    a.formatting == b.formatting
        && a.property_changes == b.property_changes
        && a.direction_scopes == b.direction_scopes
        && !has_field(a)
        && !has_field(b)
}

/// Drops empty runs, joins neighbouring runs with the same look and their neighbouring text.
fn tidy(paragraph: &mut Paragraph) {
    let mut out: Vec<ParagraphContent> = Vec::with_capacity(paragraph.content.len());
    for mut item in paragraph.content.drain(..) {
        if let ParagraphContent::Inline(InlineNode::Run(run)) = &mut item {
            run.content.retain(|piece| !matches!(piece, RunContent::Text { text, .. } if text.is_empty()));
            if run.content.is_empty() {
                continue;
            }
            if let Some(ParagraphContent::Inline(InlineNode::Run(previous))) = out.last_mut()
                && same_look(previous, run)
            {
                previous.content.append(&mut run.content);
                join_text(previous);
                continue;
            }
        }
        out.push(item);
    }
    paragraph.content = out;
}

fn join_text(run: &mut Run) {
    let mut joined: Vec<RunContent> = Vec::with_capacity(run.content.len());
    for piece in run.content.drain(..) {
        if let (Some(RunContent::Text { text: last, .. }), RunContent::Text { text, .. }) = (joined.last_mut(), &piece) {
            last.push_str(text);
            continue;
        }
        joined.push(piece);
    }
    for piece in &mut joined {
        if let RunContent::Text { text, preserve_space } = piece {
            *preserve_space = (text.trim() != text.as_str()).then_some(true);
        }
    }
    run.content = joined;
}

// ---- Edits on the body ----

fn ordered(range: DocRange) -> (DocPosition, DocPosition) {
    if range.start <= range.end { (range.start, range.end) } else { (range.end, range.start) }
}

fn ends_section(block: &BlockContent) -> bool {
    matches!(block, BlockContent::Paragraph(paragraph) if paragraph.section_properties.is_some())
}

fn replace(content: &mut Vec<BlockContent>, range: DocRange, text: &str) -> Result<(), CoreError> {
    let (start, end) = ordered(range);
    let first = spot(content, start.paragraph)?;
    let from = start.offset as usize;
    let look;
    if start.paragraph == end.paragraph {
        let paragraph = paragraph_mut(content, &first)?;
        look = look_at(paragraph, from, start != end);
        delete(paragraph, from, end.offset as usize)?;
    } else {
        let last = spot(content, end.paragraph)?;
        // Limit: a range reaching into or out of a table cell can't be replaced; deleting across cells lifts it.
        if last.hops != first.hops {
            return Err(refuse("A selection reaching into or out of a table can't be replaced yet."));
        }
        // Limit: paragraphs that end a section can't be merged away; moving the section break to a neighbour lifts it.
        let between = container(content, &first.hops)?.get(first.block..last.block).ok_or_else(lost)?;
        if between.iter().any(ends_section) {
            return Err(refuse(SECTION_END));
        }
        delete(paragraph_mut(content, &last)?, 0, end.offset as usize)?;
        let head = paragraph_mut(content, &first)?;
        look = look_at(head, from, true);
        let len = paragraph_len(head);
        delete(head, from, len)?;
        let blocks = container(content, &first.hops)?;
        let Some(BlockContent::Paragraph(tail)) = blocks.drain(first.block + 1..=last.block).last() else {
            return Err(lost());
        };
        let tail = Arc::unwrap_or_clone(tail);
        let Some(BlockContent::Paragraph(head)) = blocks.get_mut(first.block) else { return Err(lost()) };
        let head = Arc::make_mut(head);
        head.content.extend(tail.content);
        head.section_properties = tail.section_properties;
    }
    let paragraph = paragraph_mut(content, &first)?;
    insert(paragraph, from, text, look)?;
    tidy(paragraph);
    Ok(())
}

/// A new empty paragraph with `like`'s paragraph formatting and no identity of its own.
fn fresh_paragraph(like: &Paragraph, content: Vec<ParagraphContent>) -> Paragraph {
    Paragraph {
        node_type: like.node_type.clone(),
        para_id: None,
        repeated_para_id: None,
        para_id_attribute: None,
        source_ordinal: None,
        text_id: None,
        extra_attributes: Vec::new(),
        formatting: like.formatting.clone(),
        property_changes: None,
        p_pr_ins: None,
        p_pr_del: None,
        content,
        list_rendering: like.list_rendering.clone(),
        rendered_page_break_before: None,
        section_properties: None,
    }
}

fn insert_paragraph(content: &mut Vec<BlockContent>, index: u32, text: &str) -> Result<(), CoreError> {
    let mut count = 0u32;
    each_paragraph(content, &mut |_| count += 1);
    let (at, after) = match index {
        _ if index < count => (spot(content, index)?, false),
        _ if index == count && count > 0 => (spot(content, count - 1)?, true),
        _ => return Err(no_paragraph(index)),
    };
    let blocks = container(content, &at.hops)?;
    let Some(BlockContent::Paragraph(like)) = blocks.get(at.block) else { return Err(lost()) };
    let mut paragraph = fresh_paragraph(like, Vec::new());
    let look = (like.formatting.as_ref().and_then(|f| f.run_properties.clone()), Vec::new());
    insert(&mut paragraph, 0, text, look)?;
    blocks.insert(at.block + usize::from(after), BlockContent::Paragraph(Arc::new(paragraph)));
    Ok(())
}

fn delete_paragraph(content: &mut Vec<BlockContent>, index: u32) -> Result<(), CoreError> {
    let at = spot(content, index)?;
    let blocks = container(content, &at.hops)?;
    if blocks.get(at.block).is_some_and(ends_section) {
        return Err(refuse(SECTION_END));
    }
    if blocks.iter().filter(|block| matches!(block, BlockContent::Paragraph(_))).count() == 1 {
        return Err(refuse("A document or table cell needs at least one paragraph."));
    }
    if at.block + 1 == blocks.len() && at.block > 0 && !matches!(blocks[at.block - 1], BlockContent::Paragraph(_)) {
        return Err(refuse("The paragraph after a table at the end can't be deleted."));
    }
    blocks.remove(at.block);
    Ok(())
}

fn split_paragraph(content: &mut Vec<BlockContent>, at: DocPosition) -> Result<(), CoreError> {
    let place = spot(content, at.paragraph)?;
    let paragraph = paragraph_mut(content, &place)?;
    let index = cut(paragraph, at.offset as usize)?;
    let tail = paragraph.content.split_off(index);
    let mut next = fresh_paragraph(paragraph, tail);
    // The section break sits on the paragraph mark, which now ends the second half.
    next.section_properties = paragraph.section_properties.take();
    tidy(paragraph);
    tidy(&mut next);
    container(content, &place.hops)?.insert(place.block + 1, BlockContent::Paragraph(Arc::new(next)));
    Ok(())
}

fn merge_paragraph(content: &mut Vec<BlockContent>, index: u32) -> Result<(), CoreError> {
    let first = spot(content, index)?;
    let second = spot(content, index + 1)?;
    if second.hops != first.hops || second.block != first.block + 1 {
        return Err(refuse("Only a paragraph followed directly by another paragraph can be joined with it."));
    }
    let blocks = container(content, &first.hops)?;
    if blocks.get(first.block).is_some_and(ends_section) {
        return Err(refuse(SECTION_END));
    }
    let BlockContent::Paragraph(next) = blocks.remove(second.block) else { return Err(lost()) };
    let next = Arc::unwrap_or_clone(next);
    let Some(BlockContent::Paragraph(paragraph)) = blocks.get_mut(first.block) else { return Err(lost()) };
    let paragraph = Arc::make_mut(paragraph);
    paragraph.content.extend(next.content);
    paragraph.section_properties = next.section_properties;
    tidy(paragraph);
    Ok(())
}

/// A checked [`DocRunFormat`], in the model's units.
struct Patch {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    half_points: Option<f64>,
    color: Option<ColorValue>,
    font: Option<String>,
}

impl Patch {
    fn new(format: DocRunFormat) -> Result<Self, CoreError> {
        let half_points = match format.size_pt {
            Some(pt) if pt.is_finite() && (1.0..=1638.0).contains(&pt) => Some(f64::from((pt * 2.0).round())),
            Some(_) => return Err(refuse("Font size must be between 1 and 1638 points.")),
            None => None,
        };
        let color = match format.color {
            Some(color) => {
                let value = color.trim().trim_start_matches('#');
                if value.eq_ignore_ascii_case("auto") {
                    Some(ColorValue::from_attribute("auto"))
                } else if value.len() == 6 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                    Some(ColorValue::from_attribute(&value.to_ascii_uppercase()))
                } else {
                    return Err(refuse("A colour must be RRGGBB hex or auto."));
                }
            }
            None => None,
        };
        let font = match format.font {
            Some(font) if font.trim().is_empty() => return Err(refuse("A font needs a name.")),
            font => font.map(|f| f.trim().to_owned()),
        };
        Ok(Self {
            bold: format.bold,
            italic: format.italic,
            underline: format.underline,
            strike: format.strike,
            half_points,
            color,
            font,
        })
    }

    fn apply(&self, formatting: &mut Option<TextFormatting>) {
        let f = formatting.get_or_insert_with(Default::default);
        if let Some(bold) = self.bold {
            f.bold = Some(bold);
            f.bold_cs = Some(bold);
        }
        if let Some(italic) = self.italic {
            f.italic = Some(italic);
            f.italic_cs = Some(italic);
        }
        if let Some(underline) = self.underline {
            let style = if underline { "single" } else { "none" };
            f.underline = Some(UnderlineValue { style: style.to_owned(), color: None });
        }
        if let Some(strike) = self.strike {
            f.strike = Some(strike);
            if strike {
                f.double_strike = None;
            }
        }
        if let Some(half_points) = self.half_points {
            f.font_size = Some(half_points);
            f.font_size_cs = Some(half_points);
        }
        if let Some(color) = &self.color {
            f.color = Some(color.clone());
        }
        if let Some(font) = &self.font {
            let family = f.font_family.get_or_insert_with(Default::default);
            family.ascii = Some(font.clone());
            family.h_ansi = Some(font.clone());
            family.cs = Some(font.clone());
            // A theme font reference would win over the name.
            family.ascii_theme = None;
            family.h_ansi_theme = None;
            family.cs_theme = None;
        }
    }
}

fn set_run_format(content: &mut Vec<BlockContent>, range: DocRange, patch: &Patch) -> Result<(), CoreError> {
    let (start, end) = ordered(range);
    for index in start.paragraph..=end.paragraph {
        let at = spot(content, index)?;
        let paragraph = paragraph_mut(content, &at)?;
        let from = if index == start.paragraph { start.offset as usize } else { 0 };
        let to = if index == end.paragraph { end.offset as usize } else { paragraph_len(paragraph) };
        if from == to {
            continue;
        }
        let first = cut(paragraph, from)?;
        let last = cut(paragraph, to)?;
        for item in &mut paragraph.content[first..last] {
            if let ParagraphContent::Inline(InlineNode::Run(run)) = item {
                patch.apply(&mut run.formatting);
            }
        }
        tidy(paragraph);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // betteroffice-demo.docx: the betteroffice-docx 0.6.0 test corpus (Apache-2.0).
    const DEMO: &[u8] = include_bytes!("../tests/fixtures/doc_edit/betteroffice-demo.docx");
    // o_kurs.doc: Apache POI test-data (Apache-2.0).
    const LEGACY: &[u8] = include_bytes!("../tests/fixtures/doc_edit/o_kurs.doc");

    fn open(bytes: &[u8]) -> Arc<DocEditor> {
        DocEditor::open(bytes.to_vec(), OfficeFamily::Document).unwrap()
    }

    fn at(paragraph: u32, offset: u32) -> DocPosition {
        DocPosition { paragraph, offset }
    }

    fn span(paragraph: u32, start: u32, end: u32) -> DocRange {
        DocRange { start: at(paragraph, start), end: at(paragraph, end) }
    }

    fn bold() -> DocRunFormat {
        DocRunFormat { bold: Some(true), ..Default::default() }
    }

    fn parts(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
        let (_, parts) =
            docx_parse::parse_docx_s9_wire_parts_with_limits(bytes, Default::default(), &Default::default()).unwrap();
        parts.into_iter().collect()
    }

    fn text(doc: &DocEditor, index: u32) -> String {
        doc.paragraph(index).unwrap().text
    }

    #[test]
    fn edit_bold_undo_redo_and_save() {
        let doc = open(DEMO);
        assert!(text(&doc, 2).starts_with("BetterOffice is a client-side editor"));

        doc.replace_text(span(2, 0, 12), "Papercraft".into()).unwrap();
        assert!(text(&doc, 2).starts_with("Papercraft is a client-side editor"));
        doc.set_run_format(span(2, 0, 10), bold()).unwrap();
        let runs = doc.paragraph(2).unwrap().runs;
        assert_eq!(runs[0].text, "Papercraft");
        assert!(runs[0].bold);
        assert!(runs[1].text.starts_with(" is a client-side") && !runs[1].bold);

        assert!(doc.undo().unwrap());
        let paragraph = doc.paragraph(2).unwrap();
        assert!(paragraph.text.starts_with("Papercraft is"));
        assert!(paragraph.runs.iter().all(|run| !run.bold));
        assert!(doc.undo().unwrap());
        assert!(text(&doc, 2).starts_with("BetterOffice is"));
        assert!(!doc.is_modified().unwrap());
        assert!(!doc.undo().unwrap());

        assert!(doc.redo().unwrap());
        assert!(doc.redo().unwrap());
        assert!(!doc.redo().unwrap());
        assert!(doc.is_modified().unwrap());

        let again = open(&doc.save().unwrap());
        let paragraph = again.paragraph(2).unwrap();
        assert!(paragraph.text.starts_with("Papercraft is a client-side editor"));
        assert_eq!(paragraph.runs[0].text, "Papercraft");
        assert!(paragraph.runs[0].bold);
        assert!(!paragraph.runs[1].bold);
        assert_eq!(text(&again, 0), "Welcome to BetterOffice");
    }

    #[test]
    fn unedited_save_keeps_every_part() {
        let doc = open(DEMO);
        let saved = doc.save().unwrap();
        assert_eq!(saved, DEMO);
        assert_eq!(parts(&saved), parts(DEMO));

        // Undoing every edit is the opened file again.
        doc.replace_text(span(0, 0, 7), "Hello".into()).unwrap();
        doc.undo().unwrap();
        let saved = doc.save().unwrap();
        assert_eq!(parts(&saved), parts(DEMO));
    }

    #[test]
    fn edited_save_keeps_unrelated_parts() {
        let doc = open(DEMO);
        doc.replace_text(span(0, 0, 7), "Hello".into()).unwrap();
        let before = parts(DEMO);
        let after = parts(&doc.save().unwrap());
        assert_eq!(after["word/styles.xml"], before["word/styles.xml"]);
        assert_ne!(after["word/document.xml"], before["word/document.xml"]);
        for name in before.keys() {
            assert!(after.contains_key(name), "the save dropped {name}");
        }
    }

    #[test]
    fn reads_styles_runs_lists_and_tables() {
        let doc = open(DEMO);
        let title = doc.paragraph(0).unwrap();
        assert_eq!(title.style_id.as_deref(), Some("Title"));
        assert_eq!(title.style_name.as_deref(), Some("Title"));
        assert_eq!(title.runs[0].size_pt, Some(28.0));
        assert_eq!(title.runs[0].font.as_deref(), Some("Calibri Light"));
        assert_eq!(title.runs[0].color.as_deref(), Some("1F3864"));
        // Heading 1 is bold through its style alone.
        assert!(doc.paragraph(4).unwrap().runs[0].bold);

        let formatted = doc.paragraph(7).unwrap();
        let run = |label: &str| formatted.runs.iter().find(|run| run.text == label).unwrap().clone();
        assert!(run("bold").bold && !run("italic").bold);
        assert!(run("italic").italic);
        assert!(run("underline").underline);
        assert_eq!(run("colored run").color.as_deref(), Some("C00000"));

        let count = doc.paragraph_count().unwrap();
        let listed = (0..count).map(|i| doc.paragraph(i).unwrap()).find(|p| p.text.starts_with("An OOXML parser")).unwrap();
        assert_eq!(listed.list_level, Some(0));
        assert_eq!(doc.paragraph(2).unwrap().list_level, None);

        let blocks = doc.blocks().unwrap();
        let rows = blocks
            .iter()
            .find_map(|block| match block {
                DocBlock::Table { rows } => Some(rows),
                DocBlock::Paragraph { .. } => None,
            })
            .unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].cells.len(), 3);
        let header = &rows[0].cells[0].paragraphs[0];
        assert_eq!(header.text, "Stage");
        assert!(header.runs[0].bold);
        assert_eq!(header.runs[0].color.as_deref(), Some("FFFFFF"));
        // Cell paragraphs are numbered with the rest.
        assert_eq!(text(&doc, header.index), "Stage");
        assert!(doc.styles().unwrap().iter().any(|style| style.id == "Heading1" && style.name == "Heading 1"));
    }

    #[test]
    fn paragraph_edits_survive_save() {
        let doc = open(DEMO);
        let count = doc.paragraph_count().unwrap();

        doc.split_paragraph(at(0, 7)).unwrap();
        assert_eq!(text(&doc, 0), "Welcome");
        assert_eq!(text(&doc, 1), " to BetterOffice");
        assert_eq!(doc.paragraph(1).unwrap().style_id.as_deref(), Some("Title"));
        doc.merge_paragraph(0).unwrap();
        assert_eq!(text(&doc, 0), "Welcome to BetterOffice");
        assert_eq!(doc.paragraph_count().unwrap(), count);

        doc.insert_paragraph(1, "Inserted".into()).unwrap();
        doc.set_alignment(1, DocAlign::Center).unwrap();
        doc.set_style(1, "Heading1".into()).unwrap();
        assert!(doc.set_style(1, "NoSuchStyle".into()).is_err());
        // Paragraph 2 is now the subtitle.
        doc.delete_paragraph(2).unwrap();
        assert!(text(&doc, 2).starts_with("BetterOffice is"));
        assert!(text(&doc, 3).starts_with("The page you are reading"));

        // "BetterOffice| is …" to "The page| you are reading …" becomes one paragraph.
        doc.replace_text(DocRange { start: at(2, 12), end: at(3, 8) }, " and".into()).unwrap();
        assert!(text(&doc, 2).starts_with("BetterOffice and you are reading is not HTML"));
        doc.insert_page_break(at(2, 0)).unwrap();
        assert!(text(&doc, 2).starts_with("\u{c}BetterOffice and"));
        assert_eq!(doc.paragraph_count().unwrap(), count - 1);

        let again = open(&doc.save().unwrap());
        assert_eq!(again.paragraph_count().unwrap(), count - 1);
        let inserted = again.paragraph(1).unwrap();
        assert_eq!(inserted.text, "Inserted");
        assert_eq!(inserted.style_id.as_deref(), Some("Heading1"));
        assert_eq!(inserted.alignment, DocAlign::Center);
        assert!(inserted.runs[0].bold);
        assert!(text(&again, 2).starts_with("\u{c}BetterOffice and you are reading is not HTML"));
    }

    #[test]
    fn renders_pages_of_the_edited_document() {
        let doc = open(DEMO);
        let pages = doc.page_count().unwrap();
        assert!(pages >= 1);
        let png = doc.render_page(0, 0.5).unwrap().unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(doc.render_page(pages, 0.5).unwrap().is_none());

        // The text after a page break at paragraph 2 can't share the first page.
        doc.insert_page_break(at(2, 0)).unwrap();
        assert!(doc.page_count().unwrap() >= 2);
        assert!(doc.render_page(1, 0.5).unwrap().is_some());
    }

    #[test]
    fn edits_an_old_doc_and_saves_docx() {
        let doc = open(LEGACY);
        assert!(doc.converted());
        let count = doc.paragraph_count().unwrap();
        assert!(count > 0);
        let first = (0..count).find(|&i| !text(&doc, i).is_empty()).unwrap();
        doc.replace_text(span(first, 0, 0), "Edited ".into()).unwrap();
        let saved = doc.save().unwrap();
        assert!(saved.starts_with(b"PK"));
        assert!(text(&open(&saved), first).starts_with("Edited "));
    }

    #[test]
    fn refuses_bad_input() {
        assert!(DocEditor::open(DEMO.to_vec(), OfficeFamily::Spreadsheet).is_err());
        assert!(DocEditor::open(b"not a document".to_vec(), OfficeFamily::Document).is_err());
        let doc = open(DEMO);
        assert!(doc.replace_text(span(0, 0, 999), String::new()).is_err());
        assert!(doc.paragraph(9999).is_err());
        let red = DocRunFormat { color: Some("red".into()), ..Default::default() };
        assert!(doc.set_run_format(span(0, 0, 3), red).is_err());
        // A failed edit leaves nothing to undo.
        assert!(!doc.can_undo().unwrap());
    }
}
