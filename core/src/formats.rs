//! Smaller document and table formats: RTF, XML, PRN print files, dBASE and DIF.
//! Each returns the shapes the text viewer (lines) and table viewer (rows of cells) take.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use oem_cp::code_table::DECODING_TABLE_CP_MAP;

mod dbase;
mod dif;
mod print_file;
mod rtf;
mod xml;

/// A Windows or DOS code page: encoding_rs covers the Windows, ISO and CJK pages, oem_cp the DOS ones (437, 850, …).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Charset {
    Web(&'static Encoding),
    Oem(u16),
}

impl Charset {
    fn from_codepage(cp: u16) -> Option<Self> {
        use encoding_rs::*;
        let web = match cp {
            65001 => UTF_8,
            1200 => UTF_16LE,
            1201 => UTF_16BE,
            874 => WINDOWS_874,
            932 => SHIFT_JIS,
            936 => GBK,
            949 => EUC_KR,
            950 => BIG5,
            1250 => WINDOWS_1250,
            1251 => WINDOWS_1251,
            1252 => WINDOWS_1252,
            1253 => WINDOWS_1253,
            1254 => WINDOWS_1254,
            1255 => WINDOWS_1255,
            1256 => WINDOWS_1256,
            1257 => WINDOWS_1257,
            1258 => WINDOWS_1258,
            866 => IBM866,
            10000 => MACINTOSH,
            10007 => X_MAC_CYRILLIC,
            20866 => KOI8_R,
            21866 => KOI8_U,
            20932 | 51932 => EUC_JP,
            54936 => GB18030,
            // Latin-1 and Latin-5 are read as their Windows supersets, like browsers do.
            28591 => WINDOWS_1252,
            28599 => WINDOWS_1254,
            28592 => ISO_8859_2,
            28593 => ISO_8859_3,
            28594 => ISO_8859_4,
            28595 => ISO_8859_5,
            28596 => ISO_8859_6,
            28597 => ISO_8859_7,
            28598 => ISO_8859_8,
            28603 => ISO_8859_13,
            28605 => ISO_8859_15,
            _ => return DECODING_TABLE_CP_MAP.get(&cp).map(|_| Charset::Oem(cp)),
        };
        Some(Charset::Web(web))
    }

    /// Best guess for unlabelled bytes: UTF-8 when they are valid UTF-8, else chardetng's pick.
    fn guess(sample: &[u8]) -> Self {
        if std::str::from_utf8(sample).is_ok() {
            return Charset::Web(encoding_rs::UTF_8);
        }
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
        detector.feed(sample, true);
        Charset::Web(detector.guess(None, Utf8Detection::Allow))
    }

    /// Appends `bytes` decoded to `out`; bytes the code page doesn't define become U+FFFD.
    fn decode_into(self, bytes: &[u8], out: &mut String) {
        match self {
            Charset::Web(enc) => out.push_str(&enc.decode_without_bom_handling(bytes).0),
            Charset::Oem(cp) => out.push_str(&DECODING_TABLE_CP_MAP[&cp].decode_string_lossy(bytes)),
        }
    }

    fn decode(self, bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len());
        self.decode_into(bytes, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Charset;

    #[test]
    fn windows_and_dos_code_pages() {
        assert_eq!(Charset::from_codepage(1251).unwrap().decode(b"\xcf\xf0\xe8\xe2\xe5\xf2"), "Привет");
        // 0x82 0x90 0xE1 is "éÉß" in DOS Latin-1 (850), a page encoding_rs lacks.
        assert_eq!(Charset::from_codepage(850).unwrap().decode(b"\x82\x90\xe1"), "éÉß");
        assert_eq!(Charset::from_codepage(437).unwrap().decode(b"1\xf6\xab=2"), "1÷½=2");
        assert!(Charset::from_codepage(12345).is_none());
    }
}
