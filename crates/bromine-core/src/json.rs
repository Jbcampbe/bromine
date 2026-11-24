use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::CoreError;

/// A parsed JSON document plus the text it came from.
///
/// The original source is kept alongside the parsed value for two reasons: the
/// viewer can show what was actually on disk when the value has been formatted
/// into something prettier, and a document that parsed but held no keys (an
/// empty object, a bare scalar) still has a size and a line count worth
/// reporting. The parsed `Value` is what the view reads; the source is what the
/// user wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonDocument {
    pub path: PathBuf,
    pub value: Value,
    pub source: String,
}

impl JsonDocument {
    /// The document indented two spaces, for reading.
    ///
    /// Key order is whatever `preserve_order` kept from the source. That is the
    /// whole point of the feature: reordering keys would make this output a
    /// different document than the one the user opened.
    pub fn pretty(&self) -> String {
        // Serializing a `Value` cannot fail: every variant, including a map
        // key, is already a string by the time it is a `Value`. `to_string_pretty`
        // still returns a `Result`, so there is nothing to handle but the
        // impossible case.
        serde_json::to_string_pretty(&self.value)
            .expect("serializing an in-memory JSON value cannot fail")
    }

    /// The document on a single line.
    ///
    /// `Value`'s own `Display` is exactly this: compact, no whitespace between
    /// tokens. Round trips through [`Self::pretty`] and this preserve key order
    /// and values, only the whitespace differs.
    pub fn minified(&self) -> String {
        self.value.to_string()
    }

    /// How many lines [`Self::pretty`] produces.
    ///
    /// Computed from the formatted text rather than from a structural walk, so
    /// the number can never drift from what the view actually draws.
    pub fn line_count(&self) -> usize {
        self.pretty().lines().count()
    }
}

/// Read a JSON file from disk.
///
/// The file is read as UTF-8 text rather than bytes: JSON is defined in terms
/// of a Unicode character encoding, so there is no meaningful lossy reading of
/// a JSON file the way there is for a CSV export. A file that is not UTF-8 is
/// reported as a read failure, which is the truth about it.
pub fn load_json(path: &Path) -> Result<JsonDocument, CoreError> {
    let source = std::fs::read_to_string(path).map_err(|source| CoreError::Read {
        path: PathBuf::from(path),
        source,
    })?;
    parse_json(path, source)
}

/// Parse JSON text that is already in memory.
///
/// Errors carry the position `serde_json` recorded while scanning. Its own
/// `Display` appends "at line L column C" to the message, which the
/// [`CoreError::Json`] variant would then print a second time next to the same
/// coordinates, so the suffix is stripped and the message stands on its own.
pub fn parse_json(path: &Path, source: String) -> Result<JsonDocument, CoreError> {
    let value = serde_json::from_str(&source).map_err(|error| CoreError::Json {
        path: PathBuf::from(path),
        line: error.line(),
        column: error.column(),
        message: message_without_position(&error),
    })?;

    Ok(JsonDocument {
        path: PathBuf::from(path),
        value,
        source,
    })
}

/// Drop the location suffix `serde_json` appends, if it added one.
///
/// Some failures have no position at all (an I/O error reading from something
/// other than a string), in which case the message is returned untouched.
fn message_without_position(error: &serde_json::Error) -> String {
    let message = error.to_string();
    match message.find(" at line ") {
        Some(index) => message[..index].to_string(),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(source: &str) -> JsonDocument {
        parse_json(Path::new("inline.json"), source.to_string()).expect("document parses")
    }

    #[test]
    fn pretty_indents_and_keeps_the_written_key_order() {
        let document =
            parse(r#"{"zebra": 1, "apple": {"yak": true, "bee": null}, "mango": [1, 2]}"#);
        let pretty = document.pretty();

        assert!(
            pretty.contains("\n  \"zebra\": 1"),
            "not indented: {pretty}"
        );
        assert!(
            pretty.contains("\n    \"yak\": true"),
            "nested indentation is missing: {pretty}"
        );

        let zebra = pretty.find("\"zebra\"").expect("zebra key survives");
        let apple = pretty.find("\"apple\"").expect("apple key survives");
        let mango = pretty.find("\"mango\"").expect("mango key survives");
        assert!(
            zebra < apple && apple < mango,
            "keys were reordered: {pretty}"
        );

        let yak = pretty.find("\"yak\"").expect("yak key survives");
        let bee = pretty.find("\"bee\"").expect("bee key survives");
        assert!(yak < bee, "nested keys were reordered: {pretty}");
    }

    #[test]
    fn minified_is_one_line_and_keeps_the_written_key_order() {
        let document = parse("{\n  \"zebra\": 1,\n  \"apple\": 2\n}");
        let minified = document.minified();

        assert!(
            !minified.contains('\n'),
            "minified has a newline: {minified}"
        );
        assert_eq!(minified, r#"{"zebra":1,"apple":2}"#);
    }

    #[test]
    fn preserve_order_survives_both_formats() {
        // Alphabetical order would be apple, mango, zebra. The feature under
        // test is that none of that reordering happens, in either direction.
        let source = r#"{"zebra": 1, "apple": 2, "mango": 3}"#;
        let document = parse(source);

        assert_eq!(
            document.pretty(),
            "{\n  \"zebra\": 1,\n  \"apple\": 2,\n  \"mango\": 3\n}"
        );
        assert_eq!(document.minified(), r#"{"zebra":1,"apple":2,"mango":3}"#);
    }

    #[test]
    fn line_count_matches_the_formatted_text() {
        let document = parse(r#"{"a": 1, "b": [1, 2, 3], "c": {}}"#);
        let pretty = document.pretty();

        assert_eq!(document.line_count(), pretty.lines().count());
        assert!(document.line_count() > 1, "a pretty object spans lines");
    }

    #[test]
    fn a_malformed_document_reports_where_it_broke() {
        let error = parse_json(
            Path::new("broken.json"),
            "{\n  \"a\": 1,\n  \"b\": \n}".to_string(),
        )
        .unwrap_err();

        let CoreError::Json {
            path,
            line,
            column,
            message,
        } = &error
        else {
            panic!("expected a JSON error, got {error:?}");
        };

        assert_eq!(path, Path::new("broken.json"));
        assert_eq!(*line, 4, "the object closes without a value on line 4");
        assert!(*column >= 1);
        assert!(!message.is_empty());
        assert!(
            !message.contains(" at line "),
            "the position is reported twice: {message}"
        );
        assert!(
            error.to_string().starts_with("broken.json:4:"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn a_scalar_document_is_a_document() {
        let document = parse("42");

        assert_eq!(document.value, Value::from(42));
        assert_eq!(document.minified(), "42");
        assert_eq!(document.line_count(), 1);
    }

    #[test]
    fn an_array_and_an_empty_object_are_documents() {
        let array = parse("[1, \"two\", true, null]");
        assert_eq!(array.minified(), r#"[1,"two",true,null]"#);
        assert_eq!(array.pretty().lines().count(), 6);

        let object = parse("{}");
        assert_eq!(object.minified(), "{}");
        assert_eq!(object.pretty(), "{}");
        assert_eq!(object.line_count(), 1);
    }

    #[test]
    fn the_source_is_kept_verbatim() {
        let source = "{\n\t\"a\": 1\n}";
        let document = parse(source);

        assert_eq!(document.source, source);
        assert_ne!(document.pretty(), source, "pretty reformats the source");
    }

    #[test]
    fn loading_a_missing_file_reports_the_path_through_read() {
        let error = load_json(Path::new("fixtures/absent.json")).unwrap_err();

        assert!(matches!(error, CoreError::Read { .. }));
        assert!(error.to_string().contains("fixtures/absent.json"));
    }
}
