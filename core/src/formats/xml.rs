use encoding_rs::Encoding;
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::text::decode_text;

/// Show an XML file as text lines: indented two spaces per level when it parses as well-formed XML,
/// the file's own lines otherwise. Markup and entities stay as written.
#[uniffi::export]
pub fn format_xml(bytes: Vec<u8>) -> Vec<String> {
    let text = decode_xml(bytes);
    pretty(&text).unwrap_or_else(|| text.lines().map(str::to_owned).collect())
}

/// BOM first, then the `encoding="…"` of the XML declaration, then the plain-text guess.
fn decode_xml(bytes: Vec<u8>) -> String {
    if Encoding::for_bom(&bytes).is_none()
        && let Some(enc) = declared_encoding(&bytes)
    {
        return enc.decode_without_bom_handling(&bytes).0.into_owned();
    }
    decode_text(bytes)
}

fn declared_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(200)];
    if !head.starts_with(b"<?xml") {
        return None;
    }
    let decl = &head[..head.iter().position(|&b| b == b'>')?];
    let at = decl.windows(8).position(|w| w == b"encoding")? + 8;
    let rest = &decl[at..];
    let open = rest.iter().position(|&b| b == b'"' || b == b'\'')?;
    let len = rest[open + 1..].iter().position(|&b| b == rest[open])?;
    Encoding::for_label(&rest[open + 1..open + 1 + len])
}

enum Tok {
    Open(String),
    Close(String),
    /// A self-contained line: empty element, comment, declaration, processing instruction, doctype.
    Line(String),
    Text(String),
}

/// None when the text isn't well-formed XML.
fn pretty(xml: &str) -> Option<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    let mut toks = Vec::new();
    let mut text = String::new();
    let (mut depth, mut elements) = (0usize, 0usize);
    loop {
        let event = reader.read_event().ok()?;
        // quick-xml hands back byte slices of the input; they split it only at ASCII markup, so they stay valid UTF-8.
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        // Text, entity references and CDATA in a row are one run of character data.
        match &event {
            Event::Text(t) => {
                text.push_str(&s(t));
                continue;
            }
            Event::GeneralRef(r) => {
                text.push('&');
                text.push_str(&s(r));
                text.push(';');
                continue;
            }
            Event::CData(c) => {
                text.push_str("<![CDATA[");
                text.push_str(&s(c));
                text.push_str("]]>");
                continue;
            }
            _ => {}
        }
        if !text.trim().is_empty() {
            toks.push(Tok::Text(std::mem::take(&mut text)));
        }
        text.clear();
        match event {
            Event::Start(e) => {
                depth += 1;
                elements += 1;
                toks.push(Tok::Open(format!("<{}>", s(&e))));
            }
            Event::End(e) => {
                depth = depth.checked_sub(1)?;
                toks.push(Tok::Close(format!("</{}>", s(&e))));
            }
            Event::Empty(e) => {
                elements += 1;
                toks.push(Tok::Line(format!("<{}/>", s(&e))));
            }
            Event::Comment(c) => toks.push(Tok::Line(format!("<!--{}-->", s(&c)))),
            Event::Decl(d) => toks.push(Tok::Line(format!("<?{}?>", s(&d)))),
            Event::PI(p) => toks.push(Tok::Line(format!("<?{}?>", s(&p)))),
            Event::DocType(d) => toks.push(Tok::Line(format!("<!DOCTYPE {}>", s(&d).trim()))),
            Event::Eof => break,
            // Character data `continue`d in the match above.
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {}
        }
    }
    // Unclosed elements, or no markup at all (plain text saved as .xml): show the file as it is.
    if depth != 0 || elements == 0 {
        return None;
    }

    let mut lines = Vec::new();
    // Markup spanning lines (comments, long start tags) keeps its own layout after the indented first line.
    let put = |lines: &mut Vec<String>, level: usize, s: &str| {
        let mut parts = s.lines();
        lines.push(format!("{}{}", "  ".repeat(level), parts.next().unwrap_or("")));
        lines.extend(parts.map(str::to_owned));
    };
    let mut level = 0usize;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Open(open) => match (toks.get(i + 1), toks.get(i + 2)) {
                // <a>short text</a> and <a></a> stay on one line.
                (Some(Tok::Text(t)), Some(Tok::Close(close))) if !t.trim().contains('\n') => {
                    put(&mut lines, level, &format!("{open}{}{close}", t.trim()));
                    i += 2;
                }
                (Some(Tok::Close(close)), _) => {
                    put(&mut lines, level, &format!("{open}{close}"));
                    i += 1;
                }
                _ => {
                    put(&mut lines, level, open);
                    level += 1;
                }
            },
            Tok::Close(close) => {
                level = level.saturating_sub(1);
                put(&mut lines, level, close);
            }
            Tok::Line(line) => put(&mut lines, level, line),
            Tok::Text(t) => {
                for l in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    put(&mut lines, level, l);
                }
            }
        }
        i += 1;
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::format_xml;

    #[test]
    fn indents_well_formed_xml() {
        // Fixture: Apache Tika test-documents testXML.xml (Apache-2.0).
        let bytes = std::fs::read(format!("{}/tests/fixtures/formats/testXML.xml", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let lines = format_xml(bytes);
        assert_eq!(lines[0], r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        assert_eq!(lines[1], "<!--");
        assert!(lines.iter().any(|l| l == "  <dc:title>Tika test document</dc:title>"), "{lines:#?}");
        assert!(lines.iter().any(|l| l == "  <dc:rights>Archimède et Lius à Châteauneuf testing chars en été</dc:rights>"), "{lines:#?}");
        assert_eq!(lines.last().unwrap(), "</oaidc:dc>");
    }

    #[test]
    fn keeps_entities_and_mixed_content() {
        let lines = format_xml(b"<a><b>x &amp; y</b><c/>tail<d>1\n2</d></a>".to_vec());
        assert_eq!(lines, ["<a>", "  <b>x &amp; y</b>", "  <c/>", "  tail", "  <d>", "    1", "    2", "  </d>", "</a>"]);
    }

    #[test]
    fn broken_xml_is_shown_as_is() {
        assert_eq!(format_xml(b"<a>\n<b></a>".to_vec()), ["<a>", "<b></a>"]);
        assert_eq!(format_xml(b"just text".to_vec()), ["just text"]);
    }

    #[test]
    fn declared_legacy_encoding() {
        let mut bytes = br#"<?xml version="1.0" encoding="windows-1251"?><p>"#.to_vec();
        bytes.extend_from_slice(b"\xcf\xf0\xe8\xe2\xe5\xf2</p>");
        assert_eq!(format_xml(bytes)[1], "<p>Привет</p>");
    }
}
