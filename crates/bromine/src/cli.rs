//! Command-line parsing, kept out of `main` so it can be tested without
//! starting GPUI.
//!
//! A file named like a flag is a real hazard here: `bromine --version` used to
//! try to open a document called `--version`. Flags are therefore recognised
//! before anything is treated as a path, and an unknown flag is an error rather
//! than a filename.

use std::ffi::OsString;
use std::path::PathBuf;

/// What the command line asked this process to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invocation {
    /// Open a window with these files, in order.
    Run(Vec<PathBuf>),
    /// Print the version and exit successfully.
    Version,
    /// Print usage and exit successfully.
    Help,
}

pub(crate) const USAGE: &str = "\
Usage: bromine [options] [file...]

Open CSV, TSV, JSON, Markdown, or text files in tabs. With no files the
window starts empty.

Options:
  -h, --help     Show this help and exit
      --version  Show the version and exit";

pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut paths = Vec::new();

    for arg in args {
        match arg.to_str() {
            Some("--version") => return Ok(Invocation::Version),
            Some("--help" | "-h") => return Ok(Invocation::Help),
            Some(flag) if flag.starts_with('-') => {
                return Err(format!("unknown option: {flag}\n\n{USAGE}"));
            }
            // A path that is not valid UTF-8 is still a path; `load_document`
            // can decide whether it can read it.
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    Ok(Invocation::Run(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Invocation, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_runs_with_an_empty_workspace() {
        assert_eq!(parse_args(&[]), Ok(Invocation::Run(Vec::new())));
    }

    #[test]
    fn paths_become_tabs_in_order() {
        assert_eq!(
            parse_args(&["fixtures/people.csv", "fixtures/config.json"]),
            Ok(Invocation::Run(vec![
                PathBuf::from("fixtures/people.csv"),
                PathBuf::from("fixtures/config.json"),
            ]))
        );
    }

    #[test]
    fn version_is_recognised() {
        assert_eq!(parse_args(&["--version"]), Ok(Invocation::Version));
    }

    #[test]
    fn help_is_recognised_in_both_forms() {
        assert_eq!(parse_args(&["--help"]), Ok(Invocation::Help));
        assert_eq!(parse_args(&["-h"]), Ok(Invocation::Help));
    }

    #[test]
    fn an_unknown_flag_is_an_error_with_usage() {
        let error = parse_args(&["--nope"]).expect_err("an unknown flag is rejected");
        assert!(error.contains("unknown option: --nope"), "{error}");
        assert!(error.contains("Usage: bromine"), "{error}");
        assert!(
            !error.contains("Run("),
            "the message is for a person, not a debug print: {error}"
        );
    }

    #[test]
    fn a_flag_after_a_path_is_still_a_flag() {
        let error = parse_args(&["fixtures/people.csv", "--nope"])
            .expect_err("a trailing unknown flag is rejected");
        assert!(error.contains("unknown option: --nope"));
    }
}
