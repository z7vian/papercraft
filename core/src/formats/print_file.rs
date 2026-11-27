use crate::text::decode_text;

/// Read a print-to-file report (Lotus 1-2-3 / Excel .prn, space-aligned columns) as text lines, laid out the way
/// the printer would: tabs expand to 8 columns, a bare carriage return or backspace overprints the line
/// (old bold and underline), a form feed becomes a blank line between pages, trailing spaces go.
/// Columns line up only in a monospace font.
// Limit: printer escape sequences (ESC codes) drop only the ESC byte, so their argument letters show; strip them per printer language if such files show up.
#[uniffi::export]
pub fn parse_prn(bytes: Vec<u8>) -> Vec<String> {
    let text = decode_text(bytes);
    let mut lines = Vec::new();
    let mut line: Vec<char> = Vec::new();
    let mut col = 0usize;
    let mut chars = text.chars().peekable();
    let mut end_line = |line: &mut Vec<char>, col: &mut usize| {
        let len = line.iter().rposition(|c| *c != ' ').map_or(0, |p| p + 1);
        lines.push(line[..len].iter().collect::<String>());
        line.clear();
        *col = 0;
    };
    while let Some(c) = chars.next() {
        match c {
            '\n' => end_line(&mut line, &mut col),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => col = 0,
            '\u{8}' => col = col.saturating_sub(1),
            '\u{c}' => {
                if !line.is_empty() {
                    end_line(&mut line, &mut col);
                }
                end_line(&mut line, &mut col);
            }
            '\t' => col = (col / 8 + 1) * 8,
            c if c.is_control() => {}
            c => {
                if col < line.len() {
                    // Overprinting: a space never erases what is already there.
                    if c != ' ' {
                        line[col] = c;
                    }
                } else {
                    line.resize(col, ' ');
                    line.push(c);
                }
                col += 1;
            }
        }
    }
    if !line.is_empty() {
        end_line(&mut line, &mut col);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::parse_prn;

    #[test]
    fn print_file_keeps_column_layout() {
        // Fixture: US Bureau of Reclamation, Central Valley Operations delivery report (US government work,
        // public domain), via github.com/hagertynw/data-surface-water.
        let bytes = std::fs::read(format!("{}/tests/fixtures/formats/tab2201.prn", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let lines = parse_prn(bytes);
        // Leading spaces are layout: the title sits right of centre, as printed.
        assert_eq!(lines[0], format!("{}Table 22", " ".repeat(111)));
        let row = lines.iter().find(|l| l.starts_with("Arvin-Edison WSD")).unwrap();
        let total = lines.iter().find(|l| l.starts_with("Total")).unwrap();
        // Numbers stay right-aligned in their columns: January and the yearly total line up row to row.
        assert_eq!(row.find("4734").unwrap() + 4, total.find("11742").unwrap() + 5);
        assert_eq!(row.len(), total.len());
        assert!(row.ends_with("32198") && total.ends_with("716136"));
        assert_eq!(lines.last().unwrap(), "*  Data supplied by Fresno Field Division, Contracts Administration Division");
    }

    #[test]
    fn overprint_tabs_and_page_breaks() {
        let lines = parse_prn(b"Bold\rBold\r\na\tb\x0cnext\r\n__\x08\x08ok".to_vec());
        assert_eq!(lines, ["Bold", "a       b", "", "next", "ok"]);
    }
}
