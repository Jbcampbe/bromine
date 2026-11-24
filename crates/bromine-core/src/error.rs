use std::path::PathBuf;

use crate::Format;

/// Failures that a format engine can report to the caller.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed CSV: {0}")]
    Csv(#[from] csv::Error),

    #[error("column {column} does not exist (table has {columns} columns)")]
    NoSuchColumn { column: usize, columns: usize },

    #[error("row {row} does not exist (table has {rows} rows)")]
    NoSuchRow { row: usize, rows: usize },

    /// The path has no extension Bromine recognises at all.
    #[error("{path} is not a format Bromine can open")]
    UnsupportedFormat { path: PathBuf },

    /// The path is a format Bromine knows about but has no viewer for yet.
    /// Distinct from [`CoreError::UnsupportedFormat`] so the message can say
    /// "not yet" instead of "cannot", which is the truthful difference.
    #[error("{path} is {format}, which Bromine cannot show yet")]
    NotImplemented { path: PathBuf, format: Format },
}
