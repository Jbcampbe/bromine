use std::path::{Path, PathBuf};

use crate::table::Repairs;
use crate::{CoreError, Table};

/// The delimiters Bromine will guess between when opening a delimited file.
const CANDIDATE_DELIMITERS: [u8; 4] = *b",\t;|";

/// How many leading lines are inspected when guessing a delimiter.
const DELIMITER_SAMPLE_LINES: usize = 5;

/// How much of the file is decoded for delimiter detection. Sniffing must not
/// cost a full copy of a large file.
const DELIMITER_SAMPLE_BYTES: usize = 8 * 1024;

/// A parsed delimited file plus the delimiter needed to write it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvDocument {
    pub table: Table,
    pub delimiter: u8,
    /// Repairs applied to make ragged input rectangular.
    pub repairs: Repairs,
    /// True when the input was not valid UTF-8 and had to be decoded lossily.
    /// A caller that writes the file back is responsible for telling the user,
    /// because the replacement characters will overwrite the original bytes.
    pub lossy: bool,
}

impl CsvDocument {
    /// Serialize the table back to delimited text, quoting where required.
    pub fn to_csv_string(&self) -> Result<String, CoreError> {
        // A table with no columns has nothing to say. Writing it anyway would
        // emit a stray empty header, so an empty file would come back as a
        // one-column table on the next read.
        if self.table.column_count() == 0 {
            return Ok(String::new());
        }

        let mut writer = csv::WriterBuilder::new()
            .delimiter(self.delimiter)
            .from_writer(Vec::new());

        writer.write_record(self.table.headers())?;
        for row in self.table.rows() {
            writer.write_record(row)?;
        }

        let bytes = writer
            .into_inner()
            .map_err(|error| CoreError::Csv(csv::Error::from(error.into_error())))?;

        String::from_utf8(bytes).map_err(|error| {
            CoreError::Csv(csv::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })
    }

    /// Write the table back to `path`, replacing the file's contents.
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        let text = self.to_csv_string()?;
        std::fs::write(path, text).map_err(|source| CoreError::Write {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Read a delimited file from disk, guessing its delimiter.
pub fn load_csv(path: &Path) -> Result<CsvDocument, CoreError> {
    let bytes = std::fs::read(path).map_err(|source| CoreError::Read {
        path: PathBuf::from(path),
        source,
    })?;
    parse_csv_bytes(&bytes)
}

/// Parse delimited text, guessing its delimiter.
///
/// The first record is treated as the header line. Ragged records are repaired
/// rather than rejected, because real files routinely contain trailing empty
/// columns, and the repairs are reported on [`CsvDocument::repairs`].
pub fn parse_csv(source: &str) -> Result<CsvDocument, CoreError> {
    parse_csv_bytes(source.as_bytes())
}

/// Parse delimited bytes.
///
/// Non-UTF-8 input is decoded lossily rather than rejected: a Latin-1 export is
/// still worth viewing, and losing a handful of characters beats refusing to
/// open the file. Only the caller can decide whether that is acceptable for a
/// write-back, which is why [`CsvDocument::lossy`] records that it happened.
pub fn parse_csv_bytes(source: &[u8]) -> Result<CsvDocument, CoreError> {
    let source = strip_utf8_bom(source);
    let lossy = std::str::from_utf8(source).is_err();
    let sample = &source[..source.len().min(DELIMITER_SAMPLE_BYTES)];
    let delimiter = detect_delimiter(&String::from_utf8_lossy(sample));

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(source);

    let mut records = reader.byte_records();
    let headers = match records.next() {
        Some(record) => to_owned_cells(&record?),
        None => Vec::new(),
    };

    let mut rows = Vec::new();
    for record in records {
        rows.push(to_owned_cells(&record?));
    }

    let (table, repairs) = Table::load_repairs(&headers, rows);

    Ok(CsvDocument {
        table,
        delimiter,
        repairs,
        lossy,
    })
}

/// Guess which delimiter a delimited file uses.
///
/// A candidate wins only when it appears the same number of times on each of
/// the sampled lines. That consistency check is what stops a comma inside a
/// quoted TSV field from beating the tabs that actually separate the columns.
/// Ties break toward the earlier candidate, so a comma is the default.
pub fn detect_delimiter(source: &str) -> u8 {
    let lines: Vec<&str> = source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(DELIMITER_SAMPLE_LINES)
        .collect();

    let Some(first_line) = lines.first() else {
        return b',';
    };

    let mut best = (b',', 0usize);
    for candidate in CANDIDATE_DELIMITERS {
        let first_count = count_unquoted(first_line, candidate);
        if first_count == 0 {
            continue;
        }

        let consistent = lines
            .iter()
            .all(|line| count_unquoted(line, candidate) == first_count);
        if consistent && first_count > best.1 {
            best = (candidate, first_count);
        }
    }

    best.0
}

fn count_unquoted(line: &str, delimiter: u8) -> usize {
    let mut count = 0;
    let mut in_quotes = false;
    let bytes = line.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                // A doubled quote is an escaped quote and does not end the field.
                if in_quotes && bytes.get(index + 1) == Some(&b'"') {
                    index += 1;
                } else {
                    in_quotes = !in_quotes;
                }
            }
            byte if byte == delimiter && !in_quotes => count += 1,
            _ => {}
        }
        index += 1;
    }

    count
}

fn strip_utf8_bom(source: &[u8]) -> &[u8] {
    source.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(source)
}

fn to_owned_cells(record: &csv::ByteRecord) -> Vec<String> {
    record
        .iter()
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_file_into_headers_and_rows() {
        let document = parse_csv("name,score\nada,10\ngrace,9\n").unwrap();

        assert_eq!(document.delimiter, b',');
        assert_eq!(document.table.headers(), &["name", "score"]);
        assert_eq!(document.table.row_count(), 2);
        assert_eq!(document.table.get(1, 0), Some("grace"));
        assert_eq!(document.repairs.unnamed_columns, 0);
    }

    #[test]
    fn quoted_delimiters_and_newlines_stay_inside_one_cell() {
        let source = "id,note\n1,\"a, b\"\n2,\"two\nlines\"\n";
        let document = parse_csv(source).unwrap();

        assert_eq!(document.table.get(0, 1), Some("a, b"));
        assert_eq!(document.table.get(1, 1), Some("two\nlines"));
        assert_eq!(document.table.row_count(), 2);
    }

    #[test]
    fn escaped_quotes_round_trip() {
        let source = "quote\n\"he said \"\"hi\"\"\"\n";
        let document = parse_csv(source).unwrap();
        assert_eq!(document.table.get(0, 0), Some("he said \"hi\""));

        let written = document.to_csv_string().unwrap();
        let reparsed = parse_csv(&written).unwrap();
        assert_eq!(reparsed.table, document.table);
    }

    #[test]
    fn detects_tab_separated_files() {
        let document =
            parse_csv("name\trole\tteam\nada\tengineer\tcore\ngrace\tadmiral\tops\n").unwrap();

        assert_eq!(document.delimiter, b'\t');
        assert_eq!(document.table.column_count(), 3);
        assert_eq!(document.table.get(1, 2), Some("ops"));
    }

    #[test]
    fn a_comma_inside_a_quoted_tsv_field_does_not_win_detection() {
        let source = "id\tnote\n1\t\"two, commas, here\"\n2\t\"another, one\"\n";
        let document = parse_csv(source).unwrap();

        // Tabs appear once per line; commas appear twice, once, and zero times.
        assert_eq!(document.delimiter, b'\t');
        assert_eq!(document.table.get(0, 1), Some("two, commas, here"));
    }

    #[test]
    fn detects_semicolon_separated_files() {
        let document = parse_csv("a;b;c\n1;2;3\n4;5;6\n").unwrap();
        assert_eq!(document.delimiter, b';');
        assert_eq!(document.table.column_count(), 3);
    }

    #[test]
    fn falls_back_to_comma_when_no_delimiter_is_consistent() {
        assert_eq!(detect_delimiter("only_one_column\nvalue\n"), b',');
        assert_eq!(detect_delimiter(""), b',');
    }

    #[test]
    fn ragged_rows_are_padded_and_extra_columns_are_named() {
        let source = "a,b\n1,2,3\n4\n";
        let document = parse_csv(source).unwrap();

        assert_eq!(document.table.column_count(), 3);
        assert_eq!(document.table.header(2), Some("column_3"));
        assert_eq!(document.repairs.unnamed_columns, 1);
        assert_eq!(document.repairs.padded_rows, 1);
        // The short row is padded, not shifted or dropped.
        assert_eq!(document.table.row(1).unwrap(), &["4", "", ""]);
    }

    #[test]
    fn blank_lines_are_ignored_and_a_trailing_newline_is_not_a_row() {
        let document = parse_csv("a,b\n1,2\n\n3,4\n\n").unwrap();
        assert_eq!(document.table.row_count(), 2);
        assert_eq!(document.table.get(1, 0), Some("3"));
    }

    #[test]
    fn a_bom_does_not_leak_into_the_first_header() {
        let document = parse_csv("\u{feff}name,score\nada,10\n").unwrap();
        assert_eq!(document.table.header(0), Some("name"));
    }

    #[test]
    fn an_empty_file_produces_an_empty_table() {
        let document = parse_csv("").unwrap();
        assert!(document.table.is_empty());
        assert_eq!(document.table.row_count(), 0);
        assert_eq!(document.table.column_count(), 0);
    }

    #[test]
    fn an_empty_table_writes_an_empty_file_not_a_stray_header() {
        let document = parse_csv("").unwrap();
        assert_eq!(document.to_csv_string().unwrap(), "");

        // And the cycle is closed: an empty file reads back as empty.
        let reparsed = parse_csv(&document.to_csv_string().unwrap()).unwrap();
        assert_eq!(reparsed.table, document.table);
    }

    #[test]
    fn a_header_only_table_writes_just_the_header_line() {
        let document = parse_csv("name,score\n").unwrap();
        assert_eq!(document.to_csv_string().unwrap(), "name,score\n");
    }

    #[test]
    fn a_header_only_file_has_columns_but_no_rows() {
        let document = parse_csv("name,score\n").unwrap();
        assert_eq!(document.table.column_count(), 2);
        assert_eq!(document.table.row_count(), 0);
        assert!(!document.table.is_empty());
    }

    #[test]
    fn non_utf8_bytes_are_decoded_lossily_instead_of_failing() {
        // "caf" followed by a Latin-1 0xE9, then ",\n1,2\n".
        let mut source = b"caf\xe9,name\n".to_vec();
        source.extend_from_slice(b"1,2\n");

        let document = parse_csv_bytes(&source).unwrap();
        assert_eq!(document.table.header(0), Some("caf\u{fffd}"));
        assert_eq!(document.table.get(0, 1), Some("2"));

        // The caller has to be able to tell that writing this back would change
        // the file, because U+FFFD is not the byte that was read.
        assert!(document.lossy);
    }

    #[test]
    fn valid_utf8_is_not_flagged_as_lossy() {
        let document = parse_csv("name,note\nada,caf\u{e9}\n").unwrap();

        assert!(!document.lossy);
        assert_eq!(document.table.get(0, 1), Some("caf\u{e9}"));
    }

    #[test]
    fn round_trips_a_file_through_parse_and_write() {
        let source = "name,note\nada,\"likes, commas\"\ngrace,\"multi\nline\"\n";
        let document = parse_csv(source).unwrap();
        let written = document.to_csv_string().unwrap();
        let reparsed = parse_csv(&written).unwrap();

        assert_eq!(reparsed.table, document.table);
        assert_eq!(reparsed.delimiter, document.delimiter);
    }

    #[test]
    fn saving_preserves_the_detected_delimiter() {
        let directory = tempdir();
        let path = directory.join("data.tsv");
        let document = parse_csv("a\tb\n1\t2\n").unwrap();
        document.save(&path).unwrap();

        let reread = load_csv(&path).unwrap();
        assert_eq!(reread.delimiter, b'\t');
        assert_eq!(reread.table, document.table);

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn loading_a_missing_file_reports_the_path() {
        let error = load_csv(Path::new("/definitely/not/here.csv")).unwrap_err();
        assert!(matches!(error, CoreError::Read { .. }));
        assert!(error.to_string().contains("/definitely/not/here.csv"));
    }

    /// A unique scratch directory, so tests never touch the repository tree.
    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!("bromine-core-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
