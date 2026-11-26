use crate::text::decode_text;

/// Parse a CSV file into rows of cells.
/// Picks the delimiter (comma, semicolon or tab) that appears most in the first line;
/// European Excel saves CSV with semicolons.
#[uniffi::export]
pub fn parse_csv(bytes: Vec<u8>) -> Vec<Vec<String>> {
    // Limit: the whole table crosses the FFI at once; page it if 1M-row files show up.
    let text = decode_text(bytes);
    let first_line = text.lines().next().unwrap_or("");
    // max_by_key keeps the LAST max, so ties (incl. 0/0/0) fall back to comma.
    let delimiter = [b'\t', b';', b',']
        .into_iter()
        .max_by_key(|d| first_line.matches(*d as char).count())
        .unwrap();
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes())
        .records()
        .map(|r| r.map(|r| r.iter().map(String::from).collect()).unwrap_or_default())
        .collect()
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
}
