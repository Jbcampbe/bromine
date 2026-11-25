//! The CSV grid: a virtualized, sortable, searchable view over a parsed table.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bromine_core::{
    CaretMove, CsvDocument, Direction, EditKind, EditOutcome, RowView, SortKey, StepId, Table,
    TableEdits, UndoOutcome,
};
use gpui::{
    App, Bounds, ClipboardItem, Context, Div, ElementId, EntityInputHandler, FocusHandle, Font,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Stateful, StyledText, TextRun, TextSystem,
    TouchPhase, UTF16Selection, Window, div, font, point, prelude::*, px,
};

use crate::editor::{self, EditorState, TextBuffer};
use crate::text_actions::{
    Backspace, Copy, Cut, Delete, DocumentEnd, DocumentStart, Down, End, Enter, Home, Left,
    Outdent, Paste, Redo, Right, SelectAll, SelectEnd, SelectHome, SelectLeft, SelectRight,
    SelectWordLeft, SelectWordRight, Tab, Undo, Up, WordLeft, WordRight,
};

/// The axis a scroll gesture is allowed to move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

use crate::theme;

/// What the user has marked, in Excel's terms.
///
/// A row or a column is held as the thing that was clicked rather than as a
/// range of cells, because that is what copying and the row and column edits
/// act on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Selection {
    #[default]
    None,
    Cell {
        position: usize,
        column: usize,
    },
    Row {
        position: usize,
    },
    Column {
        column: usize,
    },
}

/// Row height in pixels.
///
/// `uniform_list` measures the first row and assumes every other row matches it,
/// so every row has to be exactly this tall.
pub const ROW_HEIGHT: f32 = 24.0;

/// The height of the column header row, in pixels.
const HEADER_HEIGHT: f32 = 26.0;

/// The font size of cell values, in pixels.
///
/// Column widths are measured from text, so the size has to be a number this
/// module owns rather than one inherited from the rem scale: measuring at one
/// size and drawing at another would size every column wrong.
const CELL_FONT_SIZE: f32 = 13.0;

/// Space around cell text: the horizontal padding, the cell border, and enough
/// slack for the sort indicator so a column does not have to grow when it
/// becomes the sorted one.
const CELL_PADDING: f32 = 30.0;

/// The narrowest a column may be, so a column of one-character values still has
/// a clickable header.
const MIN_COLUMN_WIDTH: f32 = 56.0;

/// The widest a column may be. One pathological value should not push every
/// other column off the screen.
const MAX_COLUMN_WIDTH: f32 = 420.0;

/// How thick the drawn scrollbars are, in pixels.
const SCROLLBAR_THICKNESS: f32 = 8.0;

/// The shortest a scrollbar thumb may get, so a file with a million rows still
/// has a handle that can be seen.
const MIN_SCROLLBAR_THUMB: f32 = 24.0;

/// The width of the row-number gutter, in pixels. Wide enough for six digits of
/// row number, which is further than any file this grid opens in one pass.
const GUTTER_WIDTH: f32 = 56.0;

/// The height of the strip of column letters above the header names.
const LETTERS_HEIGHT: f32 = 20.0;

/// A virtualized grid over a delimited file.
///
/// The horizontal padding a cell gives its text, in pixels.
///
/// The cell div carries `px_2`; the number is named here because the caret
/// maths has to agree with the padding exactly.
const CELL_PADDING_X: f32 = 8.0;

/// An open cell edit: which cell, where the caret is, and where the history
/// stood when the session began.
struct CellSession {
    /// The absolute table row, not the view position: sorting does not move it.
    row: usize,
    column: usize,
    editor: EditorState,
    /// The history head when the session opened, so cancelling can discard
    /// exactly the steps this session added.
    opened_at: Option<StepId>,
}

/// The cell being edited, borrowed from the table so the shared editor helpers
/// (typing, deletion, navigation, clipboard) work on a cell unchanged.
///
/// The text is the table cell itself rather than a view-side copy, so a save can
/// never write something other than what is on screen and `cell_at` in the tests
/// reads the live value.
struct CellBuffer<'a> {
    table: &'a mut Table,
    edits: &'a mut TableEdits,
    row: usize,
    column: usize,
}

impl CellBuffer<'_> {
    fn text(&self) -> &str {
        self.table.get(self.row, self.column).unwrap_or_default()
    }
}

impl editor::TextBuffer for CellBuffer<'_> {
    fn buffer_text(&self) -> &str {
        self.text()
    }

    fn buffer_len(&self) -> usize {
        self.text().len()
    }

    fn buffer_slice(&self, range: Range<usize>) -> &str {
        let range = clamp_range(self.text(), range);
        &self.text()[range]
    }

    fn buffer_replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        _caret_after: usize,
        kind: EditKind,
    ) -> usize {
        let current = self.text().to_string();
        let range = clamp_range(&current, range);
        let mut next = current;
        next.replace_range(range.clone(), text);
        let caret = (range.start + text.len()).min(next.len());
        // A session only opens on a cell that exists and structural edits commit
        // before they run, so this targets a live coordinate. A refusal would
        // mean the row went away under the editor, and then there is no cell
        // left to place a caret in.
        let _ = self.edits.set_cell(
            self.table,
            self.row,
            self.column,
            &next,
            CaretMove {
                before: caret_before,
                after: caret,
            },
            kind,
        );
        caret
    }

    fn buffer_undo(&mut self) -> Option<UndoOutcome> {
        let outcome = self.edits.undo_cell(self.table, self.row, self.column)?;
        Some(UndoOutcome {
            range: 0..self.text().len(),
            caret: outcome.caret.unwrap_or(0),
        })
    }

    fn buffer_redo(&mut self) -> Option<UndoOutcome> {
        let outcome = self.edits.redo_cell(self.table, self.row, self.column)?;
        Some(UndoOutcome {
            range: 0..self.text().len(),
            caret: outcome.caret.unwrap_or(0),
        })
    }

    // A cell is one line, so Home and End fall out of these for free.
    fn buffer_line_of(&self, _offset: usize) -> usize {
        0
    }

    fn buffer_line_start(&self, _line: usize) -> Option<usize> {
        Some(0)
    }

    fn buffer_line_range(&self, line: usize) -> Option<Range<usize>> {
        (line == 0).then(|| 0..self.text().len())
    }
}

/// Measures cell text the way the grid draws it: one character at a time in the
/// monospace cell font.
///
/// Resolving the font once and reusing the id keeps a per-frame caret
/// measurement cheap, which is the only measurement an edit session adds.
struct CellMetrics<'a> {
    text_system: &'a TextSystem,
    font_id: gpui::FontId,
}

impl<'a> CellMetrics<'a> {
    fn new(text_system: &'a TextSystem) -> Self {
        let font_id = text_system.resolve_font(&font(theme::monospace()));
        Self {
            text_system,
            font_id,
        }
    }

    fn advance(&self, ch: char) -> f32 {
        let ch = if ch.is_control() { ' ' } else { ch };
        self.text_system
            .advance(self.font_id, px(CELL_FONT_SIZE), ch)
            .map(|size| f32::from(size.width))
            .unwrap_or(0.0)
    }

    fn prefix_width(&self, text: &str) -> f32 {
        text.chars().map(|ch| self.advance(ch)).sum()
    }

    /// The byte offset nearest the horizontal position `x` inside `text`.
    fn offset_for_x(&self, text: &str, x: f32) -> usize {
        let mut edge = 0.0;
        for (index, ch) in text.char_indices() {
            let width = self.advance(ch);
            if x < edge + width / 2.0 {
                return index;
            }
            edge += width;
        }
        text.len()
    }
}

/// The nearest byte boundary at or before `offset`.
fn clamp_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// A byte range clamped to the text and to character boundaries.
fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = clamp_boundary(text, range.start);
    let end = clamp_boundary(text, range.end.max(start));
    start..end
}

/// The grid owns the parsed document plus a [`RowView`] that decides which rows
/// are visible and in what order. Nothing here reorders the table itself, so the
/// row drawn at a given position can always be traced back to a stable table
/// index, which is what a later edit has to target.
pub struct CsvGrid {
    path: PathBuf,
    document: CsvDocument,
    view: RowView,
    /// One width per column, measured once at load from the widest value the
    /// column has to show. Measured during render it would be recomputed on
    /// every frame.
    column_widths: Vec<f32>,
    /// Where the header is scrolled to horizontally. The header only moves on
    /// that axis, so it tracks its own handle.
    header_scroll: ScrollHandle,
    /// The axis the gesture in progress picked. A two-finger swipe keeps the
    /// axis it started on, so drift halfway through cannot turn a vertical
    /// scroll into a diagonal one.
    gesture_axis: Option<Axis>,
    /// Where the rows are scrolled to, on both axes.
    ///
    /// `ScrollHandle` is used purely as storage here: nothing in this view is a
    /// native scroll container, which is what lets one wheel handler decide
    /// which axis a gesture moves.
    body_scroll: ScrollHandle,
    /// Where the row numbers are scrolled to. They follow the rows vertically
    /// and never move sideways, so they keep answering "which row is this"
    /// while the table is panned.
    gutter_scroll: ScrollHandle,
    /// What the user has marked. Copying and the row and column edits read it.
    selection: Selection,
    /// Undo for every edit this grid has made, cell or structural.
    ///
    /// Dirtiness is derived from its head rather than tracked as a flag, so
    /// undoing back to the saved state clears the marker and a new edit after an
    /// undo sets it again even when the number of steps looks the same.
    edits: TableEdits,
    /// The history head that was last written to disk.
    saved_head: Option<StepId>,
    /// The open cell edit, if any.
    session: Option<CellSession>,
    /// Why the last keystroke or paste was refused, shown in the status strip.
    notice: Option<SharedString>,
    /// Why the last save did not happen, shown in the status strip.
    save_error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl CsvGrid {
    /// Build a grid over a document that has already been parsed.
    pub fn new(path: PathBuf, document: CsvDocument, cx: &mut Context<Self>) -> Self {
        let view = RowView::new(&document.table);
        let column_widths = measure_columns(&document, cx.text_system());
        Self {
            path,
            document,
            view,
            column_widths,
            header_scroll: ScrollHandle::new(),
            gesture_axis: None,
            body_scroll: ScrollHandle::new(),
            gutter_scroll: ScrollHandle::new(),
            selection: Selection::None,
            edits: TableEdits::default(),
            saved_head: None,
            session: None,
            notice: None,
            save_error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The file this document came from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The handle that makes the grid the target of key events.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Whether the table has edits that are not on disk yet.
    pub fn is_dirty(&self) -> bool {
        self.edits.head() != self.saved_head
    }

    /// How many rows survive the current search.
    pub fn visible_row_count(&self) -> usize {
        self.view.len()
    }

    /// How many rows the file has, ignoring the search.
    pub fn total_row_count(&self) -> usize {
        self.document.table.row_count()
    }

    /// How many columns the file has.
    pub fn column_count(&self) -> usize {
        self.document.table.column_count()
    }

    /// The current search text.
    pub fn query(&self) -> &str {
        self.view.query()
    }

    /// The current sort, or `None` for file order.
    pub fn sort(&self) -> Option<SortKey> {
        self.view.sort()
    }

    /// The table row drawn at `position`, which is the index an edit would use.
    pub fn table_row_at(&self, position: usize) -> Option<usize> {
        self.view.table_row(position)
    }

    /// The text drawn at `position` in `column`.
    pub fn cell_at(&self, position: usize, column: usize) -> Option<&str> {
        let row = self.view.table_row(position)?;
        self.document.table.get(row, column)
    }

    /// Give the grid keyboard focus.
    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    /// Whether a cell edit is open.
    pub fn is_editing(&self) -> bool {
        self.session.is_some()
    }

    /// The cell the open session is editing, as a table coordinate.
    fn session_cell(&self) -> Option<(usize, usize)> {
        self.session
            .as_ref()
            .map(|session| (session.row, session.column))
    }

    /// The value a cell holds, or "" when the row or column is gone.
    fn cell_text(&self, row: usize, column: usize) -> &str {
        self.document.table.get(row, column).unwrap_or_default()
    }

    /// The window x where a cell's text begins.
    fn cell_screen_left(&self, column: usize) -> f32 {
        let viewport = f32::from(self.body_scroll.bounds().origin.x);
        let scroll = f32::from(self.body_scroll.offset().x);
        let widths: f32 = self.column_widths.iter().take(column).sum();
        viewport + scroll + widths + CELL_PADDING_X
    }

    /// Start editing the cell at `position`.
    ///
    /// A click names the caret with `caret_x`; Enter puts it at the end, which
    /// is where someone who wants to append expects it.
    fn start_editing(
        &mut self,
        position: usize,
        column: usize,
        caret_x: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.view.table_row(position) else {
            return;
        };
        let Some(text) = self.document.table.get(row, column) else {
            return;
        };
        if self.column_widths.get(column).is_none() {
            return;
        }

        let caret = match caret_x {
            Some(x) => {
                let metrics = CellMetrics::new(cx.text_system());
                metrics.offset_for_x(text, f32::from(x) - self.cell_screen_left(column))
            }
            None => text.len(),
        };

        self.focus(window);
        // A session is a new undo group: nothing typed in it may merge into a
        // step from before, or cancelling could not discard exactly its work.
        self.edits.break_group();
        self.session = Some(CellSession {
            row,
            column,
            editor: EditorState::new(caret),
            opened_at: self.edits.head(),
        });
        self.selection = Selection::Cell { position, column };
        self.notice = None;
        self.save_error = None;
        cx.notify();
    }

    /// Close the open session, keeping what was typed.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.edits.break_group();

        // Only the committed column is measured again: a column that resized
        // as you typed would make the table jump under the caret.
        if let Some(width) = self.column_widths.get_mut(session.column) {
            *width = measure_column(&self.document.table, session.column, cx.text_system());
        }
        // The committed value can change which rows match the filter and where
        // they sort, so the view is rebuilt here, never per keystroke.
        self.view.rebuild(&self.document.table);
        self.selection = if session.column < self.column_count() {
            self.view
                .position_of(session.row)
                .map(|position| Selection::Cell {
                    position,
                    column: session.column,
                })
                .unwrap_or(Selection::None)
        } else {
            Selection::None
        };
        self.notice = None;
        self.save_error = None;
        cx.notify();
    }

    /// Close the open session, putting the cell back the way it was.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.edits
            .discard_since(&mut self.document.table, session.opened_at);
        self.view.rebuild(&self.document.table);
        self.selection = if session.column < self.column_count() {
            self.view
                .position_of(session.row)
                .map(|position| Selection::Cell {
                    position,
                    column: session.column,
                })
                .unwrap_or(Selection::None)
        } else {
            Selection::None
        };
        self.notice = None;
        cx.notify();
    }

    /// Close the session and move the mark, the way committing with Enter, Tab,
    /// Shift+Tab, or a vertical arrow does.
    fn finish_and_move(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        self.commit(cx);
        // A commit can hide the row behind the filter, and then there is no mark
        // to move and no lie worth telling about where it went.
        if matches!(self.selection, Selection::Cell { .. }) {
            self.move_selection(dx, dy, cx);
        }
    }

    /// Run one of the shared editor operations against the open cell.
    fn edit_buffer(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CellBuffer<'_>, &mut EditorState, &mut App),
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let (row, column) = (session.row, session.column);
        let mut buffer = CellBuffer {
            table: &mut self.document.table,
            edits: &mut self.edits,
            row,
            column,
        };
        change(&mut buffer, &mut session.editor, cx);
        self.after_keystroke(cx);
        true
    }

    /// Redraw after a keystroke and clear the notice the last refusal left.
    fn after_keystroke(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        cx.notify();
    }

    /// Refuse text a single-line cell cannot show faithfully.
    ///
    /// Stripping the characters instead would quietly write different bytes
    /// than the user pasted, so the whole paste is refused and explained.
    fn refuse_control(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if !text.contains('\n') && !text.contains('\r') && !text.contains('\t') {
            return false;
        }
        self.notice = Some(SharedString::from(
            "a cell cannot hold a line break or a tab",
        ));
        cx.notify();
        true
    }

    /// Whether the cell at this position and column is inside the mark.
    fn cell_is_selected(&self, position: usize, column: usize) -> bool {
        match self.selection {
            Selection::None => false,
            Selection::Cell {
                position: marked,
                column: marked_column,
            } => marked == position && marked_column == column,
            Selection::Row { position: marked } => marked == position,
            Selection::Column { column: marked } => marked == column,
        }
    }

    /// Whether this is the one cell that was clicked, rather than a cell that
    /// merely sits inside a marked row or column. The two get different washes
    /// so the anchor of a highlighted row stays visible.
    fn cell_is_focused(&self, position: usize, column: usize) -> bool {
        matches!(
            self.selection,
            Selection::Cell {
                position: marked,
                column: marked_column,
            } if marked == position && marked_column == column
        )
    }

    /// Mark the row a row number was clicked for.
    fn mark_row(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window);
        self.selection = Selection::Row { position };
        cx.notify();
    }

    /// Mark the column a letter was clicked for.
    fn mark_column(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window);
        self.selection = Selection::Column { column };
        cx.notify();
    }

    /// Mark the cell under a click, using the click's window x.
    ///
    /// The row owns this rather than each cell: a cell is a plain div, and a few
    /// hundred extra hitboxes per frame cost more than walking a few widths.
    fn select_cell_at(&mut self, position: usize, x: Pixels, cx: &mut Context<Self>) {
        let Some(column) = self.column_at(x) else {
            return;
        };
        self.selection = Selection::Cell { position, column };
        cx.notify();
    }

    /// The column whose band contains a window x, if any.
    fn column_at(&self, x: Pixels) -> Option<usize> {
        let left = f32::from(self.body_scroll.bounds().origin.x);
        let offset = f32::from(self.body_scroll.offset().x);
        let target = f32::from(x) - left - offset;
        let mut edge = 0.0;

        for (column, width) in self.column_widths.iter().enumerate() {
            let next = edge + width;
            if target < next {
                return Some(column);
            }
            edge = next;
        }

        None
    }

    // The editing actions below are only reachable while a session is open: the
    // shared bindings live in the "TextEditor" context, which the grid joins for
    // exactly as long as a cell is being edited.

    fn act_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_buffer(cx, |buffer, editor, _| editor::backspace(buffer, editor));
    }

    fn act_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_buffer(cx, |buffer, editor, _| editor::delete(buffer, editor));
    }

    fn act_left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        // Navigation ends the undo group, so the next keystroke is its own step.
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::left(buffer, editor, false));
    }

    fn act_right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::right(buffer, editor, false));
    }

    fn act_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::left(buffer, editor, true));
    }

    fn act_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::right(buffer, editor, true));
    }

    fn act_word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::word_left(buffer, editor, false)
        });
    }

    fn act_word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::word_right(buffer, editor, false)
        });
    }

    fn act_select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::word_left(buffer, editor, true)
        });
    }

    fn act_select_word_right(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::word_right(buffer, editor, true)
        });
    }

    fn act_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::home(buffer, editor, false));
    }

    fn act_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::end(buffer, editor, false));
    }

    fn act_select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::home(buffer, editor, true));
    }

    fn act_select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::end(buffer, editor, true));
    }

    fn act_document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |_buffer, editor, _| {
            editor::document_start(editor, false)
        });
    }

    fn act_document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::document_end(buffer, editor, false)
        });
    }

    fn act_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.edits.break_group();
        self.edit_buffer(cx, |buffer, editor, _| editor::select_all(buffer, editor));
    }

    /// Copy selects the caret's selection, or the whole cell when there is none.
    ///
    /// An empty selection falling back to the value is the grid's own promise
    /// ("Cmd+C copies the mark as text"), and while editing the mark is this
    /// cell, so the two rules cannot disagree.
    fn act_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, column)) = self.session_cell() else {
            return;
        };
        let value = self.cell_text(row, column).to_string();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let copied = if session.editor.has_selection() {
            let range = clamp_range(&value, session.editor.selection());
            value[range].to_string()
        } else {
            value
        };
        cx.write_to_clipboard(ClipboardItem::new_string(copied));
    }

    fn act_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_buffer(cx, |buffer, editor, cx| editor::cut(buffer, editor, cx));
    }

    fn act_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if self.refuse_control(&text, cx) {
            return;
        }
        self.edit_buffer(cx, |buffer, editor, _| {
            editor::insert(buffer, editor, &text);
        });
    }

    fn act_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, column)) = self.session_cell() else {
            return;
        };
        // Undo inside a session stops at the step it opened on: undoing past it
        // is work the user cannot see behind the caret.
        let opened_at = self.session.as_ref().and_then(|session| session.opened_at);
        if opened_at == self.edits.head() {
            return;
        }
        let Some(outcome) = self.edits.undo_cell(&mut self.document.table, row, column) else {
            return;
        };
        let caret = outcome
            .caret
            .unwrap_or(0)
            .min(self.cell_text(row, column).len());
        if let Some(session) = self.session.as_mut() {
            session.editor.marked = None;
            session.editor.move_to(caret);
        }
        self.after_keystroke(cx);
    }

    fn act_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, column)) = self.session_cell() else {
            return;
        };
        let Some(outcome) = self.edits.redo_cell(&mut self.document.table, row, column) else {
            return;
        };
        let caret = outcome
            .caret
            .unwrap_or(0)
            .min(self.cell_text(row, column).len());
        if let Some(session) = self.session.as_mut() {
            session.editor.marked = None;
            session.editor.move_to(caret);
        }
        self.after_keystroke(cx);
    }

    fn act_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_and_move(0, 1, cx);
    }

    fn act_tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_and_move(1, 0, cx);
    }

    fn act_outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_and_move(-1, 0, cx);
    }

    fn act_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_and_move(0, -1, cx);
    }

    fn act_down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_and_move(0, 1, cx);
    }

    /// Move the mark one cell, turning any other kind of mark into a cell.
    fn move_selection(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let rows = self.view.len();
        let columns = self.column_count();
        if rows == 0 || columns == 0 {
            return;
        }

        let (position, column) = match self.selection {
            Selection::Cell { position, column } => (position, column),
            Selection::Row { position } => (position, 0),
            Selection::Column { column } => (0, column),
            Selection::None => (0, 0),
        };

        self.selection = Selection::Cell {
            position: (position as isize + dy).clamp(0, rows as isize - 1) as usize,
            column: (column as isize + dx).clamp(0, columns as isize - 1) as usize,
        };
        cx.notify();
    }

    /// Copy the marked cell, row, or column to the clipboard.
    ///
    /// A row goes as tab-separated values and a column as one value per line,
    /// which is what a spreadsheet or another editor pastes back into shape.
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.selection_text() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// The mark as text, or `None` when nothing is marked.
    fn selection_text(&self) -> Option<String> {
        match self.selection {
            Selection::None => None,
            Selection::Cell { position, column } => {
                self.cell_at(position, column).map(str::to_string)
            }
            Selection::Row { position } => {
                let row = self.view.table_row(position)?;
                let cells = (0..self.column_count())
                    .map(|column| self.document.table.get(row, column).unwrap_or_default())
                    .collect::<Vec<_>>();
                Some(cells.join("\t"))
            }
            Selection::Column { column } => {
                let values = (0..self.view.len())
                    .filter_map(|position| self.cell_at(position, column))
                    .collect::<Vec<_>>();
                Some(values.join("\n"))
            }
        }
    }

    /// Write the table back over the file it came from.
    ///
    /// This is the tab's save entry point, called by the workspace's `Save`
    /// action. The grid no longer listens for Cmd+S itself: actions run before
    /// key listeners, so one global binding is the honest place for it. An open
    /// cell is committed first, because a save has to write what is on screen.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);

        if self.document.lossy {
            self.save_error = Some(SharedString::from(
                "not saved: this file is not valid UTF-8, so writing it back would lose data",
            ));
            cx.notify();
            return;
        }

        match self.document.save(&self.path) {
            Ok(()) => {
                self.saved_head = self.edits.head();
                self.save_error = None;
            }
            Err(error) => self.save_error = Some(SharedString::from(error.to_string())),
        }

        cx.notify();
    }

    /// Insert an empty row below the marked one, or at the end of the file.
    fn insert_row(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);

        let at = match self.selection {
            Selection::Cell { position, .. } | Selection::Row { position } => self
                .view
                .table_row(position + 1)
                .unwrap_or_else(|| self.total_row_count()),
            _ => self.total_row_count(),
        };

        self.edits.insert_row(&mut self.document.table, at);
        self.after_table_change(cx);
    }

    /// Remove the marked row, if one is marked.
    fn delete_row(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);

        let position = match self.selection {
            Selection::Cell { position, .. } | Selection::Row { position } => position,
            _ => return,
        };
        let Some(row) = self.view.table_row(position) else {
            return;
        };

        if self.edits.remove_row(&mut self.document.table, row).is_ok() {
            self.after_table_change(cx);
        }
    }

    /// Insert an empty column right of the marked one, or at the right edge.
    fn insert_column(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);

        let index = match self.selection {
            Selection::Cell { column, .. } | Selection::Column { column } => column + 1,
            _ => self.column_count(),
        };

        self.edits.insert_column(
            &mut self.document.table,
            index,
            format!("column {}", index + 1),
        );
        self.after_table_change(cx);
    }

    /// Remove the marked column, if one is marked.
    fn delete_column(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);

        let column = match self.selection {
            Selection::Cell { column, .. } | Selection::Column { column } => column,
            _ => return,
        };

        if self
            .edits
            .remove_column(&mut self.document.table, column)
            .is_ok()
        {
            self.after_table_change(cx);
        }
    }

    /// Rebuild everything a structural edit has to keep in step.
    ///
    /// Widths are measured again because a new column has none and a removed one
    /// must not leave its width behind. The mark is dropped because positions
    /// and column indices no longer mean what they meant before the edit.
    /// Dirtiness is not touched here: it is derived from the history head, so an
    /// undo back to the saved state says "saved" again.
    fn after_table_change(&mut self, cx: &mut Context<Self>) {
        self.view.rebuild(&self.document.table);
        self.column_widths = measure_columns(&self.document, cx.text_system());
        self.selection = Selection::None;
        self.notice = None;
        self.save_error = None;
        cx.notify();
    }

    /// Undo the last edit, cell or structural.
    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(outcome) = self.edits.undo(&mut self.document.table) else {
            return;
        };
        self.after_history_move(outcome, cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(outcome) = self.edits.redo(&mut self.document.table) else {
            return;
        };
        self.after_history_move(outcome, cx);
    }

    /// Put the mark on whatever an undo or redo restored.
    fn after_history_move(&mut self, outcome: EditOutcome, cx: &mut Context<Self>) {
        self.view.rebuild(&self.document.table);
        self.column_widths = measure_columns(&self.document, cx.text_system());
        self.selection = self.mark_for(outcome);
        self.notice = None;
        self.save_error = None;
        cx.notify();
    }

    /// The mark that names the row or column a step restored, if it still shows.
    ///
    /// A row the filter hides has no position, and pretending otherwise would
    /// put the mark on whichever row happened to move into its place.
    fn mark_for(&self, outcome: EditOutcome) -> Selection {
        match (outcome.row, outcome.column) {
            (Some(row), Some(column)) if column < self.column_count() => self
                .view
                .position_of(row)
                .map(|position| Selection::Cell { position, column })
                .unwrap_or(Selection::None),
            (Some(row), None) => self
                .view
                .position_of(row)
                .map(|position| Selection::Row { position })
                .unwrap_or(Selection::None),
            (None, Some(column)) if column < self.column_count() => Selection::Column { column },
            _ => Selection::None,
        }
    }

    /// Sort by `column`, or reverse the sort if it is already sorted by it.
    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        self.commit(cx);
        self.view.toggle_sort(&self.document.table, column);
        // Sorting moves rows, so a mark on a position would point at a different
        // row than it did before the click.
        self.selection = Selection::None;
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.commit(cx);
        self.view.set_query(&self.document.table, query);
        // Filtering hides rows, so a marked position may no longer exist.
        self.selection = Selection::None;
        cx.notify();
    }

    fn append_to_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let mut query = self.view.query().to_string();
        query.push_str(text);
        self.set_query(query, cx);
    }

    fn delete_last_query_char(&mut self, cx: &mut Context<Self>) {
        let mut query = self.view.query().to_string();
        query.pop();
        self.set_query(query, cx);
    }

    fn clear_query(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);
        self.view.clear_query(&self.document.table);
        cx.notify();
    }

    /// The unbound keys: search while nothing is being edited, and the session's
    /// own cancel while something is.
    ///
    /// Every key a cell editor needs is a `TextEditor` binding, and actions run
    /// before key listeners, so almost nothing reaches here with a session open.
    /// Escape is the exception, because nothing binds it.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();

        // An open cell owns the keyboard. Returning here is not tidiness: the
        // last arm below appends any character to the search query, and the
        // platform input handler delivers the same character to the cell, so a
        // keystroke that fell through would be typed twice.
        if self.session.is_some() {
            if key == "escape" && !event.keystroke.modifiers.platform {
                self.cancel(cx);
            }
            return;
        }

        // The grid's own shortcuts, plus undo and redo. Cmd+Z is bound only in
        // the editor context, so structural undo is reached from here when no
        // cell is open.
        if event.keystroke.modifiers.platform {
            match key {
                "c" => self.copy_selection(cx),
                "z" if event.keystroke.modifiers.shift => self.redo(cx),
                "z" => self.undo(cx),
                _ => {}
            }
            return;
        }

        // Leave control shortcuts to the platform and to whatever else is bound.
        if event.keystroke.modifiers.control {
            return;
        }

        match key {
            "backspace" => self.delete_last_query_char(cx),
            "escape" => {
                self.selection = Selection::None;
                self.clear_query(cx);
            }
            "enter" => {
                if let Selection::Cell { position, column } = self.selection {
                    self.start_editing(position, column, None, window, cx);
                }
            }
            "up" => self.move_selection(0, -1, cx),
            "down" => self.move_selection(0, 1, cx),
            "left" => self.move_selection(-1, 0, cx),
            "right" => self.move_selection(1, 0, cx),
            _ => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.is_empty()
                {
                    self.append_to_query(text, cx);
                }
            }
        }
    }

    /// The coordinate of the cell being edited, e.g. `D7`.
    ///
    /// `None` when no session is open or its row is hidden by the filter, in
    /// which case there is no coordinate an honest strip could show.
    fn editing_cell_label(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let position = self.view.position_of(session.row)?;
        Some(format!("{}{}", column_label(session.column), position + 1))
    }

    /// Anything the user should know about how the file was read.
    ///
    /// The grid must not quietly present repaired or lossily decoded data as if
    /// it were exactly what is on disk.
    fn repairs_notice(&self) -> Option<String> {
        let repairs = self.document.repairs;
        let mut parts = Vec::new();

        if repairs.unnamed_columns > 0 {
            parts.push(format!(
                "named {} unnamed column{}",
                repairs.unnamed_columns,
                plural(repairs.unnamed_columns)
            ));
        }
        if repairs.padded_rows > 0 {
            parts.push(format!(
                "padded {} short row{}",
                repairs.padded_rows,
                plural(repairs.padded_rows)
            ));
        }
        if self.document.lossy {
            parts.push(
                "decoded a file that is not valid UTF-8, so some characters are replacements"
                    .to_string(),
            );
        }

        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// The counts, the row and column edits, and the repair notice, along the
    /// bottom of the grid.
    ///
    /// The file name is not repeated here: the tab above the grid carries it.
    fn status_bar(&self, cx: &Context<Self>) -> Div {
        let total = self.total_row_count();
        let columns = self.column_count();
        let dirty = self.is_dirty();
        let counts = if self.view.len() == total {
            format!("{total} rows, {columns} columns")
        } else {
            format!("{} of {total} rows, {columns} columns", self.view.len())
        };

        let mut bar = div()
            .debug_selector(|| "status-bar".to_string())
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(SharedString::from(counts)),
            );

        // While a cell is open the value being edited tells the reader nothing
        // about where they are, so the strip names the cell the way a
        // spreadsheet's name box does.
        if let Some(cell) = self.editing_cell_label() {
            bar = bar.child(
                div()
                    .debug_selector(|| "editing-cell".to_string())
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(SharedString::from(cell)),
            );
        }

        let mut bar = bar
            // The counts stay left and the state stays right however wide the
            // window is.
            .child(div().flex_1())
            .child(self.edit_button("insert-row", "+ row", Self::insert_row, cx))
            .child(self.edit_button("delete-row", "- row", Self::delete_row, cx))
            .child(self.edit_button("insert-column", "+ col", Self::insert_column, cx))
            .child(self.edit_button("delete-column", "- col", Self::delete_column, cx));

        if let Some(notice) = self.repairs_notice() {
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(theme::warning())
                    .child(SharedString::from(notice)),
            );
        }

        if let Some(notice) = self.notice.clone() {
            bar = bar.child(div().text_xs().text_color(theme::warning()).child(notice));
        }

        if let Some(error) = self.save_error.clone() {
            bar = bar.child(div().text_xs().text_color(theme::warning()).child(error));
        }

        bar.child(
            div()
                .text_xs()
                .text_color(if dirty {
                    theme::warning()
                } else {
                    theme::text_muted()
                })
                .child(SharedString::from(if dirty {
                    "unsaved changes"
                } else {
                    "saved"
                })),
        )
    }

    /// One of the row and column buttons in the status strip.
    fn edit_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || format!("edit-{id}"))
            .px_2()
            .cursor_pointer()
            .text_xs()
            .text_color(theme::accent())
            .child(label)
            .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx)))
    }

    fn search_bar(&self, cx: &Context<Self>) -> Div {
        let query = self.view.query();
        let searching = !query.is_empty();
        let matches = self.view.len();

        let count = if searching {
            format!("{matches} of {} rows", self.total_row_count())
        } else {
            format!("{} rows", self.total_row_count())
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("search"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(if searching {
                        theme::text()
                    } else {
                        theme::text_muted()
                    })
                    .child(SharedString::from(if searching {
                        query.to_string()
                    } else {
                        "type to filter rows".to_string()
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(SharedString::from(count)),
            )
            .when(searching, |el| {
                el.child(
                    div()
                        .id("clear-search")
                        .px_2()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(theme::accent())
                        .child("clear")
                        .on_click(cx.listener(|this, _event, _window, cx| this.clear_query(cx))),
                )
            })
    }

    /// The full width of the widest row.
    fn total_width(&self) -> f32 {
        self.column_widths.iter().sum()
    }

    fn body(&self, cx: &Context<Self>) -> Stateful<Div> {
        if self.column_count() == 0 {
            return div()
                .id("grid-body")
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_color(theme::text_muted())
                        .child("this file has no columns"),
                );
        }

        // One wheel handler covers the whole grid, and nothing below it scrolls
        // natively, so exactly one thing decides which axis a gesture moves.
        div()
            .id("grid-body")
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.letters_strip(cx))
            .child(self.names_strip(cx))
            .child(self.gutter_viewport(cx))
            .child(self.rows_viewport(cx))
            .children(self.scrollbars())
    }

    /// The Excel-style letters above the header names.
    ///
    /// They mark a column; the names below sort by one. Both strips track the
    /// same handle, so they cannot disagree about how far the table is panned.
    fn letters_strip(&self, cx: &Context<Self>) -> Stateful<Div> {
        let mut cells = Vec::with_capacity(self.column_widths.len());
        for (column, width) in self.column_widths.iter().enumerate() {
            cells.push(self.letter_cell(column, *width, cx));
        }

        div()
            .id("grid-letters-scroll")
            .debug_selector(|| "grid-letters".to_string())
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(LETTERS_HEIGHT))
            .overflow_hidden()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border_subtle())
            .track_scroll(&self.header_scroll)
            .child(
                div()
                    .w(px(self.total_width() + GUTTER_WIDTH))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.corner(LETTERS_HEIGHT))
                    .children(cells),
            )
    }

    /// The column names, clipped and moved sideways with the rows.
    ///
    /// A tracked handle on a hidden-overflow element clips and translates its
    /// children without registering a wheel listener of its own, so the header
    /// follows the rows without competing for the gesture that moves them.
    fn names_strip(&self, cx: &Context<Self>) -> Stateful<Div> {
        let mut cells = Vec::with_capacity(self.column_widths.len());
        for (column, width) in self.column_widths.iter().enumerate() {
            cells.push(self.header_cell(column, *width, cx));
        }

        // The fill and the rule live on the strip rather than on the scrolled
        // content, so a table narrower than the window still paints edge to edge.
        div()
            .id("grid-header-scroll")
            .debug_selector(|| "grid-header".to_string())
            .absolute()
            .top(px(LETTERS_HEIGHT))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(HEADER_HEIGHT))
            .overflow_hidden()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .track_scroll(&self.header_scroll)
            .child(
                div()
                    .w(px(self.total_width() + GUTTER_WIDTH))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.corner(HEADER_HEIGHT))
                    .children(cells),
            )
    }

    /// The blank square where the row numbers meet the column headers.
    fn corner(&self, height: f32) -> Div {
        div()
            .w(px(GUTTER_WIDTH))
            .flex_shrink_0()
            .h(px(height))
            .border_r_1()
            .border_color(theme::border())
    }

    /// One column letter. Clicking it marks the column, which is the one thing
    /// the name below it does not already do.
    fn letter_cell(&self, column: usize, width: f32, cx: &Context<Self>) -> Stateful<Div> {
        let marked = match self.selection {
            Selection::Column { column: marked } => marked == column,
            Selection::Cell { column: marked, .. } => marked == column,
            _ => false,
        };

        div()
            .id(ElementId::named_usize("letter", column))
            .debug_selector({
                let selector = format!("letter-{column}");
                move || selector
            })
            .w(px(width))
            .flex_shrink_0()
            .h(px(LETTERS_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(CELL_FONT_SIZE - 1.0))
            .cursor_pointer()
            .text_color(theme::text_muted())
            .when(marked, |el| {
                el.bg(theme::selection()).text_color(theme::accent())
            })
            .child(SharedString::from(column_label(column)))
            .on_click(
                cx.listener(move |this, _event, window, cx| this.mark_column(column, window, cx)),
            )
    }

    /// The row numbers down the left edge, which never scroll sideways.
    fn gutter_viewport(&self, cx: &Context<Self>) -> Stateful<Div> {
        let rows = self.view.len();
        let viewport = div()
            .id("grid-gutter")
            .debug_selector(|| "grid-gutter".to_string())
            .absolute()
            .top(px(LETTERS_HEIGHT + HEADER_HEIGHT))
            .bottom(px(0.0))
            .left(px(0.0))
            .w(px(GUTTER_WIDTH))
            .overflow_hidden()
            .bg(theme::surface())
            .border_r_1()
            .border_color(theme::border())
            .track_scroll(&self.gutter_scroll);

        if rows == 0 {
            return viewport;
        }

        // The same window of rows the cells draw, so a number can never end up
        // beside a row it does not describe.
        let (first, last) = self.row_window();
        let mut cells = Vec::with_capacity(last.saturating_sub(first));
        for position in first..last {
            cells.push(self.number_cell(position, cx));
        }

        viewport.child(
            div()
                .relative()
                .flex_shrink_0()
                .w(px(GUTTER_WIDTH))
                .h(px(rows as f32 * ROW_HEIGHT))
                .children(cells),
        )
    }

    /// One row number, counting from one the way every tool that shows a row
    /// count does.
    fn number_cell(&self, position: usize, cx: &Context<Self>) -> Stateful<Div> {
        let marked = match self.selection {
            Selection::Row { position: marked } => marked == position,
            Selection::Cell {
                position: marked, ..
            } => marked == position,
            _ => false,
        };

        div()
            .id(ElementId::named_usize("row-number", position))
            .debug_selector({
                let selector = format!("row-number-{position}");
                move || selector
            })
            .absolute()
            .top(px(position as f32 * ROW_HEIGHT))
            .left(px(0.0))
            .w(px(GUTTER_WIDTH))
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .font_family(theme::monospace())
            .text_size(px(CELL_FONT_SIZE - 1.0))
            .cursor_pointer()
            .text_color(theme::text_muted())
            .when(marked, |el| {
                el.bg(theme::selection()).text_color(theme::accent())
            })
            .child(SharedString::from((position + 1).to_string()))
            .on_click(
                cx.listener(move |this, _event, window, cx| this.mark_row(position, window, cx)),
            )
    }

    /// The visible slice of rows, as positions into the view.
    ///
    /// The numbers and the cells have to agree on this window, so both ask for
    /// it here rather than each deriving it.
    fn row_window(&self) -> (usize, usize) {
        let rows = self.view.len();
        if rows == 0 {
            return (0, 0);
        }

        // Bounds are known only after a frame has been laid out. Until then draw
        // a screenful: the next frame measures the real viewport and the row
        // count settles.
        let measured = f32::from(self.body_scroll.bounds().size.height);
        let viewport_height = if measured > 0.0 {
            measured
        } else {
            ROW_HEIGHT * 40.0
        };

        let offset = f32::from(self.body_scroll.offset().y);
        let first = (((-offset) / ROW_HEIGHT).floor().max(0.0) as usize).min(rows - 1);
        let drawn = (viewport_height / ROW_HEIGHT).ceil() as usize + 1;
        (first, (first + drawn).min(rows))
    }

    /// The data rows, drawn for the part of the file that is in view.
    ///
    /// `uniform_list` used to do this, but it owns its own wheel handling and
    /// cannot be told to leave an axis alone. Rendering the visible slice by
    /// hand keeps the virtualization and hands scrolling to a single owner.
    fn rows_viewport(&self, cx: &Context<Self>) -> Stateful<Div> {
        // Absolute insets rather than `flex_1`: a flex item whose container has
        // no definite height during layout takes its content's height instead of
        // the space left over, which would make the viewport as tall as the file.
        let viewport = div()
            .id("grid-scroll")
            .debug_selector(|| "grid-scroll".to_string())
            .absolute()
            .top(px(LETTERS_HEIGHT + HEADER_HEIGHT))
            .bottom(px(0.0))
            .left(px(GUTTER_WIDTH))
            .right(px(0.0))
            .overflow_hidden()
            .track_scroll(&self.body_scroll);

        let rows = self.view.len();
        if rows == 0 {
            return viewport
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(theme::text_muted()).child("no rows match"));
        }

        let (first, last) = self.row_window();
        let visible = (first..last)
            .map(|position| self.row(position, position as f32 * ROW_HEIGHT, cx))
            .collect::<Vec<_>>();

        // The content box is what the scroll handles measure themselves
        // against, so it must keep the full height of the file.
        viewport.child(
            div()
                .relative()
                .flex_shrink_0()
                .w(px(self.total_width()))
                .h(px(rows as f32 * ROW_HEIGHT))
                .children(visible),
        )
    }

    /// Scroll by one wheel event, on one axis only.
    ///
    /// A trackpad reports both axes in the same event. With the header and the
    /// rows as two native scrollers, each took the axis it owned and a diagonal
    /// swipe moved diagonally. This handler owns both offsets instead, and picks
    /// one axis for the whole gesture rather than one per event.
    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        let (mut x, mut y) = (delta.x, delta.y);

        // Only a precise device (a trackpad) reports a gesture's start and end.
        // A mouse wheel arrives as one event at a time, so it picks per event.
        let precise = event.delta.precise();
        if precise && matches!(event.touch_phase, TouchPhase::Started) {
            self.gesture_axis = None;
        }

        let axis = match (precise, self.gesture_axis) {
            (true, Some(axis)) => Some(axis),
            _ => dominant_axis(x, y),
        };
        if precise && self.gesture_axis.is_none() {
            self.gesture_axis = axis;
        }

        match axis {
            Some(Axis::Horizontal) => y = px(0.0),
            Some(Axis::Vertical) => x = px(0.0),
            // No movement at all, which is what the first event of a gesture
            // carries: nothing to do until the fingers move.
            None => return,
        }

        if precise && matches!(event.touch_phase, TouchPhase::Ended) {
            self.gesture_axis = None;
        }

        let current = self.body_scroll.offset();
        let max = self.body_scroll.max_offset();
        let next = point(
            (current.x + x).clamp(-max.width, px(0.0)),
            (current.y + y).clamp(-max.height, px(0.0)),
        );

        if next == current {
            return;
        }

        // The header shares the horizontal offset and the row numbers share the
        // vertical one, and neither moves on the axis it does not own. Without
        // this the numbers would keep answering for the rows that were in view
        // when the file opened, which is worse than showing none.
        self.header_scroll.set_offset(point(next.x, px(0.0)));
        self.gutter_scroll.set_offset(point(px(0.0), next.y));
        self.body_scroll.set_offset(next);
        cx.notify();
    }

    /// Thin bars along the right and bottom edges showing where in the file the
    /// view is.
    ///
    /// GPUI 0.2.2 has no scrollbar widget, and a native `overflow_scroll`
    /// container would scroll itself, so the bars are drawn from the offsets
    /// above. They show where you are; dragging them is not wired up yet.
    fn scrollbars(&self) -> Vec<Div> {
        let max = self.body_scroll.max_offset();
        let viewport = self.body_scroll.bounds().size;
        let offset = self.body_scroll.offset();
        let mut bars = Vec::new();

        let viewport_height = f32::from(viewport.height);
        let overflow_y = f32::from(max.height);
        if overflow_y > 0.0 && viewport_height > 0.0 {
            let thumb = thumb_length(viewport_height, overflow_y);
            let travel = viewport_height - thumb;
            let progress = (-f32::from(offset.y) / overflow_y).clamp(0.0, 1.0);
            bars.push(
                div()
                    .debug_selector(|| "scrollbar-vertical".to_string())
                    .absolute()
                    .right(px(2.0))
                    .bottom(px(2.0 + travel * (1.0 - progress)))
                    .w(px(SCROLLBAR_THICKNESS))
                    .h(px(thumb))
                    .rounded_full()
                    .bg(theme::scrollbar()),
            );
        }

        let viewport_width = f32::from(viewport.width);
        let overflow_x = f32::from(max.width);
        if overflow_x > 0.0 && viewport_width > 0.0 {
            let thumb = thumb_length(viewport_width, overflow_x);
            let travel = viewport_width - thumb;
            let progress = (-f32::from(offset.x) / overflow_x).clamp(0.0, 1.0);
            bars.push(
                div()
                    .debug_selector(|| "scrollbar-horizontal".to_string())
                    .absolute()
                    .bottom(px(2.0))
                    .left(px(2.0 + travel * progress))
                    .h(px(SCROLLBAR_THICKNESS))
                    .w(px(thumb))
                    .rounded_full()
                    .bg(theme::scrollbar()),
            );
        }

        bars
    }

    fn header_cell(&self, column: usize, width: f32, cx: &Context<Self>) -> Stateful<Div> {
        let label = self.document.table.header(column).unwrap_or_default();
        let sort = self.view.sort().filter(|key| key.column == column);
        let sorted = sort.is_some();
        // One glyph, so the sorted column needs no extra room and columns do not
        // move when the sort changes.
        let indicator = match sort {
            Some(SortKey {
                direction: Direction::Ascending,
                ..
            }) => "\u{2191}",
            Some(SortKey {
                direction: Direction::Descending,
                ..
            }) => "\u{2193}",
            None => "",
        };

        div()
            .id(ElementId::named_usize("column", column))
            .debug_selector({
                let selector = format!("header-{column}");
                move || selector
            })
            .w(px(width))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h_full()
            .overflow_hidden()
            .cursor_pointer()
            .when(sorted, |el| {
                el.bg(theme::surface_active()).text_color(theme::accent())
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .font_family(theme::monospace())
                    .text_size(px(CELL_FONT_SIZE))
                    .truncate()
                    .child(SharedString::from(label.to_string())),
            )
            .child(
                div()
                    .text_size(px(CELL_FONT_SIZE))
                    .text_color(theme::text_muted())
                    .child(SharedString::from(indicator)),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| this.toggle_sort(column, cx)))
    }

    fn row(&self, position: usize, top: f32, cx: &Context<Self>) -> Stateful<Div> {
        let table = &self.document.table;
        let row = self.view.table_row(position);
        let mut cells = Vec::with_capacity(table.column_count());

        for (column, width) in self.column_widths.iter().enumerate() {
            // A cell can be missing when a row is shorter than the header, and
            // `SharedString` cannot borrow from the table, so own the text.
            let text = row
                .and_then(|row| table.get(row, column))
                .unwrap_or_default()
                .to_string();

            let session = self
                .session
                .as_ref()
                .filter(|session| Some(session.row) == row && session.column == column);

            let content = match session {
                Some(session) => self.editing_cell(column, session, cx),
                None => div()
                    .w_full()
                    .min_w(px(0.0))
                    .font_family(theme::monospace())
                    .text_size(px(CELL_FONT_SIZE))
                    .truncate()
                    .child(text),
            };

            cells.push(
                div()
                    .w(px(*width))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px_2()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(theme::border_subtle())
                    .when(self.cell_is_selected(position, column), |el| {
                        el.bg(theme::selection())
                    })
                    .when(self.cell_is_focused(position, column), |el| {
                        el.bg(theme::selection_strong())
                    })
                    .child(content),
            );
        }

        div()
            .id(ElementId::named_usize("row", position))
            .debug_selector({
                let selector = format!("row-{position}");
                move || selector
            })
            .absolute()
            .top(px(top))
            .left(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .w(px(self.total_width()))
            .h(px(ROW_HEIGHT))
            .when(position % 2 == 1, |el| el.bg(theme::row_alternate()))
            // One listener per row rather than one per cell: a cell is a plain
            // div, so marking a cell and placing a caret both only need the
            // column the pointer fell in.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.focus(window);
                    this.click_cell(position, row, event, window, cx);
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    this.drag_caret(row, event, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                    if let Some(session) = this.session.as_mut() {
                        session.editor.dragging = false;
                    }
                }),
            )
            .children(cells)
    }

    /// Handle a left click on the row at `position`.
    ///
    /// A click inside the open cell places the caret. A click anywhere else
    /// commits first, so nothing downstream has to reason about a half-open
    /// edit, and a double click starts one.
    fn click_cell(
        &mut self,
        position: usize,
        row: Option<usize>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(column) = self.column_at(event.position.x) else {
            return;
        };

        if let Some(session) = self.session.as_ref() {
            if Some(session.row) == row && session.column == column {
                if event.click_count < 2 {
                    self.place_caret(column, event.position.x, event.modifiers.shift, true, cx);
                }
                return;
            }
            self.commit(cx);
        }

        // Committing can move the row under a sort or hide it behind the
        // filter, so the position is looked up again before it is used.
        let position = row
            .and_then(|row| self.view.position_of(row))
            .unwrap_or(position);

        if event.click_count == 2 {
            self.start_editing(position, column, Some(event.position.x), window, cx);
        } else {
            self.select_cell_at(position, event.position.x, cx);
        }
    }

    /// Put the caret at the window x `x` inside the open cell.
    fn place_caret(
        &mut self,
        column: usize,
        x: Pixels,
        extend: bool,
        start_drag: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if session.column != column {
            return;
        }
        let text = self.cell_text(session.row, column).to_string();
        let metrics = CellMetrics::new(cx.text_system());
        let offset = metrics.offset_for_x(&text, f32::from(x) - self.cell_screen_left(column));

        self.edits.break_group();
        if let Some(session) = self.session.as_mut() {
            if extend {
                session.editor.select_to(offset);
            } else {
                session.editor.move_to(offset);
            }
            if start_drag {
                session.editor.dragging = true;
            }
        }
        cx.notify();
    }

    /// Extend the cell's selection while a drag that began inside it continues.
    ///
    /// A pointer dragged past the value's edges clamps to its ends: there is no
    /// autoscroll inside a cell, and selecting something off screen that the
    /// user cannot see would be worse than stopping.
    fn drag_caret(&mut self, row: Option<usize>, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if !session.editor.dragging || !event.dragging() || Some(session.row) != row {
            return;
        }
        let column = session.column;
        self.place_caret(column, event.position.x, true, false, cx);
    }

    /// The cell being edited: the value with its selection wash and a steady
    /// caret, scrolled sideways so the caret stays inside the cell.
    fn editing_cell(&self, column: usize, session: &CellSession, cx: &Context<Self>) -> Div {
        let text = self.cell_text(session.row, column).to_string();
        let metrics = CellMetrics::new(cx.text_system());
        let caret = clamp_boundary(&text, session.editor.caret);
        let caret_x = metrics.prefix_width(&text[..caret]);
        let width = self
            .column_widths
            .get(column)
            .copied()
            .unwrap_or(MIN_COLUMN_WIDTH);
        let usable = (width - 2.0 * CELL_PADDING_X).max(1.0);
        // Keep the caret one character inside the cell, so typing at the end of
        // a long value scrolls the text rather than hiding the caret.
        let advance = metrics.advance('M').max(1.0);
        let offset = (caret_x + advance - usable).max(0.0);

        let selection = clamp_range(&text, session.editor.selection());
        let marked = session
            .editor
            .marked
            .as_ref()
            .map(|range| clamp_range(&text, range.clone()));
        let runs = text_runs(&text, selection, marked);

        div()
            .relative()
            .w_full()
            .min_w(px(0.0))
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .font_family(theme::monospace())
            .text_size(px(CELL_FONT_SIZE))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(-offset))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .child(StyledText::new(text).with_runs(runs)),
            )
            .child(
                div()
                    .debug_selector(|| "cell-caret".to_string())
                    .absolute()
                    .top(px(3.0))
                    .left(px(caret_x - offset))
                    .w(px(1.0))
                    .h(px(ROW_HEIGHT - 8.0))
                    .bg(theme::accent()),
            )
            // The platform input handler needs an element to paint, and its
            // bounds are what place the IME candidate window beside the text.
            .child(div().absolute().top(px(0.0)).left(px(0.0)).w_full().child(
                editor::InputBridgeElement::new(cx.entity(), self.focus_handle.clone()),
            ))
    }
}

/// Split `text` into runs so the selection is washed and a composition is
/// underlined, without changing the bytes.
///
/// `StyledText` asserts that the runs cover the text exactly, so the boundaries
/// of the selection and the composition are the only places a run may start.
fn text_runs(text: &str, selection: Range<usize>, marked: Option<Range<usize>>) -> Vec<TextRun> {
    let mut boundaries = vec![0, text.len(), selection.start, selection.end];
    if let Some(marked) = &marked {
        boundaries.push(marked.start);
        boundaries.push(marked.end);
    }
    boundaries.retain(|edge| *edge <= text.len());
    boundaries.sort_unstable();
    boundaries.dedup();

    let font = font(theme::monospace());
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let (start, end) = (pair[0], pair[1]);
            if start == end {
                return None;
            }
            let selected =
                !selection.is_empty() && start >= selection.start && end <= selection.end;
            let composing = marked
                .as_ref()
                .is_some_and(|marked| start >= marked.start && end <= marked.end);
            Some(TextRun {
                len: end - start,
                font: font.clone(),
                color: theme::text(),
                background_color: selected.then(theme::selection),
                underline: composing.then(|| gpui::UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(theme::accent()),
                    wavy: false,
                }),
                strikethrough: None,
            })
        })
        .collect()
}

impl Render for CsvGrid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("csv-grid")
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .text_sm()
            // The shared bindings only match while a cell is open. Outside a
            // session the grid stays out of the context, which is what keeps
            // Cmd+C for its own mark and typing for the search box.
            .when(self.session.is_some(), |el| el.key_context("TextEditor"))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::act_left))
            .on_action(cx.listener(Self::act_right))
            .on_action(cx.listener(Self::act_up))
            .on_action(cx.listener(Self::act_down))
            .on_action(cx.listener(Self::act_home))
            .on_action(cx.listener(Self::act_end))
            .on_action(cx.listener(Self::act_document_start))
            .on_action(cx.listener(Self::act_document_end))
            .on_action(cx.listener(Self::act_word_left))
            .on_action(cx.listener(Self::act_word_right))
            .on_action(cx.listener(Self::act_select_left))
            .on_action(cx.listener(Self::act_select_right))
            .on_action(cx.listener(Self::act_select_home))
            .on_action(cx.listener(Self::act_select_end))
            .on_action(cx.listener(Self::act_select_word_left))
            .on_action(cx.listener(Self::act_select_word_right))
            .on_action(cx.listener(Self::act_select_all))
            .on_action(cx.listener(Self::act_backspace))
            .on_action(cx.listener(Self::act_delete))
            .on_action(cx.listener(Self::act_enter))
            .on_action(cx.listener(Self::act_tab))
            .on_action(cx.listener(Self::act_outdent))
            .on_action(cx.listener(Self::act_copy))
            .on_action(cx.listener(Self::act_cut))
            .on_action(cx.listener(Self::act_paste))
            .on_action(cx.listener(Self::act_undo))
            .on_action(cx.listener(Self::act_redo))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| this.focus(window)),
            )
            .child(self.search_bar(cx))
            .child(self.body(cx))
            .child(self.status_bar(cx))
    }
}

/// The platform input handler for the open cell.
///
/// GPUI has no text-field widget, so the grid becomes one for as long as a
/// session is open: the bridge element inside the edited cell registers this
/// during paint, which is what makes IME, the character palette, and paste work.
/// The bytes belong to the cell in the table, and every replacement goes through
/// the same history the renderer reads.
impl EntityInputHandler for CsvGrid {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (row, column) = self.session_cell()?;
        let text = self.cell_text(row, column);
        let range = clamp_range(text, editor::range_from_utf16(text, &range_utf16));
        actual_range.replace(editor::range_to_utf16(text, &range));
        Some(text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let session = self.session.as_ref()?;
        let text = self.cell_text(session.row, session.column);
        Some(editor::utf16_selection(text, &session.editor))
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let session = self.session.as_ref()?;
        let text = self.cell_text(session.row, session.column);
        session
            .editor
            .marked
            .as_ref()
            .map(|range| editor::range_to_utf16(text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(session) = self.session.as_mut() {
            session.editor.marked = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_control(text, cx) {
            return;
        }
        let Some((row, column)) = self.session_cell() else {
            return;
        };
        let range = {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let value = self.cell_text(row, column).to_string();
            let caret_before = session.editor.caret;
            let range = range_utf16
                .as_ref()
                .map(|range| editor::range_from_utf16(&value, range))
                .or_else(|| session.editor.marked.clone())
                .unwrap_or_else(|| session.editor.selection());
            (clamp_range(&value, range), caret_before)
        };

        let mut buffer = CellBuffer {
            table: &mut self.document.table,
            edits: &mut self.edits,
            row,
            column,
        };
        let caret = buffer.buffer_replace(range.0, text, range.1, 0, EditKind::Typing);
        if let Some(session) = self.session.as_mut() {
            session.editor.marked = None;
            session.editor.move_to(caret);
        }
        self.after_keystroke(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_control(new_text, cx) {
            return;
        }
        let Some((row, column)) = self.session_cell() else {
            return;
        };
        let (range, caret_before) = {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let value = self.cell_text(row, column).to_string();
            let range = range_utf16
                .as_ref()
                .map(|range| editor::range_from_utf16(&value, range))
                .or_else(|| session.editor.marked.clone())
                .unwrap_or_else(|| session.editor.selection());
            (clamp_range(&value, range), session.editor.caret)
        };
        let start = range.start;

        let mut buffer = CellBuffer {
            table: &mut self.document.table,
            edits: &mut self.edits,
            row,
            column,
        };
        let caret = buffer.buffer_replace(range, new_text, caret_before, 0, EditKind::Typing);
        if let Some(session) = self.session.as_mut() {
            session.editor.marked = if new_text.is_empty() {
                None
            } else {
                Some(start..start + new_text.len())
            };
            session.editor.move_to(caret);
            if let Some(selected) = new_selected_range {
                let selected = editor::range_from_utf16(new_text, &selected);
                session.editor.select_to(start + selected.end);
            }
        }
        self.after_keystroke(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (row, column) = self.session_cell()?;
        let text = self.cell_text(row, column);
        let offset = clamp_boundary(text, editor::offset_from_utf16(text, range_utf16.start));
        let metrics = CellMetrics::new(cx.text_system());
        let x = element_bounds.origin.x + px(metrics.prefix_width(&text[..offset]));
        Some(Bounds::new(
            point(x, element_bounds.origin.y),
            gpui::size(px(2.0), px(ROW_HEIGHT - 8.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (row, column) = self.session_cell()?;
        let text = self.cell_text(row, column);
        let metrics = CellMetrics::new(cx.text_system());
        let offset =
            metrics.offset_for_x(text, f32::from(position.x) - self.cell_screen_left(column));
        Some(editor::offset_to_utf16(text, offset))
    }
}

/// Width for every column, measured from the widest value it has to show.
///
/// The file is scanned once at load, comparing lengths only. The single widest
/// value per column is then measured, so the cost is one measurement per column
/// rather than one per cell, and a 200k-row file pays it once instead of on
/// every frame.
fn measure_columns(document: &CsvDocument, text_system: &TextSystem) -> Vec<f32> {
    let table = &document.table;
    (0..table.column_count())
        .map(|column| measure_column(table, column, text_system))
        .collect()
}

/// Width for one column, measured from the widest value it has to show.
///
/// A committed cell re-measures only its own column, which is why this exists
/// apart from [`measure_columns`]: growing one column must not cost a scan of
/// every other one on every commit.
fn measure_column(table: &Table, column: usize, text_system: &TextSystem) -> f32 {
    // Seed with the header, so a column is always wide enough for its own name.
    let header = table.header(column).unwrap_or_default();
    let mut widest = (header.chars().count(), header);

    for row in table.rows() {
        let Some(cell) = row.get(column).map(String::as_str) else {
            continue;
        };
        let chars = cell.chars().count();
        if chars > widest.0 {
            widest = (chars, cell);
        }
    }

    let font = font(theme::monospace());
    let font_size = px(CELL_FONT_SIZE);
    column_width(text_width(widest.1, text_system, &font, font_size))
}

/// The width `text` occupies when drawn in the grid's font, in pixels.
///
/// Summed one character at a time. The grid draws in a monospace family, so this
/// is exact for the Latin text a delimited file mostly holds and close enough
/// for anything else. Summing stops once the text is wider than a column may
/// get, so one pathological value cannot make opening a file slow.
fn text_width(text: &str, text_system: &TextSystem, font: &Font, font_size: Pixels) -> f32 {
    let font_id = text_system.resolve_font(font);
    let limit = MAX_COLUMN_WIDTH - CELL_PADDING;
    let mut advances: HashMap<char, f32> = HashMap::new();
    let mut width = 0.0;

    for ch in text.chars() {
        // A quoted value can hold a newline or a tab; measure it as a space so
        // the column still accounts for the character.
        let ch = if ch.is_control() { ' ' } else { ch };
        let advance = *advances.entry(ch).or_insert_with(|| {
            text_system
                .advance(font_id, font_size, ch)
                .map(|size| f32::from(size.width))
                .unwrap_or(0.0)
        });
        width += advance;
        if width >= limit {
            break;
        }
    }

    width
}

/// Clamp a measured text width into the range a column is allowed to occupy.
fn column_width(text_width: f32) -> f32 {
    (text_width + CELL_PADDING).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
}

/// Which axis a delta mostly moves along, or `None` when it moves neither.
fn dominant_axis(x: Pixels, y: Pixels) -> Option<Axis> {
    if x == px(0.0) && y == px(0.0) {
        None
    } else if x.abs() > y.abs() {
        Some(Axis::Horizontal)
    } else {
        Some(Axis::Vertical)
    }
}

/// How long a scrollbar thumb should be, given the length of its track and how
/// far the content runs past it.
fn thumb_length(track: f32, overflow: f32) -> f32 {
    let visible = track / (track + overflow);
    // max-then-min rather than clamp: a track shorter than the minimum thumb
    // makes `clamp` panic, and a tiny window should not crash the grid.
    (track * visible).max(MIN_SCROLLBAR_THUMB).min(track)
}

/// Excel's column labels: A to Z, then AA, AB, and so on.
///
/// The labels a reader expects come from the same sequence a spreadsheet uses,
/// so the seventh column of a narrow table is `G` rather than `7`.
fn column_label(column: usize) -> String {
    let mut label = String::new();
    let mut index = column + 1;

    while index > 0 {
        let remainder = (index - 1) % 26;
        label.insert(0, (b'A' + remainder as u8) as char);
        index = (index - 1) / 26;
    }

    label
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bromine_core::{load_csv, parse_csv};
    use gpui::{
        Entity, Modifiers, Point, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext,
    };

    fn document() -> CsvDocument {
        parse_csv("name,commits\nada,412\ngrace,88\nalan,1337\n").expect("the fixture parses")
    }

    fn grid(cx: &mut TestAppContext) -> (Entity<CsvGrid>, &mut VisualTestContext) {
        cx.add_window_view(|_window, cx| CsvGrid::new(PathBuf::from("people.csv"), document(), cx))
    }

    fn focus(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) {
        let handle = cx.update(|_window, app| grid.read(app).focus_handle().clone());
        cx.update(|window, _app| handle.focus(window));
    }

    fn visible_rows(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) -> usize {
        cx.update(|_window, app| grid.read(app).visible_row_count())
    }

    /// 500 rows across 8 wide columns: taller and wider than any test window, so
    /// both scrollbars have somewhere to go.
    fn tall_grid(cx: &mut TestAppContext) -> (Entity<CsvGrid>, &mut VisualTestContext) {
        cx.add_window_view(|_window, cx| {
            let headers: Vec<String> = (0..8).map(|column| format!("column{column}")).collect();
            let mut source = headers.join(",");
            for row in 0..500 {
                let values: Vec<String> = (0..8)
                    .map(|column| format!("value {column} of row {row} with some length"))
                    .collect();
                source.push('\n');
                source.push_str(&values.join(","));
            }
            source.push('\n');
            let document = parse_csv(&source).expect("the fixture parses");
            CsvGrid::new(PathBuf::from("tall.csv"), document, cx)
        })
    }

    fn wheel_with(dx: f32, dy: f32, touch_phase: TouchPhase) -> ScrollWheelEvent {
        ScrollWheelEvent {
            position: point(px(120.0), px(200.0)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase,
        }
    }

    fn wheel(dx: f32, dy: f32) -> ScrollWheelEvent {
        wheel_with(dx, dy, TouchPhase::Moved)
    }

    /// Point the mouse at the grid, so the wheel event is dispatched to it.
    fn hover_the_grid(cx: &mut VisualTestContext) {
        cx.simulate_mouse_move(point(px(120.0), px(200.0)), None, Modifiers::none());
    }

    /// Draw a frame, so bounds reflect what the last event left behind.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
    }

    fn scroll_offset(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) -> Point<Pixels> {
        cx.update(|_window, app| grid.read(app).body_scroll.offset())
    }

    fn column_values(
        cx: &mut VisualTestContext,
        grid: &Entity<CsvGrid>,
        column: usize,
    ) -> Vec<String> {
        cx.update(|_window, app| {
            let grid = grid.read(app);
            (0..grid.visible_row_count())
                .map(|position| {
                    grid.cell_at(position, column)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
    }

    #[gpui::test]
    fn typing_searches_and_backspace_and_escape_widen_the_result_again(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        focus(cx, &grid);

        assert_eq!(visible_rows(cx, &grid), 3);

        cx.simulate_input("ada");
        assert_eq!(
            cx.update(|_window, app| grid.read(app).query().to_string()),
            "ada"
        );
        assert_eq!(visible_rows(cx, &grid), 1);
        assert_eq!(column_values(cx, &grid, 0), ["ada"]);

        // Three backspaces, one per typed character: a query of "a" would still
        // match all three rows and hide a broken delete.
        cx.simulate_keystrokes("backspace backspace backspace");
        assert_eq!(
            cx.update(|_window, app| grid.read(app).query().to_string()),
            ""
        );
        assert_eq!(visible_rows(cx, &grid), 3);

        cx.simulate_input("grace");
        assert_eq!(visible_rows(cx, &grid), 1);
        assert_eq!(column_values(cx, &grid, 0), ["grace"]);

        cx.simulate_keystrokes("escape");
        assert_eq!(
            cx.update(|_window, app| grid.read(app).query().to_string()),
            ""
        );
        assert_eq!(visible_rows(cx, &grid), 3);
        assert_eq!(column_values(cx, &grid, 0), ["ada", "grace", "alan"]);
    }

    #[gpui::test]
    fn clicking_a_header_sorts_by_it_and_clicking_again_reverses_it(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);

        assert_eq!(column_values(cx, &grid, 1), ["412", "88", "1337"]);

        let header = cx
            .debug_bounds("header-1")
            .expect("the commits header was laid out");
        let centre = header.center();

        cx.simulate_mouse_down(centre, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(centre, MouseButton::Left, Modifiers::none());

        assert_eq!(
            cx.update(|_window, app| grid.read(app).sort()),
            Some(SortKey {
                column: 1,
                direction: Direction::Ascending
            })
        );
        // 88 before 412 is the point: commits sorts as a number, not as text.
        assert_eq!(column_values(cx, &grid, 1), ["88", "412", "1337"]);

        cx.simulate_mouse_down(centre, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(centre, MouseButton::Left, Modifiers::none());

        assert_eq!(
            cx.update(|_window, app| grid.read(app).sort()),
            Some(SortKey {
                column: 1,
                direction: Direction::Descending
            })
        );
        assert_eq!(column_values(cx, &grid, 1), ["1337", "412", "88"]);
    }

    #[gpui::test]
    fn sorting_keeps_every_row_traceable_to_its_table_index(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);

        let before = cx.update(|_window, app| {
            let grid = grid.read(app);
            (0..grid.visible_row_count())
                .map(|position| grid.table_row_at(position).unwrap_or_default())
                .collect::<Vec<_>>()
        });

        let header = cx
            .debug_bounds("header-1")
            .expect("the header was laid out");
        cx.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::none());

        let after = cx.update(|_window, app| {
            let grid = grid.read(app);
            (0..grid.visible_row_count())
                .map(|position| grid.table_row_at(position).unwrap_or_default())
                .collect::<Vec<_>>()
        });

        assert_eq!(before, [0, 1, 2]);
        // Row 1 (grace, 88) sorts first, and every index is still a real row.
        assert_eq!(after, [1, 0, 2]);
    }

    #[gpui::test]
    fn a_ragged_file_is_shown_with_a_notice_about_the_repair(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let document = parse_csv("a,b\n1\n2,3,4\n").expect("the fixture parses");
            CsvGrid::new(PathBuf::from("ragged.csv"), document, cx)
        });

        let notice = cx.update(|_window, app| grid.read(app).repairs_notice());
        let notice = notice.expect("a repaired file must say so");
        assert!(notice.contains("padded"), "unexpected notice: {notice}");
        assert!(notice.contains("named"), "unexpected notice: {notice}");
    }

    #[test]
    fn a_measured_width_clamps_to_the_range_a_column_may_occupy() {
        assert_eq!(column_width(0.0), MIN_COLUMN_WIDTH);
        assert_eq!(column_width(MAX_COLUMN_WIDTH), MAX_COLUMN_WIDTH);
        assert_eq!(column_width(MAX_COLUMN_WIDTH * 10.0), MAX_COLUMN_WIDTH);
        // Inside the range the measurement is used as it stands, plus padding.
        assert_eq!(column_width(200.0), 230.0);
    }

    #[gpui::test]
    fn column_widths_follow_the_values_a_column_holds(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            // "note" is the longer header, and it also holds the wider value, so
            // the single-character column beside it has to stay narrow.
            let document =
                parse_csv("i,note\n1,short\n2,a considerably longer value\n").expect("parses");
            CsvGrid::new(PathBuf::from("wide.csv"), document, cx)
        });

        let widths = cx.update(|_window, app| grid.read(app).column_widths.clone());
        assert_eq!(widths.len(), 2);
        assert!(widths[1] > widths[0], "widths were {widths:?}");
        // One character plus padding is under the floor, so it lands on it.
        assert_eq!(widths[0], MIN_COLUMN_WIDTH);
    }

    #[gpui::test]
    fn a_header_wider_than_every_value_sets_the_column_width(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let document = parse_csv("a_very_long_header_name,b\n1,2\n").expect("parses");
            CsvGrid::new(PathBuf::from("headers.csv"), document, cx)
        });

        let widths = cx.update(|_window, app| grid.read(app).column_widths.clone());
        assert!(
            widths[0] > MIN_COLUMN_WIDTH,
            "the header was narrower than its floor: {widths:?}"
        );
        assert_eq!(widths[1], MIN_COLUMN_WIDTH);
    }

    #[gpui::test]
    fn one_enormous_value_does_not_make_its_column_enormous(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let enormous = "y".repeat(20_000);
            let document = parse_csv(&format!("enormous,next\n{enormous},b\n")).expect("parses");
            CsvGrid::new(PathBuf::from("enormous.csv"), document, cx)
        });

        let widths = cx.update(|_window, app| grid.read(app).column_widths.clone());
        assert_eq!(widths[0], MAX_COLUMN_WIDTH);
        assert!(widths[1] < MAX_COLUMN_WIDTH, "widths were {widths:?}");
    }

    #[gpui::test]
    fn columns_are_laid_out_at_the_widths_that_were_measured(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let document = parse_csv("identifier,note\n1,short\n2,a considerably longer value\n")
                .expect("parses");
            CsvGrid::new(PathBuf::from("measured.csv"), document, cx)
        });

        let widths = cx.update(|_window, app| grid.read(app).column_widths.clone());

        for (selector, expected) in [("header-0", widths[0]), ("header-1", widths[1])] {
            let bounds = cx.debug_bounds(selector).expect("the header was laid out");
            let actual = f32::from(bounds.size.width);
            assert!(
                (actual - expected).abs() < 0.5,
                "{selector} is {actual} wide but was measured at {expected}"
            );
        }
    }

    #[gpui::test]
    fn a_wide_table_scrolls_sideways_instead_of_squeezing_its_columns(cx: &mut TestAppContext) {
        let (grid, cx) = cx.add_window_view(|_window, cx| {
            // Eight columns of long values are wider than the test window, so
            // the last column can only be reached by scrolling.
            let headers: Vec<String> = (0..8).map(|column| format!("column{column}")).collect();
            let values: Vec<String> = (0..8)
                .map(|column| format!("value {column} that is deliberately long indeed"))
                .collect();
            let document = parse_csv(&format!("{}\n{}\n", headers.join(","), values.join(",")))
                .expect("parses");
            CsvGrid::new(PathBuf::from("wide.csv"), document, cx)
        });

        let widths = cx.update(|_window, app| grid.read(app).column_widths.clone());
        let total: f32 = widths.iter().sum();
        let scroll = cx
            .debug_bounds("grid-scroll")
            .expect("the scroll container was laid out");
        let viewport = f32::from(scroll.size.width);

        assert!(
            total > viewport,
            "the fixture has to be wider than the window: {total} vs {viewport}"
        );

        // Squeezed columns would end at the right edge of the viewport. Laid out
        // at their measured widths they run past it, which is what scrolls.
        let last = cx
            .debug_bounds("header-7")
            .expect("the last header was laid out");
        assert!(
            f32::from(last.right()) > f32::from(scroll.right()),
            "the last header ends at {} but the viewport at {}",
            last.right(),
            scroll.right()
        );
        assert!(
            (f32::from(last.size.width) - widths[7]).abs() < 0.5,
            "the last column was squeezed to {}",
            last.size.width
        );
    }

    #[gpui::test]
    fn a_diagonal_wheel_event_moves_only_the_dominant_axis(cx: &mut TestAppContext) {
        let (grid, cx) = tall_grid(cx);
        hover_the_grid(cx);

        // Mostly vertical, with a little sideways drift. Two native scrollers
        // used to take one axis each and move on both; now one handler gives the
        // whole gesture to the larger delta.
        cx.simulate_event(wheel_with(0.0, 0.0, TouchPhase::Started));
        cx.simulate_event(wheel(-30.0, -120.0));
        cx.simulate_event(wheel_with(0.0, 0.0, TouchPhase::Ended));
        let offset = scroll_offset(cx, &grid);
        assert_eq!(offset.x, px(0.0), "the horizontal axis moved as well");
        assert_eq!(offset.y, px(-120.0));

        // A new gesture, mostly horizontal: only that axis may move.
        cx.simulate_event(wheel_with(0.0, 0.0, TouchPhase::Started));
        cx.simulate_event(wheel(-240.0, -20.0));
        let offset = scroll_offset(cx, &grid);
        assert_eq!(offset.x, px(-240.0));
        assert_eq!(offset.y, px(-120.0), "the vertical axis moved as well");
    }

    #[gpui::test]
    fn a_gesture_keeps_the_axis_it_started_on(cx: &mut TestAppContext) {
        let (grid, cx) = tall_grid(cx);
        hover_the_grid(cx);

        cx.simulate_event(wheel_with(0.0, -60.0, TouchPhase::Started));
        // Drifting sideways halfway through must not start a sideways pan.
        cx.simulate_event(wheel_with(-120.0, -10.0, TouchPhase::Moved));
        let offset = scroll_offset(cx, &grid);
        assert_eq!(offset.x, px(0.0), "the gesture changed axis mid-swipe");
        assert_eq!(offset.y, px(-70.0));

        // Lifting the fingers ends the gesture, and the next one is free to
        // choose its own axis.
        cx.simulate_event(wheel_with(0.0, 0.0, TouchPhase::Ended));
        cx.simulate_event(wheel_with(-120.0, -10.0, TouchPhase::Moved));
        let offset = scroll_offset(cx, &grid);
        assert_eq!(offset.x, px(-120.0));
        assert_eq!(offset.y, px(-70.0));
    }

    #[gpui::test]
    fn a_vertical_wheel_event_never_pans_sideways(cx: &mut TestAppContext) {
        let (grid, cx) = tall_grid(cx);
        hover_the_grid(cx);

        cx.simulate_event(wheel(0.0, -48.0));

        let offset = scroll_offset(cx, &grid);
        assert_eq!(offset.x, px(0.0));
        assert_eq!(offset.y, px(-48.0));
    }

    #[gpui::test]
    fn scrolling_sideways_moves_the_header_with_the_rows(cx: &mut TestAppContext) {
        let (_grid, cx) = tall_grid(cx);
        hover_the_grid(cx);

        let before = cx
            .debug_bounds("header-0")
            .expect("the header was laid out");
        cx.simulate_event(wheel(-200.0, 0.0));
        draw(cx);
        let after = cx
            .debug_bounds("header-0")
            .expect("the header was laid out");

        assert_eq!(
            after.origin.y, before.origin.y,
            "the header moved vertically"
        );
        assert!(
            after.origin.x < before.origin.x,
            "the header did not follow the rows: {} then {}",
            before.origin.x,
            after.origin.x
        );
    }

    #[gpui::test]
    fn only_the_rows_in_view_are_drawn(cx: &mut TestAppContext) {
        let (_grid, cx) = tall_grid(cx);

        // Only the first screenful is laid out, so the last row of a 500-row
        // file does not exist as an element yet.
        assert!(cx.debug_bounds("row-0").is_some());
        assert!(cx.debug_bounds("row-499").is_none());

        // It can only appear if scrolling moved the drawn window to the end of
        // the file. (`debug_bounds` keeps keys from earlier frames, so the
        // absence of row 0 afterwards is not assertable; its appearance is.)
        hover_the_grid(cx);
        cx.simulate_event(wheel(0.0, -20_000.0));
        draw(cx);
        assert!(
            cx.debug_bounds("row-499").is_some(),
            "the end of the file should be drawn once scrolled there"
        );
    }

    #[gpui::test]
    fn scrollbars_appear_only_for_content_that_overflows(cx: &mut TestAppContext) {
        let (_grid, cx) = grid(cx);

        assert!(cx.debug_bounds("scrollbar-vertical").is_none());
        assert!(cx.debug_bounds("scrollbar-horizontal").is_none());
    }

    #[gpui::test]
    fn scrollbars_track_the_offsets_they_stand_for(cx: &mut TestAppContext) {
        let (_grid, cx) = tall_grid(cx);
        // The bars are sized from the viewport measured in the frame before, so
        // draw once before looking for them.
        draw(cx);

        let vertical = cx
            .debug_bounds("scrollbar-vertical")
            .expect("a tall file has a vertical scrollbar");
        let horizontal = cx
            .debug_bounds("scrollbar-horizontal")
            .expect("a wide file has a horizontal scrollbar");
        assert!(horizontal.size.width > px(0.0));
        assert_eq!(vertical.size.width, px(SCROLLBAR_THICKNESS));
        assert_eq!(horizontal.size.height, px(SCROLLBAR_THICKNESS));

        hover_the_grid(cx);
        cx.simulate_event(wheel(0.0, -480.0));
        draw(cx);

        let after = cx
            .debug_bounds("scrollbar-vertical")
            .expect("still scrollable");
        assert_eq!(
            after.size.height, vertical.size.height,
            "scrolling must not resize the thumb"
        );
        assert!(
            after.origin.y > vertical.origin.y,
            "the thumb should move down the track: {:?} then {:?}",
            vertical,
            after
        );
    }

    #[test]
    fn a_scrollbar_thumb_shrinks_with_the_content_but_never_vanishes() {
        // Nothing to scroll: the thumb fills the track.
        assert_eq!(thumb_length(1000.0, 0.0), 1000.0);
        // Ten times the content: a tenth of the track.
        assert_eq!(thumb_length(1000.0, 9000.0), 100.0);
        // A million rows: still a visible handle.
        assert_eq!(thumb_length(1000.0, 1_000_000.0), MIN_SCROLLBAR_THUMB);
    }

    #[gpui::test]
    fn the_counts_and_state_sit_below_the_grid(cx: &mut TestAppContext) {
        let (_grid, cx) = grid(cx);

        let viewport = cx
            .debug_bounds("grid-scroll")
            .expect("the grid was laid out");
        let status = cx
            .debug_bounds("status-bar")
            .expect("the status bar was laid out");

        assert!(
            f32::from(status.origin.y) >= f32::from(viewport.bottom()),
            "the status bar is at {} but the grid ends at {}",
            status.origin.y,
            viewport.bottom()
        );
    }

    /// Click the centre of an element, the way a pointer would.
    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not laid out"));
        let centre = bounds.center();
        cx.simulate_mouse_down(centre, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(centre, MouseButton::Left, Modifiers::none());
    }

    /// Click inside an element, `dx` pixels from its left edge.
    fn click_at(cx: &mut VisualTestContext, selector: &'static str, dx: f32) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not laid out"));
        let position = point(bounds.origin.x + px(dx), bounds.center().y);
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    }

    fn mark(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) -> Selection {
        cx.update(|_window, app| grid.read(app).selection)
    }

    fn clipboard_text(cx: &mut VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[test]
    fn column_letters_run_from_a_to_z_then_double_up() {
        assert_eq!(column_label(0), "A");
        assert_eq!(column_label(25), "Z");
        assert_eq!(column_label(26), "AA");
        assert_eq!(column_label(51), "AZ");
        assert_eq!(column_label(52), "BA");
        assert_eq!(column_label(701), "ZZ");
        assert_eq!(column_label(702), "AAA");
    }

    #[gpui::test]
    fn every_row_has_a_number_and_every_column_has_a_letter(cx: &mut TestAppContext) {
        let (_grid, cx) = grid(cx);

        let first = cx
            .debug_bounds("row-number-0")
            .expect("the first row number was laid out");
        let third = cx
            .debug_bounds("row-number-2")
            .expect("the third row number was laid out");
        let letters = cx
            .debug_bounds("letter-0")
            .expect("the first column letter was laid out");
        let second = cx
            .debug_bounds("letter-1")
            .expect("the second column letter was laid out");
        let row = cx
            .debug_bounds("row-0")
            .expect("the first row was laid out");
        let header = cx
            .debug_bounds("header-0")
            .expect("the first header was laid out");

        // One number per row, one row tall apart.
        assert_eq!(third.origin.y - first.origin.y, px(ROW_HEIGHT * 2.0));
        // The numbers sit beside the data, not on top of it.
        assert!(
            first.right() <= row.origin.x,
            "the gutter at {} runs into the cells at {}",
            first.right(),
            row.origin.x
        );
        // Letters share the cells' left edges, which is the only reason they are
        // safe to read as a column label.
        assert_eq!(letters.origin.x, header.origin.x);
        assert!(second.origin.x > letters.origin.x);
    }

    #[gpui::test]
    fn clicking_a_row_number_or_a_letter_marks_that_row_or_column(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);

        click(cx, "row-number-1");
        assert_eq!(mark(cx, &grid), Selection::Row { position: 1 });

        click(cx, "letter-1");
        assert_eq!(mark(cx, &grid), Selection::Column { column: 1 });
    }

    #[gpui::test]
    fn clicking_a_cell_marks_it(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        let first_width = cx.update(|_window, app| grid.read(app).column_widths[0]);

        click_at(cx, "row-1", 10.0);
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 0
            }
        );

        click_at(cx, "row-1", first_width + 10.0);
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 1
            }
        );
    }

    #[gpui::test]
    fn cmd_c_copies_the_marked_cell_row_or_column(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        focus(cx, &grid);

        click_at(cx, "row-1", 10.0);
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(clipboard_text(cx).as_deref(), Some("grace"));

        click(cx, "row-number-2");
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(clipboard_text(cx).as_deref(), Some("alan\t1337"));

        click(cx, "letter-1");
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(clipboard_text(cx).as_deref(), Some("412\n88\n1337"));
    }

    #[gpui::test]
    fn arrow_keys_move_the_mark_one_cell_at_a_time(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        focus(cx, &grid);

        cx.simulate_keystrokes("down");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 0
            }
        );

        cx.simulate_keystrokes("right right");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 1
            },
            "the right edge must stop rather than fall off the table"
        );

        cx.simulate_keystrokes("up up");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 0,
                column: 1
            }
        );
    }

    #[gpui::test]
    fn the_status_strip_adds_and_removes_rows_and_columns(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);

        click(cx, "edit-insert-row");
        assert_eq!(visible_rows(cx, &grid), 4);
        assert!(cx.update(|_window, app| grid.read(app).is_dirty()));
        draw(cx);

        click(cx, "row-number-0");
        click(cx, "edit-delete-row");
        assert_eq!(visible_rows(cx, &grid), 3);
        draw(cx);

        click(cx, "edit-insert-column");
        assert_eq!(cx.update(|_window, app| grid.read(app).column_count()), 3);
        draw(cx);

        click(cx, "letter-2");
        click(cx, "edit-delete-column");
        assert_eq!(cx.update(|_window, app| grid.read(app).column_count()), 2);
    }

    #[gpui::test]
    fn an_edit_marks_the_document_dirty_and_save_writes_it_back(cx: &mut TestAppContext) {
        let directory = std::env::temp_dir().join(format!("bromine-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("edits.csv");
        std::fs::write(&path, "name,commits\nada,412\n").expect("the fixture is written");

        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let document = load_csv(&path).expect("the fixture loads");
            CsvGrid::new(path.clone(), document, cx)
        });
        focus(cx, &grid);

        click(cx, "edit-insert-row");
        assert_eq!(visible_rows(cx, &grid), 2);
        assert!(cx.update(|_window, app| grid.read(app).is_dirty()));

        cx.update(|_window, app| grid.update(app, |grid, cx| grid.save(cx)));

        assert!(!cx.update(|_window, app| grid.read(app).is_dirty()));
        let reloaded = load_csv(&path).expect("the saved file still loads");
        assert_eq!(reloaded.table.row_count(), 2);
        assert_eq!(reloaded.table.get(0, 1), Some("412"));

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[gpui::test]
    fn a_lossily_decoded_file_is_not_written_back(cx: &mut TestAppContext) {
        let directory = std::env::temp_dir().join(format!("bromine-lossy-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("lossy.csv");
        let original = b"name,commits\nada,412\n".to_vec();
        std::fs::write(&path, &original).expect("the fixture is written");

        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let mut document = load_csv(&path).expect("the fixture loads");
            document.lossy = true;
            CsvGrid::new(path.clone(), document, cx)
        });
        focus(cx, &grid);

        click(cx, "edit-insert-row");
        cx.update(|_window, app| grid.update(app, |grid, cx| grid.save(cx)));

        assert!(
            cx.update(|_window, app| grid.read(app).save_error.is_some()),
            "a lossy document must say why it did not save"
        );
        assert_eq!(
            std::fs::read(&path).expect("the file is readable"),
            original,
            "a lossy document must leave the file alone"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[gpui::test]
    fn the_row_numbers_follow_the_rows_and_never_move_sideways(cx: &mut TestAppContext) {
        let (_grid, cx) = tall_grid(cx);
        hover_the_grid(cx);

        cx.simulate_event(wheel(0.0, -480.0));
        draw(cx);

        let number = cx
            .debug_bounds("row-number-20")
            .expect("the twentieth row number is in view");
        let row = cx
            .debug_bounds("row-20")
            .expect("the twentieth row is in view");
        assert_eq!(
            number.origin.y, row.origin.y,
            "the number and its row must share a top edge"
        );

        // Sideways movement belongs to the cells, not to the gutter. A new
        // gesture rather than a continued one, because a gesture keeps the axis
        // it started on.
        cx.simulate_event(wheel_with(-240.0, 0.0, TouchPhase::Started));
        draw(cx);

        let after = cx
            .debug_bounds("row-number-20")
            .expect("the number is still in view");
        assert_eq!(
            after.origin.x, number.origin.x,
            "the gutter must not pan with the cells"
        );
        assert_eq!(after.origin.y, number.origin.y);

        let moved = cx.debug_bounds("row-20").expect("the row is still in view");
        assert_eq!(
            moved.origin.x,
            row.origin.x - px(240.0),
            "the cells should have panned while the numbers held still"
        );
    }

    // --- cell editing ---

    /// Bind the shared editing keys, the way the binary does at launch.
    ///
    /// Without these bindings a keystroke would reach the grid's own listener and
    /// be typed into the search box, which is the failure these tests exist to
    /// catch rather than repeat.
    fn bind_editing_keys(cx: &mut VisualTestContext) {
        cx.update(|_window, app| crate::text_actions::bind_text_editor_keys(app));
    }

    /// The x offset from a row's left edge where a column's text begins.
    fn cell_x(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>, column: usize) -> f32 {
        cx.update(|_window, app| {
            let grid = grid.read(app);
            grid.column_widths.iter().take(column).sum::<f32>() + CELL_PADDING_X + 2.0
        })
    }

    /// Double click inside a row, `dx` from its left edge.
    ///
    /// `simulate_mouse_down` hardcodes a click count of one, so the pair is
    /// dispatched by hand.
    fn double_click_at(cx: &mut VisualTestContext, selector: &'static str, dx: f32) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not laid out"));
        let position = point(bounds.origin.x + px(dx), bounds.center().y);
        for click_count in [1, 2] {
            cx.simulate_event(MouseDownEvent {
                position,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            });
            cx.simulate_event(MouseUpEvent {
                position,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
            });
        }
    }

    /// Open an edit on a cell with the caret at the end of its value.
    ///
    /// Every test that types wants to append, and where a double click puts the
    /// caret on its own is `double_clicking_a_cell_opens_an_edit…`'s job.
    fn edit_cell(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>, row: usize, column: usize) {
        draw(cx);
        let x = cell_x(cx, grid, column);
        let selector: &'static str = match row {
            0 => "row-0",
            1 => "row-1",
            2 => "row-2",
            _ => panic!("the tests only edit rows 0 to 2"),
        };
        double_click_at(cx, selector, x);
        draw(cx);
        cx.simulate_keystrokes("end");
    }

    fn cell(
        cx: &mut VisualTestContext,
        grid: &Entity<CsvGrid>,
        position: usize,
        column: usize,
    ) -> String {
        cx.update(|_window, app| {
            grid.read(app)
                .cell_at(position, column)
                .unwrap_or_default()
                .to_string()
        })
    }

    fn caret(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) -> Option<usize> {
        cx.update(|_window, app| {
            grid.read(app)
                .session
                .as_ref()
                .map(|session| session.editor.caret)
        })
    }

    fn editing(cx: &mut VisualTestContext, grid: &Entity<CsvGrid>) -> bool {
        cx.update(|_window, app| grid.read(app).is_editing())
    }

    #[gpui::test]
    fn double_clicking_a_cell_opens_an_edit_with_the_caret_under_the_pointer(
        cx: &mut TestAppContext,
    ) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        draw(cx);
        let x = cell_x(cx, &grid, 1);
        double_click_at(cx, "row-0", x);
        draw(cx);

        assert!(editing(cx, &grid));
        assert_eq!(
            caret(cx, &grid),
            Some(0),
            "the click was at the value's start"
        );
    }

    #[gpui::test]
    fn enter_on_a_marked_cell_opens_an_edit_with_the_caret_at_the_end(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        click_at(cx, "row-0", 10.0);
        cx.simulate_keystrokes("enter");
        draw(cx);

        assert!(editing(cx, &grid));
        assert_eq!(caret(cx, &grid), Some(3), "the caret appends to \"ada\"");
    }

    #[gpui::test]
    fn typing_into_an_open_cell_changes_that_cell_and_leaves_the_query_alone(
        cx: &mut TestAppContext,
    ) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("x");

        assert_eq!(cell(cx, &grid, 0, 1), "412x");
        assert_eq!(
            cx.update(|_window, app| grid.read(app).query().to_string()),
            ""
        );
        assert_eq!(
            visible_rows(cx, &grid),
            3,
            "typing in a cell must not filter the table"
        );
    }

    #[gpui::test]
    fn backspace_edits_the_cell_rather_than_the_query(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_keystrokes("backspace");

        assert_eq!(cell(cx, &grid, 0, 1), "41");
        assert_eq!(
            cx.update(|_window, app| grid.read(app).query().to_string()),
            ""
        );
    }

    #[gpui::test]
    fn enter_commits_and_marks_the_cell_one_row_down(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 0);
        cx.simulate_input("!");
        cx.simulate_keystrokes("enter");

        assert!(!editing(cx, &grid));
        assert_eq!(cell(cx, &grid, 0, 0), "ada!");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 0
            }
        );
    }

    #[gpui::test]
    fn tab_commits_and_marks_the_next_column(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 0);
        cx.simulate_input("!");
        cx.simulate_keystrokes("tab");

        assert!(!editing(cx, &grid));
        assert_eq!(cell(cx, &grid, 0, 0), "ada!");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 0,
                column: 1
            }
        );
    }

    #[gpui::test]
    fn escape_restores_the_value_and_leaves_no_undo_step(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 1, 1);
        cx.simulate_input("9");
        assert_eq!(cell(cx, &grid, 1, 1), "889");

        cx.simulate_keystrokes("escape");
        assert!(!editing(cx, &grid));
        assert_eq!(cell(cx, &grid, 1, 1), "88");

        // Cancelling is not an edit, so there is nothing to undo and nothing to
        // save, and the next Cmd+Z must not bring the cancelled text back.
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(cell(cx, &grid, 1, 1), "88");
        assert!(!cx.update(|_window, app| grid.read(app).is_dirty()));
    }

    #[gpui::test]
    fn escape_after_a_commit_undoes_the_previous_edit_not_the_cancelled_one(
        cx: &mut TestAppContext,
    ) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 0);
        cx.simulate_input("1");
        cx.simulate_keystrokes("enter");
        assert_eq!(cell(cx, &grid, 0, 0), "ada1");

        // The mark is on row 1 now, so row 0 is opened again by hand.
        edit_cell(cx, &grid, 0, 0);
        cx.simulate_input("2");
        cx.simulate_keystrokes("escape");
        assert_eq!(cell(cx, &grid, 0, 0), "ada1");

        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            cell(cx, &grid, 0, 0),
            "ada",
            "undo reaches the committed edit, not the cancelled one"
        );
    }

    #[gpui::test]
    fn clicking_another_cell_commits_the_open_edit(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("x");
        draw(cx);
        click_at(cx, "row-1", 10.0);

        assert!(!editing(cx, &grid));
        assert_eq!(cell(cx, &grid, 0, 1), "412x");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 1,
                column: 0
            }
        );
    }

    #[gpui::test]
    fn sorting_while_editing_commits_first(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("x");
        draw(cx);
        click(cx, "header-1");

        assert!(!editing(cx, &grid));
        // The sort moved the rows, so the value is read from the table rather
        // than from the position it used to occupy.
        assert_eq!(
            cx.update(|_window, app| grid.read(app).document.table.get(0, 1).map(str::to_string)),
            Some("412x".to_string())
        );
        assert!(cx.update(|_window, app| grid.read(app).sort()).is_some());
    }

    #[gpui::test]
    fn the_left_and_right_keys_move_the_caret_while_a_cell_is_open(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_keystrokes("left left");
        assert_eq!(caret(cx, &grid), Some(1));
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 0,
                column: 1
            },
            "the mark must not slide out from under the open cell"
        );

        cx.simulate_keystrokes("right");
        assert_eq!(caret(cx, &grid), Some(2));
    }

    #[gpui::test]
    fn the_up_and_down_keys_commit_and_move_the_mark(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 1, 1);
        cx.simulate_input("9");
        cx.simulate_keystrokes("down");

        assert!(!editing(cx, &grid));
        assert_eq!(cell(cx, &grid, 1, 1), "889");
        assert_eq!(
            mark(cx, &grid),
            Selection::Cell {
                position: 2,
                column: 1
            }
        );
    }

    #[gpui::test]
    fn cmd_c_inside_a_cell_copies_the_selection(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_keystrokes("home shift-end cmd-c");

        assert_eq!(clipboard_text(cx).as_deref(), Some("412"));
    }

    #[gpui::test]
    fn a_paste_with_a_line_break_is_refused_and_explained(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.write_to_clipboard(ClipboardItem::new_string("a\nb".to_string()));
        cx.simulate_keystrokes("cmd-v");

        assert_eq!(cell(cx, &grid, 0, 1), "412", "the paste changed nothing");
        assert!(
            cx.update(|_window, app| grid.read(app).notice.is_some()),
            "a refused paste has to say why"
        );
        assert!(!cx.update(|_window, app| grid.read(app).is_dirty()));
    }

    #[gpui::test]
    fn undo_and_redo_of_a_cell_edit_survive_switching_cells(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("x");
        cx.simulate_keystrokes("enter");
        edit_cell(cx, &grid, 1, 1);
        cx.simulate_input("y");
        cx.simulate_keystrokes("enter");

        cx.simulate_keystrokes("cmd-z");
        assert_eq!(cell(cx, &grid, 1, 1), "88");
        assert_eq!(cell(cx, &grid, 0, 1), "412x");

        cx.simulate_keystrokes("cmd-z");
        assert_eq!(cell(cx, &grid, 0, 1), "412");

        cx.simulate_keystrokes("shift-cmd-z");
        assert_eq!(cell(cx, &grid, 0, 1), "412x");
    }

    #[gpui::test]
    fn undo_after_a_row_insert_removes_that_row_and_restores_the_mark(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        // Marked first, because an insert with nothing marked lands at the end
        // and the mark is what names the row the insert created.
        click_at(cx, "row-0", 10.0);
        click(cx, "edit-insert-row");
        assert_eq!(visible_rows(cx, &grid), 4);

        cx.simulate_keystrokes("cmd-z");

        assert_eq!(visible_rows(cx, &grid), 3);
        assert_eq!(mark(cx, &grid), Selection::Row { position: 1 });
        assert!(!cx.update(|_window, app| grid.read(app).is_dirty()));
    }

    #[gpui::test]
    fn a_committed_value_widens_only_its_own_column(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);
        let before = cx.update(|_window, app| grid.read(app).column_widths.clone());

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input(" with a much longer value than before");
        cx.simulate_keystrokes("enter");

        let after = cx.update(|_window, app| grid.read(app).column_widths.clone());
        assert!(after[1] > before[1], "the edited column grows to fit");
        assert_eq!(after[0], before[0], "the other column keeps its width");
    }

    #[gpui::test]
    fn a_value_that_stops_matching_the_filter_drops_the_mark_on_commit(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        cx.simulate_input("ada");
        assert_eq!(visible_rows(cx, &grid), 1);

        edit_cell(cx, &grid, 0, 0);
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("zzz");
        cx.simulate_keystrokes("enter");

        assert_eq!(visible_rows(cx, &grid), 0);
        assert_eq!(mark(cx, &grid), Selection::None);
    }

    #[gpui::test]
    fn the_caret_and_the_strip_name_the_cell_being_edited(cx: &mut TestAppContext) {
        let (grid, cx) = grid(cx);
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 1, 1);

        assert!(
            cx.debug_bounds("cell-caret").is_some(),
            "an open cell draws a caret"
        );
        let label = cx
            .debug_bounds("editing-cell")
            .expect("the strip names the edited cell");
        let row = cx.debug_bounds("row-1").expect("the row is drawn");
        assert!(label.origin.y > row.origin.y, "the label sits in the strip");
    }

    #[gpui::test]
    fn save_commits_the_open_cell_and_clears_the_unsaved_state(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("bromine-cell-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("cells.csv");
        std::fs::write(&path, "name,commits\nada,412\n").expect("the fixture is written");

        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let document = load_csv(&path).expect("the fixture loads");
            CsvGrid::new(path.clone(), document, cx)
        });
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("9");
        cx.update(|_window, app| grid.update(app, |grid, cx| grid.save(cx)));

        assert!(!editing(cx, &grid), "saving closes the open cell");
        assert!(!cx.update(|_window, app| grid.read(app).is_dirty()));
        let saved = std::fs::read_to_string(&path).expect("the file is readable");
        assert_eq!(saved, "name,commits\nada,4129\n");

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[gpui::test]
    fn a_lossy_file_can_be_edited_but_still_refuses_to_save(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("bromine-cell-lossy-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("lossy.csv");
        let original = b"name,commits\nada,412\n".to_vec();
        std::fs::write(&path, &original).expect("the fixture is written");

        let (grid, cx) = cx.add_window_view(|_window, cx| {
            let mut document = load_csv(&path).expect("the fixture loads");
            document.lossy = true;
            CsvGrid::new(path.clone(), document, cx)
        });
        bind_editing_keys(cx);
        focus(cx, &grid);

        edit_cell(cx, &grid, 0, 1);
        cx.simulate_input("9");
        cx.simulate_keystrokes("enter");
        assert_eq!(cell(cx, &grid, 0, 1), "4129", "the edit is kept in memory");

        cx.update(|_window, app| grid.update(app, |grid, cx| grid.save(cx)));
        assert!(cx.update(|_window, app| grid.read(app).save_error.is_some()));
        assert_eq!(
            std::fs::read(&path).expect("the file is readable"),
            original,
            "a lossy document must leave the file alone"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }
}
