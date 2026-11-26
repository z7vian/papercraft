use crate::CoreError;
use image::ImageFormat;

/// Biggest side we hand to Android. Larger bitmaps crash the Canvas ("too large bitmap").
const MAX_SIDE: u32 = 4096;

#[derive(uniffi::Record)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, ready for `Bitmap.copyPixelsFromBuffer`.
    pub pixels: Vec<u8>,
}

/// Decode a TIFF (Android can't). Only the first page of multi-page TIFFs.
#[uniffi::export]
pub fn decode_tiff(bytes: Vec<u8>) -> Result<RgbaImage, CoreError> {
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Tiff)
        .map_err(|e| CoreError::BadFile(format!("Can't read this TIFF: {e}")))?;
    let img = if img.width().max(img.height()) > MAX_SIDE { img.thumbnail(MAX_SIDE, MAX_SIDE) } else { img };
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
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    #[test]
    fn decodes_and_premultiplies() {
        let src = RgbaImage::from_pixel(3, 2, Rgba([200, 100, 50, 128]));
        let mut tiff = Cursor::new(Vec::new());
        src.write_to(&mut tiff, ImageFormat::Tiff).unwrap();
        let out = decode_tiff(tiff.into_inner()).unwrap();
        assert_eq!((out.width, out.height), (3, 2));
        assert_eq!(&out.pixels[..4], &[100, 50, 25, 128]);
    }

    #[test]
    fn junk_is_an_error() {
        assert!(decode_tiff(b"not a tiff".to_vec()).is_err());
    }
}
