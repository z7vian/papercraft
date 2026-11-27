//! Find in a cell grid: which cells of a sheet show a query, for the find bar to count and jump to.
//! A child of `sheet`, so it reads the parsed sheets `SheetDoc` keeps.

use super::SheetDoc;
use crate::CoreError;

/// A cell whose displayed value holds the query, 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SheetFindCell {
    pub row: u32,
    pub col: u32,
}

#[uniffi::export]
impl SheetDoc {
    /// The cells of `sheet` whose value, as Excel displays it, contains `query`, row by row, at most
    /// `limit`. Case-insensitive the way Kotlin's `indexOf(ignoreCase = true)` is, so it agrees with
    /// find in the other viewers; accent-sensitive. Cells a merge covers show nothing, so only the
    /// merge's first cell can match.
    pub fn find_cells(&self, sheet: u32, query: String, limit: u32) -> Result<Vec<SheetFindCell>, CoreError> {
        let needle: Vec<char> = query.chars().map(fold).collect();
        self.with(sheet, |book, loaded| {
            let mut found = Vec::new();
            if needle.is_empty() {
                return found;
            }
            let mut folded = Vec::new();
            for (at, cell) in loaded.sheet.iter_cells() {
                if found.len() >= limit as usize {
                    break;
                }
                let shown = book.display(cell.style.unwrap_or(0), &cell.value).text;
                folded.clear();
                folded.extend(shown.chars().map(fold));
                if !folded.windows(needle.len()).any(|window| window == needle.as_slice()) {
                    continue;
                }
                let covered = loaded.sheet.merges.iter().any(|range| {
                    range.contains(at) && (range.start.row, range.start.col) != (at.row, at.col)
                });
                if !covered {
                    found.push(SheetFindCell { row: at.row, col: at.col });
                }
            }
            found
        })
    }
}

/// One character case-folded as Kotlin's `Char.equals(ignoreCase = true)` compares: upper case, then
/// lower case, each only when it maps to one character.
fn fold(c: char) -> char {
    let upper = single(c.to_uppercase()).unwrap_or(c);
    single(upper.to_lowercase()).unwrap_or(upper)
}

fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::office::OfficeFamily;

    // Fixtures in tests/fixtures/sheets are Apache POI test-data (Apache-2.0).
    fn open(name: &str) -> Arc<SheetDoc> {
        let bytes = std::fs::read(format!("{}/tests/fixtures/sheets/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        SheetDoc::open(bytes, OfficeFamily::Spreadsheet, None).unwrap()
    }

    fn at(row: u32, col: u32) -> SheetFindCell {
        SheetFindCell { row, col }
    }

    // 47737.xlsx: A5 of the first sheet reads "Heading".
    #[test]
    fn finds_cells_by_position_ignoring_case() {
        let doc = open("47737.xlsx");
        for query in ["Heading", "HEADING", "eadin"] {
            let hits = doc.find_cells(0, query.into(), 5_000).unwrap();
            assert!(hits.contains(&at(4, 0)), "{query}: {hits:?}");
            assert!(hits.windows(2).all(|pair| (pair[0].row, pair[0].col) < (pair[1].row, pair[1].col)), "row by row: {hits:?}");
            for hit in &hits {
                let cell = doc.cells(0, hit.row, 1, hit.col, 1).unwrap();
                assert!(cell[0].text.to_lowercase().contains(&query.to_lowercase()), "{hit:?} {:?}", cell[0].text);
            }
        }
        assert!(doc.find_cells(0, "no such text anywhere".into(), 5_000).unwrap().is_empty());
        assert!(doc.find_cells(0, String::new(), 5_000).unwrap().is_empty());
        assert!(doc.find_cells(2, "Heading".into(), 5_000).is_err());
    }

    // simple-monthly-budget.xlsx: H4 shows 0.62 as "62%"; E4 anchors the merge E4:G5 and shows 2336.
    #[test]
    fn matches_displayed_text_and_merge_anchors() {
        let doc = open("simple-monthly-budget.xlsx");
        assert!(doc.find_cells(0, "62%".into(), 5_000).unwrap().contains(&at(3, 7)));
        let merged = doc.find_cells(0, "2336".into(), 5_000).unwrap();
        assert!(merged.contains(&at(3, 4)), "{merged:?}");
        assert!(!merged.iter().any(|hit| (3..=4).contains(&hit.row) && (5..=6).contains(&hit.col)), "{merged:?}");
        let all = doc.find_cells(0, "$".into(), 5_000).unwrap();
        assert!(all.len() > 2, "{all:?}");
        assert_eq!(doc.find_cells(0, "$".into(), 2).unwrap(), all[..2]);
    }

    #[test]
    fn folds_case_like_kotlin() {
        assert_eq!(fold('A'), 'a');
        assert_eq!(fold('É'), 'é');
        assert_eq!(fold('é'), 'é');
        assert_ne!(fold('é'), fold('e'));
        // 'ß' upper-cases to two letters, so it stays itself, as in Kotlin.
        assert_eq!(fold('ß'), 'ß');
    }
}
