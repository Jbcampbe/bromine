//! Turning the URLs macOS hands to an app into paths.
//!
//! Finder does not pass a double-clicked file as a command-line argument. It
//! sends an `openURLs` event after launch, and GPUI surfaces that event as a
//! list of URL strings. This module is the last step: decode those strings into
//! paths the workspace can open.

use std::path::PathBuf;

/// The path behind a `file://` URL, or `None` for any other scheme.
///
/// `Url::to_file_path` also decodes percent escapes, which matters for the
/// names developers actually have: `data 2024.csv` arrives as
/// `file:///Users/me/data%202024.csv`.
pub(crate) fn path_from_url(url: &str) -> Option<PathBuf> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_becomes_a_path() {
        assert_eq!(
            path_from_url("file:///tmp/people.csv"),
            Some(PathBuf::from("/tmp/people.csv"))
        );
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            path_from_url("file:///tmp/data%202024.json"),
            Some(PathBuf::from("/tmp/data 2024.json"))
        );
    }

    #[test]
    fn localhost_names_the_local_machine() {
        assert_eq!(
            path_from_url("file://localhost/tmp/people.tsv"),
            Some(PathBuf::from("/tmp/people.tsv"))
        );
    }

    #[test]
    fn a_non_file_url_is_ignored() {
        assert_eq!(path_from_url("https://example.com/data.csv"), None);
    }

    #[test]
    fn a_malformed_url_is_ignored() {
        assert_eq!(path_from_url("not a url"), None);
    }
}
