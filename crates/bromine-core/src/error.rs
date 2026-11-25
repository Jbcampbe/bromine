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

    /// A JSON document did not parse. The position is carried on the variant
    /// rather than folded into the message because a viewer wants to jump to
    /// the offending line, and a caller should not have to parse a string to
    /// find out where the problem is. `serde_json` already tracks the byte
    /// position while it scans, so it is reported rather than guessed at.
    #[error("{path}:{line}:{column}: {message}")]
    Json {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },

    #[error("column {column} does not exist (table has {columns} columns)")]
    NoSuchColumn { column: usize, columns: usize },

    #[error("row {row} does not exist (table has {rows} rows)")]
    NoSuchRow { row: usize, rows: usize },

    /// A recorded edit no longer fits the table it is being replayed onto.
    /// Replaying it anyway would write a value into a different cell than the
    /// one the user changed, so it is refused instead.
    #[error("this edit no longer fits the table (it expected {expected} rows and found {found})")]
    StaleEdit { expected: usize, found: usize },

    /// The file holds NUL bytes near the start, so it is not text and no
    /// editor can show it without inventing a decoding.
    #[error("{path} looks like a binary file, so it cannot be shown as text")]
    Binary { path: PathBuf },

    /// A file that had to be decoded lossily cannot be written back, because
    /// the replacement characters would overwrite bytes the viewer could not
    /// read.
    #[error("not saved: {path} is not valid UTF-8, so writing it back would lose data")]
    Lossy { path: PathBuf },

    /// The path has no extension Bromine recognises at all.
    #[error("{path} is not a format Bromine can open")]
    UnsupportedFormat { path: PathBuf },

    /// The path is a format Bromine knows about but has no viewer for yet.
    /// Distinct from [`CoreError::UnsupportedFormat`] so the message can say
    /// "not yet" instead of "cannot", which is the truthful difference.
    #[error("{path} is {format}, which Bromine cannot show yet")]
    NotImplemented { path: PathBuf, format: Format },
}
