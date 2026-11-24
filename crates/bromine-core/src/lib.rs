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
