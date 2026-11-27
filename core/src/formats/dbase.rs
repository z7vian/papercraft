use super::Charset;
use crate::CoreError;

/// Parse a dBASE table (.dbf: dBASE III–7, FoxPro, Visual FoxPro, Clipper) into rows of cells, field names first.
/// Text uses the code page in the header's language driver byte, or a guess when there is none.
/// Deleted records are left out; dates read YYYY-MM-DD, logicals TRUE/FALSE, nulls empty.
// Limit: memo fields (M, G, P, dBASE B) live in a side .dbt/.fpt file the viewer never receives, so they show empty;
// read the side file too once files open with folder access.
#[uniffi::export]
pub fn parse_dbf(bytes: Vec<u8>) -> Result<Vec<Vec<String>>, CoreError> {
    // Limit: the whole table crosses the FFI at once; page it if 1M-row files show up.
    let damaged = || CoreError::BadFile("This dBASE file is damaged.".into());
    if bytes.len() < 33 {
        return Err(CoreError::BadFile("This isn't a dBASE file.".into()));
    }
    let version = bytes[0];
    let dbase7 = version & 0x07 == 4;
    let visual_foxpro = matches!(version, 0x30..=0x32);
    let records = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let record_len = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
    if record_len == 0 || header_len > bytes.len() {
        return Err(damaged());
    }

    // Field descriptors: 32 bytes each from offset 32, or 48 bytes from 68 in dBASE 7; 0x0D ends the list.
    let (first, size) = if dbase7 { (68, 48) } else { (32, 32) };
    let mut fields = Vec::new();
    let mut offset = 1; // byte 0 of a record is the deleted flag
    let mut flag_bits = 0;
    let mut pos = first;
    while pos + size <= header_len && bytes[pos] != 0x0D {
        let d = &bytes[pos..pos + size];
        let (name, kind, len, decimals, flags) =
            if dbase7 { (&d[..32], d[32], d[33] as usize, d[34], 0) } else { (&d[..11], d[11], d[16] as usize, d[17], d[18]) };
        // Clipper and FoxPro store character fields over 255 bytes with the high byte in the decimals slot.
        let len = if kind == b'C' && !dbase7 { len | (decimals as usize) << 8 } else { len };
        // Visual FoxPro gives each varchar/varbinary field, then each nullable field, a bit in _NullFlags.
        let mut bit = |on: bool| {
            on.then(|| {
                flag_bits += 1;
                flag_bits - 1
            })
        };
        let var_bit = bit(visual_foxpro && matches!(kind, b'V' | b'Q'));
        let null_bit = bit(visual_foxpro && flags & 0x02 != 0);
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        fields.push(Field { name: name.to_vec(), kind, offset, len, decimals, var_bit, null_bit });
        offset += len;
        pos += size;
    }
    if fields.is_empty() || offset > record_len {
        return Err(damaged());
    }
    let count = records.min((bytes.len() - header_len) / record_len);
    let record = |i: usize| &bytes[header_len + i * record_len..][..record_len];
    let null_flags = fields.iter().find(|f| f.kind == b'0').map(|f| (f.offset, f.len));

    let charset = code_page(bytes[29]).and_then(Charset::from_codepage).unwrap_or_else(|| {
        // No language driver: guess from the text fields of the first records.
        let mut sample = Vec::new();
        for i in 0..count {
            for f in fields.iter().filter(|f| matches!(f.kind, b'C' | b'V')) {
                sample.extend_from_slice(&record(i)[f.offset..][..f.len]);
            }
            if sample.len() > 64 * 1024 {
                break;
            }
        }
        Charset::guess(&sample)
    });

    let shown: Vec<&Field> = fields.iter().filter(|f| f.kind != b'0').collect();
    let mut rows = Vec::with_capacity(count + 1);
    rows.push(shown.iter().map(|f| charset.decode(&f.name).trim().to_owned()).collect());
    for i in 0..count {
        let rec = record(i);
        match rec[0] {
            b'*' => continue,
            0x1A => break, // end-of-file marker before the counted records end
            _ => {}
        }
        let flags = null_flags.map_or(&[][..], |(at, len)| &rec[at..at + len]);
        let flag = |bit: Option<usize>| bit.is_some_and(|b| flags.get(b / 8).is_some_and(|byte| byte >> (b % 8) & 1 == 1));
        rows.push(
            shown
                .iter()
                .map(|f| {
                    let raw = &rec[f.offset..f.offset + f.len];
                    if flag(f.null_bit) {
                        return String::new();
                    }
                    // A set varchar bit means the value is shorter than the field; its length is the last byte.
                    let raw = if flag(f.var_bit) { &raw[..(*raw.last().unwrap_or(&0) as usize).min(raw.len())] } else { raw };
                    f.value(raw, charset, dbase7, visual_foxpro)
                })
                .collect(),
        );
    }
    Ok(rows)
}

struct Field {
    name: Vec<u8>,
    kind: u8,
    offset: usize,
    len: usize,
    decimals: u8,
    var_bit: Option<usize>,
    null_bit: Option<usize>,
}

impl Field {
    fn value(&self, raw: &[u8], charset: Charset, dbase7: bool, visual_foxpro: bool) -> String {
        let ascii = || String::from_utf8_lossy(raw).trim_matches(['\0', ' ']).to_owned();
        match (self.kind, raw.len()) {
            (b'C' | b'V', _) => {
                let end = raw.iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |p| p + 1);
                charset.decode(&raw[..end])
            }
            (b'N' | b'F', _) => ascii(),
            (b'L', _) => match raw.first() {
                Some(b'T' | b't' | b'Y' | b'y') => "TRUE".into(),
                Some(b'F' | b'f' | b'N' | b'n') => "FALSE".into(),
                _ => String::new(), // '?' or blank: not set
            },
            (b'D', 8) => match ascii() {
                d if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) && d != "00000000" => {
                    format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..])
                }
                d => d,
            },
            // dBASE 7 stores integers big-endian with the sign bit flipped so they sort bytewise.
            (b'I' | b'+', 4) if dbase7 => (u32::from_be_bytes(raw.try_into().unwrap()) ^ 0x8000_0000).cast_signed().to_string(),
            (b'I' | b'+', 4) => i32::from_le_bytes(raw.try_into().unwrap()).to_string(),
            (b'O', 8) => {
                let bits = u64::from_be_bytes(raw.try_into().unwrap());
                let bits = if bits & 1 << 63 != 0 { bits ^ 1 << 63 } else { !bits };
                f64::from_bits(bits).to_string()
            }
            (b'B', 8) if visual_foxpro => {
                format!("{:.*}", self.decimals as usize, f64::from_le_bytes(raw.try_into().unwrap()))
            }
            (b'Y', 8) => {
                // Currency: a 64-bit integer of ten-thousandths.
                let v = i64::from_le_bytes(raw.try_into().unwrap());
                let sign = if v < 0 { "-" } else { "" };
                format!("{sign}{}.{:04}", v.unsigned_abs() / 10_000, v.unsigned_abs() % 10_000)
            }
            (b'T', 8) => {
                let day = i32::from_le_bytes(raw[..4].try_into().unwrap());
                let ms = i32::from_le_bytes(raw[4..].try_into().unwrap()).max(0) / 1000;
                if day <= 0 {
                    return String::new();
                }
                let (y, m, d) = from_julian_day(day.into());
                format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", ms / 3600, ms / 60 % 60, ms % 60)
            }
            // Limit: dBASE 7 timestamps (@) show empty: no sample file confirms their byte layout; decode once one turns up.
            // Memo pointers (M, G, P, dBASE B) and binary fields (Q, W) have no text in the table itself.
            (b'@' | b'M' | b'G' | b'P' | b'B' | b'Q' | b'W', _) => String::new(),
            _ => charset.decode(raw).trim_matches(['\0', ' ']).to_owned(),
        }
    }
}

/// Gregorian (year, month, day) of a Julian day number (Fliegel & Van Flandern).
fn from_julian_day(jd: i64) -> (i64, i64, i64) {
    let l = jd + 68569;
    let n = 4 * l / 146_097;
    let l = l - (146_097 * n + 3) / 4;
    let i = 4000 * (l + 1) / 1_461_001;
    let l = l - 1461 * i / 4 + 31;
    let j = 80 * l / 2447;
    let day = l - 2447 * j / 80;
    let l = j / 11;
    (100 * (n - 49) + i + l, j + 2 - 12 * l, day)
}

/// Windows/DOS code page of a dBASE language driver ID (header byte 29); None when unset or unknown.
fn code_page(ldid: u8) -> Option<u16> {
    Some(match ldid {
        0x01 | 0x09 | 0x0B | 0x0D | 0x0F | 0x11 | 0x15 | 0x18 | 0x19 | 0x1B => 437,
        0x02 | 0x0A | 0x0E | 0x10 | 0x12 | 0x14 | 0x16 | 0x1A | 0x1D | 0x25 | 0x37 => 850,
        0x03 | 0x57 | 0x58 | 0x59 => 1252,
        0x04 => 10000,
        0x08 | 0x17 | 0x66 => 865,
        0x13 | 0x7B => 932,
        0x1C | 0x6C => 863,
        0x1F | 0x22 | 0x23 | 0x40 | 0x64 | 0x87 => 852,
        0x24 => 860,
        0x26 | 0x65 => 866,
        0x4D | 0x7A => 936,
        0x4E | 0x79 => 949,
        0x4F | 0x78 => 950,
        0x50 | 0x7C => 874,
        0x67 => 861,
        0x6A | 0x86 => 737,
        0x6B | 0x88 => 857,
        0x7D => 1255,
        0x7E => 1256,
        0x96 => 10007,
        0xC8 => 1250,
        0xC9 => 1251,
        0xCA => 1254,
        0xCB => 1253,
        0xCC => 1257,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{from_julian_day, parse_dbf};

    fn fixture(name: &str) -> Vec<Vec<String>> {
        let path = format!("{}/tests/fixtures/formats/{name}", env!("CARGO_MANIFEST_DIR"));
        parse_dbf(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn character_numeric_logical_and_date_fields() {
        // Fixture: LibreOffice sc/qa/unit/data/dbf/pass/DBTest.dbf (MPL-2.0).
        let rows = fixture("DBTest.dbf");
        assert_eq!(rows.len(), 6);
        assert_eq!(
            rows[0],
            ["RowDsc", "Field1", "Field2", "Field3", "RowNumber", "BooleanFld", "IntFld", "DoubleFld", "StringFld", "TimeFld", "DateFld"]
        );
        assert_eq!(rows[1], ["$FirstRecord$ $Record1$", "", "5", "", "1", "TRUE", "1", "34.55660", "a", "0.5208333333", "1978-01-26"]);
        assert_eq!(rows[3], ["$Record3$", "", "5", "0.0847453704", "3", "FALSE", "5", "6.23640", "c", "0.8104166667", "1978-01-26"]);
        assert_eq!(rows[2][5], "");
    }

    #[test]
    fn unlabelled_code_pages_are_detected() {
        // Fixtures: Apache Tika test-documents (Apache-2.0); neither sets a language driver.
        let rows = fixture("testDBF.dbf");
        assert_eq!(rows.len(), 21);
        assert_eq!(rows[0], ["TEXT_FIELD", "NUMERIC_FI", "DATE_FIELD"]);
        assert_eq!(rows[1], ["Whan that aprill", "2.5", "2016-05-24"]);
        assert_eq!(rows[2][0], "普林斯顿大学");
        assert_eq!(rows[3][0], "جامعة برنستون");
        // GBK text; its leading space is part of the stored value.
        let rows = fixture("testDBF_gb18030.dbf");
        assert_eq!(rows, [["TEXTFIELD"], ["It was the best of times"], [" 虽然该指令经过重新发布"], ["it was the worst of times"]]);
    }

    #[test]
    fn visual_foxpro_binary_fields_and_language_driver() {
        // Built by hand: Visual FoxPro header, code page 1251 (driver 0xC9), integer, currency, datetime, nullable char.
        let mut dbf = vec![0x30, 126, 1, 1];
        dbf.extend_from_slice(&1u32.to_le_bytes());
        let header_len = 32 + 5 * 32 + 1;
        dbf.extend_from_slice(&(header_len as u16).to_le_bytes());
        dbf.extend_from_slice(&(1u16 + 4 + 4 + 8 + 8 + 1).to_le_bytes());
        dbf.resize(29, 0);
        dbf.extend_from_slice(&[0xC9, 0, 0]);
        for (name, kind, len, flags) in [("NAME", b'C', 4u8, 0u8), ("QTY", b'I', 4, 0), ("PRICE", b'Y', 8, 0), ("AT", b'T', 8, 0), ("_NullFlags", b'0', 1, 0)] {
            let mut d = [0u8; 32];
            d[..name.len()].copy_from_slice(name.as_bytes());
            (d[11], d[16], d[18]) = (kind, len, flags);
            dbf.extend_from_slice(&d);
        }
        dbf[32 + 18] = 0x02; // NAME is nullable
        dbf.push(0x0D);
        dbf.push(b' ');
        dbf.extend_from_slice(b"\xcc\xe8\xf0 ");
        dbf.extend_from_slice(&(-7i32).to_le_bytes());
        dbf.extend_from_slice(&123_450i64.to_le_bytes());
        dbf.extend_from_slice(&2_451_545i32.to_le_bytes());
        dbf.extend_from_slice(&((13 * 3600 + 5 * 60 + 9) * 1000i32).to_le_bytes());
        dbf.push(0);
        assert_eq!(parse_dbf(dbf.clone()).unwrap(), [vec!["NAME", "QTY", "PRICE", "AT"], vec!["Мир", "-7", "12.3450", "2000-01-01 13:05:09"]]);
        // Null bit set: the nullable field is empty.
        *dbf.last_mut().unwrap() = 1;
        assert_eq!(parse_dbf(dbf).unwrap()[1][0], "");
        assert_eq!(from_julian_day(2_299_161), (1582, 10, 15));
    }

    #[test]
    fn rejects_non_dbase_bytes() {
        assert!(parse_dbf(b"name,value\n".to_vec()).is_err());
    }
}
