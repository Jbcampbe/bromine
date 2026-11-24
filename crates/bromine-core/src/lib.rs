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
mod error;
mod table;
mod view;

pub use csv::{CsvDocument, load_csv, parse_csv};
pub use error::CoreError;
pub use table::{Repairs, Table};
pub use view::{Direction, RowView, SortKey, compare_values};

/// Formats Bromine can open today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Markdown,
}

impl Format {
    /// Guess a format from a file path's extension.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "csv" | "tsv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "md" | "markdown" | "mdx" => Some(Self::Markdown),
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
        })
    }
}

/// Load whatever `path` holds, or explain why it cannot be shown.
///
/// This is the only place that decides which format engine to use. Keeping the
/// decision here means the command line and the window cannot disagree about
/// what is supported, and that a new format is added in one place.
pub fn load_document(path: &std::path::Path) -> Result<CsvDocument, CoreError> {
    match Format::from_path(path) {
        Some(Format::Csv) => load_csv(path),
        Some(format) => Err(CoreError::NotImplemented {
            path: path.to_path_buf(),
            format,
        }),
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

        assert_eq!(document.table.header(0), Some("name"));
        assert_eq!(document.table.row_count(), 6);
    }

    #[test]
    fn a_known_format_without_a_viewer_says_not_yet_rather_than_never() {
        let error = load_document(Path::new("package.json")).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("JSON"), "unexpected message: {message}");
        assert!(
            message.contains("cannot show yet"),
            "unexpected message: {message}"
        );
        assert!(matches!(error, CoreError::NotImplemented { .. }));
    }

    #[test]
    fn an_unknown_extension_is_reported_as_unopenable() {
        let error = load_document(Path::new("notes.txt")).unwrap_err();

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
