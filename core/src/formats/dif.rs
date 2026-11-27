use crate::CoreError;
use crate::text::decode_text;

/// Parse a DIF (Data Interchange Format, VisiCalc/Lotus/Excel) file into rows of cells.
/// Numbers are shown in their shortest form, logical values as TRUE/FALSE, missing values as #N/A.
#[uniffi::export]
pub fn parse_dif(bytes: Vec<u8>) -> Result<Vec<Vec<String>>, CoreError> {
    // Limit: the whole table crosses the FFI at once; page it if 1M-row files show up.
    let text = decode_text(bytes);
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| l.trim().eq_ignore_ascii_case("TABLE")) {
        return Err(CoreError::BadFile("This isn't a DIF file.".into()));
    }
    // Header: (topic, "vector,value", "string") triples, up to the DATA topic. The first one, TABLE, is read.
    lines.next();
    lines.next();
    loop {
        let topic = lines.next().ok_or_else(|| CoreError::BadFile("This DIF file has no data section.".into()))?;
        lines.next();
        lines.next();
        if topic.trim().eq_ignore_ascii_case("DATA") {
            break;
        }
    }
    // Data: ("type,number", "string") pairs; type -1 is BOT (begin row) or EOD, 0 a number, 1 a string.
    let mut rows: Vec<Vec<String>> = Vec::new();
    while let Some(head) = lines.next() {
        let head = head.trim();
        if head.eq_ignore_ascii_case("EOD") {
            break;
        }
        let string = lines.next().unwrap_or("").trim();
        let Some((kind, number)) = head.split_once(',') else { continue };
        let cell = match kind.trim() {
            "-1" => {
                if string.eq_ignore_ascii_case("BOT") {
                    rows.push(Vec::new());
                } else if string.eq_ignore_ascii_case("EOD") {
                    break;
                }
                continue;
            }
            "0" => match string.to_ascii_uppercase().as_str() {
                "V" => number.trim().parse::<f64>().map_or_else(|_| number.trim().to_owned(), |n| n.to_string()),
                "TRUE" => "TRUE".into(),
                "FALSE" => "FALSE".into(),
                "NA" => "#N/A".into(),
                "ERROR" => "#VALUE!".into(),
                _ => String::new(),
            },
            "1" => unquote(string),
            _ => continue,
        };
        match rows.last_mut() {
            Some(row) => row.push(cell),
            None => rows.push(vec![cell]),
        }
    }
    Ok(rows)
}

/// `"a ""b"""` → `a "b"`; unquoted strings stay as they are.
fn unquote(s: &str) -> String {
    match s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_dif;

    // Fixtures: LibreOffice sc/qa/unit/data/dif (MPL-2.0).
    fn fixture(name: &str) -> Vec<Vec<String>> {
        let path = format!("{}/tests/fixtures/formats/{name}", env!("CARGO_MANIFEST_DIR"));
        parse_dif(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn strings_numbers_and_blanks() {
        let blank = |n: usize| vec![""; n];
        let rows = fixture("universal-content.dif");
        assert_eq!(rows, [
            [vec!["1", "String1", "6"], blank(5)].concat(),
            [vec!["2", "String2", "5"], blank(5)].concat(),
            [vec!["", "", "-1"], blank(5)].concat(),
            [vec!["", "", "11"], blank(5)].concat(),
        ]);
    }

    #[test]
    fn excel_row_with_stray_end_marker() {
        // Excel file with an empty special cell before EOD (LibreOffice bug 150599); stored with LF line ends here.
        let rows = fixture("tdf150599.dif");
        assert_eq!(rows, [(1..=32).map(|n| n.to_string()).collect::<Vec<_>>()]);
    }

    #[test]
    fn logicals_quotes_and_bad_files() {
        let dif = "TABLE\n0,1\n\"\"\nDATA\n0,0\n\"\"\n-1,0\nBOT\n0,1\nTRUE\n0,0\nNA\n1,0\n\"say \"\"hi\"\"\"\n0,2.50\nV\n-1,0\nEOD\n";
        let expected = [["TRUE", "#N/A", "say \"hi\"", "2.5"]];
        assert_eq!(parse_dif(dif.as_bytes().to_vec()).unwrap(), expected);
        // Excel writes CRLF line ends.
        assert_eq!(parse_dif(dif.replace('\n', "\r\n").into_bytes()).unwrap(), expected);
        assert!(parse_dif(b"name,value\n".to_vec()).is_err());
    }
}
