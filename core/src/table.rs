use crate::text::decode_text;

/// Lines [`parse_csv`] reads to pick the delimiter.
const SAMPLE_LINES: usize = 20;

/// Parse a CSV file into rows of cells.
/// Picks the delimiter (comma, semicolon or tab) from the first lines; European Excel saves CSV with
/// semicolons.
#[uniffi::export]
pub fn parse_csv(bytes: Vec<u8>) -> Vec<Vec<String>> {
    // Limit: the whole table crosses the FFI at once; page it if 1M-row files show up.
    let text = decode_text(bytes);
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter(&text))
        .from_reader(text.as_bytes())
        .byte_records()
        // A flexible reader over a string has no error to report; should one come, the rows read so far stay.
        .map_while(Result::ok)
        .map(|record| record.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect())
        .collect()
}

/// The delimiter that splits the most of the first non-blank lines into the same number of fields,
/// then the one making more fields; comma when nothing decides. A title line above the table can't
/// outvote the rows under it.
fn delimiter(text: &str) -> u8 {
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).take(SAMPLE_LINES).collect();
    // max_by_key keeps the LAST max, so ties fall back to comma.
    [b'\t', b';', b',']
        .into_iter()
        .max_by_key(|&delimiter| {
            let counts: Vec<usize> = lines.iter().map(|line| unquoted(line, delimiter)).collect();
            let mut best = (0, 0);
            for &count in counts.iter().filter(|&&count| count > 0) {
                best = best.max((counts.iter().filter(|&&other| other == count).count(), count));
            }
            best
        })
        .unwrap()
}

/// How often `delimiter` appears in `line` outside double quotes.
fn unquoted(line: &str, delimiter: u8) -> usize {
    let mut quoted = false;
    line.bytes()
        .filter(|&byte| {
            if byte == b'"' {
                quoted = !quoted;
            }
            !quoted && byte == delimiter
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::parse_csv;

    #[test]
    fn comma_quotes_and_ragged_rows() {
        let rows = parse_csv(b"name,note\nAnn,\"a, b\"\nBob\n".to_vec());
        assert_eq!(rows, [vec!["name", "note"], vec!["Ann", "a, b"], vec!["Bob"]]);
    }

    #[test]
    fn semicolon_and_tab() {
        assert_eq!(parse_csv(b"a;b,c;d".to_vec()), [vec!["a", "b,c", "d"]]);
        assert_eq!(parse_csv(b"a\tb".to_vec()), [vec!["a", "b"]]);
    }

    #[test]
    fn a_title_line_does_not_pick_the_delimiter() {
        let rows = parse_csv(b"Sales; Q3 report\nregion,month,total\nNorth,\"Jan; Feb\",10\nSouth,Mar,12\n".to_vec());
        assert_eq!(rows[1], ["region", "month", "total"]);
        assert_eq!(rows[2], ["North", "Jan; Feb", "10"]);
    }
}
