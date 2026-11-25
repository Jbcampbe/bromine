//! The editing foundation shared by the text, Markdown, and CSV views.
//!
//! [`EditorState`] is the caret and selection; everything that changes the
//! document lives in this module so every view behaves the same way for typing,
//! deletion, undo, and clipboard operations. The views add only the parts that
//! need their own layout: where a click lands, and how a wrapped line is found.

use std::ops::Range;

use bromine_core::{EditKind, TextDocument};
use gpui::{
    App, Bounds, ClipboardItem, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    IntoElement, Pixels, UTF16Selection, Window,
};
use unicode_segmentation::UnicodeSegmentation;

/// The caret, the selection anchor, and the small bits of state a text field
/// needs between keystrokes.
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    /// Where the caret is, as a source byte offset.
    pub caret: usize,
    /// The fixed end of the selection. Equal to `caret` when nothing is marked.
    pub anchor: usize,
    /// The range of an IME composition, underlined until it is committed.
    pub marked: Option<Range<usize>>,
    /// The x a series of Up and Down presses tries to keep, in pixels.
    pub desired_x: Option<f32>,
    /// Whether a drag selection is in progress.
    pub dragging: bool,
}

impl EditorState {
    /// Start with the caret at `offset`.
    pub fn new(offset: usize) -> Self {
        Self {
            caret: offset,
            anchor: offset,
            ..Self::default()
        }
    }

    /// The marked range, normalised.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    /// Move the caret and drop the selection.
    pub fn move_to(&mut self, offset: usize) {
        self.caret = offset;
        self.anchor = offset;
        self.desired_x = None;
    }

    /// Move the caret while keeping the anchor fixed.
    pub fn select_to(&mut self, offset: usize) {
        self.caret = offset;
        self.desired_x = None;
    }
}

/// The document operations the editor helpers need.
///
/// Implemented by both [`TextDocument`] and the Markdown model, so typing,
/// navigation, and clipboard handling have one implementation per behaviour
/// rather than one per view.
pub trait TextBuffer {
    fn buffer_text(&self) -> &str;
    fn buffer_len(&self) -> usize;
    fn buffer_slice(&self, range: Range<usize>) -> &str;
    fn buffer_replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        caret_after: usize,
        kind: EditKind,
    ) -> usize;
    fn buffer_undo(&mut self) -> Option<bromine_core::UndoOutcome>;
    fn buffer_redo(&mut self) -> Option<bromine_core::UndoOutcome>;
    fn buffer_line_of(&self, offset: usize) -> usize;
    fn buffer_line_start(&self, line: usize) -> Option<usize>;
    fn buffer_line_range(&self, line: usize) -> Option<Range<usize>>;
}

impl TextBuffer for TextDocument {
    fn buffer_text(&self) -> &str {
        self.text()
    }
    fn buffer_len(&self) -> usize {
        self.len()
    }
    fn buffer_slice(&self, range: Range<usize>) -> &str {
        self.slice(range)
    }
    fn buffer_replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        caret_after: usize,
        kind: EditKind,
    ) -> usize {
        self.replace(range, text, caret_before, caret_after, kind)
    }
    fn buffer_undo(&mut self) -> Option<bromine_core::UndoOutcome> {
        self.undo()
    }
    fn buffer_redo(&mut self) -> Option<bromine_core::UndoOutcome> {
        self.redo()
    }
    fn buffer_line_of(&self, offset: usize) -> usize {
        self.line_of(offset)
    }
    fn buffer_line_start(&self, line: usize) -> Option<usize> {
        self.line_start(line)
    }
    fn buffer_line_range(&self, line: usize) -> Option<Range<usize>> {
        self.line_range(line)
    }
}

impl TextBuffer for bromine_core::MarkdownDocument {
    fn buffer_text(&self) -> &str {
        self.source()
    }
    fn buffer_len(&self) -> usize {
        self.text().len()
    }
    fn buffer_slice(&self, range: Range<usize>) -> &str {
        self.text().slice(range)
    }
    fn buffer_replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        caret_after: usize,
        kind: EditKind,
    ) -> usize {
        self.replace(range, text, caret_before, caret_after, kind)
    }
    fn buffer_undo(&mut self) -> Option<bromine_core::UndoOutcome> {
        self.undo()
    }
    fn buffer_redo(&mut self) -> Option<bromine_core::UndoOutcome> {
        self.redo()
    }
    fn buffer_line_of(&self, offset: usize) -> usize {
        self.text().line_of(offset)
    }
    fn buffer_line_start(&self, line: usize) -> Option<usize> {
        self.text().line_start(line)
    }
    fn buffer_line_range(&self, line: usize) -> Option<Range<usize>> {
        self.text().line_range(line)
    }
}

/// Insert `text`, replacing the selection if there is one.
pub fn insert(document: &mut impl TextBuffer, editor: &mut EditorState, text: &str) -> usize {
    let selection = editor.selection();
    let before = editor.caret;
    let after = selection.start + text.len();
    let caret = document.buffer_replace(selection, text, before, after, EditKind::Typing);
    editor.marked = None;
    editor.move_to(caret);
    caret
}

/// Delete backwards: the selection, or the grapheme before the caret.
pub fn backspace(document: &mut impl TextBuffer, editor: &mut EditorState) {
    let selection = editor.selection();
    if !selection.is_empty() {
        let caret = selection.start;
        document.buffer_replace(selection, "", editor.caret, caret, EditKind::Backspace);
        editor.marked = None;
        editor.move_to(caret);
        return;
    }

    let caret = editor.caret;
    let previous = grapheme_before(document.buffer_text(), caret);
    if previous < caret {
        document.buffer_replace(previous..caret, "", caret, previous, EditKind::Backspace);
        editor.marked = None;
        editor.move_to(previous);
    }
}

/// Delete forwards: the selection, or the grapheme after the caret.
pub fn delete(document: &mut impl TextBuffer, editor: &mut EditorState) {
    let selection = editor.selection();
    if !selection.is_empty() {
        let caret = selection.start;
        document.buffer_replace(selection, "", editor.caret, caret, EditKind::Delete);
        editor.marked = None;
        editor.move_to(caret);
        return;
    }

    let caret = editor.caret;
    let next = grapheme_after(document.buffer_text(), caret);
    if next > caret {
        document.buffer_replace(caret..next, "", caret, caret, EditKind::Delete);
        editor.marked = None;
        editor.move_to(caret);
    }
}

/// Undo one step and put the caret back where the edit started.
pub fn undo(document: &mut impl TextBuffer, editor: &mut EditorState) -> bool {
    match document.buffer_undo() {
        Some(outcome) => {
            editor.marked = None;
            editor.move_to(outcome.caret.min(document.buffer_len()));
            true
        }
        None => false,
    }
}

/// Redo one step.
pub fn redo(document: &mut impl TextBuffer, editor: &mut EditorState) -> bool {
    match document.buffer_redo() {
        Some(outcome) => {
            editor.marked = None;
            editor.move_to(outcome.caret.min(document.buffer_len()));
            true
        }
        None => false,
    }
}

/// The selection, or the whole document when nothing is selected.
pub fn copy_text(document: &impl TextBuffer, editor: &EditorState) -> String {
    let selection = editor.selection();
    if selection.is_empty() {
        document.buffer_text().to_string()
    } else {
        document.buffer_slice(selection).to_string()
    }
}

/// Copy the selection to the clipboard.
pub fn copy(document: &impl TextBuffer, editor: &EditorState, cx: &mut App) {
    if editor.has_selection() {
        cx.write_to_clipboard(ClipboardItem::new_string(copy_text(document, editor)));
    }
}

/// Copy and delete the selection.
pub fn cut(document: &mut impl TextBuffer, editor: &mut EditorState, cx: &mut App) {
    if !editor.has_selection() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(copy_text(document, editor)));
    let selection = editor.selection();
    let caret = selection.start;
    document.buffer_replace(selection, "", editor.caret, caret, EditKind::Other);
    editor.marked = None;
    editor.move_to(caret);
}

/// Insert the clipboard's text over the selection.
pub fn paste(document: &mut impl TextBuffer, editor: &mut EditorState, cx: &mut App) {
    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
        insert(document, editor, &text);
    }
}

pub fn select_all(document: &impl TextBuffer, editor: &mut EditorState) {
    editor.anchor = 0;
    editor.caret = document.buffer_len();
}

/// Move one grapheme left or right.
pub fn left(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let selection = editor.selection();
    let target = if !extend && !selection.is_empty() {
        selection.start
    } else {
        grapheme_before(document.buffer_text(), editor.caret)
    };
    if extend {
        editor.select_to(target);
    } else {
        editor.move_to(target);
    }
}

pub fn right(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let selection = editor.selection();
    let target = if !extend && !selection.is_empty() {
        selection.end
    } else {
        grapheme_after(document.buffer_text(), editor.caret)
    };
    if extend {
        editor.select_to(target);
    } else {
        editor.move_to(target);
    }
}

pub fn word_left(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let target = word_before(document.buffer_text(), editor.caret);
    if extend {
        editor.select_to(target);
    } else {
        editor.move_to(target);
    }
}

pub fn word_right(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let target = word_after(document.buffer_text(), editor.caret);
    if extend {
        editor.select_to(target);
    } else {
        editor.move_to(target);
    }
}

pub fn home(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let start = document
        .buffer_line_start(document.buffer_line_of(editor.caret))
        .unwrap_or(0);
    if extend {
        editor.select_to(start);
    } else {
        editor.move_to(start);
    }
}

pub fn end(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let line = document.buffer_line_of(editor.caret);
    let end = document
        .buffer_line_range(line)
        .map(|range| range.end)
        .unwrap_or(0);
    if extend {
        editor.select_to(end);
    } else {
        editor.move_to(end);
    }
}

pub fn document_start(editor: &mut EditorState, extend: bool) {
    if extend {
        editor.select_to(0);
    } else {
        editor.move_to(0);
    }
}

pub fn document_end(document: &impl TextBuffer, editor: &mut EditorState, extend: bool) {
    let end = document.buffer_len();
    if extend {
        editor.select_to(end);
    } else {
        editor.move_to(end);
    }
}

/// The boundary of the grapheme before `offset`.
pub fn grapheme_before(text: &str, offset: usize) -> usize {
    let offset = clamp_boundary(text, offset);
    text.grapheme_indices(true)
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

/// The boundary after the grapheme at `offset`.
pub fn grapheme_after(text: &str, offset: usize) -> usize {
    let offset = clamp_boundary(text, offset);
    text.grapheme_indices(true)
        .find_map(|(index, grapheme)| {
            (index + grapheme.len() > offset).then_some(index + grapheme.len())
        })
        .unwrap_or(text.len())
}

/// The start of the word before the caret, skipping punctuation and space.
pub fn word_before(text: &str, offset: usize) -> usize {
    let offset = clamp_boundary(text, offset);
    let mut index = offset;
    let mut saw_word = false;
    for (position, character) in text[..offset].char_indices().rev() {
        let word = character.is_alphanumeric();
        if saw_word && !word {
            return index;
        }
        if word {
            saw_word = true;
            index = position;
        } else if saw_word {
            return index;
        } else {
            index = position;
        }
    }
    if saw_word { index } else { 0 }
}

/// The end of the word after the caret.
pub fn word_after(text: &str, offset: usize) -> usize {
    let offset = clamp_boundary(text, offset);
    let mut saw_word = false;
    for (position, character) in text[offset..].char_indices() {
        let word = character.is_alphanumeric();
        if saw_word && !word {
            return offset + position;
        }
        if word {
            saw_word = true;
        }
    }
    text.len()
}

fn clamp_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Convert a UTF-8 offset to UTF-16 units, which is what the platform speaks.
pub fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let offset = clamp_boundary(text, offset);
    text[..offset].encode_utf16().count()
}

/// Convert UTF-16 units to a UTF-8 byte offset.
pub fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for character in text.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += character.len_utf16();
        utf8 += character.len_utf8();
    }
    utf8
}

pub fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start)..offset_to_utf16(text, range.end)
}

pub fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    offset_from_utf16(text, range.start)..offset_from_utf16(text, range.end)
}

/// The platform input bridge. Call this during paint with the element's bounds.
///
/// GPUI 0.2.2 has no text-field widget; a view becomes a real editor by
/// implementing [`EntityInputHandler`] and handing itself to
/// `window.handle_input` while it paints. That is what makes IME, the character
/// palette, and the candidate window work.
pub struct TextInputBridge;

impl TextInputBridge {
    pub fn paint<V: EntityInputHandler + 'static>(
        view: Entity<V>,
        focus_handle: &FocusHandle,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(focus_handle, ElementInputHandler::new(bounds, view), cx);
    }
}

/// The custom element that makes a view a text input and paints nothing.
///
/// It exists because GPUI only installs the platform input handler while an
/// element paints. Every view that edits text (the plain-text view, the Markdown
/// view, and the CSV grid's cell editor) hangs one of these in its tree and
/// draws its own text.
pub struct InputBridgeElement<V: EntityInputHandler + 'static> {
    view: Entity<V>,
    focus_handle: FocusHandle,
}

impl<V: EntityInputHandler + 'static> InputBridgeElement<V> {
    pub fn new(view: Entity<V>, focus_handle: FocusHandle) -> Self {
        Self { view, focus_handle }
    }
}

impl<V: EntityInputHandler + 'static> IntoElement for InputBridgeElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<V: EntityInputHandler + 'static> gpui::Element for InputBridgeElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        TextInputBridge::paint(self.view.clone(), &self.focus_handle, bounds, window, cx);
    }
}

/// The selected range in the platform's UTF-16 terms.
pub fn utf16_selection(text: &str, editor: &EditorState) -> UTF16Selection {
    UTF16Selection {
        range: range_to_utf16(text, &editor.selection()),
        reversed: editor.caret < editor.anchor,
    }
}
