use std::io::Cursor;

use crate::CoreError;
use image::error::ImageError;
use image::{ImageFormat, ImageReader, Limits};

// Limit: TIFFs whose decoded first page needs more than 128 MB (about 42 MP of 8-bit RGB) fail as too big;
// decode strip by strip into the shrunk image to open bigger scans.
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(uniffi::Record)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, ready for `Bitmap.copyPixelsFromBuffer`.
    pub pixels: Vec<u8>,
}

/// Decode a TIFF (Android can't), shrunk so neither side exceeds `max_side`. Only the first page of multi-page TIFFs.
#[uniffi::export]
pub fn decode_tiff(bytes: Vec<u8>, max_side: u32) -> Result<RgbaImage, CoreError> {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Tiff);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        ImageError::Limits(_) => CoreError::BadFile("This TIFF is too big to open.".into()),
        e => CoreError::BadFile(format!("Can't read this TIFF: {e}")),
    })?;
    let img = if img.width().max(img.height()) > max_side { img.thumbnail(max_side, max_side) } else { img };
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        let a = px[3] as u16;
        if a < 255 {
            for c in 0..3 {
                px[c] = (px[c] as u16 * a / 255) as u8;
            }
        }
    }
    Ok(RgbaImage { width: rgba.width(), height: rgba.height(), pixels: rgba.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::decode_tiff;
    use crate::CoreError;
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn tiff(width: u32, height: u32, px: Rgba<u8>) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        RgbaImage::from_pixel(width, height, px).write_to(&mut out, ImageFormat::Tiff).unwrap();
        out.into_inner()
    }

    /// A grey TIFF header claiming `side`×`side` pixels in one uncompressed strip, with no pixel data.
    fn tiff_header(side: u32) -> Vec<u8> {
        // (tag, type, value): type 3 SHORT, 4 LONG; one value each, stored in the entry.
        let entries: [(u16, u16, u32); 8] = [
            (256, 4, side),                // ImageWidth
            (257, 4, side),                // ImageLength
            (258, 3, 8),                   // BitsPerSample
            (259, 3, 1),                   // Compression: none
            (262, 3, 1),                   // Photometric: black is zero
            (273, 4, 8 + 2 + 8 * 12 + 4), // StripOffsets: just past the IFD
            (278, 4, side),                // RowsPerStrip
            (279, 4, side * side),         // StripByteCounts
        ];
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, kind, value) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(1u32.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        out.extend(0u32.to_le_bytes()); // no next IFD
        out
    }

    #[test]
    fn decodes_and_premultiplies() {
        let out = decode_tiff(tiff(3, 2, Rgba([200, 100, 50, 128])), 4096).unwrap();
        assert_eq!((out.width, out.height), (3, 2));
        assert_eq!(&out.pixels[..4], &[100, 50, 25, 128]);
    }

    #[test]
    fn shrinks_to_max_side_keeping_shape() {
        let out = decode_tiff(tiff(40, 20, Rgba([0, 0, 0, 255])), 10).unwrap();
        assert_eq!((out.width, out.height), (10, 5));
        assert_eq!(out.pixels.len(), 10 * 5 * 4);
    }

    #[test]
    fn junk_is_an_error() {
        assert!(decode_tiff(b"not a tiff".to_vec(), 4096).is_err());
    }

    #[test]
    fn huge_tiff_fails_before_allocating() {
        // 12000² 8-bit grey is 144 MB decoded: over the cap.
        match decode_tiff(tiff_header(12_000), 4096) {
            Err(CoreError::BadFile(msg)) => assert_eq!(msg, "This TIFF is too big to open."),
            _ => panic!("a huge TIFF must fail as too big"),
        }
    }
}
