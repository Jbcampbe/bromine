//! An editable UTF-8 text buffer shared by the plain-text and Markdown views.
//!
//! The document owns the source string, the line ending it was written with,
//! and the undo history. Nothing here knows about carets or pixels: the views
//! keep the selection and translate it into byte ranges, and this type simply
//! applies the edits it is given.
//!
//! Text is normalised to `\n` in memory so every line-based operation has one
//! shape to reason about. Saving converts back to the recorded line ending, so
//! opening and saving a CRLF file does not rewrite every line.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::CoreError;

/// How many leading bytes are inspected for NUL when deciding whether a file
/// is binary. A NUL past this point is unusual in a real text file and does not
/// justify reading a whole large file twice.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// The line ending a file used when it was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// What kind of edit produced an undo step.
///
/// Only the two run-of-the-mill kinds coalesce. Anything else (`Other`) closes
/// the current group, which is what makes an undo stop at a paste, an Enter, or
/// a structural table edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Backspace,
    Delete,
    Other,
}

/// What an undo or redo put back in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoOutcome {
    /// The byte range now holding the restored text.
    pub range: Range<usize>,
    /// Where the caret belonged before the edit that was undone.
    pub caret: usize,
}

#[derive(Debug, Clone)]
struct UndoStep {
    /// The range in the current document that this step replaces.
    range: Range<usize>,
    removed: String,
    inserted: String,
    caret_before: usize,
    caret_after: usize,
    kind: EditKind,
}

/// An editable text document loaded from disk.
#[derive(Debug, Clone)]
pub struct TextDocument {
    path: PathBuf,
    text: String,
    line_ending: LineEnding,
    bom: bool,
    lossy: bool,
    dirty: bool,
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// The kind of edit currently open for coalescing, if any. Navigation calls
    /// [`Self::break_undo_group`], which is what makes arrow keys split an undo
    /// step instead of folding the next keystroke into the previous one.
    open_group: Option<EditKind>,
}

impl TextDocument {
    /// Read a file from disk as editable text.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let bytes = std::fs::read(path).map_err(|source| CoreError::Read {
            path: PathBuf::from(path),
            source,
        })?;
        Self::from_bytes(path, bytes)
    }

    /// Build a document from bytes that are already in memory.
    ///
    /// This is the path tests take, and the one [`Self::load`] delegates to.
    /// Invalid UTF-8 is decoded lossily and recorded, because showing a Latin-1
    /// log is more useful than refusing it; writing it back is refused later
    /// rather than damaging the original.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Result<Self, CoreError> {
        let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
        if sniff.contains(&0) {
            return Err(CoreError::Binary {
                path: PathBuf::from(path),
            });
        }

        let (bom, rest) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (true, rest),
            None => (false, bytes.as_slice()),
        };
        let lossy = std::str::from_utf8(rest).is_err();
        let decoded = String::from_utf8_lossy(rest).into_owned();

        let line_ending = detect_line_ending(&decoded);
        let text = normalize_line_endings(&decoded);

        Ok(Self {
            path: PathBuf::from(path),
            text,
            line_ending,
            bom,
            lossy,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: None,
        })
    }

    /// Build a document from text that is already decoded.
    ///
    /// The line ending is detected rather than assumed, so a caller does not
    /// have to know what was on disk to keep a CRLF file's shape.
    pub fn from_str(path: &Path, source: &str) -> Self {
        let (bom, rest) = match source.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, source),
        };
        Self {
            path: PathBuf::from(path),
            line_ending: detect_line_ending(rest),
            text: normalize_line_endings(rest),
            bom,
            lossy: false,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            open_group: None,
        }
    }

    /// The file this document came from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The whole source, normalised to `\n`.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The source length in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The line ending that will be written back.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Whether the file began with a UTF-8 BOM, which is written back.
    pub fn has_bom(&self) -> bool {
        self.bom
    }

    /// Whether loading had to replace bytes that were not valid UTF-8.
    ///
    /// Such a document can be read but never saved, because the replacement
    /// characters would destroy the bytes the decoder could not represent.
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }

    /// Whether there are edits that are not on disk yet.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// One line's text, without its newline.
    pub fn line_text(&self, line: usize) -> &str {
        let Some(range) = self.line_range(line) else {
            return "";
        };
        &self.text[range]
    }

    /// How many lines the document has.
    ///
    /// A trailing newline opens an empty final line, which is where the caret
    /// belongs after pressing Enter at the end of a file.
    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|byte| *byte == b'\n').count() + 1
    }

    /// The byte range of one line, excluding its newline.
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        if line >= self.line_count() {
            return None;
        }
        let start = self.line_start(line)?;
        let end = self.text[start..]
            .find('\n')
            .map(|offset| start + offset)
            .unwrap_or(self.text.len());
        Some(start..end)
    }

    /// The byte offset where a line begins.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return Some(0);
        }
        let mut seen = 0;
        for (offset, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                seen += 1;
                if seen == line {
                    return Some(offset + 1);
                }
            }
        }
        (seen == line).then_some(self.text.len())
    }

    /// The line a byte offset falls on, counting from zero.
    ///
    /// An offset equal to the document length belongs to the last line.
    pub fn line_of(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        self.text[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
    }

    /// The byte column of an offset within its line.
    pub fn column_of(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        offset.saturating_sub(self.line_start(self.line_of(offset)).unwrap_or(0))
    }

    /// The byte offset of a line and column, clamped to the line.
    pub fn offset_of(&self, line: usize, column: usize) -> usize {
        let Some(range) = self.line_range(line) else {
            return self.text.len();
        };
        (range.start + column.min(range.len())).min(range.end)
    }

    /// The offset at the start of a line.
    ///
    /// Clamping past the end to the document length keeps navigation code from
    /// having to special-case the final empty line.
    pub fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// The text inside a byte range.
    pub fn slice(&self, range: Range<usize>) -> &str {
        &self.text[range.start.min(self.text.len())..range.end.min(self.text.len())]
    }

    /// Apply one edit and record it for undo.
    ///
    /// `caret_before` and `caret_after` are the view's offsets; recording both
    /// lets undo put the caret back where the user left it rather than at the
    /// start of the restored text. The returned offset is the one to place the
    /// caret at after the edit.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        caret_after: usize,
        kind: EditKind,
    ) -> usize {
        let start = self.clamp_offset(range.start);
        let end = self.clamp_offset(range.end.max(start));
        let range = start..end;
        let removed = self.text[range.clone()].to_string();

        self.text.replace_range(range.clone(), text);
        let caret_after = self.clamp_offset(caret_after);
        self.dirty = true;
        self.redo.clear();

        let merged = self.merge_into_open_step(&range, &removed, text, caret_after, kind);
        if !merged {
            self.undo.push(UndoStep {
                range: range.start..range.start + text.len(),
                removed,
                inserted: text.to_string(),
                caret_before,
                caret_after,
                kind,
            });
        }

        self.open_group = match kind {
            EditKind::Other => None,
            kind => Some(kind),
        };

        caret_after
    }

    /// Fold a new edit into the previous undo step when the two are one gesture.
    ///
    /// Returns whether it merged. The conditions are deliberately narrow:
    /// typing must continue immediately after the previous insertion, and
    /// backspacing must continue at the boundary the last one left behind.
    fn merge_into_open_step(
        &mut self,
        range: &Range<usize>,
        removed: &str,
        inserted: &str,
        caret_after: usize,
        kind: EditKind,
    ) -> bool {
        if self.open_group != Some(kind) {
            return false;
        }
        let Some(last) = self.undo.last_mut() else {
            return false;
        };
        if last.kind != kind {
            return false;
        }

        match kind {
            EditKind::Typing => {
                if !removed.is_empty() || range.end != range.start {
                    return false;
                }
                // The previous step's range ends where this insertion starts.
                if last.range.end != range.start {
                    return false;
                }
                last.range.end = range.start + inserted.len();
                last.inserted.push_str(inserted);
                last.caret_after = caret_after;
                true
            }
            EditKind::Backspace => {
                if !inserted.is_empty() {
                    return false;
                }
                if range.end != last.range.start {
                    return false;
                }
                // A deletion's post-edit range is empty; only its start moves
                // back over the text the previous backspace removed.
                last.range = range.start..range.start;
                last.removed = format!("{removed}{}", last.removed);
                last.caret_after = caret_after;
                true
            }
            EditKind::Delete => {
                if !inserted.is_empty() {
                    return false;
                }
                if range.start != last.range.start {
                    return false;
                }
                // Forward deletes all happen at the same point, so the
                // post-edit range stays empty while `removed` grows.
                last.removed.push_str(removed);
                last.caret_after = caret_after;
                true
            }
            EditKind::Other => false,
        }
    }

    /// End the current undo group, so the next keystroke starts a new step.
    ///
    /// Navigation, a click, and any structural action call this. Without it a
    /// caret move in the middle of typing would fold the next word into the
    /// previous undo step.
    pub fn break_undo_group(&mut self) {
        self.open_group = None;
    }

    /// Undo the most recent step.
    pub fn undo(&mut self) -> Option<UndoOutcome> {
        let step = self.undo.pop()?;
        self.text.replace_range(step.range.clone(), &step.removed);
        let outcome = UndoOutcome {
            range: step.range.start..step.range.start + step.removed.len(),
            caret: step.caret_before,
        };
        self.redo.push(step);
        self.open_group = None;
        self.dirty = true;
        Some(outcome)
    }

    /// Redo the step that was last undone.
    pub fn redo(&mut self) -> Option<UndoOutcome> {
        let step = self.redo.pop()?;
        self.text.replace_range(step.range.clone(), &step.inserted);
        let outcome = UndoOutcome {
            range: step.range.start..step.range.start + step.inserted.len(),
            caret: step.caret_after,
        };
        self.undo.push(step);
        self.open_group = None;
        self.dirty = true;
        Some(outcome)
    }

    /// Whether there is anything to undo or redo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Write the document back over its file.
    ///
    /// A lossily decoded file is refused rather than written with replacement
    /// characters, which is the same rule the CSV grid follows.
    pub fn save(&mut self) -> Result<(), CoreError> {
        if self.lossy {
            return Err(CoreError::Lossy {
                path: self.path.clone(),
            });
        }

        let body = match self.line_ending {
            LineEnding::Lf => self.text.clone(),
            LineEnding::Crlf => self.text.replace('\n', "\r\n"),
        };
        let mut out = String::with_capacity(body.len() + 3);
        if self.bom {
            out.push('\u{feff}');
        }
        out.push_str(&body);

        std::fs::write(&self.path, out).map_err(|source| CoreError::Write {
            path: self.path.clone(),
            source,
        })?;
        self.dirty = false;
        Ok(())
    }
}

fn detect_line_ending(source: &str) -> LineEnding {
    match source.find('\n') {
        Some(index) if index > 0 && source.as_bytes()[index - 1] == b'\r' => LineEnding::Crlf,
        _ => LineEnding::Lf,
    }
}

fn normalize_line_endings(source: &str) -> String {
    if !source.contains('\r') {
        return source.to_string();
    }
    source.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(source: &str) -> TextDocument {
        TextDocument::from_str(Path::new("notes.txt"), source)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bromine-text-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        directory
    }

    #[test]
    fn a_roundtrip_preserves_the_text_and_leaves_it_clean() {
        let directory = temp_dir("roundtrip");
        let path = directory.join("notes.txt");
        std::fs::write(&path, "alpha\nbeta\n").expect("the file is written");

        let mut document = TextDocument::load(&path).expect("the file loads");
        assert_eq!(document.text(), "alpha\nbeta\n");
        assert!(!document.is_dirty());
        assert!(!document.is_lossy());

        document.replace(0..5, "ALPHA", 0, 5, EditKind::Typing);
        assert!(document.is_dirty());
        document.save().expect("the file saves");
        assert!(!document.is_dirty());
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file reads back"),
            "ALPHA\nbeta\n"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[test]
    fn a_bom_is_stripped_from_the_text_and_written_back() {
        let path = Path::new("bom.txt");
        let bytes = vec![0xEF, 0xBB, 0xBF, b'h', b'i'];
        let document = TextDocument::from_bytes(path, bytes).expect("the file loads");

        assert_eq!(document.text(), "hi");
        assert!(document.has_bom());
    }

    #[test]
    fn crlf_is_detected_normalised_and_restored_on_save() {
        let directory = temp_dir("crlf");
        let path = directory.join("crlf.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").expect("the file is written");

        let mut document = TextDocument::load(&path).expect("the file loads");
        assert_eq!(document.line_ending(), LineEnding::Crlf);
        assert_eq!(document.text(), "one\ntwo\n");
        assert_eq!(document.line_count(), 3);

        document.replace(0..3, "ONE", 0, 3, EditKind::Typing);
        document.save().expect("the file saves");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file reads back"),
            "ONE\r\ntwo\r\n"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[test]
    fn a_nul_byte_in_the_first_kilobytes_is_binary() {
        let error = TextDocument::from_bytes(Path::new("blob.bin"), vec![b'a', 0, b'b'])
            .expect_err("a NUL byte means binary");
        assert!(matches!(error, CoreError::Binary { .. }));
        assert!(error.to_string().contains("binary"));
    }

    #[test]
    fn invalid_utf8_loads_lossily_and_refuses_to_save() {
        let directory = temp_dir("lossy");
        let path = directory.join("latin1.txt");
        std::fs::write(&path, [b'c', b'a', b'f', 0xE9]).expect("the file is written");

        let mut document = TextDocument::load(&path).expect("the file loads");
        assert!(document.is_lossy());
        assert!(document.text().contains('\u{fffd}'));

        let error = document.save().expect_err("a lossy document cannot save");
        assert!(matches!(error, CoreError::Lossy { .. }));
        assert_eq!(
            std::fs::read(&path).expect("the file is readable"),
            [b'c', b'a', b'f', 0xE9],
            "the original bytes must be left alone"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[test]
    fn consecutive_typing_is_one_undo_step() {
        let mut document = document("");
        for (index, character) in "abc".chars().enumerate() {
            let caret = index;
            document.replace(
                caret..caret,
                &character.to_string(),
                caret,
                caret + 1,
                EditKind::Typing,
            );
        }
        assert_eq!(document.text(), "abc");

        let outcome = document.undo().expect("there is one step");
        assert_eq!(document.text(), "");
        assert_eq!(outcome.caret, 0);
        assert!(!document.can_undo());
    }

    #[test]
    fn navigation_breaks_the_typing_group() {
        let mut document = document("");
        document.replace(0..0, "a", 0, 1, EditKind::Typing);
        document.break_undo_group();
        document.replace(1..1, "b", 1, 2, EditKind::Typing);

        document.undo().expect("the second group is undone");
        assert_eq!(document.text(), "a");
        document.undo().expect("the first group is undone");
        assert_eq!(document.text(), "");
    }

    #[test]
    fn consecutive_backspaces_are_one_undo_step() {
        let mut document = document("abc");
        document.replace(2..3, "", 3, 2, EditKind::Backspace);
        document.replace(1..2, "", 2, 1, EditKind::Backspace);

        assert_eq!(document.text(), "a");
        let outcome = document.undo().expect("both deletes are one step");
        assert_eq!(document.text(), "abc");
        assert_eq!(outcome.caret, 3);
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut document = document("");
        document.replace(0..0, "a", 0, 1, EditKind::Typing);
        document.undo().expect("the edit is undone");
        assert!(document.can_redo());

        document.replace(0..0, "b", 0, 1, EditKind::Typing);
        assert!(!document.can_redo());
        assert_eq!(document.text(), "b");
    }

    #[test]
    fn undo_and_redo_restore_the_carets_they_recorded() {
        let mut document = document("abc");
        document.replace(1..2, "X", 2, 2, EditKind::Other);
        assert_eq!(document.text(), "aXc");

        let undone = document.undo().expect("the edit is undone");
        assert_eq!(document.text(), "abc");
        assert_eq!(undone.caret, 2);

        let redone = document.redo().expect("the edit is redone");
        assert_eq!(document.text(), "aXc");
        assert_eq!(redone.caret, 2);
    }

    #[test]
    fn lines_and_offsets_agree_at_the_end_of_the_file() {
        let document = document("one\ntwo\n");
        assert_eq!(document.line_count(), 3);
        assert_eq!(document.line_range(0), Some(0..3));
        assert_eq!(document.line_range(1), Some(4..7));
        assert_eq!(document.line_range(2), Some(8..8));
        assert_eq!(document.line_of(8), 2);
        assert_eq!(document.column_of(5), 1);
        assert_eq!(document.offset_of(1, 1), 5);
        assert_eq!(document.offset_of(9, 0), 8);
    }
}
