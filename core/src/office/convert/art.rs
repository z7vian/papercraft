//! OfficeArt drawing records ([MS-ODRAW]), the layer Word and Excel 97-2003 share for pictures:
//! the picture store (`BStore`) and the shapes that show its pictures.

use office_oxide::cfb::{BlipFormat, extract_blip_images};
use office_oxide::ir::ImageFormat;

/// `OfficeArtDggContainer`: the drawing group, which holds the picture store.
const DGG_CONTAINER: u16 = 0xF000;
/// `OfficeArtBStoreContainer`: the document's pictures, in `pib` order.
const BSTORE: u16 = 0xF001;
/// `OfficeArtSpContainer`: one shape.
const SP_CONTAINER: u16 = 0xF004;
/// `OfficeArtFBSE`: one picture store entry.
const FBSE: u16 = 0xF007;
/// `OfficeArtFSP`: the shape's id.
const FSP: u16 = 0xF00A;
/// `OfficeArtFOPT` and its secondary and tertiary siblings: the shape's properties.
const FOPTS: [u16; 3] = [0xF00B, 0xF121, 0xF122];
/// `OfficeArtClientAnchor`: where the host application placed the shape.
const CLIENT_ANCHOR: u16 = 0xF010;
/// Property `pib`: the 1-based store index of the picture the shape shows.
const PIB: u16 = 0x0104;
/// Containers nest; real files go a handful deep.
const MAX_DEPTH: u32 = 16;

/// One OfficeArt record: `recInstance`, `recType` and its body (cut at the end of the data).
pub(super) struct Record<'a> {
    pub instance: u16,
    pub kind: u16,
    pub container: bool,
    pub body: &'a [u8],
}

/// The sibling records laid end to end in `data`.
pub(super) fn records(data: &[u8]) -> impl Iterator<Item = Record<'_>> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let header = data.get(pos..pos + 8)?;
        let ver_inst = u16::from_le_bytes([header[0], header[1]]);
        let kind = u16::from_le_bytes([header[2], header[3]]);
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let start = pos + 8;
        let end = start.saturating_add(len).min(data.len());
        pos = start.saturating_add(len);
        Some(Record { instance: ver_inst >> 4, kind, container: ver_inst & 0xF == 0xF, body: &data[start..end] })
    })
}

/// A shape that shows a picture: its id, its options and its host anchor.
pub(super) struct Shape<'a> {
    pub spid: u32,
    pub pib: u32,
    props: Vec<(u16, u32)>,
    pub anchor: Option<&'a [u8]>,
}

impl Shape<'_> {
    /// The simple (non-complex) property `pid`, when the shape sets it.
    pub fn prop(&self, pid: u16) -> Option<u32> {
        self.props.iter().find(|(id, _)| *id == pid).map(|&(_, v)| v)
    }
}

/// Every picture shape in `data`, groups included, in file order.
pub(super) fn picture_shapes(data: &[u8]) -> Vec<Shape<'_>> {
    let mut out = Vec::new();
    collect_shapes(data, 0, &mut out);
    out
}

fn collect_shapes<'a>(data: &'a [u8], depth: u32, out: &mut Vec<Shape<'a>>) {
    if depth > MAX_DEPTH {
        return;
    }
    for rec in records(data) {
        if rec.kind == SP_CONTAINER {
            if let Some(shape) = shape(rec.body) {
                out.push(shape);
            }
        }
        if rec.container {
            collect_shapes(rec.body, depth + 1, out);
        }
    }
}

fn shape(body: &[u8]) -> Option<Shape<'_>> {
    let mut spid = None;
    let mut props = Vec::new();
    let mut anchor = None;
    for rec in records(body) {
        match rec.kind {
            FSP if rec.body.len() >= 4 => spid = Some(u32::from_le_bytes(rec.body[..4].try_into().ok()?)),
            CLIENT_ANCHOR => anchor = Some(rec.body),
            kind if FOPTS.contains(&kind) => {
                // `recInstance` counts the 6-byte property entries; complex data follows them.
                for entry in rec.body.chunks_exact(6).take(rec.instance as usize) {
                    let id = u16::from_le_bytes([entry[0], entry[1]]);
                    if id & 0x8000 == 0 {
                        props.push((id & 0x3FFF, u32::from_le_bytes([entry[2], entry[3], entry[4], entry[5]])));
                    }
                }
            },
            _ => {},
        }
    }
    let pib = props.iter().find(|(id, _)| *id == PIB).map(|&(_, v)| v)?;
    Some(Shape { spid: spid?, pib, props, anchor })
}

/// A decoded picture, ready for an OOXML media part.
#[derive(Clone)]
pub(super) struct Picture {
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

impl Picture {
    fn from_blip(format: BlipFormat, data: Vec<u8>) -> Option<Self> {
        let format = match format {
            // A BLIP stores a DIB without its file header; image readers need it.
            BlipFormat::Dib => return Some(Self { format: ImageFormat::Bmp, data: bmp_file(&data)? }),
            other => ImageFormat::from_blip(&other)?,
        };
        Some(Self { format, data })
    }

    /// Whether `other` holds this picture's bytes as office_oxide decodes them (a DIB without its header).
    pub fn same_bytes(&self, other: &[u8]) -> bool {
        self.data == other || (self.format == ImageFormat::Bmp && self.data.get(14..) == Some(other))
    }

    /// The part name extension and its content type.
    pub fn media(&self) -> (&'static str, &'static str) {
        (self.format.extension(), self.format.content_type())
    }
}

/// The first picture in a run of BLIP or FBSE records.
pub(super) fn first_picture(data: &[u8]) -> Option<Picture> {
    let blip = extract_blip_images(data).into_iter().next()?;
    Picture::from_blip(blip.format, blip.data)
}

/// The picture store of the drawing group that starts `data` (a `DggContainer` record), indexed by
/// `pib - 1`. A store entry whose picture lives outside the container (Word keeps it in the
/// `WordDocument` stream) is read from `delay`.
pub(super) fn picture_store(data: &[u8], delay: &[u8]) -> Vec<Option<Picture>> {
    let Some(group) = records(data).next().filter(|r| r.kind == DGG_CONTAINER) else {
        return Vec::new();
    };
    match records(group.body).find(|r| r.kind == BSTORE) {
        Some(store) => store_entries(store.body, delay),
        None => Vec::new(),
    }
}

/// The pictures of a run of store entries (`OfficeArtFBSE` records), indexed by `pib - 1`.
pub(super) fn store_entries(data: &[u8], delay: &[u8]) -> Vec<Option<Picture>> {
    records(data).map(|entry| if entry.kind == FBSE { fbse_picture(entry.body, delay) } else { None }).collect()
}

/// `OfficeArtFBSE`: 36 fixed bytes, the name, then the BLIP when it is embedded; otherwise `foDelay`
/// and `size` locate it in the delay stream.
fn fbse_picture(body: &[u8], delay: &[u8]) -> Option<Picture> {
    let size = u32::from_le_bytes(body.get(20..24)?.try_into().ok()?) as usize;
    let offset = u32::from_le_bytes(body.get(28..32)?.try_into().ok()?) as usize;
    let embedded = 36 + *body.get(33)? as usize;
    if body.len() > embedded {
        return first_picture(&body[embedded..]);
    }
    first_picture(delay.get(offset..offset.checked_add(size)?)?)
}

/// A `BITMAPFILEHEADER` in front of a DIB, so it is a .bmp file.
fn bmp_file(dib: &[u8]) -> Option<Vec<u8>> {
    let header_len = u32::from_le_bytes(dib.get(..4)?.try_into().ok()?);
    let bit_count = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32::from_le_bytes(dib.get(16..20)?.try_into().ok()?);
    let used = u32::from_le_bytes(dib.get(32..36)?.try_into().ok()?);
    let colors = match (used, bit_count) {
        (0, 1 | 4 | 8) => 1u32 << bit_count,
        (n, _) => n,
    };
    // BI_BITFIELDS keeps three colour masks after a 40-byte header.
    let masks = if compression == 3 && header_len == 40 { 12 } else { 0 };
    let pixels = 14 + header_len + masks + colors * 4;
    let total = u32::try_from(dib.len() + 14).ok()?;
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&pixels.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}
