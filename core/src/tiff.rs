use crate::CoreError;
use image::ImageFormat;

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
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::Tiff)
        .map_err(|e| CoreError::BadFile(format!("Can't read this TIFF: {e}")))?;
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
    use image::{ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn tiff(width: u32, height: u32, px: Rgba<u8>) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        RgbaImage::from_pixel(width, height, px).write_to(&mut out, ImageFormat::Tiff).unwrap();
        out.into_inner()
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
}
