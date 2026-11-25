//! Format engines for Bromine.
//!
//! This crate holds everything that is not UI: parsing a file into a model,
//! querying that model, and writing it back out. It deliberately does not
//! depend on GPUI, so every behaviour here is testable with plain `cargo test`
//! and reachable from a headless harness.
//!
//! The design rule for this crate is that *the data and the view of the data
//! are separate*:
//!
//! - [`Table`] owns the parsed cells and is the only thing that mutates.
//! - [`RowView`] is a derived, ordered, filtered list of indices into a
//!   [`Table`]. Sorting and searching never reorder the underlying rows, so an
//!   edit always targets a stable absolute row index.

mod csv;
mod edits;
mod error;
mod json;
mod markdown;
mod table;
mod text;
mod view;

pub use csv::{CsvDocument, load_csv, parse_csv};
pub use edits::{CaretMove, EditOutcome, StepId, TableEdits};
pub use error::CoreError;
pub use json::{JsonDocument, load_json, parse_json};
pub use markdown::{
    Alignment, Block, BlockId, BlockKind, CellEdit, CellPosition, ImageInfo, ImageLocation,
    ListMarker, MarkdownDocument, Row, RowKind, Run, RunStyle, TableBlock, TableCell,
    load_markdown,
};
pub use table::{ColumnInsertion, ColumnRemoval, Repairs, Table};
pub use text::{EditKind, LineEnding, TextDocument, UndoOutcome};
pub use view::{Direction, RowView, SortKey, compare_values};

/// Formats Bromine can open today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Markdown,
    Text,
}

impl Format {
    /// Guess a format from a file path's extension.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "csv" | "tsv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "md" | "markdown" | "mdx" => Some(Self::Markdown),
            "txt" | "text" | "log" => Some(Self::Text),
            _ => None,
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Csv => "CSV",
            Self::Json => "JSON",
            Self::Markdown => "Markdown",
            Self::Text => "text",
        })
    }
}

/// A file Bromine can show, already parsed by the engine for its format.
///
/// The engines do not know about each other, so this enum is the one place
/// where a loaded file and the viewer that can draw it are matched up. A tab is
/// built from the variant, which is why adding a format means one arm in
/// [`load_document`] and one arm in the window, not a change to every caller.
#[derive(Debug, Clone)]
pub enum Document {
    Csv(CsvDocument),
    Json(JsonDocument),
    Markdown(MarkdownDocument),
    Text(TextDocument),
}

/// Load whatever `path` holds, or explain why it cannot be shown.
///
/// This is the only place that decides which format engine to use. Keeping the
/// decision here means the command line and the window cannot disagree about
/// what is supported, and that a new format is added in one place.
pub fn load_document(path: &std::path::Path) -> Result<Document, CoreError> {
    match Format::from_path(path) {
        Some(Format::Csv) => load_csv(path).map(Document::Csv),
        Some(Format::Json) => load_json(path).map(Document::Json),
        Some(Format::Markdown) => load_markdown(path).map(Document::Markdown),
        Some(Format::Text) => TextDocument::load(path).map(Document::Text),
        None => Err(CoreError::UnsupportedFormat {
            path: path.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }

    #[test]
    fn a_csv_path_loads_through_the_dispatch() {
        let document = load_document(&fixture("people.csv")).expect("people.csv loads");
        let Document::Csv(document) = document else {
            panic!("a .csv path must dispatch to the CSV engine");
        };

        assert_eq!(document.table.header(0), Some("name"));
        assert_eq!(document.table.row_count(), 6);
    }

    #[test]
    fn an_empty_csv_still_dispatches_as_a_csv_document() {
        let document = load_document(&fixture("empty.csv")).expect("empty.csv loads");
        let Document::Csv(document) = document else {
            panic!("an empty .csv path must still dispatch to the CSV engine");
        };

        assert_eq!(document.table.column_count(), 0);
        assert_eq!(document.table.row_count(), 0);
    }

    #[test]
    fn a_json_path_loads_through_the_dispatch() {
        let directory =
            std::env::temp_dir().join(format!("bromine-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("person.json");
        std::fs::write(&path, r#"{"name": "ada", "commits": 412}"#)
            .expect("the JSON file is written");

        let document = load_document(&path).expect("the JSON file loads");

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");

        let Document::Json(document) = document else {
            panic!("a .json path must dispatch to the JSON engine");
        };
        assert_eq!(document.value["name"], "ada");
        assert!(document.pretty().contains("\"name\": \"ada\""));
    }

    #[test]
    fn a_markdown_path_loads_through_the_dispatch() {
        let document = load_document(&fixture("notes.md")).expect("notes.md loads");
        let Document::Markdown(document) = document else {
            panic!("a .md path must dispatch to the Markdown engine");
        };
        assert!(document.rows().len() > 1);
        assert!(
            document
                .blocks()
                .iter()
                .any(|block| matches!(block.kind, BlockKind::Heading(1))),
            "the fixture starts with a heading"
        );
    }

    #[test]
    fn a_text_path_loads_through_the_dispatch() {
        let document = load_document(&fixture("readme.txt")).expect("readme.txt loads");
        let Document::Text(document) = document else {
            panic!("a .txt path must dispatch to the text engine");
        };
        assert!(document.text().contains("Bromine"));
    }

    #[test]
    fn every_text_extension_dispatches_to_the_text_engine() {
        for name in ["notes.txt", "notes.text", "server.log"] {
            assert_eq!(
                Format::from_path(Path::new(name)),
                Some(Format::Text),
                "{name} should be text"
            );
        }
        for name in ["notes.md", "notes.markdown", "notes.mdx"] {
            assert_eq!(
                Format::from_path(Path::new(name)),
                Some(Format::Markdown),
                "{name} should be Markdown"
            );
        }
    }

    #[test]
    fn an_unknown_extension_is_reported_as_unopenable() {
        let error = load_document(Path::new("notes.toml")).unwrap_err();

        assert!(matches!(error, CoreError::UnsupportedFormat { .. }));
        assert!(
            error
                .to_string()
                .contains("is not a format Bromine can open")
        );
    }

    #[test]
    fn a_missing_file_still_reports_the_path() {
        let error = load_document(Path::new("fixtures/absent.csv")).unwrap_err();

        assert!(matches!(error, CoreError::Read { .. }));
        assert!(error.to_string().contains("fixtures/absent.csv"));
    }
}
