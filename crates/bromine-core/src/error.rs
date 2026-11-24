use std::path::PathBuf;

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
}
