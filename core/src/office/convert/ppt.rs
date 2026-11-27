//! PowerPoint 97-2003 decks (WPS .dps included) as PresentationML, rebuilt from their records ([MS-PPT]).
//! Every slide, master and notes page keeps its shapes at their place, size, rotation and flip, with their
//! fill, outline, pictures (the picture store is read by `art.rs`) and formatted text.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::io::Cursor;

use office_oxide::DocumentFormat;
use office_oxide::cfb::CfbReader;
use office_oxide::core::opc::{OpcWriter, PartName};
use office_oxide::core::relationships::rel_types;

use super::art::{self, Picture, Record};
use super::{esc, via_model};
use crate::CoreError;
use crate::office::bad;

pub(super) fn to_pptx(bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    // Limit: encrypted decks and decks whose edit history can't be followed keep office_oxide's text-only reading; decrypting the stream lifts the first.
    let Some(streams) = Streams::read(&bytes) else { return via_model(bytes, DocumentFormat::Ppt) };
    let Some(deck) = Deck::parse(&streams) else { return via_model(bytes, DocumentFormat::Ppt) };
    // Limit: hyperlinks, tables as PowerPoint 2007 tables, charts and OLE objects (shown as their preview picture), animations and transitions are not written.
    write(&deck).map_err(|e| bad("file", e))
}

macro_rules! w {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
    }};
}

// PowerPoint records ([MS-PPT] §2.13.24).
const DOCUMENT: u16 = 0x03E8;
const DOCUMENT_ATOM: u16 = 0x03E9;
const SLIDE: u16 = 0x03EE;
const SLIDE_ATOM: u16 = 0x03EF;
const NOTES: u16 = 0x03F0;
const NOTES_ATOM: u16 = 0x03F1;
const ENVIRONMENT: u16 = 0x03F2;
const SLIDE_PERSIST: u16 = 0x03F3;
const MAIN_MASTER: u16 = 0x03F8;
const DRAWING_GROUP: u16 = 0x040B;
const DRAWING: u16 = 0x040C;
const FONT_COLLECTION: u16 = 0x07D5;
const COLOR_SCHEME: u16 = 0x07F0;
const PLACEHOLDER: u16 = 0x0BC3;
const OUTLINE_REF: u16 = 0x0F9E;
const TEXT_HEADER: u16 = 0x0F9F;
const TEXT_CHARS: u16 = 0x0FA0;
const STYLE_TEXT_PROP: u16 = 0x0FA1;
const TX_MASTER_STYLE: u16 = 0x0FA3;
const TEXT_BYTES: u16 = 0x0FA8;
const FONT_ENTITY: u16 = 0x0FB7;
const SLIDE_LIST: u16 = 0x0FF0;
const USER_EDIT: u16 = 0x0FF5;
const PERSIST_DIRECTORY: u16 = 0x1772;
const CRYPT_SESSION: u16 = 0x2F14;

// OfficeArt records ([MS-ODRAW] §2.2).
const DG_CONTAINER: u16 = 0xF002;
const SPGR_CONTAINER: u16 = 0xF003;
const SP_CONTAINER: u16 = 0xF004;
const FSPGR: u16 = 0xF009;
const FSP: u16 = 0xF00A;
const FOPTS: [u16; 3] = [0xF00B, 0xF121, 0xF122];
const CLIENT_TEXTBOX: u16 = 0xF00D;
const CHILD_ANCHOR: u16 = 0xF00F;
const CLIENT_ANCHOR: u16 = 0xF010;
const CLIENT_DATA: u16 = 0xF011;

// `OfficeArtFSP` flags.
const DELETED: u32 = 0x0008;
const HAVE_MASTER: u32 = 0x0020;
const FLIP_H: u32 = 0x0040;
const FLIP_V: u32 = 0x0080;
const BACKGROUND: u32 = 0x0400;

// Shape properties ([MS-ODRAW] §2.3).
const ROTATION: u16 = 0x0004;
const TEXT_LEFT: u16 = 0x0081;
const TEXT_TOP: u16 = 0x0082;
const TEXT_RIGHT: u16 = 0x0083;
const TEXT_BOTTOM: u16 = 0x0084;
const WRAP_TEXT: u16 = 0x0085;
const ANCHOR_TEXT: u16 = 0x0087;
const TEXT_FLOW: u16 = 0x0088;
const CROP_TOP: u16 = 0x0100;
const CROP_BOTTOM: u16 = 0x0101;
const CROP_LEFT: u16 = 0x0102;
const CROP_RIGHT: u16 = 0x0103;
const PIB: u16 = 0x0104;
const FILL_TYPE: u16 = 0x0180;
const FILL_COLOR: u16 = 0x0181;
const FILL_OPACITY: u16 = 0x0182;
const FILL_BACK_COLOR: u16 = 0x0183;
const FILL_BLIP: u16 = 0x0186;
const FILL_ANGLE: u16 = 0x018B;
const FILL_FOCUS: u16 = 0x018C;
const FILL_FLAGS: u16 = 0x01BF;
const LINE_COLOR: u16 = 0x01C0;
const LINE_WIDTH: u16 = 0x01CB;
const LINE_DASHING: u16 = 0x01CE;
const LINE_START_ARROW: u16 = 0x01D0;
const LINE_END_ARROW: u16 = 0x01D1;
const LINE_FLAGS: u16 = 0x01FF;
const MASTER_SHAPE: u16 = 0x0301;
const GROUP_FLAGS: u16 = 0x03BF;
/// Boolean property bits; each has a "use" bit 16 places higher saying the value is set.
const FILLED: u32 = 0x10;
const LINE: u32 = 0x08;
const HIDDEN: u32 = 0x02;

// `SlideAtom`/`NotesAtom` flags: what the page takes from its master.
const MASTER_OBJECTS: u16 = 0x1;
const MASTER_SCHEME: u16 = 0x2;
const MASTER_BACKGROUND: u16 = 0x4;

// MSOSPT shape types.
const LINE_SHAPE: u16 = 20;
const PICTURE_FRAME: u16 = 75;
const TEXT_BOX: u16 = 202;

/// Groups nest; real files go a handful deep.
const MAX_DEPTH: u32 = 16;
/// Edits a deck's history holds; PowerPoint compacts long before this.
const MAX_EDITS: usize = 4096;
/// PowerPoint's default colour scheme: background, text, shadow, title, fill, accent, hyperlink, followed hyperlink.
const DEFAULT_SCHEME: [[u8; 3]; 8] =
    [[0xFF, 0xFF, 0xFF], [0, 0, 0], [0x80, 0x80, 0x80], [0, 0, 0], [0xBB, 0xE0, 0xE3], [0x33, 0x33, 0x99], [0, 0x99, 0x99], [0x99, 0xCC, 0]];

const DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;
const CLR_MAP_OVR: &str = "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>";
const TREE_HEAD: &str = r#"<p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#;
const CT_PRESENTATION: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
const CT_LAYOUT: &str = "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
const CT_MASTER: &str = "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
const CT_NOTES: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
const CT_NOTES_MASTER: &str = "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
const CT_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
const CT_PRES_PROPS: &str = "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml";

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    u32_at(data, at).map(|v| v as i32)
}

/// The record whose header starts at `offset`.
fn record_at(stream: &[u8], offset: usize) -> Option<Record<'_>> {
    art::records(stream.get(offset..)?).next()
}

/// Master units (576 per inch) as EMU (914400 per inch).
fn emu(master: i64) -> i64 {
    master * 3175 / 2
}

/// The streams a deck is read from.
struct Streams {
    document: Vec<u8>,
    current_user: Vec<u8>,
    pictures: Vec<u8>,
}

impl Streams {
    fn read(bytes: &[u8]) -> Option<Self> {
        let mut cfb = CfbReader::new(Cursor::new(bytes)).ok()?;
        // A PowerPoint 95 dual-storage file keeps its PowerPoint 97 copy in a storage of its own.
        let (document, current_user) = match cfb.open_stream_by_path("PP97_DUALSTORAGE/PowerPoint Document") {
            Ok(document) => (document, cfb.open_stream_by_path("PP97_DUALSTORAGE/Current User").ok()?),
            Err(_) => (cfb.open_stream("PowerPoint Document").ok()?, cfb.open_stream("Current User").ok()?),
        };
        let pictures = cfb.open_stream("Pictures").unwrap_or_default();
        Some(Self { document, current_user, pictures })
    }
}

/// Where each persist object starts in the document stream, newest edit winning, and the persist id of the
/// `DocumentContainer`. `None` for an encrypted deck or a broken edit chain.
fn persist_directory(stream: &[u8], current_user: &[u8]) -> Option<(HashMap<u32, usize>, u32)> {
    // CurrentUserAtom: record header, size, headerToken, then offsetToCurrentEdit.
    let mut offset = u32_at(current_user, 16)? as usize;
    let mut offsets = HashMap::new();
    let mut seen = HashSet::new();
    let mut document = None;
    let mut crypt = None;
    loop {
        if !seen.insert(offset) || seen.len() > MAX_EDITS {
            return None;
        }
        // UserEditAtom: lastSlideIdRef, version, offsetLastEdit, offsetPersistDirectory, docPersistIdRef, ...
        let edit = record_at(stream, offset).filter(|r| r.kind == USER_EDIT)?;
        if document.is_none() {
            document = Some(u32_at(edit.body, 16)?);
            crypt = u32_at(edit.body, 28);
        }
        let directory = record_at(stream, u32_at(edit.body, 12)? as usize).filter(|r| r.kind == PERSIST_DIRECTORY)?;
        let mut at = 0;
        while let Some(head) = u32_at(directory.body, at) {
            at += 4;
            let (first, count) = (head & 0xF_FFFF, head >> 20);
            for i in 0..count {
                let Some(start) = u32_at(directory.body, at) else { break };
                at += 4;
                offsets.entry(first + i).or_insert(start as usize);
            }
        }
        match u32_at(edit.body, 8)? {
            0 => break,
            last => offset = last as usize,
        }
    }
    let encrypted = crypt.and_then(|id| offsets.get(&id)).and_then(|&at| record_at(stream, at)).is_some_and(|r| r.kind == CRYPT_SESSION);
    (!encrypted).then_some((offsets, document?))
}

/// One text of a page: its `TextTypeEnum`, its UTF-16 code units and its `StyleTextPropAtom`.
#[derive(Default)]
struct TextBlock<'a> {
    kind: u32,
    text: Vec<u16>,
    style: Option<&'a [u8]>,
}

/// Adds `rec` to the text being read when it is a text record; a header starts a new text.
fn push_text<'a>(blocks: &mut Vec<TextBlock<'a>>, rec: &Record<'a>) {
    if rec.kind == TEXT_HEADER {
        blocks.push(TextBlock { kind: u32_at(rec.body, 0).unwrap_or(4), ..Default::default() });
        return;
    }
    if !matches!(rec.kind, TEXT_CHARS | TEXT_BYTES | STYLE_TEXT_PROP) {
        return;
    }
    if blocks.is_empty() {
        blocks.push(TextBlock { kind: 4, ..Default::default() });
    }
    let Some(block) = blocks.last_mut() else { return };
    match rec.kind {
        TEXT_CHARS => block.text = rec.body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect(),
        TEXT_BYTES => block.text = rec.body.iter().map(|&b| u16::from(b)).collect(),
        _ => block.style = Some(rec.body),
    }
}

/// One `SlidePersistAtom` of a `SlideListWithTextContainer` and the outline texts that follow it.
#[derive(Default)]
struct ListEntry<'a> {
    persist: u32,
    id: u32,
    texts: Vec<TextBlock<'a>>,
}

fn slide_list(body: &[u8]) -> Vec<ListEntry<'_>> {
    let mut entries: Vec<ListEntry<'_>> = Vec::new();
    for rec in art::records(body) {
        if rec.kind == SLIDE_PERSIST {
            if let (Some(persist), Some(id)) = (u32_at(rec.body, 0), u32_at(rec.body, 12)) {
                entries.push(ListEntry { persist, id, texts: Vec::new() });
            }
        } else if let Some(entry) = entries.last_mut() {
            push_text(&mut entry.texts, &rec);
        }
    }
    entries
}

#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `RectStruct`-like anchors: left, top, right, bottom.
fn rect(body: &[u8]) -> Option<Rect> {
    Some(Rect { left: i32_at(body, 0)?, top: i32_at(body, 4)?, right: i32_at(body, 8)?, bottom: i32_at(body, 12)? })
}

/// PowerPoint's client anchor: top, left, right, bottom in master units, 16-bit (`SmallRectStruct`) or 32-bit.
fn client_anchor(body: &[u8]) -> Option<Rect> {
    if body.len() >= 16 {
        return Some(Rect { top: i32_at(body, 0)?, left: i32_at(body, 4)?, right: i32_at(body, 8)?, bottom: i32_at(body, 12)? });
    }
    let at = |i| u16_at(body, i).map(|v| i32::from(v as i16));
    Some(Rect { top: at(0)?, left: at(2)?, right: at(4)?, bottom: at(6)? })
}

enum ShapeText<'a> {
    Inline(TextBlock<'a>),
    /// Index of the page's outline text (in the slide list) the shape shows.
    Outline(usize),
}

#[derive(Default)]
struct Shape<'a> {
    spid: u32,
    /// MSOSPT shape type.
    kind: u16,
    flags: u32,
    props: Vec<(u16, u32)>,
    /// `OfficeArtClientAnchor`, in master units.
    anchor: Option<Rect>,
    /// `OfficeArtChildAnchor`, in the parent group's coordinates.
    child: Option<Rect>,
    /// `OfficeArtFSPGR`: a group's own coordinates.
    space: Option<Rect>,
    /// `PlaceholderAtom`: placement id and position.
    placeholder: Option<(u8, i32)>,
    text: Option<ShapeText<'a>>,
    group: bool,
    children: Vec<Shape<'a>>,
}

impl Shape<'_> {
    fn own(&self, id: u16) -> Option<u32> {
        self.props.iter().find(|p| p.0 == id).map(|p| p.1)
    }
}

fn shape(body: &[u8]) -> Option<Shape<'_>> {
    let mut s = Shape::default();
    let mut found = false;
    for rec in art::records(body) {
        match rec.kind {
            FSP => {
                s.kind = rec.instance;
                s.spid = u32_at(rec.body, 0)?;
                s.flags = u32_at(rec.body, 4)?;
                found = true;
            },
            FSPGR => s.space = rect(rec.body),
            CHILD_ANCHOR => s.child = rect(rec.body),
            CLIENT_ANCHOR => s.anchor = client_anchor(rec.body),
            CLIENT_DATA => {
                for data in art::records(rec.body).filter(|r| r.kind == PLACEHOLDER) {
                    if let (Some(position), Some(&placement)) = (i32_at(data.body, 0), data.body.get(4)) {
                        s.placeholder = Some((placement, position));
                    }
                }
            },
            CLIENT_TEXTBOX => s.text = textbox(rec.body),
            kind if FOPTS.contains(&kind) => {
                // `recInstance` counts the 6-byte property entries; complex data follows them.
                for entry in rec.body.chunks_exact(6).take(usize::from(rec.instance)) {
                    let id = u16::from_le_bytes([entry[0], entry[1]]);
                    if id & 0x8000 == 0 {
                        s.props.push((id & 0x3FFF, u32::from_le_bytes([entry[2], entry[3], entry[4], entry[5]])));
                    }
                }
            },
            _ => {},
        }
    }
    found.then_some(s)
}

fn textbox(body: &[u8]) -> Option<ShapeText<'_>> {
    let mut blocks = Vec::new();
    for rec in art::records(body) {
        if rec.kind == OUTLINE_REF {
            return Some(ShapeText::Outline(u32_at(rec.body, 0)? as usize));
        }
        push_text(&mut blocks, &rec);
    }
    blocks.into_iter().next().map(ShapeText::Inline)
}

/// An `OfficeArtSpgrContainer`: its first shape is the group itself.
fn group(body: &[u8], depth: u32) -> Option<Shape<'_>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut head = None;
    let mut children = Vec::new();
    for rec in art::records(body) {
        match rec.kind {
            SP_CONTAINER if head.is_none() => head = Some(shape(rec.body)?),
            SP_CONTAINER => children.extend(shape(rec.body)),
            SPGR_CONTAINER => children.extend(group(rec.body, depth + 1)),
            _ => {},
        }
    }
    let mut head = head?;
    head.group = true;
    head.children = children;
    Some(head)
}

/// A page's `PPDrawing`: its shapes in drawing order and its background shape.
fn drawing(body: &[u8]) -> (Vec<Shape<'_>>, Option<Shape<'_>>) {
    let (mut shapes, mut background) = (Vec::new(), None);
    let Some(dg) = art::records(body).find(|r| r.kind == DG_CONTAINER) else { return (shapes, background) };
    for rec in art::records(dg.body) {
        match rec.kind {
            SPGR_CONTAINER => {
                if let Some(top) = group(rec.body, 0) {
                    shapes = top.children;
                }
            },
            SP_CONTAINER => {
                if let Some(s) = shape(rec.body).filter(|s| s.flags & BACKGROUND != 0) {
                    background = Some(s);
                }
            },
            _ => {},
        }
    }
    (shapes, background)
}

/// A slide, a master (main or title master) or a notes page.
#[derive(Default)]
struct Sheet<'a> {
    /// Slide id, master id or notes id from the slide list.
    id: u32,
    /// `SLIDE`, `MAIN_MASTER` or `NOTES`.
    container: u16,
    master_id: u32,
    notes_id: u32,
    flags: u16,
    scheme: Option<[[u8; 3]; 8]>,
    /// `TxMasterStyleAtom` bodies by text type (masters only).
    styles: [Option<&'a [u8]>; 9],
    shapes: Vec<Shape<'a>>,
    background: Option<Shape<'a>>,
    outline: Vec<TextBlock<'a>>,
}

impl<'a> Sheet<'a> {
    fn read(stream: &'a [u8], offset: usize, entry: ListEntry<'a>) -> Option<Self> {
        let rec = record_at(stream, offset).filter(|r| matches!(r.kind, SLIDE | MAIN_MASTER | NOTES))?;
        let mut sheet = Sheet { id: entry.id, container: rec.kind, outline: entry.texts, ..Default::default() };
        for child in art::records(rec.body) {
            match child.kind {
                // SlideAtom: geom, rgPlaceholderTypes, masterIdRef, notesIdRef, slideFlags.
                SLIDE_ATOM => {
                    sheet.master_id = u32_at(child.body, 12).unwrap_or(0);
                    sheet.notes_id = u32_at(child.body, 16).unwrap_or(0);
                    sheet.flags = u16_at(child.body, 20).unwrap_or(0);
                },
                NOTES_ATOM => sheet.flags = u16_at(child.body, 4).unwrap_or(0),
                DRAWING => (sheet.shapes, sheet.background) = drawing(child.body),
                COLOR_SCHEME if child.instance == 1 => {
                    let colour = |i: usize| -> Option<[u8; 3]> { child.body.get(i * 4..i * 4 + 3)?.try_into().ok() };
                    sheet.scheme = (0..8).map(colour).collect::<Option<Vec<_>>>().and_then(|c| c.try_into().ok());
                },
                TX_MASTER_STYLE => {
                    if let Some(slot) = sheet.styles.get_mut(usize::from(child.instance)) {
                        *slot = Some(child.body);
                    }
                },
                _ => {},
            }
        }
        Some(sheet)
    }
}

struct Deck<'a> {
    slide_size: (i64, i64),
    notes_size: (i64, i64),
    fonts: Vec<String>,
    pictures: Vec<Option<Picture>>,
    /// The document's default style for text that is not in a placeholder.
    other_style: Option<&'a [u8]>,
    masters: Vec<Sheet<'a>>,
    slides: Vec<Sheet<'a>>,
    notes: Vec<Sheet<'a>>,
    notes_master: Option<Sheet<'a>>,
}

impl<'a> Deck<'a> {
    fn parse(streams: &'a Streams) -> Option<Self> {
        let stream = &streams.document[..];
        let (offsets, document) = persist_directory(stream, &streams.current_user)?;
        let doc = record_at(stream, *offsets.get(&document)?).filter(|r| r.kind == DOCUMENT)?;
        let mut deck = Deck {
            slide_size: (emu(5760), emu(4320)),
            notes_size: (emu(4320), emu(5760)),
            fonts: Vec::new(),
            pictures: Vec::new(),
            other_style: None,
            masters: Vec::new(),
            slides: Vec::new(),
            notes: Vec::new(),
            notes_master: None,
        };
        let mut lists: [Vec<ListEntry<'a>>; 3] = Default::default();
        let mut notes_master = None;
        for rec in art::records(doc.body) {
            match rec.kind {
                // DocumentAtom: slideSize, notesSize, serverZoom, notesMasterPersistIdRef.
                DOCUMENT_ATOM => {
                    let size = |at| Some((emu(i32_at(rec.body, at)?.into()), emu(i32_at(rec.body, at + 4)?.into())));
                    deck.slide_size = size(0)?;
                    deck.notes_size = size(8)?;
                    notes_master = u32_at(rec.body, 24);
                },
                ENVIRONMENT => {
                    for child in art::records(rec.body) {
                        match child.kind {
                            FONT_COLLECTION => {
                                deck.fonts = art::records(child.body).filter(|r| r.kind == FONT_ENTITY).map(|r| font_name(r.body)).collect();
                            },
                            TX_MASTER_STYLE if child.instance == 4 => deck.other_style = Some(child.body),
                            _ => {},
                        }
                    }
                },
                DRAWING_GROUP => deck.pictures = art::picture_store(rec.body, &streams.pictures),
                SLIDE_LIST => {
                    if let Some(list) = lists.get_mut(usize::from(rec.instance)) {
                        *list = slide_list(rec.body);
                    }
                },
                _ => {},
            }
        }
        // A page the directory can't find fails the whole deck, so office_oxide's reading shows every slide instead.
        let read = |list: &mut Vec<ListEntry<'a>>| -> Option<Vec<Sheet<'a>>> {
            list.drain(..).map(|entry| Sheet::read(stream, *offsets.get(&entry.persist)?, entry)).collect()
        };
        let [slides, masters, notes] = &mut lists;
        deck.slides = read(slides)?;
        deck.masters = read(masters)?;
        deck.notes = read(notes)?;
        deck.notes_master = notes_master.and_then(|id| Sheet::read(stream, *offsets.get(&id)?, ListEntry::default()));
        deck.masters.iter().any(|m| m.container == MAIN_MASTER).then_some(deck)
    }

    fn master(&self, sheet: &Sheet<'a>) -> Option<&Sheet<'a>> {
        self.masters.iter().find(|m| sheet.master_id != 0 && m.id == sheet.master_id)
    }

    /// The main master whose text styles `sheet` uses: a title master's own master.
    fn main_master(&self, sheet: &Sheet<'a>) -> Option<&Sheet<'a>> {
        let mut found = self.master(sheet);
        if let Some(title) = found.filter(|m| m.container != MAIN_MASTER) {
            found = self.master(title);
        }
        found.filter(|m| m.container == MAIN_MASTER).or_else(|| self.masters.iter().find(|m| m.container == MAIN_MASTER))
    }

    /// The colour scheme `sheet` draws with: its own, or its master's when it follows the master.
    fn scheme(&self, sheet: &Sheet<'a>) -> [[u8; 3]; 8] {
        let mut current = sheet;
        for _ in 0..3 {
            if current.container == MAIN_MASTER || current.flags & MASTER_SCHEME == 0 {
                if let Some(scheme) = current.scheme {
                    return scheme;
                }
            }
            match self.master(current) {
                Some(master) => current = master,
                None => break,
            }
        }
        sheet.scheme.unwrap_or(DEFAULT_SCHEME)
    }
}

/// `FontEntityAtom.lfFaceName`: 32 UTF-16 code units, NUL-padded.
fn font_name(body: &[u8]) -> String {
    let units: Vec<u16> =
        body.get(..64).unwrap_or(body).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Little-endian reads that advance through a record body.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Paragraph formatting (`TextPFException`); unset fields come from the master style.
#[derive(Clone, Copy, Default)]
struct Para {
    bullet: Option<bool>,
    bullet_has_font: Option<bool>,
    bullet_font: Option<u16>,
    bullet_char: Option<u16>,
    align: Option<u16>,
    line: Option<i16>,
    before: Option<i16>,
    after: Option<i16>,
    left: Option<i16>,
    indent: Option<i16>,
}

impl Para {
    fn or(self, under: Self) -> Self {
        Self {
            bullet: self.bullet.or(under.bullet),
            bullet_has_font: self.bullet_has_font.or(under.bullet_has_font),
            bullet_font: self.bullet_font.or(under.bullet_font),
            bullet_char: self.bullet_char.or(under.bullet_char),
            align: self.align.or(under.align),
            line: self.line.or(under.line),
            before: self.before.or(under.before),
            after: self.after.or(under.after),
            left: self.left.or(under.left),
            indent: self.indent.or(under.indent),
        }
    }

    /// The fields `masks` (`PFMasks`) selects, in the order [MS-PPT] §2.9.18 lays them out.
    fn read(c: &mut Bytes<'_>, masks: u32) -> Option<Self> {
        let mut p = Self::default();
        let has = |bit: u32| masks & 1 << bit != 0;
        if masks & 0xF != 0 {
            let flags = c.u16()?;
            p.bullet = has(0).then_some(flags & 1 != 0);
            p.bullet_has_font = has(1).then_some(flags & 2 != 0);
        }
        if has(7) {
            p.bullet_char = Some(c.u16()?);
        }
        if has(4) {
            p.bullet_font = Some(c.u16()?);
        }
        if has(6) {
            c.take(2)?; // bulletSize
        }
        if has(5) {
            c.take(4)?; // bulletColor
        }
        if has(11) {
            p.align = Some(c.u16()?);
        }
        if has(12) {
            p.line = Some(c.i16()?);
        }
        if has(13) {
            p.before = Some(c.i16()?);
        }
        if has(14) {
            p.after = Some(c.i16()?);
        }
        if has(8) {
            p.left = Some(c.i16()?);
        }
        if has(10) {
            p.indent = Some(c.i16()?);
        }
        if has(15) {
            c.take(2)?; // defaultTabSize
        }
        if has(20) {
            let stops = c.u16()?;
            c.take(usize::from(stops) * 4)?;
        }
        if has(16) {
            c.take(2)?; // fontAlign
        }
        if masks & 0x7 << 17 != 0 {
            c.take(2)?; // wrapFlags
        }
        if has(21) {
            c.take(2)?; // textDirection
        }
        Some(p)
    }
}

/// Character formatting (`TextCFException`); unset fields come from the master style.
#[derive(Clone, Copy, Default)]
struct Chars {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    font: Option<u16>,
    size: Option<u16>,
    /// `ColorIndexStruct`: red, green, blue, scheme index (0xFE for the RGB value).
    color: Option<[u8; 4]>,
}

impl Chars {
    fn or(self, under: Self) -> Self {
        Self {
            bold: self.bold.or(under.bold),
            italic: self.italic.or(under.italic),
            underline: self.underline.or(under.underline),
            font: self.font.or(under.font),
            size: self.size.or(under.size),
            color: self.color.or(under.color),
        }
    }

    /// The fields `masks` (`CFMasks`) selects, in the order [MS-PPT] §2.9.43 lays them out.
    fn read(c: &mut Bytes<'_>, masks: u32) -> Option<Self> {
        let mut f = Self::default();
        let has = |bit: u32| masks & 1 << bit != 0;
        if masks & 0x3FFF != 0 {
            let style = c.u16()?;
            f.bold = has(0).then_some(style & 1 != 0);
            f.italic = has(1).then_some(style & 2 != 0);
            f.underline = has(2).then_some(style & 4 != 0);
        }
        if has(16) {
            f.font = Some(c.u16()?);
        }
        for bit in [21, 22, 23] {
            if has(bit) {
                c.take(2)?; // East Asian, ANSI and symbol font refs
            }
        }
        if has(17) {
            f.size = Some(c.u16()?);
        }
        if has(18) {
            f.color = Some(c.take(4)?.try_into().ok()?);
        }
        if has(19) {
            c.take(2)?; // position
        }
        Some(f)
    }
}

/// Formatting runs as (end, value), ends exclusive and increasing.
type Runs<T> = Vec<(usize, T)>;

/// The value of the run covering code unit `at`.
fn run_at<T>(runs: &[(usize, T)], at: usize) -> Option<&T> {
    runs.get(runs.partition_point(|r| r.0 <= at)).map(|r| &r.1)
}

/// A `StyleTextPropAtom` over a text of `len` code units: paragraph runs (indent level and formatting), then
/// character runs. Both cover the text plus one.
fn text_style(data: Option<&[u8]>, len: usize) -> (Runs<(u16, Para)>, Runs<Chars>) {
    let mut c = Bytes(data.unwrap_or_default());
    let (mut paras, mut chars) = (Vec::new(), Vec::new());
    let mut end = 0usize;
    while end <= len {
        let Some((count, level, para)) = (|| {
            let count = c.u32()? as usize;
            let level = c.u16()?;
            let masks = c.u32()?;
            Some((count, level, Para::read(&mut c, masks)?))
        })() else {
            break;
        };
        if count == 0 {
            break;
        }
        end = end.saturating_add(count);
        paras.push((end, (level.min(4), para)));
    }
    end = 0;
    while end <= len {
        let Some((count, run)) = (|| {
            let count = c.u32()? as usize;
            let masks = c.u32()?;
            Some((count, Chars::read(&mut c, masks)?))
        })() else {
            break;
        };
        if count == 0 {
            break;
        }
        end = end.saturating_add(count);
        chars.push((end, run));
    }
    (paras, chars)
}

/// A main master's text styles by text type, each up to five indent levels.
#[derive(Default)]
struct Styles {
    levels: [Vec<(Para, Chars)>; 9],
}

impl Styles {
    fn read(master: Option<&Sheet<'_>>, other: Option<&[u8]>) -> Self {
        let mut styles = Self::default();
        for (kind, levels) in styles.levels.iter_mut().enumerate() {
            let data = master.and_then(|m| m.styles[kind]).or(if kind == 4 { other } else { None });
            let Some(data) = data else { continue };
            let mut c = Bytes(data);
            let count = c.u16().unwrap_or(0);
            for _ in 0..count.min(5) {
                let level = (|| {
                    // Types from CenterBody on name each level's indent first.
                    if kind >= 5 {
                        c.take(2)?;
                    }
                    let masks = c.u32()?;
                    let para = Para::read(&mut c, masks)?;
                    let masks = c.u32()?;
                    Some((para, Chars::read(&mut c, masks)?))
                })();
                let Some(level) = level else { break };
                levels.push(level);
            }
        }
        styles
    }

    /// The formatting text of type `kind` gets at indent `level`: the level, then the levels above it, then
    /// the type it derives from (body for the body variants, title for the centred title), as PowerPoint does.
    fn level(&self, kind: u32, level: u16) -> (Para, Chars) {
        let (mut para, mut chars) = (Para::default(), Chars::default());
        let mut kind = kind as usize;
        loop {
            if let Some(levels) = self.levels.get(kind) {
                for (p, c) in levels.iter().take(usize::from(level) + 1).rev() {
                    para = para.or(*p);
                    chars = chars.or(*c);
                }
            }
            kind = match kind {
                5 | 7 | 8 => 1,
                6 => 0,
                _ => break,
            };
        }
        (para, chars)
    }
}

/// A colour as six hex digits.
struct Hex([u8; 3]);

impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2])
    }
}

fn solid_fill(xml: &mut String, rgb: [u8; 3], alpha: Option<i64>) {
    w!(xml, r#"<a:solidFill><a:srgbClr val="{}""#, Hex(rgb));
    match alpha {
        Some(alpha) => w!(xml, r#"><a:alpha val="{alpha}"/></a:srgbClr></a:solidFill>"#),
        None => xml.push_str("/></a:solidFill>"),
    }
}

/// Where a shape sits, in its parent's coordinates.
struct Frame {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    /// Clockwise, in 60000ths of a degree.
    rot: i64,
    flip_h: bool,
    flip_v: bool,
}

/// `a:xfrm`; `children` is a group's own coordinate box.
fn xfrm(xml: &mut String, f: &Frame, children: Option<(i64, i64, i64, i64)>) {
    xml.push_str("<a:xfrm");
    if f.rot != 0 {
        w!(xml, r#" rot="{}""#, f.rot);
    }
    if f.flip_h {
        xml.push_str(r#" flipH="1""#);
    }
    if f.flip_v {
        xml.push_str(r#" flipV="1""#);
    }
    w!(xml, r#"><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/>"#, f.x, f.y, f.w, f.h);
    if let Some((x, y, w, h)) = children {
        w!(xml, r#"<a:chOff x="{x}" y="{y}"/><a:chExt cx="{w}" cy="{h}"/>"#);
    }
    xml.push_str("</a:xfrm>");
}

/// The DrawingML preset for an MSOSPT shape type.
// Limit: adjust handles keep their preset defaults, free-form shapes are not drawn (their text is) and unlisted types draw as rectangles; mapping `adjustValue` and `pVertices` to `avLst` and `custGeom` lifts it.
fn preset(kind: u16) -> &'static str {
    match kind {
        2 => "roundRect",
        3 => "ellipse",
        4 => "diamond",
        5 => "triangle",
        6 => "rtTriangle",
        7 => "parallelogram",
        8 => "trapezoid",
        9 => "hexagon",
        10 => "octagon",
        11 => "plus",
        12 => "star5",
        13 | 14 => "rightArrow",
        15 => "homePlate",
        16 => "cube",
        17 => "wedgeRoundRectCallout",
        18 => "star16",
        19 => "arc",
        LINE_SHAPE => "line",
        21 => "plaque",
        22 => "can",
        23 => "donut",
        32 => "straightConnector1",
        33 => "bentConnector2",
        34 => "bentConnector3",
        35 => "bentConnector4",
        36 => "bentConnector5",
        37 => "curvedConnector2",
        38 => "curvedConnector3",
        39 => "curvedConnector4",
        40 => "curvedConnector5",
        55 => "chevron",
        56 => "pentagon",
        58 => "star8",
        59 => "star16",
        60 => "star32",
        61 => "wedgeRectCallout",
        62 => "wedgeRoundRectCallout",
        63 => "wedgeEllipseCallout",
        64 => "wave",
        65 => "foldedCorner",
        66 => "leftArrow",
        67 => "downArrow",
        68 => "upArrow",
        69 => "leftRightArrow",
        70 => "upDownArrow",
        71 => "irregularSeal1",
        72 => "irregularSeal2",
        73 => "lightningBolt",
        74 => "heart",
        76 => "quadArrow",
        84 => "bevel",
        85 => "leftBracket",
        86 => "rightBracket",
        87 => "leftBrace",
        88 => "rightBrace",
        92 => "star24",
        93 => "stripedRightArrow",
        94 => "notchedRightArrow",
        95 => "blockArc",
        96 => "smileyFace",
        97 => "verticalScroll",
        98 => "horizontalScroll",
        99 => "circularArrow",
        109 => "flowChartProcess",
        110 => "flowChartDecision",
        111 => "flowChartInputOutput",
        112 => "flowChartPredefinedProcess",
        113 => "flowChartInternalStorage",
        114 => "flowChartDocument",
        115 => "flowChartMultidocument",
        116 => "flowChartTerminator",
        117 => "flowChartPreparation",
        118 => "flowChartManualInput",
        119 => "flowChartManualOperation",
        120 => "flowChartConnector",
        121 => "flowChartPunchedCard",
        122 => "flowChartPunchedTape",
        123 => "flowChartSummingJunction",
        124 => "flowChartOr",
        125 => "flowChartCollate",
        126 => "flowChartSort",
        127 => "flowChartExtract",
        128 => "flowChartMerge",
        130 => "flowChartOnlineStorage",
        131 => "flowChartMagneticTape",
        132 => "flowChartMagneticDisk",
        133 => "flowChartMagneticDrum",
        134 => "flowChartDisplay",
        135 => "flowChartDelay",
        176 => "flowChartAlternateProcess",
        177 => "flowChartOffpageConnector",
        183 => "sun",
        184 => "moon",
        185 => "bracketPair",
        186 => "bracePair",
        187 => "star4",
        188 => "doubleWave",
        _ => "rect",
    }
}

fn is_line(kind: u16) -> bool {
    kind == LINE_SHAPE || (32..=40).contains(&kind)
}

/// The PresentationML placeholder type of a `PlaceholderEnum` value.
fn placeholder_type(placement: u8) -> Option<&'static str> {
    Some(match placement {
        1 | 13 | 17 => "title",
        3 | 15 => "ctrTitle",
        4 | 16 => "subTitle",
        2 | 6 | 12 | 14 | 18 => "body",
        7 => "dt",
        8 => "sldNum",
        9 => "ftr",
        10 => "hdr",
        5 | 11 => "sldImg",
        19 | 25 => "obj",
        20 => "chart",
        21 => "tbl",
        22 => "clipArt",
        23 => "dgm",
        24 => "media",
        26 => "pic",
        _ => return None,
    })
}

/// `a:lnSpc`, `a:spcBef` or `a:spcAft`: a percentage of the line, or (negative) master units.
fn spacing(xml: &mut String, tag: &str, value: Option<i16>) {
    match value {
        Some(v) if v >= 0 => w!(xml, r#"<{tag}><a:spcPct val="{}"/></{tag}>"#, i64::from(v) * 1000),
        Some(v) => w!(xml, r#"<{tag}><a:spcPts val="{}"/></{tag}>"#, (-i64::from(v)).min(1584) * 100 / 8),
        None => {},
    }
}

type Opc = OpcWriter<Cursor<Vec<u8>>>;

fn part(name: &str) -> office_oxide::core::Result<PartName> {
    PartName::new(name)
}

struct Package {
    opc: Opc,
    /// Picture store index → media part, relative to `/ppt/`.
    media: HashMap<u32, String>,
}

/// Writes one page's XML part.
struct Page<'w, 'd, 'a> {
    pkg: &'w mut Package,
    deck: &'d Deck<'a>,
    /// Master and notes master shapes by id, for the properties a placeholder takes from its master.
    masters: &'d HashMap<u32, &'d Shape<'a>>,
    part: PartName,
    scheme: [[u8; 3]; 8],
    styles: &'d Styles,
    outline: &'d [TextBlock<'a>],
    ids: u32,
    /// Picture store index → this part's relationship id.
    images: HashMap<u32, String>,
}

impl<'w, 'd, 'a> Page<'w, 'd, 'a> {
    fn new(
        pkg: &'w mut Package,
        deck: &'d Deck<'a>,
        masters: &'d HashMap<u32, &'d Shape<'a>>,
        part: PartName,
        scheme: [[u8; 3]; 8],
        styles: &'d Styles,
        outline: &'d [TextBlock<'a>],
    ) -> Self {
        Self { pkg, deck, masters, part, scheme, styles, outline, ids: 1, images: HashMap::new() }
    }

    fn next_id(&mut self) -> u32 {
        self.ids += 1;
        self.ids
    }

    /// `<p:{root}>` for `sheet`; `tail` follows `p:cSld`.
    fn sheet(&mut self, root: &str, sheet: &Sheet<'a>, tail: &str) -> office_oxide::core::Result<String> {
        let master = matches!(root, "sldMaster" | "notesMaster");
        let mut xml = format!("{DECL}<p:{root} {NS}");
        if !master && sheet.container != 0 && sheet.flags & MASTER_OBJECTS == 0 {
            xml.push_str(r#" showMasterSp="0""#);
        }
        xml.push_str("><p:cSld>");
        if master || sheet.flags & MASTER_BACKGROUND == 0 {
            if let Some(background) = &sheet.background {
                xml.push_str("<p:bg><p:bgPr>");
                self.fill(&mut xml, background, true)?;
                xml.push_str("<a:effectLst/></p:bgPr></p:bg>");
            }
        }
        xml.push_str(TREE_HEAD);
        for s in &sheet.shapes {
            self.shape(&mut xml, s)?;
        }
        w!(xml, "</p:spTree></p:cSld>{tail}</p:{root}>");
        Ok(xml)
    }

    /// The master shape a placeholder inherits its unset properties from.
    fn master_of(&self, s: &Shape<'a>) -> Option<&'d Shape<'a>> {
        if s.flags & HAVE_MASTER == 0 {
            return None;
        }
        self.masters.get(&s.own(MASTER_SHAPE)?).copied()
    }

    fn prop(&self, s: &Shape<'a>, id: u16) -> Option<u32> {
        s.own(id).or_else(|| self.master_of(s)?.own(id))
    }

    /// A boolean property bit, when the shape (or its master) sets it.
    fn flag(&self, s: &Shape<'a>, id: u16, bit: u32) -> Option<bool> {
        let set = |shape: &Shape<'a>| shape.own(id).filter(|v| v & bit << 16 != 0).map(|v| v & bit != 0);
        set(s).or_else(|| set(self.master_of(s)?))
    }

    /// An `OfficeArtCOLORREF`; scheme colours resolve against the page's scheme.
    // Limit: system colours (fSysIndex) fall back to the default colour; resolving them against the fill and line colours lifts it.
    fn color(&self, v: u32) -> Option<[u8; 3]> {
        let [r, g, b, flags] = v.to_le_bytes();
        if flags & 0x08 != 0 {
            return self.scheme.get(usize::from(r)).copied();
        }
        (flags & 0x10 == 0).then_some([r, g, b])
    }

    fn text_color(&self, c: [u8; 4]) -> Option<[u8; 3]> {
        match c[3] {
            0xFE => Some([c[0], c[1], c[2]]),
            i => self.scheme.get(usize::from(i)).copied(),
        }
    }

    fn frame(&self, s: &Shape<'a>) -> Option<Frame> {
        // A child anchor is in its group's own units; a client anchor in master units.
        let (r, scale) = match (s.child, s.anchor) {
            (Some(r), _) => (r, false),
            (None, Some(r)) => (r, true),
            (None, None) => (self.master_of(s)?.anchor?, true),
        };
        let to = |v: i32| if scale { emu(v.into()) } else { v.into() };
        let span = |a: i32, b: i32| (if scale { emu(i64::from(b) - i64::from(a)) } else { i64::from(b) - i64::from(a) }).max(0);
        let (mut x, mut y) = (to(r.left), to(r.top));
        let (mut w, mut h) = (span(r.left, r.right), span(r.top, r.bottom));
        let rot = (i64::from(self.prop(s, ROTATION).unwrap_or(0) as i32) * 60_000 / 65_536).rem_euclid(21_600_000);
        // The anchor of a shape turned 45-135 or 225-315 degrees bounds it turned a quarter more.
        if (rot + 2_700_000) / 5_400_000 % 2 == 1 {
            x += (w - h) / 2;
            y += (h - w) / 2;
            (w, h) = (h, w);
        }
        Some(Frame { x, y, w, h, rot, flip_h: s.flags & FLIP_H != 0, flip_v: s.flags & FLIP_V != 0 })
    }

    fn shape(&mut self, xml: &mut String, s: &Shape<'a>) -> office_oxide::core::Result<()> {
        if s.flags & DELETED != 0 || self.flag(s, GROUP_FLAGS, HIDDEN) == Some(true) {
            return Ok(());
        }
        let Some(frame) = self.frame(s) else { return Ok(()) };
        if s.group {
            return self.group(xml, s, &frame);
        }
        if let Some(pib) = self.prop(s, PIB).filter(|&pib| pib > 0) {
            if let Some(rid) = self.image(pib)? {
                self.picture(xml, s, &frame, &rid);
                return Ok(());
            }
        }
        // A picture frame whose picture the store lacks draws nothing.
        if s.kind == PICTURE_FRAME {
            return Ok(());
        }
        self.sp(xml, s, &frame)
    }

    fn group(&mut self, xml: &mut String, s: &Shape<'a>, f: &Frame) -> office_oxide::core::Result<()> {
        let id = self.next_id();
        w!(xml, r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="{id}" name="Group {id}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr>"#);
        let children = match s.space {
            Some(c) => (c.left.into(), c.top.into(), (i64::from(c.right) - i64::from(c.left)).max(0), (i64::from(c.bottom) - i64::from(c.top)).max(0)),
            None => (f.x, f.y, f.w, f.h),
        };
        xfrm(xml, f, Some(children));
        xml.push_str("</p:grpSpPr>");
        for child in &s.children {
            self.shape(xml, child)?;
        }
        xml.push_str("</p:grpSp>");
        Ok(())
    }

    /// The relationship to picture `pib` of the store, adding its media part on first use.
    fn image(&mut self, pib: u32) -> office_oxide::core::Result<Option<String>> {
        if let Some(rid) = self.images.get(&pib) {
            return Ok(Some(rid.clone()));
        }
        let Some(Some(picture)) = self.deck.pictures.get((pib as usize).wrapping_sub(1)) else { return Ok(None) };
        let target = match self.pkg.media.get(&pib) {
            Some(target) => target.clone(),
            None => {
                let (ext, content_type) = picture.media();
                let target = format!("media/image{}.{ext}", self.pkg.media.len() + 1);
                self.pkg.opc.register_default_content_type(ext, content_type);
                self.pkg.opc.add_part(&part(&format!("/ppt/{target}"))?, content_type, &picture.data)?;
                self.pkg.media.insert(pib, target.clone());
                target
            },
        };
        let rid = self.pkg.opc.add_part_rel(&self.part, rel_types::IMAGE, &format!("../{target}"));
        self.images.insert(pib, rid.clone());
        Ok(Some(rid))
    }

    fn placeholder(&self, xml: &mut String, s: &Shape<'a>) {
        let Some((kind, position)) = s.placeholder.and_then(|(placement, position)| Some((placeholder_type(placement)?, position))) else {
            return;
        };
        w!(xml, r#"<p:ph type="{kind}""#);
        if !matches!(kind, "title" | "ctrTitle") && position > 0 {
            w!(xml, r#" idx="{position}""#);
        }
        xml.push_str("/>");
    }

    fn picture(&mut self, xml: &mut String, s: &Shape<'a>, f: &Frame, rid: &str) {
        let id = self.next_id();
        w!(xml, r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="Picture {id}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr>"#);
        self.placeholder(xml, s);
        w!(xml, r#"</p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="{rid}"/>"#);
        // Crops are 16.16 fractions of the picture; DrawingML counts thousandths of a percent.
        let crop = |id| self.prop(s, id).map_or(0, |v| i64::from(v as i32) * 100_000 / 65_536);
        let (l, t, r, b) = (crop(CROP_LEFT), crop(CROP_TOP), crop(CROP_RIGHT), crop(CROP_BOTTOM));
        if (l, t, r, b) != (0, 0, 0, 0) {
            w!(xml, r#"<a:srcRect l="{l}" t="{t}" r="{r}" b="{b}"/>"#);
        }
        xml.push_str("<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>");
        xfrm(xml, f, None);
        xml.push_str(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#);
        self.line(xml, s, false);
        xml.push_str("</p:spPr></p:pic>");
    }

    fn sp(&mut self, xml: &mut String, s: &Shape<'a>, f: &Frame) -> office_oxide::core::Result<()> {
        let id = self.next_id();
        let name = if s.placeholder.is_some() { "Placeholder" } else { "Shape" };
        let text_box = if s.kind == TEXT_BOX { r#" txBox="1""# } else { "" };
        w!(xml, r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name} {id}"/><p:cNvSpPr{text_box}/><p:nvPr>"#);
        self.placeholder(xml, s);
        xml.push_str("</p:nvPr></p:nvSpPr><p:spPr>");
        xfrm(xml, f, None);
        w!(xml, r#"<a:prstGeom prst="{}"><a:avLst/></a:prstGeom>"#, preset(s.kind));
        // Free-form shapes (type 0) keep only their text; placeholders draw nothing their master doesn't.
        let drawn = s.kind != 0;
        let plain = drawn && s.placeholder.is_none();
        if drawn && !is_line(s.kind) {
            self.fill(xml, s, plain)?;
        } else {
            xml.push_str("<a:noFill/>");
        }
        if drawn {
            self.line(xml, s, plain);
        } else {
            xml.push_str("<a:ln><a:noFill/></a:ln>");
        }
        xml.push_str("</p:spPr>");
        let outline = self.outline;
        let block = match &s.text {
            Some(ShapeText::Inline(block)) => Some(block),
            Some(ShapeText::Outline(i)) => outline.get(*i),
            None => None,
        };
        if let Some(block) = block {
            self.text_body(xml, s, block);
        }
        xml.push_str("</p:sp>");
        Ok(())
    }

    // Limit: pattern fills draw in their foreground colour and gradients as two-colour linear ones; reading `fillShadeColors` and the pattern bitmaps lifts it.
    fn fill(&mut self, xml: &mut String, s: &Shape<'a>, default: bool) -> office_oxide::core::Result<()> {
        if !self.flag(s, FILL_FLAGS, FILLED).unwrap_or(default) {
            xml.push_str("<a:noFill/>");
            return Ok(());
        }
        let color = self.prop(s, FILL_COLOR).and_then(|c| self.color(c)).unwrap_or([0xFF; 3]);
        let alpha = self.prop(s, FILL_OPACITY).map(|o| (i64::from(o) * 100_000 / 65_536).clamp(0, 100_000)).filter(|&a| a < 100_000);
        match self.prop(s, FILL_TYPE).unwrap_or(0) {
            // Texture (tiled) and picture (stretched).
            kind @ (2 | 3) => {
                if let Some(rid) = self.prop(s, FILL_BLIP).filter(|&pib| pib > 0).map(|pib| self.image(pib)).transpose()?.flatten() {
                    let mode = if kind == 2 { "<a:tile/>" } else { "<a:stretch><a:fillRect/></a:stretch>" };
                    w!(xml, r#"<a:blipFill rotWithShape="1"><a:blip r:embed="{rid}"/>{mode}</a:blipFill>"#);
                    return Ok(());
                }
            },
            // Shaded fills: fillColor first, fillBackColor last at fillFocus percent along a vector that points
            // up at angle 0 and turns counterclockwise.
            4..=8 => {
                let back = self.prop(s, FILL_BACK_COLOR).and_then(|c| self.color(c)).unwrap_or([0xFF; 3]);
                let angle = self.prop(s, FILL_ANGLE).map_or(0, |a| i64::from(a as i32) / 65_536);
                let focus = self.prop(s, FILL_FOCUS).map_or(0, |f| i64::from(f as i32).abs().min(100));
                xml.push_str("<a:gradFill rotWithShape=\"1\"><a:gsLst>");
                let mut stop = |pos: i64, rgb: [u8; 3]| w!(xml, r#"<a:gs pos="{}"><a:srgbClr val="{}"/></a:gs>"#, pos * 1000, Hex(rgb));
                match focus {
                    0 => {
                        stop(0, back);
                        stop(100, color);
                    },
                    100 => {
                        stop(0, color);
                        stop(100, back);
                    },
                    middle => {
                        stop(0, color);
                        stop(middle, back);
                        stop(100, color);
                    },
                }
                w!(xml, r#"</a:gsLst><a:lin ang="{}" scaled="0"/></a:gradFill>"#, (270 - angle).rem_euclid(360) * 60_000);
                return Ok(());
            },
            _ => {},
        }
        solid_fill(xml, color, alpha);
        Ok(())
    }

    fn line(&self, xml: &mut String, s: &Shape<'a>, default: bool) {
        if !self.flag(s, LINE_FLAGS, LINE).unwrap_or(default) {
            xml.push_str("<a:ln><a:noFill/></a:ln>");
            return;
        }
        w!(xml, r#"<a:ln w="{}">"#, self.prop(s, LINE_WIDTH).map_or(9525, |w| w.min(20_116_800)));
        solid_fill(xml, self.prop(s, LINE_COLOR).and_then(|c| self.color(c)).unwrap_or([0; 3]), None);
        let dash = match self.prop(s, LINE_DASHING) {
            Some(1) => "sysDash",
            Some(2) => "sysDot",
            Some(3) => "sysDashDot",
            Some(4) => "sysDashDotDot",
            Some(5) => "dot",
            Some(6) => "dash",
            Some(7) => "lgDash",
            Some(8) => "dashDot",
            Some(9) => "lgDashDot",
            Some(10) => "lgDashDotDot",
            _ => "",
        };
        if !dash.is_empty() {
            w!(xml, r#"<a:prstDash val="{dash}"/>"#);
        }
        for (id, tag) in [(LINE_START_ARROW, "a:headEnd"), (LINE_END_ARROW, "a:tailEnd")] {
            let kind = match self.prop(s, id) {
                Some(1) => "triangle",
                Some(2) => "stealth",
                Some(3) => "diamond",
                Some(4) => "oval",
                Some(5) => "arrow",
                _ => continue,
            };
            w!(xml, r#"<{tag} type="{kind}"/>"#);
        }
        xml.push_str("</a:ln>");
    }

    fn text_body(&self, xml: &mut String, s: &Shape<'a>, block: &TextBlock<'a>) {
        let inset = |id, default| self.prop(s, id).unwrap_or(default);
        let wrap = if self.prop(s, WRAP_TEXT) == Some(2) { "none" } else { "square" };
        let anchor = self.prop(s, ANCHOR_TEXT).unwrap_or(0);
        let (left, top, right, bottom) = (inset(TEXT_LEFT, 91_440), inset(TEXT_TOP, 45_720), inset(TEXT_RIGHT, 91_440), inset(TEXT_BOTTOM, 45_720));
        let side = match anchor {
            1 | 4 => "ctr",
            2 | 5 | 7 | 9 => "b",
            _ => "t",
        };
        w!(xml, r#"<p:txBody><a:bodyPr wrap="{wrap}" lIns="{left}" tIns="{top}" rIns="{right}" bIns="{bottom}" anchor="{side}""#);
        if matches!(anchor, 3 | 4 | 5 | 8 | 9) {
            xml.push_str(r#" anchorCtr="1""#);
        }
        match self.prop(s, TEXT_FLOW) {
            Some(1 | 5) => xml.push_str(r#" vert="vert""#),
            Some(2) => xml.push_str(r#" vert="vert270""#),
            _ => {},
        }
        xml.push_str(r#" rtlCol="0"/><a:lstStyle/>"#);
        self.paragraphs(xml, block);
        xml.push_str("</p:txBody>");
    }

    /// The text as paragraphs (split at CR) of runs (split where the formatting changes; a vertical tab is a
    /// line break), each with its formatting resolved against the master's text styles.
    // Limit: bullet size and colour, picture and numbered bullets, tab stops and superscript are not written; the `bulletSize`, `bulletColor`, PP9 and `position` fields lift it.
    fn paragraphs(&self, xml: &mut String, block: &TextBlock<'a>) {
        let text = &block.text;
        let (paras, runs) = text_style(block.style, text.len());
        let mut start = 0;
        loop {
            let end = text[start..].iter().position(|&u| u == 0x0D).map_or(text.len(), |p| start + p);
            let (level, own) = run_at(&paras, start).copied().unwrap_or_default();
            let (master_para, master_chars) = self.styles.level(block.kind, level);
            xml.push_str("<a:p>");
            self.para_props(xml, level, &own.or(master_para));
            let mut at = start;
            while at < end {
                let run_end = runs.get(runs.partition_point(|r| r.0 <= at)).map_or(end, |r| r.0.min(end));
                let chars = run_at(&runs, at).copied().unwrap_or_default().or(master_chars);
                for (i, piece) in text[at..run_end].split(|&u| u == 0x0B).enumerate() {
                    if i > 0 {
                        xml.push_str("<a:br/>");
                    }
                    if !piece.is_empty() {
                        xml.push_str("<a:r>");
                        self.run_props(xml, "a:rPr", &chars);
                        w!(xml, "<a:t>{}</a:t></a:r>", esc(&String::from_utf16_lossy(piece)));
                    }
                }
                at = run_end;
            }
            let chars = run_at(&runs, end).copied().unwrap_or_default().or(master_chars);
            self.run_props(xml, "a:endParaRPr", &chars);
            xml.push_str("</a:p>");
            if end >= text.len() {
                break;
            }
            start = end + 1;
        }
    }

    fn para_props(&self, xml: &mut String, level: u16, p: &Para) {
        xml.push_str("<a:pPr");
        if level > 0 {
            w!(xml, r#" lvl="{level}""#);
        }
        if let Some(left) = p.left {
            w!(xml, r#" marL="{}""#, emu(left.into()));
        }
        if let Some(indent) = p.indent {
            w!(xml, r#" indent="{}""#, emu(i64::from(indent) - i64::from(p.left.unwrap_or(0))));
        }
        let align = match p.align {
            Some(0) => "l",
            Some(1) => "ctr",
            Some(2) => "r",
            Some(3) => "just",
            Some(4) => "dist",
            Some(5) => "thaiDist",
            Some(6) => "justLow",
            _ => "",
        };
        if !align.is_empty() {
            w!(xml, r#" algn="{align}""#);
        }
        xml.push('>');
        spacing(xml, "a:lnSpc", p.line);
        spacing(xml, "a:spcBef", p.before);
        spacing(xml, "a:spcAft", p.after);
        if p.bullet == Some(true) {
            let face = p.bullet_font.filter(|_| p.bullet_has_font == Some(true)).and_then(|f| self.deck.fonts.get(usize::from(f)));
            if let Some(face) = face {
                w!(xml, r#"<a:buFont typeface="{}"/>"#, esc(face));
            }
            let bullet = p.bullet_char.and_then(|c| char::from_u32(c.into())).filter(|&c| c >= ' ').unwrap_or('\u{2022}');
            w!(xml, r#"<a:buChar char="{}"/>"#, esc(bullet.encode_utf8(&mut [0; 4])));
        } else {
            xml.push_str("<a:buNone/>");
        }
        xml.push_str("</a:pPr>");
    }

    fn run_props(&self, xml: &mut String, tag: &str, c: &Chars) {
        w!(xml, r#"<{tag} sz="{}""#, u32::from(c.size.unwrap_or(18).clamp(1, 4000)) * 100);
        if c.bold == Some(true) {
            xml.push_str(r#" b="1""#);
        }
        if c.italic == Some(true) {
            xml.push_str(r#" i="1""#);
        }
        if c.underline == Some(true) {
            xml.push_str(r#" u="sng""#);
        }
        xml.push('>');
        if let Some(rgb) = c.color.and_then(|c| self.text_color(c)) {
            solid_fill(xml, rgb, None);
        }
        if let Some(face) = c.font.and_then(|f| self.deck.fonts.get(usize::from(f))) {
            w!(xml, r#"<a:latin typeface="{}"/>"#, esc(face));
        }
        w!(xml, "</{tag}>");
    }
}

fn index_shapes<'d, 'a>(shapes: &'d [Shape<'a>], out: &mut HashMap<u32, &'d Shape<'a>>) {
    for s in shapes {
        out.insert(s.spid, s);
        index_shapes(&s.children, out);
    }
}

/// A theme carrying the master's colour scheme and the deck's first font.
fn theme(scheme: &[[u8; 3]; 8], font: &str) -> String {
    let mut colors = String::new();
    let slots = [("dk1", 1), ("lt1", 0), ("dk2", 3), ("lt2", 2), ("accent1", 4), ("accent2", 5), ("accent3", 6), ("accent4", 7)];
    for (slot, i) in slots {
        w!(colors, r#"<a:{slot}><a:srgbClr val="{}"/></a:{slot}>"#, Hex(scheme[i]));
    }
    w!(
        colors,
        r#"<a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="{}"/></a:hlink><a:folHlink><a:srgbClr val="{}"/></a:folHlink>"#,
        Hex(scheme[6]),
        Hex(scheme[7])
    );
    let font = esc(font);
    let fill = r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#;
    let line = r#"<a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>"#;
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        r#"{DECL}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office"><a:themeElements><a:clrScheme name="Office">{colors}</a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{line}{line}{line}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#
    )
}

// Limit: footers, dates and slide numbers set in Header and Footer are not shown on slides; writing the master's placeholders onto each slide from `HeadersFootersAtom` lifts it.
fn write(deck: &Deck<'_>) -> office_oxide::core::Result<Vec<u8>> {
    let mut pkg = Package { opc: OpcWriter::new(Cursor::new(Vec::new()))?, media: HashMap::new() };
    let presentation = part("/ppt/presentation.xml")?;
    pkg.opc.add_package_rel(rel_types::OFFICE_DOCUMENT, "ppt/presentation.xml");
    let mut masters = HashMap::new();
    for sheet in deck.masters.iter().chain(&deck.notes_master) {
        index_shapes(&sheet.shapes, &mut masters);
    }
    let masters = &masters;
    let font = deck.fonts.first().map_or("Arial", String::as_str);
    let default_styles = Styles::read(None, deck.other_style);
    let mains: Vec<&Sheet<'_>> = deck.masters.iter().filter(|m| m.container == MAIN_MASTER).collect();
    let styles: HashMap<u32, Styles> = mains.iter().map(|&m| (m.id, Styles::read(Some(m), deck.other_style))).collect();
    let styles_of = |sheet| deck.main_master(sheet).and_then(|m| styles.get(&m.id)).unwrap_or(&default_styles);

    // Each main master becomes a slide master with a plain layout, plus one layout per title master on it.
    // Master and layout ids share one range.
    let mut next_id = 0x8000_0000u32;
    let mut master_list = String::new();
    let mut layouts: HashMap<u32, String> = HashMap::new();
    let mut layout_count = 0;
    for (i, &master) in mains.iter().enumerate() {
        let n = i + 1;
        let master_part = part(&format!("/ppt/slideMasters/slideMaster{n}.xml"))?;
        let rid = pkg.opc.add_part_rel(&presentation, rel_types::SLIDE_MASTER, &format!("slideMasters/slideMaster{n}.xml"));
        w!(master_list, r#"<p:sldMasterId id="{next_id}" r:id="{rid}"/>"#);
        next_id += 1;
        let master_styles = &styles[&master.id];
        let orphan = |t: &Sheet<'_>| i == 0 && !mains.iter().any(|m| m.id == t.master_id);
        let titles = deck.masters.iter().filter(|t| t.container == SLIDE && (t.master_id == master.id || orphan(t)));
        let mut layout_list = String::new();
        for layout in std::iter::once(master).chain(titles) {
            layout_count += 1;
            let target = format!("slideLayouts/slideLayout{layout_count}.xml");
            let layout_part = part(&format!("/ppt/{target}"))?;
            let rid = pkg.opc.add_part_rel(&master_part, rel_types::SLIDE_LAYOUT, &format!("../{target}"));
            pkg.opc.add_part_rel(&layout_part, rel_types::SLIDE_MASTER, &format!("../slideMasters/slideMaster{n}.xml"));
            w!(layout_list, r#"<p:sldLayoutId id="{next_id}" r:id="{rid}"/>"#);
            next_id += 1;
            layouts.insert(layout.id, format!("../{target}"));
            let empty = Sheet::default();
            let content = if layout.container == SLIDE { layout } else { &empty };
            let xml = Page::new(&mut pkg, deck, masters, layout_part.clone(), deck.scheme(content), master_styles, &[]).sheet(
                "sldLayout",
                content,
                CLR_MAP_OVR,
            )?;
            pkg.opc.add_part(&layout_part, CT_LAYOUT, xml.as_bytes())?;
        }
        pkg.opc.add_part_rel(&master_part, rel_types::THEME, &format!("../theme/theme{n}.xml"));
        let scheme = deck.scheme(master);
        pkg.opc.add_part(&part(&format!("/ppt/theme/theme{n}.xml"))?, CT_THEME, theme(&scheme, font).as_bytes())?;
        let tail = format!("{CLR_MAP}<p:sldLayoutIdLst>{layout_list}</p:sldLayoutIdLst>");
        let xml = Page::new(&mut pkg, deck, masters, master_part.clone(), scheme, master_styles, &[]).sheet("sldMaster", master, &tail)?;
        pkg.opc.add_part(&master_part, CT_MASTER, xml.as_bytes())?;
    }

    let mut slide_list = String::new();
    let mut has_notes = false;
    for (i, slide) in deck.slides.iter().enumerate() {
        let n = i + 1;
        let target = format!("slides/slide{n}.xml");
        let slide_part = part(&format!("/ppt/{target}"))?;
        let rid = pkg.opc.add_part_rel(&presentation, rel_types::SLIDE, &target);
        w!(slide_list, r#"<p:sldId id="{}" r:id="{rid}"/>"#, 256 + i);
        let layout = layouts.get(&slide.master_id).map_or("../slideLayouts/slideLayout1.xml", String::as_str);
        pkg.opc.add_part_rel(&slide_part, rel_types::SLIDE_LAYOUT, layout);
        let slide_styles = styles_of(slide);
        if let Some(notes) = deck.notes.iter().find(|x| slide.notes_id != 0 && x.id == slide.notes_id) {
            has_notes = true;
            let notes_target = format!("notesSlides/notesSlide{n}.xml");
            let notes_part = part(&format!("/ppt/{notes_target}"))?;
            pkg.opc.add_part_rel(&slide_part, rel_types::NOTES_SLIDE, &format!("../{notes_target}"));
            pkg.opc.add_part_rel(&notes_part, rel_types::NOTES_MASTER, "../notesMasters/notesMaster1.xml");
            pkg.opc.add_part_rel(&notes_part, rel_types::SLIDE, &format!("../{target}"));
            let xml = Page::new(&mut pkg, deck, masters, notes_part.clone(), deck.scheme(notes), slide_styles, &notes.outline).sheet(
                "notes",
                notes,
                CLR_MAP_OVR,
            )?;
            pkg.opc.add_part(&notes_part, CT_NOTES, xml.as_bytes())?;
        }
        let xml = Page::new(&mut pkg, deck, masters, slide_part.clone(), deck.scheme(slide), slide_styles, &slide.outline).sheet(
            "sld",
            slide,
            CLR_MAP_OVR,
        )?;
        pkg.opc.add_part(&slide_part, CT_SLIDE, xml.as_bytes())?;
    }

    let mut notes_list = String::new();
    if has_notes {
        let notes_master_part = part("/ppt/notesMasters/notesMaster1.xml")?;
        let rid = pkg.opc.add_part_rel(&presentation, rel_types::NOTES_MASTER, "notesMasters/notesMaster1.xml");
        w!(notes_list, r#"<p:notesMasterIdLst><p:notesMasterId r:id="{rid}"/></p:notesMasterIdLst>"#);
        let n = mains.len() + 1;
        pkg.opc.add_part_rel(&notes_master_part, rel_types::THEME, &format!("../theme/theme{n}.xml"));
        let empty = Sheet::default();
        let sheet = deck.notes_master.as_ref().unwrap_or(&empty);
        let scheme = deck.scheme(sheet);
        pkg.opc.add_part(&part(&format!("/ppt/theme/theme{n}.xml"))?, CT_THEME, theme(&scheme, font).as_bytes())?;
        let xml = Page::new(&mut pkg, deck, masters, notes_master_part.clone(), scheme, &default_styles, &[]).sheet("notesMaster", sheet, CLR_MAP)?;
        pkg.opc.add_part(&notes_master_part, CT_NOTES_MASTER, xml.as_bytes())?;
    }

    pkg.opc.add_part_rel(&presentation, rel_types::PRES_PROPS, "presProps.xml");
    pkg.opc.add_part(&part("/ppt/presProps.xml")?, CT_PRES_PROPS, format!("{DECL}<p:presentationPr {NS}/>").as_bytes())?;
    if !slide_list.is_empty() {
        slide_list = format!("<p:sldIdLst>{slide_list}</p:sldIdLst>");
    }
    // `ST_SlideSizeCoordinate`: 1 to 56 inches.
    let size = |v: i64| v.clamp(914_400, 51_206_400);
    let xml = format!(
        r#"{DECL}<p:presentation {NS}><p:sldMasterIdLst>{master_list}</p:sldMasterIdLst>{notes_list}{slide_list}<p:sldSz cx="{}" cy="{}"/><p:notesSz cx="{}" cy="{}"/></p:presentation>"#,
        size(deck.slide_size.0),
        size(deck.slide_size.1),
        deck.notes_size.0.max(1),
        deck.notes_size.1.max(1),
    );
    pkg.opc.add_part(&presentation, CT_PRESENTATION, xml.as_bytes())?;
    Ok(pkg.opc.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    // Fixtures from Apache POI test-data (Apache-2.0): text_shapes.ppt, Single_Coloured_Page_With_Fonts_and_Alignments.ppt,
    // 54541_cropped_bitmap.ppt, bug61881.ppt.
    use std::io::Cursor;

    use office_oxide::core::opc::{OpcReader, PartName};

    use super::to_pptx;
    use crate::office::{OfficeDoc, OfficeFamily};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/pptshapes/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn parts(package: &[u8]) -> OpcReader<Cursor<Vec<u8>>> {
        OpcReader::new(Cursor::new(package.to_vec())).unwrap()
    }

    fn part(package: &[u8], name: &str) -> String {
        String::from_utf8(parts(package).read_part(&PartName::new(name).unwrap()).unwrap()).unwrap()
    }

    /// The `<{tag}>` element around the first occurrence of `inner`.
    fn element<'a>(xml: &'a str, tag: &str, inner: &str) -> &'a str {
        let at = xml.find(inner).unwrap_or_else(|| panic!("{inner} missing: {xml}"));
        let start = xml[..at].rfind(&format!("<{tag}>")).unwrap();
        let end = at + xml[at..].find(&format!("</{tag}>")).unwrap();
        &xml[start..end]
    }

    fn shape_with<'a>(xml: &'a str, text: &str) -> &'a str {
        element(xml, "p:sp", &format!("<a:t>{text}</a:t>"))
    }

    fn run_with<'a>(xml: &'a str, text: &str) -> &'a str {
        element(xml, "a:r", &format!("<a:t>{text}</a:t>"))
    }

    #[test]
    fn ppt_shapes_keep_place_size_and_kind() {
        let pptx = to_pptx(fixture("text_shapes.ppt")).unwrap();
        let presentation = part(&pptx, "/ppt/presentation.xml");
        assert!(presentation.contains(r#"<p:sldSz cx="9144000" cy="6858000"/>"#), "{presentation}");
        let slide = part(&pptx, "/ppt/slides/slide1.xml");
        // ClientAnchor top 1584, left 3744, right 4656, bottom 2160 master units (576 per inch).
        let rectangle = shape_with(&slide, "Rectangle");
        assert!(rectangle.contains(r#"<a:off x="5943600" y="2514600"/><a:ext cx="1447800" cy="914400"/>"#), "{rectangle}");
        assert!(rectangle.contains(r#"<a:prstGeom prst="rect">"#), "{rectangle}");
        // Filled with the scheme's fill colour, outlined in its line colour.
        assert!(rectangle.contains(r#"<a:solidFill><a:srgbClr val="BBE0E3"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="000000"/>"#), "{rectangle}");
        let ellipse = shape_with(&slide, "Ellipse");
        assert!(ellipse.contains(r#"<a:off x="3962400" y="3276600"/><a:ext cx="914400" cy="914400"/>"#), "{ellipse}");
        assert!(ellipse.contains(r#"<a:prstGeom prst="ellipse">"#), "{ellipse}");
        let octagon = shape_with(&slide, "Octagon");
        assert!(octagon.contains(r#"<a:off x="5105400" y="5029200"/><a:ext cx="1524000" cy="914400"/>"#), "{octagon}");
        assert!(octagon.contains(r#"<a:prstGeom prst="octagon">"#), "{octagon}");
        let text_box = shape_with(&slide, "Text in a TextBox");
        assert!(text_box.contains(r#"<p:cNvSpPr txBox="1"/>"#), "{text_box}");
        assert!(text_box.contains(r#"<a:bodyPr wrap="none""#), "{text_box}");
        // The title placeholder shows its outline text from the slide list.
        let title = shape_with(&slide, "Title Placeholder");
        assert!(title.contains(r#"<p:ph type="title"/>"#), "{title}");
        assert!(title.contains(r#"<a:ext cx="8229600" cy="1143000"/>"#), "{title}");
    }

    #[test]
    fn ppt_text_keeps_run_formatting() {
        let pptx = to_pptx(fixture("Single_Coloured_Page_With_Fonts_and_Alignments.ppt")).unwrap();
        let slide = part(&pptx, "/ppt/slides/slide1.xml");
        // The title takes 44 pt from the master's title style and is right-aligned on the slide.
        let title = shape_with(&slide, "This is a title, it\u{2019}s in black");
        assert!(title.contains(r#"<p:ph type="ctrTitle"/>"#), "{title}");
        assert!(title.contains(r#"algn="r""#), "{title}");
        assert!(run_with(&slide, "This is a title, it\u{2019}s in black").contains(r#"<a:rPr sz="4400">"#), "{title}");
        let bold = run_with(&slide, "This is the subtitle, in bold");
        assert!(bold.contains(r#"<a:rPr sz="2000" b="1">"#), "{bold}");
        let blue = run_with(&slide, "This bit is blue and italic");
        assert!(blue.contains(r#"<a:rPr sz="2000" i="1"><a:solidFill><a:srgbClr val="333399"/>"#), "{blue}");
        let red = run_with(&slide, "This bit is red (normal)");
        assert!(red.contains(r#"<a:rPr sz="2000"><a:solidFill><a:srgbClr val="FF3300"/>"#), "{red}");
        let underlined = "This bit is underlined, and in Times New Roman, but at a larger font size of 24 (rather than 20)";
        let underlined = run_with(&slide, underlined);
        assert!(underlined.contains(r#"<a:rPr sz="2400" u="sng">"#), "{underlined}");
        assert!(underlined.contains(r#"<a:latin typeface="Times New Roman"/>"#), "{underlined}");
        let red_paragraph = element(&slide, "a:p", "<a:t>This bit is red (normal)</a:t>");
        assert!(red_paragraph.contains(r#"algn="r""#), "{red_paragraph}");
    }

    #[test]
    fn ppt_pictures_keep_size_crop_and_rotated_groups() {
        let pptx = to_pptx(fixture("54541_cropped_bitmap.ppt")).unwrap();
        let slide = part(&pptx, "/ppt/slides/slide1.xml");
        // Six frames show the deck's one picture, stored once.
        assert_eq!(slide.matches("<p:pic>").count(), 6, "{slide}");
        let media: Vec<PartName> = parts(&pptx).part_names().into_iter().filter(|p| p.as_str().starts_with("/ppt/media/")).collect();
        assert_eq!(media.len(), 1, "{media:?}");
        assert!(parts(&pptx).read_part(&media[0]).unwrap().starts_with(b"\x89PNG"));
        // Top 3632, left 752, right 2976, bottom 4248; cropped 0x8787/65536 from the top and -0x2D2D/65536 at the bottom.
        let picture = element(&slide, "p:pic", r#"<a:srcRect l="0" t="52940" r="0" b="-17646"/>"#);
        assert!(picture.contains(r#"<a:off x="1193800" y="5765800"/><a:ext cx="3530600" cy="977900"/>"#), "{picture}");
        // A group turned 12.6 degrees, its children in its own coordinates.
        assert!(
            slide.contains(r#"<a:xfrm rot="755916"><a:off x="5715000" y="3000375"/><a:ext cx="2400300" cy="2660650"/><a:chOff x="5715376" y="3000987"/><a:chExt cx="2400367" cy="2659427"/></a:xfrm>"#),
            "{slide}"
        );
        // A group turned -46.8 degrees: its anchor bounds it turned a quarter more, so width and height swap about the centre.
        assert!(
            slide.contains(r#"<a:xfrm rot="18792219"><a:off x="6333860" y="3878531"/><a:ext cx="2659427" cy="904339"/><a:chOff x="4876800" y="3956050"/><a:chExt cx="5041900" cy="1714500"/></a:xfrm>"#),
            "{slide}"
        );
    }

    #[test]
    fn ppt_lines_and_autoshapes_keep_geometry() {
        let pptx = to_pptx(fixture("bug61881.ppt")).unwrap();
        // Slide 8: lines in the master scheme's white, one flipped with an arrowhead, one dashed and thicker.
        let slide = part(&pptx, "/ppt/slides/slide8.xml");
        assert!(
            slide.contains(r#"<a:xfrm flipH="1"><a:off x="2286000" y="1371600"/><a:ext cx="1905000" cy="228600"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="9525"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:tailEnd type="triangle"/></a:ln>"#),
            "{slide}"
        );
        assert!(
            slide.contains(r#"<a:off x="1752600" y="1676400"/><a:ext cx="609600" cy="0"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="28575"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:prstDash val="dash"/></a:ln>"#),
            "{slide}"
        );
        // Slide 3: three triangles in nested groups, each with its own fill.
        let slide = part(&pptx, "/ppt/slides/slide3.xml");
        assert_eq!(slide.matches(r#"<a:prstGeom prst="triangle">"#).count(), 3, "{slide}");
        assert!(slide.contains(r#"<a:off x="816" y="1104"/><a:ext cx="3936" cy="2640"/>"#), "{slide}");
        for fill in ["3366CC", "008000", "FF00FF"] {
            assert!(slide.contains(&format!(r#"<a:solidFill><a:srgbClr val="{fill}"/></a:solidFill><a:ln w="9525">"#)), "{fill}: {slide}");
        }
        assert!(
            slide.contains(r#"<a:off x="1295400" y="1752600"/><a:ext cx="6248400" cy="4235450"/><a:chOff x="816" y="1104"/><a:chExt cx="3936" cy="2668"/>"#),
            "{slide}"
        );
    }

    #[test]
    fn converted_ppt_opens_and_renders() {
        for (name, slides, text) in [
            ("text_shapes.ppt", 1, "Text in a TextBox"),
            ("Single_Coloured_Page_With_Fonts_and_Alignments.ppt", 1, "This bit is blue and italic"),
            ("54541_cropped_bitmap.ppt", 1, ""),
            ("bug61881.ppt", 28, "Ultrasound"),
        ] {
            let doc = OfficeDoc::open(fixture(name), OfficeFamily::Presentation).unwrap();
            assert_eq!(doc.page_count(), Some(slides), "{name}");
            let png = doc.render_page(0, 0.25).unwrap().unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{name}");
            let index = if name == "bug61881.ppt" { 7 } else { 0 };
            assert!(doc.page_text(index).unwrap().unwrap().contains(text), "{name}");
        }
    }
}
