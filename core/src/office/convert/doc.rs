//! Word 97-2003 documents as WordprocessingML. office_oxide reads text, formatting, tables and notes into
//! its model, but appends every picture at the end of the document at a default size. This module reads
//! each picture's anchor, size and placement from the binary file and puts it back beside the paragraph
//! that holds it, at its own size.

use std::collections::HashMap;
use std::io::Cursor;

use office_oxide::cfb::CfbReader;
use office_oxide::ir::{
    DocumentIR, Element, FloatAnchor, FloatingImage, Image, ImagePositioning, InlineContent, List, Section, TextWrap,
};
use office_oxide::{Document, DocumentFormat};

use super::art::{self, Picture};
use super::write_model;
use crate::CoreError;
use crate::office::bad;

pub(super) fn to_docx(bytes: Vec<u8>) -> Result<Vec<u8>, CoreError> {
    // Word 6/95 and encrypted files have no layout this reader knows; office_oxide's own reading stands.
    let found = Layout::read(&bytes);
    let doc = Document::from_reader(Cursor::new(bytes), DocumentFormat::Doc).map_err(|e| bad("file", e))?;
    let mut ir = doc.to_ir();
    if let Some(found) = found {
        place(&mut ir, found);
    }
    write_model(&ir, DocumentFormat::Docx)
}

/// Twips to EMU.
const EMU_PER_TWIP: i64 = 635;
/// `sprmCPicLocation`: where a picture character's `PICF` sits in the `Data` stream.
const SPRM_PIC_LOCATION: u16 = 0x6A03;
/// `sprmCFSpec`: the character is a special one (a picture, not the letter).
const SPRM_SPEC: u16 = 0x0855;
/// `sprmCFData`: the location holds form-field data, not a picture.
const SPRM_DATA: u16 = 0x0806;
/// `PICF.mfpf.mm` of an OfficeArt picture: plain, and with a file name.
const MM_SHAPE: u16 = 0x0064;
const MM_SHAPEFILE: u16 = 0x0066;
/// FibRgFcLcb97 pairs ([MS-DOC] §2.5.6), by index.
const FIB_PLCF_BTE_CHPX: usize = 12;
const FIB_CLX: usize = 33;
const FIB_PLC_SPA_MOM: usize = 40;
const FIB_DGG_INFO: usize = 50;
/// Shape properties ([MS-ODRAW] §2.3.4): horizontal and vertical reference frames.
const POSRELH: u16 = 0x0390;
const POSRELV: u16 = 0x0392;
/// How far ahead of the last matched paragraph the next one is looked for.
const MATCH_WINDOW: usize = 64;

/// What the binary file says about its main-text pictures, and the paragraphs they sit in.
struct Layout {
    paragraphs: Vec<Paragraph>,
    pictures: Vec<Placed>,
}

/// A main-text paragraph: its character range and its letters and digits, lowercased, the way the
/// model's paragraphs are compared with it.
struct Paragraph {
    end: u32,
    first_letter: Option<u32>,
    key: String,
}

struct Placed {
    cp: u32,
    picture: Picture,
    width: Option<u64>,
    height: Option<u64>,
    floating: Option<FloatingImage>,
}

/// One piece of the piece table: a run of characters stored contiguously.
struct Piece {
    cp: u32,
    cp_end: u32,
    fc: u32,
    compressed: bool,
}

impl Layout {
    fn read(bytes: &[u8]) -> Option<Self> {
        let mut cfb = CfbReader::new(Cursor::new(bytes)).ok()?;
        let word = cfb.open_stream("WordDocument").ok()?;
        let flags = u16_at(&word, 0x0A)?;
        // Word 97 and later only (wIdent), unencrypted (fEncrypted), with a FIB long enough to hold fcDggInfo.
        if u16_at(&word, 0)? != 0xA5EC || flags & 0x0100 != 0 || u16_at(&word, 0x98)? as usize <= FIB_DGG_INFO {
            return None;
        }
        let table = cfb.open_stream(if flags & 0x0200 != 0 { "1Table" } else { "0Table" }).ok()?;
        let data = if cfb.has_stream("Data") { cfb.open_stream("Data").ok()? } else { Vec::new() };
        let fib = |i: usize| -> Option<&[u8]> {
            let fc = u32_at(&word, 0x9A + i * 8)? as usize;
            let lcb = u32_at(&word, 0x9A + i * 8 + 4)? as usize;
            if lcb == 0 { Some(&[]) } else { table.get(fc..fc.checked_add(lcb)?) }
        };
        let ccp_text = u32_at(&word, 0x4C)?;
        let pieces = pieces(fib(FIB_CLX)?)?;
        let text = main_text(&word, &pieces, ccp_text);
        let paragraphs = paragraphs(&text);
        let mut pictures = inline_pictures(&word, &data, &pieces, &text, fib(FIB_PLCF_BTE_CHPX)?);
        pictures.extend(floating_pictures(&word, fib(FIB_PLC_SPA_MOM)?, fib(FIB_DGG_INFO)?, ccp_text));
        pictures.sort_by_key(|p| p.cp);
        Some(Layout { paragraphs, pictures })
    }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(data: &[u8], at: usize) -> Option<i64> {
    Some(i32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?).into())
}

fn i16_at(data: &[u8], at: usize) -> Option<i64> {
    Some(i16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?).into())
}

/// The piece table in the `Clx` ([MS-DOC] §2.9.38): property runs to skip, then the `Pcdt`.
fn pieces(clx: &[u8]) -> Option<Vec<Piece>> {
    let mut pos = 0;
    while *clx.get(pos)? == 0x01 {
        pos += 3 + u16_at(clx, pos + 1)? as usize;
    }
    if *clx.get(pos)? != 0x02 {
        return None;
    }
    let len = u32_at(clx, pos + 1)? as usize;
    let plc = clx.get(pos + 5..(pos + 5).checked_add(len)?)?;
    let n = len.checked_sub(4)? / 12;
    (0..n)
        .map(|i| {
            let fc = u32_at(plc, (n + 1) * 4 + i * 8 + 2)?;
            let compressed = fc & 0x4000_0000 != 0;
            Some(Piece {
                cp: u32_at(plc, i * 4)?,
                cp_end: u32_at(plc, i * 4 + 4)?,
                fc: if compressed { (fc & 0x3FFF_FFFF) / 2 } else { fc },
                compressed,
            })
        })
        .collect()
}

/// The main text, one `char` per character position.
fn main_text(word: &[u8], pieces: &[Piece], ccp_text: u32) -> Vec<char> {
    let mut text = vec!['\0'; ccp_text as usize];
    for piece in pieces {
        let (start, end) = (piece.cp.min(ccp_text), piece.cp_end.min(ccp_text));
        let count = end.saturating_sub(start) as usize;
        let fc = piece.fc as usize;
        if piece.compressed {
            let Some(raw) = word.get(fc..fc + count) else { continue };
            let (decoded, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(raw);
            for (slot, c) in text[start as usize..].iter_mut().zip(decoded.chars()) {
                *slot = c;
            }
        } else {
            let Some(raw) = word.get(fc..fc + count * 2) else { continue };
            for (slot, unit) in text[start as usize..].iter_mut().zip(raw.chunks_exact(2)) {
                *slot = char::from_u32(u16::from_le_bytes([unit[0], unit[1]]).into()).unwrap_or('\u{FFFD}');
            }
        }
    }
    text
}

/// The paragraphs of the main text (ended by a paragraph or cell mark), without field codes.
fn paragraphs(text: &[char]) -> Vec<Paragraph> {
    let mut out = Vec::new();
    // Per open field: whether its code (not its result) is being read.
    let mut fields: Vec<bool> = Vec::new();
    let mut key = String::new();
    let mut first_letter = None;
    for (cp, &c) in text.iter().enumerate() {
        match c {
            '\u{13}' => fields.push(true),
            '\u{14}' => {
                if let Some(code) = fields.last_mut() {
                    *code = false;
                }
            },
            '\u{15}' => {
                fields.pop();
            },
            '\r' | '\u{7}' => {
                out.push(Paragraph { end: cp as u32 + 1, first_letter: first_letter.take(), key: std::mem::take(&mut key) });
            },
            c if c.is_alphanumeric() && !fields.contains(&true) => {
                first_letter.get_or_insert(cp as u32);
                key.extend(c.to_lowercase());
            },
            _ => {},
        }
    }
    if !key.is_empty() || first_letter.is_some() {
        out.push(Paragraph { end: text.len() as u32, first_letter, key });
    }
    out
}

/// The character positions stored in file bytes `fc_start..fc_end`.
fn cps_of(pieces: &[Piece], fc_start: u32, fc_end: u32) -> impl Iterator<Item = u32> + '_ {
    pieces.iter().flat_map(move |p| {
        let width = if p.compressed { 1 } else { 2 };
        let piece_end = p.fc.saturating_add((p.cp_end.saturating_sub(p.cp)).saturating_mul(width));
        let (from, to) = (fc_start.max(p.fc), fc_end.min(piece_end));
        (from..to).step_by(width as usize).map(move |fc| p.cp + (fc - p.fc) / width)
    })
}

/// Inline pictures: picture characters (0x01) whose character properties point at a `PICF` in the `Data` stream.
fn inline_pictures(word: &[u8], data: &[u8], pieces: &[Piece], text: &[char], plc: &[u8]) -> Vec<Placed> {
    let mut out = Vec::new();
    let n = plc.len().saturating_sub(4) / 8;
    for i in 0..n {
        let Some(pn) = u32_at(plc, (n + 1) * 4 + i * 4) else { break };
        let at = (pn & 0x003F_FFFF) as usize * 512;
        let Some(page) = word.get(at..at + 512) else { continue };
        let runs = page[511] as usize;
        for r in 0..runs {
            let (Some(fc_start), Some(fc_end)) = (u32_at(page, r * 4), u32_at(page, r * 4 + 4)) else { break };
            let offset = page.get(4 * (runs + 1) + r).map_or(0, |&b| b as usize * 2);
            if offset == 0 {
                continue;
            }
            let Some(&len) = page.get(offset) else { continue };
            let Some(grpprl) = page.get(offset + 1..offset + 1 + len as usize) else { continue };
            let (mut location, mut special, mut form_data) = (None, false, false);
            for (sprm, operand) in sprms(grpprl) {
                match sprm {
                    SPRM_PIC_LOCATION => location = u32_at(operand, 0),
                    SPRM_SPEC => special = operand.first().is_some_and(|&b| b != 0),
                    SPRM_DATA => form_data = operand.first().is_some_and(|&b| b != 0),
                    _ => {},
                }
            }
            let Some(location) = location.filter(|_| special && !form_data) else { continue };
            for cp in cps_of(pieces, fc_start, fc_end) {
                if text.get(cp as usize) != Some(&'\u{1}') {
                    continue;
                }
                if let Some((picture, width, height)) = inline_picture(data, location as usize) {
                    out.push(Placed { cp, picture, width, height, floating: None });
                }
            }
        }
    }
    out
}

/// The `(sprm, operand)` pairs of a property list ([MS-DOC] §2.2.5.1).
fn sprms(grpprl: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let sprm = u16_at(grpprl, pos)?;
        pos += 2;
        let size = match sprm >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => {
                let size = *grpprl.get(pos)? as usize;
                pos += 1;
                size
            },
        };
        let operand = grpprl.get(pos..pos + size)?;
        pos += size;
        Some((sprm, operand))
    })
}

/// The picture of the `PICF` at `at` ([MS-DOC] §2.9.192) and its displayed size in EMU.
fn inline_picture(data: &[u8], at: usize) -> Option<(Picture, Option<u64>, Option<u64>)> {
    let len = u32_at(data, at)? as usize;
    let mm = u16_at(data, at + 6)?;
    // Limit: pictures stored the Word 6 way (a bare metafile or bitmap after the PICF) keep office_oxide's placement at the document's end; decoding their payload here lifts it.
    if u16_at(data, at + 4)? != 0x44 || (mm != MM_SHAPE && mm != MM_SHAPEFILE) {
        return None;
    }
    let end = at.checked_add(len)?.min(data.len());
    let mut pos = at + 0x44;
    if mm == MM_SHAPEFILE {
        pos += 1 + *data.get(pos)? as usize;
    }
    // OfficeArtInlineSpContainer: the shape, then its own store entries.
    let rest = data.get(pos..end)?;
    let shape = art::records(rest).next()?;
    let shape_len = 8 + shape.body.len();
    let pib = art::picture_shapes(&rest[..shape_len]).first().map_or(1, |s| s.pib);
    let store = art::store_entries(rest.get(shape_len..)?, &[]);
    let picture = store.into_iter().nth((pib as usize).checked_sub(1)?)??;
    // Scaled goal size less the crop, in twips ([MS-DOC] PICF: mx/my are 1/1000).
    let size = |goal: i64, crop_a: i64, crop_b: i64, scale: i64| {
        let twips = (goal - crop_a - crop_b) * scale / 1000;
        (twips > 0).then(|| (twips * EMU_PER_TWIP) as u64)
    };
    let (dxa, dya) = (i16_at(data, at + 28)?, i16_at(data, at + 30)?);
    let (mx, my) = (i64::from(u16_at(data, at + 32)?), i64::from(u16_at(data, at + 34)?));
    let (left, top, right, bottom) = (i16_at(data, at + 36)?, i16_at(data, at + 38)?, i16_at(data, at + 40)?, i16_at(data, at + 42)?);
    // Limit: a cropped picture shows whole at its cropped size (the model has no crop); a srcRect in the written picture lifts it.
    Some((picture, size(dxa, left, right, mx), size(dya, top, bottom, my)))
}

/// Floating pictures: shapes anchored in the main text (`PlcSpaMom`) that show a stored picture.
fn floating_pictures(word: &[u8], plc: &[u8], content: &[u8], ccp_text: u32) -> Vec<Placed> {
    let mut out = Vec::new();
    if plc.is_empty() || content.is_empty() {
        return out;
    }
    // OfficeArtContent: the drawing group, then each drawing behind a one-byte label (0 = main text).
    let store = art::picture_store(content, word);
    let Some(group) = art::records(content).next() else { return out };
    let mut pos = 8 + group.body.len();
    let mut shapes = Vec::new();
    while let Some(&label) = content.get(pos) {
        let Some(drawing) = art::records(&content[pos + 1..]).next() else { break };
        let len = 8 + drawing.body.len();
        if label == 0 {
            shapes.extend(art::picture_shapes(&content[pos + 1..(pos + 1 + len).min(content.len())]));
        }
        pos += 1 + len;
    }
    // PlcfSpa: n+1 character positions, then n 26-byte FSPA ([MS-DOC] §2.9.253).
    let n = plc.len().saturating_sub(4) / 30;
    for i in 0..n {
        let (Some(cp), Some(spa)) = (u32_at(plc, i * 4), plc.get((n + 1) * 4 + i * 26..(n + 1) * 4 + (i + 1) * 26)) else {
            break;
        };
        let (Some(spid), Some(left), Some(top), Some(right), Some(bottom), Some(flags)) =
            (u32_at(spa, 0), i32_at(spa, 4), i32_at(spa, 8), i32_at(spa, 12), i32_at(spa, 16), u16_at(spa, 20))
        else {
            continue;
        };
        let Some(shape) = shapes.iter().find(|s| s.spid == spid) else { continue };
        let Some(Some(picture)) = store.get((shape.pib as usize).wrapping_sub(1)) else { continue };
        if cp >= ccp_text {
            continue;
        }
        let h_anchor = match shape.prop(POSRELH).unwrap_or(u32::from((flags >> 1) & 3) & 3) {
            0 => FloatAnchor::Margin,
            1 => FloatAnchor::Page,
            _ => FloatAnchor::Column,
        };
        let v_anchor = match shape.prop(POSRELV).unwrap_or(u32::from((flags >> 3) & 3)) {
            0 => FloatAnchor::Margin,
            1 => FloatAnchor::Page,
            _ => FloatAnchor::Paragraph,
        };
        let text_wrap = match (flags >> 5) & 0xF {
            1 => TextWrap::TopAndBottom,
            3 if flags & 0x4000 != 0 => TextWrap::Behind,
            3 => TextWrap::InFront,
            4 => TextWrap::Tight,
            5 => TextWrap::Through,
            _ => TextWrap::Square,
        };
        let (width, height) = (((right - left) * EMU_PER_TWIP).max(0) as u64, ((bottom - top) * EMU_PER_TWIP).max(0) as u64);
        out.push(Placed {
            cp,
            picture: picture.clone(),
            width: Some(width),
            height: Some(height),
            floating: Some(FloatingImage {
                x_emu: left * EMU_PER_TWIP,
                y_emu: top * EMU_PER_TWIP,
                width_emu: width,
                height_emu: height,
                h_anchor,
                v_anchor,
                text_wrap,
                allow_overlap: true,
            }),
        });
    }
    out
}

/// Where a picture goes among the model's paragraphs (counted in document order, tables and lists included).
enum Slot {
    Before(usize),
    After(usize),
    End,
}

/// Puts each picture beside the model paragraph that matches the file paragraph holding it.
fn place(ir: &mut DocumentIR, found: Layout) {
    if found.pictures.is_empty() {
        return;
    }
    // office_oxide's own copies, appended at the end at a default size, give way to the placed ones.
    if let Some(last) = ir.sections.last_mut() {
        let mut taken = vec![false; found.pictures.len()];
        last.elements.retain(|e| {
            let Element::Image(img) = e else { return true };
            let Some(bytes) = img.data.as_deref().filter(|_| img.display_width_emu.is_none()) else { return true };
            match found.pictures.iter().enumerate().position(|(i, p)| !taken[i] && p.picture.same_bytes(bytes)) {
                Some(i) => {
                    taken[i] = true;
                    false
                },
                None => true,
            }
        });
    }

    let mut keys = Vec::new();
    for section in &ir.sections {
        leaf_keys(&section.elements, &mut keys);
    }
    // Match file paragraphs to model paragraphs in order; empty ones carry no evidence.
    let mut matched = vec![None; found.paragraphs.len()];
    let mut cursor = 0;
    for (i, p) in found.paragraphs.iter().enumerate() {
        if p.key.is_empty() {
            continue;
        }
        if let Some(j) = (cursor..keys.len().min(cursor + MATCH_WINDOW)).find(|&j| keys[j] == p.key) {
            matched[i] = Some(j);
            cursor = j + 1;
        }
    }

    let mut before: HashMap<usize, Vec<Element>> = HashMap::new();
    let mut after: HashMap<usize, Vec<Element>> = HashMap::new();
    let mut end = Vec::new();
    for placed in found.pictures {
        let para = found.paragraphs.partition_point(|p| p.end <= placed.cp);
        let leading = found.paragraphs.get(para).and_then(|p| p.first_letter).is_none_or(|first| first > placed.cp);
        let slot = match matched.get(para).copied().flatten() {
            Some(j) if leading => Slot::Before(j),
            Some(j) => Slot::After(j),
            None => match matched[..para.min(matched.len())].iter().rev().find_map(|m| *m) {
                Some(j) => Slot::After(j),
                None => matched.iter().skip(para + 1).find_map(|m| *m).map_or(Slot::End, Slot::Before),
            },
        };
        let image = Element::Image(Image {
            data: Some(placed.picture.data),
            format: Some(placed.picture.format),
            display_width_emu: placed.width,
            display_height_emu: placed.height,
            positioning: placed.floating.map_or(ImagePositioning::Inline, ImagePositioning::Floating),
            ..Default::default()
        });
        match slot {
            Slot::Before(j) => before.entry(j).or_default().push(image),
            Slot::After(j) => after.entry(j).or_default().push(image),
            Slot::End => end.push(image),
        }
    }

    let mut next = 0;
    for section in &mut ir.sections {
        insert(&mut section.elements, &mut next, &mut before, &mut after);
    }
    if !end.is_empty() {
        if ir.sections.is_empty() {
            ir.sections.push(Section::default());
        }
        if let Some(last) = ir.sections.last_mut() {
            last.elements.extend(end);
        }
    }
}

/// Letters and digits of `content`, lowercased.
fn inline_key(content: &[InlineContent]) -> String {
    let mut key = String::new();
    for item in content {
        if let InlineContent::Text(span) = item {
            for c in span.text.chars().filter(|c| c.is_alphanumeric()) {
                key.extend(c.to_lowercase());
            }
        }
    }
    key
}

/// The key of every paragraph and heading in `elements`, in document order.
fn leaf_keys(elements: &[Element], out: &mut Vec<String>) {
    for e in elements {
        match e {
            Element::Paragraph(p) => out.push(inline_key(&p.content)),
            Element::Heading(h) => out.push(inline_key(&h.content)),
            Element::Table(t) => {
                for cell in t.rows.iter().flat_map(|r| &r.cells) {
                    leaf_keys(&cell.content, out);
                }
            },
            Element::List(l) => list_keys(l, out),
            _ => {},
        }
    }
}

fn list_keys(list: &List, out: &mut Vec<String>) {
    for item in &list.items {
        leaf_keys(&item.content, out);
        if let Some(nested) = &item.nested {
            list_keys(nested, out);
        }
    }
}

/// Inserts the pictures around paragraph `n` (same order as [`leaf_keys`]).
fn insert(
    elements: &mut Vec<Element>,
    next: &mut usize,
    before: &mut HashMap<usize, Vec<Element>>,
    after: &mut HashMap<usize, Vec<Element>>,
) {
    for mut e in std::mem::take(elements) {
        match &mut e {
            Element::Paragraph(_) | Element::Heading(_) => {
                let n = *next;
                *next += 1;
                elements.extend(before.remove(&n).unwrap_or_default());
                elements.push(e);
                elements.extend(after.remove(&n).unwrap_or_default());
                continue;
            },
            Element::Table(t) => {
                for cell in t.rows.iter_mut().flat_map(|r| &mut r.cells) {
                    insert(&mut cell.content, next, before, after);
                }
            },
            Element::List(l) => insert_list(l, next, before, after),
            _ => {},
        }
        elements.push(e);
    }
}

fn insert_list(
    list: &mut List,
    next: &mut usize,
    before: &mut HashMap<usize, Vec<Element>>,
    after: &mut HashMap<usize, Vec<Element>>,
) {
    for item in &mut list.items {
        insert(&mut item.content, next, before, after);
        if let Some(nested) = &mut item.nested {
            insert_list(nested, next, before, after);
        }
    }
}
