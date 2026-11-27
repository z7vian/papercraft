use std::collections::HashMap;

use super::Charset;
use crate::CoreError;

/// Read an RTF file's text as paragraphs, one line each.
/// Decodes `\'hh` bytes in the code page of the current font (or `\ansicpg`) and `\uN` Unicode escapes;
/// skips font tables, styles, pictures, headers and other non-body groups.
/// Table cells are joined with tabs, one table row per line.
// Limit: character styling (bold, size, colour) is dropped; return styled runs once the text viewer can show them.
#[uniffi::export]
pub fn parse_rtf(bytes: Vec<u8>) -> Result<Vec<String>, CoreError> {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    if !bytes[start..].starts_with(b"{\\rtf") {
        return Err(CoreError::BadFile("This isn't an RTF file.".into()));
    }
    let mut parser = Parser::default();
    parser.run(&bytes[start..]);
    Ok(parser.finish())
}

#[derive(Clone, Copy, PartialEq)]
enum Dest {
    Body,
    FontTable,
    Skip,
}

#[derive(Clone, Copy)]
struct Group {
    dest: Dest,
    /// Bytes standing in for each `\uN` in readers without Unicode (`\ucN`).
    uc: usize,
    charset: Charset,
}

struct Parser {
    stack: Vec<Group>,
    group: Group,
    /// Document code page (`\ansicpg`), used by fonts with the default charset.
    ansi: Charset,
    default_font: Option<i32>,
    fonts: HashMap<i32, Charset>,
    /// Font being defined inside the font table.
    table_font: Option<i32>,
    /// Text bytes not decoded yet: double-byte code pages split one character over two `\'hh`.
    bytes: Vec<u8>,
    bytes_charset: Charset,
    /// UTF-16 units from `\uN`, decoded together so surrogate pairs join.
    units: Vec<u16>,
    /// Fallback characters still to drop after a `\uN`.
    skip: usize,
    line: String,
    lines: Vec<String>,
}

impl Default for Parser {
    fn default() -> Self {
        let ansi = Charset::Web(encoding_rs::WINDOWS_1252);
        Parser {
            stack: Vec::new(),
            group: Group { dest: Dest::Body, uc: 1, charset: ansi },
            ansi,
            default_font: None,
            fonts: HashMap::new(),
            table_font: None,
            bytes: Vec::new(),
            bytes_charset: ansi,
            units: Vec::new(),
            skip: 0,
            line: String::new(),
            lines: Vec::new(),
        }
    }
}

/// Non-starred groups whose text isn't part of the body.
const SKIPPED: &[&str] = &[
    "colortbl", "stylesheet", "info", "pict", "object", "header", "headerl", "headerr", "headerf", "footer",
    "footerl", "footerr", "footerf", "footnote", "fldinst", "listtable", "listoverridetable", "revtbl", "rsidtbl",
    "xmlnstbl", "filetbl", "themedata", "colorschememapping", "datastore", "latentstyles", "generator", "pgdsctbl",
    "ftnsep", "ftnsepc", "ftncn", "aftnsep", "aftnsepc", "aftncn", "txe", "xe", "tc", "nonshppict", "private",
];

impl Parser {
    fn run(&mut self, rtf: &[u8]) {
        let mut i = 0;
        while i < rtf.len() {
            let b = rtf[i];
            i += 1;
            match b {
                b'{' => {
                    self.flush();
                    self.skip = 0;
                    self.stack.push(self.group);
                }
                b'}' => {
                    self.flush();
                    self.skip = 0;
                    let closed = self.group;
                    if let Some(parent) = self.stack.pop() {
                        self.group = parent;
                    }
                    // Body text that sets no font uses \deff, known once the font table is read.
                    if closed.dest == Dest::FontTable && self.group.dest == Dest::Body {
                        self.group.charset = self.font_charset(self.default_font);
                    }
                }
                b'\\' => i = self.control(rtf, i),
                b'\r' | b'\n' => {}
                _ => self.text_byte(b),
            }
        }
    }

    /// Handles the control after a backslash at `rtf[i - 1]`; returns the index after it.
    fn control(&mut self, rtf: &[u8], mut i: usize) -> usize {
        let Some(&c) = rtf.get(i) else { return i };
        if !c.is_ascii_alphabetic() {
            i += 1;
            match c {
                b'\'' => {
                    let hex = rtf.get(i..i + 2).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
                    if let Some(byte) = hex {
                        self.text_byte(byte);
                        return i + 2;
                    }
                }
                b'*' => {
                    self.flush();
                    self.group.dest = Dest::Skip;
                }
                b'\\' | b'{' | b'}' => self.text_byte(c),
                _ if self.take_skip() => {}
                b'~' => self.push_char('\u{a0}'),
                b'_' => self.push_char('\u{2011}'),
                b'\r' | b'\n' => self.end_line(),
                _ => {} // \- optional hyphen, \: index subentry, \| formula
            }
            return i;
        }
        let word_start = i;
        while i < rtf.len() && rtf[i].is_ascii_alphabetic() {
            i += 1;
        }
        let word = std::str::from_utf8(&rtf[word_start..i]).unwrap_or("");
        let num_start = i;
        if rtf.get(i) == Some(&b'-') {
            i += 1;
        }
        while i < rtf.len() && rtf[i].is_ascii_digit() {
            i += 1;
        }
        let param = std::str::from_utf8(&rtf[num_start..i]).ok().and_then(|n| n.parse::<i64>().ok());
        if rtf.get(i) == Some(&b' ') {
            i += 1;
        }
        if word == "bin" {
            // Raw binary data follows; never text.
            self.flush();
            return i.saturating_add(param.unwrap_or(0).max(0) as usize).min(rtf.len());
        }
        self.word(word, param);
        i
    }

    fn word(&mut self, word: &str, param: Option<i64>) {
        let n = param.unwrap_or(0);
        // Font table entries: \fN starts one, \fcharsetN / \cpgN give its code page.
        if self.group.dest == Dest::FontTable {
            match word {
                "f" => self.table_font = Some(n as i32),
                "fcharset" => {
                    if let (Some(font), Some(cs)) = (self.table_font, charset_of(n)) {
                        self.fonts.insert(font, cs);
                    }
                }
                "cpg" => {
                    if let (Some(font), Some(cs)) = (self.table_font, Charset::from_codepage(n as u16)) {
                        self.fonts.insert(font, cs);
                    }
                }
                _ => {}
            }
            return;
        }
        match word {
            "fonttbl" => {
                self.flush();
                self.group.dest = Dest::FontTable;
                return;
            }
            "ansicpg" => {
                if let Some(cs) = Charset::from_codepage(n as u16) {
                    self.set_ansi(cs);
                }
                return;
            }
            "ansi" => return self.set_ansi(Charset::Web(encoding_rs::WINDOWS_1252)),
            "mac" => return self.set_ansi(Charset::Web(encoding_rs::MACINTOSH)),
            "pc" => return self.set_ansi(Charset::Oem(437)),
            "pca" => return self.set_ansi(Charset::Oem(850)),
            "deff" => {
                self.default_font = Some(n as i32);
                return;
            }
            "uc" => {
                self.group.uc = n.max(0) as usize;
                return;
            }
            _ if SKIPPED.contains(&word) => {
                self.flush();
                self.group.dest = Dest::Skip;
                return;
            }
            _ => {}
        }
        if self.group.dest == Dest::Skip || self.take_skip() {
            return;
        }
        match word {
            "u" => {
                self.flush_bytes();
                // \uN is a signed 16-bit value: \u-10179 is U+D83D.
                self.units.push(n as i16 as u16);
                self.skip = self.group.uc;
            }
            "f" => self.group.charset = self.font_charset(Some(n as i32)),
            "par" | "line" | "sect" | "page" | "row" => self.end_line(),
            "tab" | "cell" | "nestcell" => self.push_char('\t'),
            "emdash" => self.push_char('—'),
            "endash" => self.push_char('–'),
            "lquote" => self.push_char('‘'),
            "rquote" => self.push_char('’'),
            "ldblquote" => self.push_char('“'),
            "rdblquote" => self.push_char('”'),
            "bullet" => self.push_char('•'),
            "emspace" | "enspace" | "qmspace" => self.push_char(' '),
            _ => {}
        }
    }

    fn set_ansi(&mut self, cs: Charset) {
        self.flush();
        self.ansi = cs;
        self.group.charset = cs;
    }

    fn font_charset(&self, font: Option<i32>) -> Charset {
        font.and_then(|f| self.fonts.get(&f).copied()).unwrap_or(self.ansi)
    }

    /// Drops one fallback character after a `\uN`; true if it did.
    fn take_skip(&mut self) -> bool {
        let skipping = self.skip > 0;
        if skipping {
            self.skip -= 1;
        }
        skipping
    }

    fn text_byte(&mut self, b: u8) {
        if self.group.dest != Dest::Body || self.take_skip() {
            return;
        }
        self.flush_units();
        if self.bytes_charset != self.group.charset {
            self.flush_bytes();
            self.bytes_charset = self.group.charset;
        }
        self.bytes.push(b);
    }

    fn push_char(&mut self, c: char) {
        if self.group.dest != Dest::Body {
            return;
        }
        self.flush();
        self.line.push(c);
    }

    fn end_line(&mut self) {
        if self.group.dest != Dest::Body {
            return;
        }
        self.flush();
        // A table row ends with \cell then \row; drop the tab the last cell left.
        let line = std::mem::take(&mut self.line);
        self.lines.push(line.strip_suffix('\t').map(str::to_owned).unwrap_or(line));
    }

    fn flush(&mut self) {
        self.flush_bytes();
        self.flush_units();
    }

    fn flush_bytes(&mut self) {
        if !self.bytes.is_empty() {
            self.bytes_charset.decode_into(&self.bytes, &mut self.line);
            self.bytes.clear();
        }
    }

    fn flush_units(&mut self) {
        if !self.units.is_empty() {
            self.line.extend(char::decode_utf16(self.units.drain(..)).map(|c| c.unwrap_or('\u{fffd}')));
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.flush();
        if !self.line.is_empty() {
            self.lines.push(self.line);
        }
        self.lines
    }
}

/// Code page of an RTF `\fcharsetN`; None means the document's `\ansicpg`.
fn charset_of(fcharset: i64) -> Option<Charset> {
    let cp = match fcharset {
        77 => 10000,
        128 => 932,
        129 | 130 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        254 | 255 => 437,
        // 0 ANSI, 1 default, 2 symbol: the document code page.
        _ => return None,
    };
    Charset::from_codepage(cp)
}

#[cfg(test)]
mod tests {
    use super::parse_rtf;

    // Fixtures: Apache Tika test-documents (Apache-2.0).
    fn fixture(name: &str) -> Vec<String> {
        let path = format!("{}/tests/fixtures/formats/{name}", env!("CARGO_MANIFEST_DIR"));
        parse_rtf(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn word_document_skips_tables_and_info() {
        // Word 2003 file whose font table, styles and \info title come before the body.
        assert_eq!(fixture("testRTF.rtf"), ["Test d’indexation Word", ""]);
    }

    #[test]
    fn unicode_escapes_join_surrogate_pairs() {
        // Six Gothic letters, each a \uN surrogate pair with a '?' fallback.
        assert_eq!(fixture("testRTFUnicodeGothic.rtf"), ["𐌲𐌿𐍄𐌹𐍃𐌺"]);
    }

    #[test]
    fn hex_bytes_use_the_document_code_page() {
        // \ansicpg1250 (Polish).
        assert_eq!(fixture("testRTFWindowsCodepage1250.rtf"), ["zażółć gęślą jaźń", "ZAŻÓŁĆ GĘŚLĄ JAŹŃ", ""]);
    }

    #[test]
    fn hex_bytes_use_the_font_charset() {
        // \ansicpg1252 but the body font is \fcharset238 (Central European).
        let lines = fixture("testRTFWordPadCzechCharacters.rtf");
        assert_eq!(lines[0], "Článek týdne");
        assert_eq!(lines[1], "Část svitku s textem Knihy Izajáš");
        assert!(lines[2].contains("Mrtvého moře") && lines[2].contains("2. stol. př.\u{2009}n.\u{2009}l."), "{}", lines[2]);
    }

    #[test]
    fn double_byte_shift_jis() {
        // \ansicpg932: each Japanese character is two \'hh bytes.
        assert_eq!(fixture("testRTF-ms932.rtf"), ["Hello", "こんにちは", "Test", "テスト", "", ""]);
    }

    #[test]
    fn table_cells_and_rejects_non_rtf() {
        let rtf = br"{\rtf1\ansi {\trowd a\cell b\cell\row}\pard after\par}".to_vec();
        assert_eq!(parse_rtf(rtf).unwrap(), ["a\tb", "after"]);
        assert!(parse_rtf(b"plain text".to_vec()).is_err());
    }
}
