use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8};

/// Turn a text file's bytes into a string.
/// Handles BOMs (UTF-8/UTF-16), plain UTF-8, and legacy encodings like GBK or Windows-1252.
#[uniffi::export]
pub fn decode_text(bytes: Vec<u8>) -> String {
    let encoding = match Encoding::for_bom(&bytes) {
        Some((enc, _)) => enc,
        None if std::str::from_utf8(&bytes).is_ok() => UTF_8,
        None => {
            let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
            detector.feed(&bytes, true);
            detector.guess(None, Utf8Detection::Allow)
        }
    };
    // decode() strips the BOM and replaces bad bytes with U+FFFD instead of failing.
    encoding.decode(&bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::decode_text;
    use encoding_rs::{GBK, WINDOWS_1252};

    #[test]
    fn utf8_with_and_without_bom() {
        assert_eq!(decode_text("héllo".as_bytes().to_vec()), "héllo");
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi".to_vec()), "hi");
    }

    #[test]
    fn utf16le_bom() {
        assert_eq!(decode_text(vec![0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
    }

    #[test]
    fn legacy_encodings() {
        for (enc, text) in [
            (GBK, "这是一个测试文件，用来检查中文编码是否正确。"),
            (WINDOWS_1252, "Café crème brûlée, naïve façade, déjà vu à la française."),
        ] {
            assert_eq!(decode_text(enc.encode(text).0.into_owned()), text, "{}", enc.name());
        }
    }
}
