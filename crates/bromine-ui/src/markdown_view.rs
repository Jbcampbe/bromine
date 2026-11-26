//! The Markdown editor.
//!
//! Markdown is shown as prose with its punctuation hidden, except on the block
//! the caret is in. That is the Bear arrangement: reading is quiet, and editing
//! reveals exactly the markers around what is being edited, in place. The source
//! is the only truth; every edit reparses the document, so row N stays line N
//! and a click can always be turned back into a byte offset.

use std::ops::Range;
use std::path::{Path, PathBuf};

use bromine_core::{
    Alignment, BlockId, CellPosition, EditKind, MarkdownDocument, Row, RowKind, UndoOutcome,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, Element, ElementId, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point, Render, ScrollHandle,
    SharedString, Style, TextAlign, UTF16Selection, Window, div, point, prelude::*, px, relative,
    size, uniform_list,
};

use crate::editor::{self, EditorState, TextInputBridge};
use crate::markdown_image::image_element;
use crate::markdown_row::{
    CellLayouts, RowLayouts, ShapedTextElement, TextKey, base_style, row_display_offset,
    row_source_offset, row_visual,
};
use crate::markdown_table::table_element;
use crate::text_actions::{
    Backspace, Copy, Cut, Delete, DocumentEnd, DocumentStart, Down, End, Enter, Home, Left,
    Outdent, Paste, Redo, Right, SelectAll, SelectDown, SelectEnd, SelectHome, SelectLeft,
    SelectRight, SelectUp, SelectWordLeft, SelectWordRight, Tab, ToggleBold, ToggleItalic, Undo,
    Up, WordLeft, WordRight,
};
use crate::theme;

/// The widest the prose column gets. Past this, long lines become hard to track
/// and the eye spends more time finding the start of the next line.
pub(crate) const COLUMN_MAX_WIDTH: f32 = 720.0;

/// The padding between the column and the window edge.
const COLUMN_PADDING: f32 = 32.0;

/// The height of the read-only fallback's rows.
const FALLBACK_ROW_HEIGHT: f32 = 20.0;

/// A file over this size is opened read-only.
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// A file with more rows than this is opened read-only.
const MAX_ROWS: usize = 20_000;

/// The width of the gutter that holds list markers and checkboxes.
const PREFIX_WIDTH: f32 = 26.0;

pub struct MarkdownView {
    path: PathBuf,
    focus_handle: FocusHandle,
    pub(crate) document: MarkdownDocument,
    pub(crate) editor: EditorState,
    pub(crate) active_block: Option<BlockId>,
    pub(crate) cell: Option<CellPosition>,
    pub(crate) row_layouts: RowLayouts,
    pub(crate) cell_layouts: CellLayouts,
    scroll: ScrollHandle,
    save_error: Option<SharedString>,
    read_only: Option<SharedString>,
}

impl MarkdownView {
    pub fn new(path: PathBuf, document: MarkdownDocument, cx: &mut Context<Self>) -> Self {
        let read_only = if document.text().len() > MAX_FILE_BYTES {
            Some(SharedString::from(format!(
                "read only: {} KB is over the {MAX_FILE_BYTES} byte editing limit",
                document.text().len() / 1024
            )))
        } else if document.rows().len() > MAX_ROWS {
            Some(SharedString::from(format!(
                "read only: {} rows is over the {MAX_ROWS} row editing limit",
                document.rows().len()
            )))
        } else {
            None
        };

        Self {
            path,
            focus_handle: cx.focus_handle(),
            document,
            editor: EditorState::new(0),
            active_block: None,
            cell: None,
            row_layouts: RowLayouts::default(),
            cell_layouts: CellLayouts::default(),
            scroll: ScrollHandle::new(),
            save_error: None,
            read_only,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    pub fn is_dirty(&self) -> bool {
        self.document.is_dirty()
    }

    pub fn document(&self) -> &MarkdownDocument {
        &self.document
    }

    pub fn caret(&self) -> usize {
        self.editor.caret
    }

    pub fn selection(&self) -> Range<usize> {
        self.editor.selection()
    }

    pub fn active_block(&self) -> Option<BlockId> {
        self.active_block
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only.is_some()
    }

    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref().map(|error| &**error)
    }

    /// Write the document back over its file.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        match self.document.save() {
            Ok(()) => self.save_error = None,
            Err(error) => self.save_error = Some(SharedString::from(error.to_string())),
        }
        cx.notify();
    }

    /// Record the shaped layout of a row or cell for click and IME mapping.
    pub(crate) fn record_text_layout(
        &mut self,
        key: TextKey,
        record: crate::markdown_row::TextLayoutRecord,
    ) {
        match key {
            TextKey::Row(index) => {
                self.row_layouts.insert(index, record);
            }
            TextKey::Cell { .. } => {
                self.cell_layouts.insert(key, record);
            }
        }
    }

    fn refresh_active(&mut self) {
        self.active_block = self.document.block_of_offset(self.editor.caret);
        self.cell = self.document.cell_at_offset(self.editor.caret);
    }

    fn current_cell(&self) -> Option<(CellPosition, Range<usize>)> {
        let position = self.cell?;
        let cell = self
            .document
            .tables()
            .get(position.table)?
            .cell(position.row, position.column)?;
        Some((position, cell.source.clone()))
    }

    /// Apply an edit, keeping a table cell's pipes out of reach.
    ///
    /// A cell's source range excludes its pipes and padding, so a selection that
    /// starts in one cell and ends in another is clamped to the cell the caret
    /// is in. That is the only place an edit can safely land.
    fn apply_edit(&mut self, range: Range<usize>, text: &str, kind: EditKind) -> usize {
        let range = range.start.min(self.document.text().len())
            ..range
                .end
                .min(self.document.text().len())
                .max(range.start.min(self.document.text().len()));
        let before = self.editor.caret;

        if let Some((position, cell)) = self.current_cell() {
            let start = range.start.clamp(cell.start, cell.end);
            let end = range.end.clamp(cell.start, cell.end).max(start);
            if let Some(edit) = self.document.replace_cell(
                position,
                start - cell.start..end - cell.start,
                text,
                before,
                kind,
            ) {
                return edit.caret;
            }
            return self.editor.caret;
        }

        self.document
            .replace(range.clone(), text, before, range.start + text.len(), kind)
    }

    fn after_edit(&mut self, caret: usize, cx: &mut Context<Self>) {
        self.editor.marked = None;
        self.editor.move_to(caret.min(self.document.text().len()));
        self.refresh_active();
        cx.notify();
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        let caret = self.apply_edit(self.editor.selection(), text, EditKind::Typing);
        self.after_edit(caret, cx);
    }

    fn on_row_mouse_down(
        &mut self,
        row: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only.is_some() {
            return;
        }
        self.focus(window);
        self.document.break_undo_group();
        self.editor.dragging = true;
        let offset = self.offset_for_row_position(row, event.position);
        if event.modifiers.shift {
            self.editor.select_to(offset);
        } else {
            self.editor.move_to(offset);
        }
        self.refresh_active();
        cx.notify();
    }

    pub(crate) fn on_cell_mouse_down(
        &mut self,
        table: usize,
        row: usize,
        column: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only.is_some() {
            return;
        }
        self.focus(window);
        self.document.break_undo_group();
        self.editor.dragging = true;
        let position = CellPosition { table, row, column };
        let offset = self
            .cell_layouts
            .get(&TextKey::Cell { table, row, column })
            .and_then(|record| {
                if !record.bounds.contains(&event.position) {
                    return None;
                }
                let local = event.position - record.bounds.origin;
                let display = match record
                    .line
                    .closest_index_for_position(local, record.line_height)
                {
                    Ok(index) | Err(index) => index,
                };
                let cell = self.document.tables().get(table)?.cell(row, column)?;
                Some((cell.source.start + display).min(cell.source.end))
            })
            .or_else(|| {
                self.document
                    .tables()
                    .get(table)?
                    .cell(row, column)
                    .map(|cell| cell.source.start)
            })
            .unwrap_or(0);

        self.cell = Some(position);
        self.active_block = self.document.block_of_offset(offset);
        self.editor.move_to(offset);
        cx.notify();
    }

    fn offset_for_row_position(&self, row: usize, position: Point<gpui::Pixels>) -> usize {
        if let Some(record) = self.row_layouts.get(&row)
            && record.bounds.contains(&position)
        {
            let local = position - record.bounds.origin;
            let display = match record.line.index_for_position(local, record.line_height) {
                Ok(index) | Err(index) => index,
            };
            return row_source_offset(self, row, display);
        }
        self.document
            .rows()
            .get(row)
            .map(|row| row.source.start)
            .unwrap_or(0)
    }

    fn offset_for_position(&self, position: Point<gpui::Pixels>) -> usize {
        for (row, record) in &self.row_layouts {
            if record.bounds.contains(&position) {
                let local = position - record.bounds.origin;
                let display = match record
                    .line
                    .closest_index_for_position(local, record.line_height)
                {
                    Ok(index) | Err(index) => index,
                };
                return row_source_offset(self, *row, display);
            }
        }
        for (key, record) in &self.cell_layouts {
            if !record.bounds.contains(&position) {
                continue;
            }
            let TextKey::Cell { table, row, column } = *key else {
                continue;
            };
            let local = position - record.bounds.origin;
            let display = match record
                .line
                .closest_index_for_position(local, record.line_height)
            {
                Ok(index) | Err(index) => index,
            };
            if let Some(cell) = self
                .document
                .tables()
                .get(table)
                .and_then(|table| table.cell(row, column))
            {
                return (cell.source.start + display).min(cell.source.end);
            }
        }
        self.editor.caret
    }

    fn vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let line = self.document.text().line_of(self.editor.caret);
        let last = self.document.rows().len().saturating_sub(1);
        let target_line = (line as isize + delta).clamp(0, last as isize) as usize;

        let desired = self.editor.desired_x.unwrap_or_else(|| {
            let x = self
                .row_layouts
                .get(&line)
                .and_then(|record| {
                    let display = row_display_offset(self, line, self.editor.caret);
                    record
                        .line
                        .position_for_index(display, record.line_height)
                        .map(|position| f32::from(position.x))
                })
                .unwrap_or(0.0);
            self.editor.desired_x = Some(x);
            x
        });

        let target = self
            .row_layouts
            .get(&target_line)
            .map(|record| {
                let position = point(px(desired), record.line_height / 2.0);
                let display = match record
                    .line
                    .closest_index_for_position(position, record.line_height)
                {
                    Ok(index) | Err(index) => index,
                };
                row_source_offset(self, target_line, display)
            })
            .or_else(|| {
                self.document
                    .rows()
                    .get(target_line)
                    .map(|row| row.source.start)
            })
            .unwrap_or(0);

        self.document.break_undo_group();
        if extend {
            self.editor.select_to(target);
        } else {
            self.editor.move_to(target);
        }
        self.editor.desired_x = Some(desired);
        self.refresh_active();
        cx.notify();
    }

    fn navigate_cell(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(position) = self.cell else {
            return;
        };
        let Some(table) = self.document.tables().get(position.table) else {
            return;
        };
        let columns = table.column_count().max(1);
        let current = position.row.saturating_mul(columns) + position.column;
        let last = table.row_count().saturating_mul(columns).saturating_sub(1);
        let target = (current as isize + delta).clamp(0, last as isize) as usize;
        let row = target / columns;
        let column = target % columns;
        let Some(cell) = table.cell(row, column) else {
            return;
        };
        self.cell = Some(CellPosition {
            table: position.table,
            row,
            column,
        });
        let start = cell.source.start;
        self.active_block = self.document.block_of_offset(start);
        self.document.break_undo_group();
        self.editor.move_to(start);
        cx.notify();
    }

    fn list_prefix(&self, row: &Row) -> Option<String> {
        let line = &self.document.source()[row.source.clone()];
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];

        let (marker, rest) = if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            (trimmed[..1].to_string(), rest)
        } else {
            let digits = trimmed
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .count();
            if digits == 0 {
                return None;
            }
            let after = &trimmed[digits..];
            let mut characters = after.chars();
            match (characters.next(), characters.next()) {
                (Some('.'), Some(' ')) => {
                    let number: u64 = trimmed[..digits].parse().unwrap_or(1);
                    (format!("{}.", number + 1), &after[2..])
                }
                (Some(')'), Some(' ')) => {
                    let number: u64 = trimmed[..digits].parse().unwrap_or(1);
                    (format!("{})", number + 1), &after[2..])
                }
                _ => return None,
            }
        };

        let task =
            if rest.starts_with("[ ] ") || rest.starts_with("[x] ") || rest.starts_with("[X] ") {
                "[ ] "
            } else {
                ""
            };
        Some(format!("{indent}{marker} {task}"))
    }

    fn enter(&mut self, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if self.cell.is_some() {
            self.navigate_cell(1, cx);
            return;
        }

        let line = self.document.text().line_of(self.editor.caret);
        if let Some(row) = self.document.rows().get(line)
            && row.kind == RowKind::ListItem
            && let Some(prefix) = self.list_prefix(row)
        {
            let text = format!("\n{prefix}");
            let caret = self.apply_edit(self.editor.selection(), &text, EditKind::Typing);
            self.after_edit(caret, cx);
            return;
        }

        self.insert_text("\n", cx);
    }

    fn indent(&mut self, outdent: bool, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if self.cell.is_some() {
            self.navigate_cell(if outdent { -1 } else { 1 }, cx);
            return;
        }

        let line = self.document.text().line_of(self.editor.caret);
        let Some(row) = self.document.rows().get(line) else {
            return;
        };
        let start = row.source.start;

        if outdent {
            let mut remove = 0;
            let source = self.document.source();
            while remove < 2
                && source
                    .as_bytes()
                    .get(start + remove)
                    .is_some_and(|byte| *byte == b' ')
            {
                remove += 1;
            }
            if remove > 0 {
                let caret = self.apply_edit(start..start + remove, "", EditKind::Other);
                self.after_edit(caret, cx);
            }
            return;
        }

        let caret = self.apply_edit(start..start, "  ", EditKind::Typing);
        self.after_edit(caret, cx);
    }

    fn wrap_selection(&mut self, marker: &str, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        let selection = self.editor.selection();
        if selection.is_empty() {
            let caret = self.apply_edit(
                selection.clone(),
                &format!("{marker}{marker}"),
                EditKind::Typing,
            );
            self.editor.marked = None;
            self.editor.move_to(caret - marker.len());
            self.refresh_active();
            cx.notify();
            return;
        }

        let text = self.document.source()[selection.clone()].to_string();
        let replacement = format!("{marker}{text}{marker}");
        let caret = self.apply_edit(selection.clone(), &replacement, EditKind::Typing);
        self.editor.marked = None;
        self.editor.move_to(selection.start + marker.len());
        self.editor.anchor = caret.saturating_sub(marker.len());
        self.refresh_active();
        cx.notify();
    }

    fn toggle_task(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        let Some(row) = self.document.rows().get(row) else {
            return;
        };
        let source = self.document.source()[row.source.clone()].to_string();
        let Some(at) = source.find('[') else {
            return;
        };
        let Some(state) = source[at..].chars().nth(1) else {
            return;
        };
        let replacement = match state {
            ' ' => "[x]",
            'x' | 'X' => "[ ]",
            _ => return,
        };
        let start = row.source.start + at;
        self.document.break_undo_group();
        let caret = self.apply_edit(start..start + 3, replacement, EditKind::Other);
        self.editor.marked = None;
        self.editor.move_to(caret.min(self.document.text().len()));
        self.refresh_active();
        cx.notify();
    }

    pub(crate) fn table_add_row(&mut self, table: usize, cx: &mut Context<Self>) {
        let at = self.cell.map(|cell| cell.row).unwrap_or(0);
        if self.document.add_table_row(table, at).is_some() {
            self.after_table_edit(table, cx);
        }
    }

    pub(crate) fn table_remove_row(&mut self, table: usize, cx: &mut Context<Self>) {
        let Some(row) = self.cell.map(|cell| cell.row) else {
            return;
        };
        if row == 0 {
            return;
        }
        if self.document.remove_table_row(table, row - 1).is_some() {
            self.after_table_edit(table, cx);
        }
    }

    pub(crate) fn table_add_column(&mut self, table: usize, cx: &mut Context<Self>) {
        let at = self.cell.map(|cell| cell.column + 1).unwrap_or(0);
        if self.document.add_table_column(table, at).is_some() {
            self.after_table_edit(table, cx);
        }
    }

    pub(crate) fn table_remove_column(&mut self, table: usize, cx: &mut Context<Self>) {
        let Some(column) = self.cell.map(|cell| cell.column) else {
            return;
        };
        if self.document.remove_table_column(table, column).is_some() {
            self.after_table_edit(table, cx);
        }
    }

    pub(crate) fn table_align(
        &mut self,
        table: usize,
        alignment: Alignment,
        cx: &mut Context<Self>,
    ) {
        let column = self.cell.map(|cell| cell.column).unwrap_or(0);
        if self
            .document
            .set_table_alignment(table, column, alignment)
            .is_some()
        {
            self.after_table_edit(table, cx);
        }
    }

    fn after_table_edit(&mut self, table: usize, cx: &mut Context<Self>) {
        if let Some(position) = self.cell
            && let Some(block) = self.document.tables().get(table)
        {
            let row = position.row.min(block.row_count().saturating_sub(1));
            let column = position.column.min(block.column_count().saturating_sub(1));
            if let Some(cell) = block.cell(row, column) {
                self.cell = Some(CellPosition { table, row, column });
                self.editor.marked = None;
                self.editor.move_to(cell.source.start);
            }
        }
        self.refresh_active();
        cx.notify();
    }
}

impl EntityInputHandler for MarkdownView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let source = self.document.source();
        let range = editor::range_from_utf16(source, &range_utf16);
        let start = range.start.min(source.len());
        let end = range.end.min(source.len()).max(start);
        actual_range.replace(editor::range_to_utf16(source, &(start..end)));
        Some(source[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(editor::utf16_selection(
            self.document.source(),
            &self.editor,
        ))
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.editor
            .marked
            .as_ref()
            .map(|range| editor::range_to_utf16(self.document.source(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.editor.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only.is_some() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| editor::range_from_utf16(self.document.source(), range))
            .or_else(|| self.editor.marked.clone())
            .unwrap_or_else(|| self.editor.selection());
        let caret = self.apply_edit(range, text, EditKind::Typing);
        self.after_edit(caret, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only.is_some() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| editor::range_from_utf16(self.document.source(), range))
            .or_else(|| self.editor.marked.clone())
            .unwrap_or_else(|| self.editor.selection());
        let start = range.start;
        let caret = self.apply_edit(range, new_text, EditKind::Typing);
        if new_text.is_empty() {
            self.editor.marked = None;
        } else {
            self.editor.marked = Some(start..start + new_text.len());
        }
        self.editor.move_to(caret);
        if let Some(selected) = new_selected_range {
            let selected = editor::range_from_utf16(new_text, &selected);
            self.editor.select_to(start + selected.end);
        }
        self.refresh_active();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        let offset = editor::offset_from_utf16(self.document.source(), range_utf16.start);
        let row = self.document.text().line_of(offset);

        if let Some(record) = self.row_layouts.get(&row) {
            let display = row_display_offset(self, row, offset);
            if let Some(position) = record.line.position_for_index(display, record.line_height) {
                return Some(Bounds::new(
                    point(
                        record.bounds.left() + position.x,
                        record.bounds.top() + position.y,
                    ),
                    size(px(2.0), record.line_height),
                ));
            }
        }

        for (key, record) in &self.cell_layouts {
            let TextKey::Cell { table, row, column } = *key else {
                continue;
            };
            let Some(cell) = self
                .document
                .tables()
                .get(table)
                .and_then(|table| table.cell(row, column))
            else {
                continue;
            };
            if offset < cell.source.start || offset > cell.source.end {
                continue;
            }
            if let Some(position) = record
                .line
                .position_for_index(offset - cell.source.start, record.line_height)
            {
                return Some(Bounds::new(
                    point(
                        record.bounds.left() + position.x,
                        record.bounds.top() + position.y,
                    ),
                    size(px(2.0), record.line_height),
                ));
            }
        }

        Some(Bounds::new(element_bounds.origin, size(px(2.0), px(20.0))))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_position(position);
        Some(editor::offset_to_utf16(self.document.source(), offset))
    }
}

impl MarkdownView {
    fn row_element(
        &self,
        entity: Entity<MarkdownView>,
        index: usize,
        focused: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let row = &self.document.rows()[index];
        let base = base_style(row);

        if row.kind == RowKind::Rule {
            return div()
                .w_full()
                .flex_shrink_0()
                .py(px(8.0))
                .child(div().w_full().h(px(1.0)).bg(theme::border()))
                .into_any_element();
        }

        if matches!(row.kind, RowKind::TableRow { .. } | RowKind::TableDelimiter) {
            return div().into_any_element();
        }

        // A line that only carried opening or closing HTML tags is syntax, not
        // content. Collapsing it keeps a run of `<p>` wrappers from adding
        // blank lines to the page; the active block still reveals the tag.
        if row.kind == RowKind::Html
            && self.active_block != Some(row.block)
            && row.text.trim().is_empty()
        {
            return div().into_any_element();
        }

        let align = match row.alignment {
            Alignment::Center => TextAlign::Center,
            Alignment::Right => TextAlign::Right,
            _ => TextAlign::Left,
        };

        let block = self.document.block(row.block);
        let is_first_in_block = block
            .map(|block| block.rows.start == index)
            .unwrap_or(false);
        let is_last_in_block = block
            .map(|block| block.rows.end.saturating_sub(1) == index)
            .unwrap_or(false);

        let mut container = div()
            .debug_selector({
                let selector = format!("md-row-{index}");
                move || selector
            })
            .flex()
            .flex_row()
            .items_start()
            .w_full()
            // Rows are items in the column's flex layout, which is measured
            // against the viewport. A row that may shrink is squeezed below
            // its text's height in a document taller than the window, and its
            // wrapped lines paint over the row that follows.
            .flex_shrink_0()
            .min_h(px(base.line_height()))
            .when(row.quote_depth > 0, |element| {
                element
                    .border_l(px((row.quote_depth.min(4) as f32) * 2.0))
                    .border_color(theme::quote_bar())
                    .pl_3()
            })
            .when(
                row.list_depth > 0 && self.active_block != Some(row.block),
                |element| element.pl(px(row.list_depth as f32 * 16.0)),
            )
            .when(matches!(row.kind, RowKind::Heading(_)), |element| {
                let level = match row.kind {
                    RowKind::Heading(level) => level,
                    _ => 1,
                };
                element.pt(px(match level {
                    1 => 16.0,
                    2 => 12.0,
                    3 => 8.0,
                    _ => 5.0,
                }))
            })
            .when(
                matches!(row.kind, RowKind::Code | RowKind::Fence),
                |element| {
                    element
                        .bg(theme::code_surface())
                        .px_3()
                        .when(is_first_in_block, |element| element.rounded_t_sm())
                        .when(is_last_in_block, |element| element.rounded_b_sm())
                },
            )
            .when(
                matches!(row.kind, RowKind::Paragraph | RowKind::ListItem),
                |element| element.pb(px(3.0)),
            );

        if let Some(marker) = &row.list {
            // The active block shows its source, marker included, so the
            // gutter stays empty to avoid drawing the bullet twice. A task's
            // checkbox is the exception: it is a control, not punctuation.
            let label = if row.task.is_some() || self.active_block == Some(row.block) {
                String::new()
            } else if marker.ordered {
                format!("{}.", marker.number.unwrap_or(1))
            } else {
                "\u{2022}".to_string()
            };
            let mut gutter = div()
                .w(px(PREFIX_WIDTH))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .pr_2()
                .text_color(theme::text_muted())
                .text_sm()
                .child(label);

            if let Some(checked) = row.task {
                let toggle = div()
                    .debug_selector({
                        let selector = format!("md-task-{index}");
                        move || selector
                    })
                    .w(px(14.0))
                    .h(px(14.0))
                    .mt(px(3.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(theme::text_muted())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .when(checked, |element| {
                        element.bg(theme::accent()).text_color(theme::background())
                    })
                    .child(if checked { "\u{2713}" } else { "" })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| this.toggle_task(index, cx)),
                    );
                gutter = gutter.child(toggle);
            }

            container = container.child(gutter);
        } else if row.task.is_some() || row.list_depth > 0 {
            container = container.child(div().w(px(PREFIX_WIDTH)).flex_shrink_0());
        }

        let visual = row_visual(self, index, focused);
        let shaped = ShapedTextElement::new(
            entity,
            TextKey::Row(index),
            visual.text,
            visual.runs,
            visual.font_size,
            visual.line_height,
            visual.caret,
            align,
        );

        let mut content = div().flex_1().min_w(px(0.0));

        if let Some(image) = &row.image {
            content = content.child(image_element(image));
        }

        if row.image.is_some() && self.active_block != Some(row.block) {
            container = container.child(content);
        } else {
            container = container.child(content.child(shaped));
        }

        container
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.on_row_mouse_down(index, event, window, cx)
                }),
            )
            .into_any_element()
    }

    fn editor_body(&self, entity: Entity<MarkdownView>, focused: bool, cx: &Context<Self>) -> Div {
        let mut rows = Vec::with_capacity(self.document.rows().len());
        let mut index = 0;
        while index < self.document.rows().len() {
            let row = &self.document.rows()[index];
            if matches!(row.kind, RowKind::TableRow { .. } | RowKind::TableDelimiter) {
                if let Some(block) = self.document.block(row.block)
                    && block.rows.start == index
                {
                    rows.push(table_element(self, entity.clone(), row.block, cx));
                }
                index += 1;
                continue;
            }
            rows.push(self.row_element(entity.clone(), index, focused, cx));
            index += 1;
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .py(px(COLUMN_PADDING))
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN_MAX_WIDTH))
                    .px(px(COLUMN_PADDING))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
    }

    fn fallback_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let line_count = self.document.text().line_count();
        uniform_list(
            "markdown-fallback",
            line_count,
            move |range, _window, cx| {
                let view = entity.read(cx);
                range
                    .map(|line| {
                        div()
                            .flex()
                            .flex_row()
                            .h(px(FALLBACK_ROW_HEIGHT))
                            .items_center()
                            .child(
                                div()
                                    .w(px(52.0))
                                    .flex_shrink_0()
                                    .pr_2()
                                    .text_color(theme::text_muted())
                                    .text_xs()
                                    .flex()
                                    .justify_end()
                                    .child(SharedString::from(format!("{}", line + 1))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .font_family(theme::monospace())
                                    .text_xs()
                                    .child(SharedString::from(
                                        view.document.text().line_text(line).to_string(),
                                    )),
                            )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .h_full()
        .into_any_element()
    }

    fn status_strip(&self) -> Div {
        let line = self.document.text().line_of(self.editor.caret) + 1;
        let column = self.document.text().column_of(self.editor.caret) + 1;
        let dirty = self.document.is_dirty();

        let mut strip = div()
            .debug_selector(|| "markdown-status".to_string())
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(SharedString::from(format!("{line}:{column}"))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(SharedString::from(format!(
                        "{} lines",
                        self.document.text().line_count()
                    ))),
            )
            .child(div().flex_1());

        if let Some(error) = self.save_error.clone() {
            strip = strip.child(div().text_xs().text_color(theme::warning()).child(error));
        } else if let Some(reason) = self.read_only.clone() {
            strip = strip.child(div().text_xs().text_color(theme::warning()).child(reason));
        } else if self.document.is_lossy() {
            strip = strip.child(
                div()
                    .text_xs()
                    .text_color(theme::warning())
                    .child("read only: not valid UTF-8"),
            );
        } else {
            strip = strip.child(
                div()
                    .text_xs()
                    .text_color(if dirty {
                        theme::warning()
                    } else {
                        theme::text_muted()
                    })
                    .child(if dirty { "unsaved changes" } else { "saved" }),
            );
        }

        strip
    }
}

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.row_layouts.clear();
        self.cell_layouts.clear();

        let focused = self.focus_handle.is_focused(window);
        let entity = cx.entity();
        let read_only = self.read_only.is_some();

        let mut root = div()
            .id("markdown-view")
            .debug_selector(|| "markdown-view".to_string())
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .key_context("TextEditor")
            .track_focus(&self.focus_handle)
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up));

        if !read_only {
            root = root
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
                .on_action(cx.listener(Self::act_select_up))
                .on_action(cx.listener(Self::act_select_down))
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
                .on_action(cx.listener(Self::act_toggle_bold))
                .on_action(cx.listener(Self::act_toggle_italic))
                .child(InputBridgeElement {
                    view: entity.clone(),
                    focus_handle: self.focus_handle.clone(),
                });
        }

        let body = if read_only {
            self.fallback_body(cx).into_any_element()
        } else {
            div()
                .id("markdown-scroll")
                .debug_selector(|| "markdown-scroll".to_string())
                .flex_1()
                .min_h(px(0.0))
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(self.editor_body(entity, focused, cx))
                .into_any_element()
        };

        root.child(body).child(self.status_strip())
    }
}

impl MarkdownView {
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only.is_some() || !self.editor.dragging || !event.dragging() {
            return;
        }
        let offset = self.offset_for_position(event.position);
        self.editor.select_to(offset);
        self.refresh_active();
        cx.notify();
    }

    fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.editor.dragging = false;
    }
}

macro_rules! navigation {
    ($name:ident, $action:ty, $helper:path, $extend:expr) => {
        fn $name(&mut self, _: &$action, _: &mut Window, cx: &mut Context<Self>) {
            if self.read_only.is_some() {
                return;
            }
            self.document.break_undo_group();
            $helper(&self.document, &mut self.editor, $extend);
            self.refresh_active();
            cx.notify();
        }
    };
}

impl MarkdownView {
    navigation!(act_left, Left, editor::left, false);
    navigation!(act_right, Right, editor::right, false);
    navigation!(act_word_left, WordLeft, editor::word_left, false);
    navigation!(act_word_right, WordRight, editor::word_right, false);
    navigation!(act_home, Home, editor::home, false);
    navigation!(act_end, End, editor::end, false);
    navigation!(act_select_left, SelectLeft, editor::left, true);
    navigation!(act_select_right, SelectRight, editor::right, true);
    navigation!(
        act_select_word_left,
        SelectWordLeft,
        editor::word_left,
        true
    );
    navigation!(
        act_select_word_right,
        SelectWordRight,
        editor::word_right,
        true
    );
    navigation!(act_select_home, SelectHome, editor::home, true);
    navigation!(act_select_end, SelectEnd, editor::end, true);

    fn act_document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        self.document.break_undo_group();
        editor::document_start(&mut self.editor, false);
        self.refresh_active();
        cx.notify();
    }

    fn act_document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        self.document.break_undo_group();
        editor::document_end(&self.document, &mut self.editor, false);
        self.refresh_active();
        cx.notify();
    }

    fn act_up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, false, cx);
    }

    fn act_down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, false, cx);
    }

    fn act_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, true, cx);
    }

    fn act_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, true, cx);
    }

    fn act_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        self.document.break_undo_group();
        editor::select_all(&self.document, &mut self.editor);
        self.refresh_active();
        cx.notify();
    }

    fn act_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if let Some((_, cell)) = self.current_cell() {
            let selection = self.editor.selection();
            let start = selection.start.clamp(cell.start, cell.end);
            let end = selection.end.clamp(cell.start, cell.end);
            if end > start {
                let caret = self.apply_edit(start..end, "", EditKind::Backspace);
                self.after_edit(caret, cx);
                return;
            }
            let previous = editor::grapheme_before(self.document.source(), self.editor.caret);
            if previous >= cell.start && previous < self.editor.caret {
                let caret = self.apply_edit(previous..self.editor.caret, "", EditKind::Backspace);
                self.after_edit(caret, cx);
            }
            return;
        }
        self.document.break_undo_group();
        editor::backspace(&mut self.document, &mut self.editor);
        self.refresh_active();
        cx.notify();
    }

    fn act_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if let Some((_, cell)) = self.current_cell() {
            let selection = self.editor.selection();
            let start = selection.start.clamp(cell.start, cell.end);
            let end = selection.end.clamp(cell.start, cell.end);
            if end > start {
                let caret = self.apply_edit(start..end, "", EditKind::Delete);
                self.after_edit(caret, cx);
                return;
            }
            let next = editor::grapheme_after(self.document.source(), self.editor.caret);
            if next <= cell.end && next > self.editor.caret {
                let caret = self.apply_edit(self.editor.caret..next, "", EditKind::Delete);
                self.after_edit(caret, cx);
            }
            return;
        }
        self.document.break_undo_group();
        editor::delete(&mut self.document, &mut self.editor);
        self.refresh_active();
        cx.notify();
    }

    fn act_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.enter(cx);
    }

    fn act_tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(false, cx);
    }

    fn act_outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(true, cx);
    }

    fn act_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.editor.has_selection() {
            let text = self
                .document
                .text()
                .slice(self.editor.selection())
                .to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn act_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() || !self.editor.has_selection() {
            return;
        }
        let text = self
            .document
            .text()
            .slice(self.editor.selection())
            .to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let caret = self.apply_edit(self.editor.selection(), "", EditKind::Other);
        self.after_edit(caret, cx);
    }

    fn act_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let caret = self.apply_edit(self.editor.selection(), &text, EditKind::Typing);
            self.after_edit(caret, cx);
        }
    }

    fn act_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if let Some(UndoOutcome { caret, .. }) = self.document.undo() {
            self.editor.marked = None;
            self.editor.move_to(caret.min(self.document.text().len()));
            self.refresh_active();
            cx.notify();
        }
    }

    fn act_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only.is_some() {
            return;
        }
        if let Some(UndoOutcome { caret, .. }) = self.document.redo() {
            self.editor.marked = None;
            self.editor.move_to(caret.min(self.document.text().len()));
            self.refresh_active();
            cx.notify();
        }
    }

    fn act_toggle_bold(&mut self, _: &ToggleBold, _: &mut Window, cx: &mut Context<Self>) {
        self.wrap_selection("**", cx);
    }

    fn act_toggle_italic(&mut self, _: &ToggleItalic, _: &mut Window, cx: &mut Context<Self>) {
        self.wrap_selection("*", cx);
    }
}

/// Paint the platform input handler while the view is on screen.
struct InputBridgeElement {
    view: Entity<MarkdownView>,
    focus_handle: FocusHandle,
}

impl IntoElement for InputBridgeElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InputBridgeElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        TextInputBridge::paint(self.view.clone(), &self.focus_handle, bounds, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};
    use std::path::Path;

    fn view<'a>(
        cx: &'a mut TestAppContext,
        source: &str,
    ) -> (Entity<MarkdownView>, &'a mut VisualTestContext) {
        let document = MarkdownDocument::from_str(Path::new("notes.md"), source);
        let (view, cx) = cx.add_window_view(|_window, cx| {
            MarkdownView::new(PathBuf::from("notes.md"), document, cx)
        });
        cx.update(|_window, app| crate::text_actions::bind_text_editor_keys(app));
        let handle = cx.update(|_window, app| view.read(app).focus_handle().clone());
        cx.update(|window, _app| handle.focus(window));
        (view, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
    }

    fn text(cx: &mut VisualTestContext, view: &Entity<MarkdownView>) -> String {
        cx.update(|_window, app| view.read(app).document().source().to_string())
    }

    fn put_caret(cx: &mut VisualTestContext, view: &Entity<MarkdownView>, offset: usize) {
        cx.update(|_window, app| {
            view.update(app, |view, _cx| {
                view.editor.move_to(offset);
                view.active_block = view.document.block_of_offset(offset);
                view.cell = view.document.cell_at_offset(offset);
            })
        });
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("the element is drawn");
        let position = bounds.center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    }

    #[gpui::test]
    fn typing_inserts_at_the_caret_and_marks_the_document_dirty(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "# Title\n\nHello world\n");
        put_caret(cx, &view, 9);

        cx.simulate_input("Oh ");

        assert_eq!(text(cx, &view), "# Title\n\nOh Hello world\n");
        assert_eq!(cx.update(|_window, app| view.read(app).caret()), 12);
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));
    }

    #[gpui::test]
    fn typed_text_is_one_undo_step(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "Hello\n");
        put_caret(cx, &view, 0);

        cx.simulate_input("abc");
        assert_eq!(text(cx, &view), "abcHello\n");

        cx.simulate_keystrokes("cmd-z");
        assert_eq!(text(cx, &view), "Hello\n");
        assert_eq!(cx.update(|_window, app| view.read(app).caret()), 0);
    }

    #[gpui::test]
    fn markers_are_hidden_until_their_block_is_active(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "A **bold** word\n\nSecond\n");
        draw(cx);

        let inactive = cx.update(|_window, app| {
            let view = view.read(app);
            row_visual(view, 0, false).text.to_string()
        });
        assert_eq!(inactive, "A bold word");

        click(cx, "md-row-0");
        draw(cx);

        assert_eq!(
            cx.update(|_window, app| view.read(app).active_block()),
            Some(BlockId(0))
        );
        let active = cx.update(|_window, app| {
            let view = view.read(app);
            row_visual(view, 0, false).text.to_string()
        });
        assert_eq!(active, "A **bold** word");
    }

    #[gpui::test]
    fn a_click_places_the_caret_in_the_clicked_text(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abcdef\n");
        draw(cx);
        let bounds = cx.debug_bounds("md-row-0").expect("the row is drawn");
        let position = point(bounds.left() + px(40.0), bounds.top() + px(4.0));
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());

        let caret = cx.update(|_window, app| view.read(app).caret());
        assert!(caret > 0 && caret <= 6, "caret landed at {caret}");
        assert_eq!(
            cx.update(|_window, app| view.read(app).active_block()),
            Some(BlockId(0))
        );
    }

    /// A document taller than the window must scroll, not squeeze its rows.
    /// Each row's shaped text has to fit inside the row, or wrapped lines are
    /// painted on top of the row that follows.
    #[gpui::test]
    fn wrapped_rows_keep_their_height_in_a_long_document(cx: &mut TestAppContext) {
        let item = "- A list item long enough to wrap onto several lines once it is \
                    shaped, with `inline code`, **bold text**, and plain prose after it.\n";
        let source = format!("## Heading\n\n{}\nend\n", item.repeat(40));
        let (view, cx) = view(cx, &source);
        draw(cx);

        let window_height = cx.update(|window, _app| window.viewport_size().height);
        let records = cx.update(|_window, app| {
            let mut records: Vec<_> = view
                .read(app)
                .row_layouts
                .iter()
                .map(|(index, record)| {
                    (*index, record.bounds, record.line.size(record.line_height))
                })
                .collect();
            records.sort_by_key(|(index, _, _)| *index);
            records
        });
        let bottom = records
            .last()
            .map(|(_, bounds, _)| bounds.bottom())
            .unwrap();
        assert!(
            bottom > window_height,
            "the document should be taller than the window ({bottom:?} <= {window_height:?})"
        );

        for pair in records.windows(2) {
            let (index, bounds, shaped) = pair[0];
            let (_, next, _) = pair[1];
            assert!(
                bounds.top() + shaped.height <= next.top() + px(1.0),
                "row {index}'s text ends at {:?} but the next row starts at {:?}",
                bounds.top() + shaped.height,
                next.top()
            );
        }
    }

    #[gpui::test]
    fn enter_continues_a_list(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "- one\n");
        put_caret(cx, &view, 5);

        cx.simulate_keystrokes("enter");

        assert_eq!(text(cx, &view), "- one\n- \n");
    }

    #[gpui::test]
    fn enter_continues_an_ordered_list_with_the_next_number(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "1. one\n");
        put_caret(cx, &view, 6);

        cx.simulate_keystrokes("enter");

        assert_eq!(text(cx, &view), "1. one\n2. \n");
    }

    #[gpui::test]
    fn clicking_a_checkbox_toggles_the_source(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "- [ ] task\n");
        draw(cx);
        click(cx, "md-task-0");

        assert_eq!(text(cx, &view), "- [x] task\n");
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));
    }

    #[gpui::test]
    fn html_blocks_draw_their_text_without_their_tags(cx: &mut TestAppContext) {
        let (view, cx) = view(
            cx,
            "<p align=\"center\">\n  <strong>Hello</strong> <em>world</em>\n</p>\n",
        );
        draw(cx);

        let rows = cx.update(|_window, app| {
            view.read(app)
                .document()
                .rows()
                .iter()
                .map(|row| (row.text.clone(), row.alignment))
                .collect::<Vec<_>>()
        });
        let text = rows
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert!(!text.contains('<'), "HTML tags must stay hidden: {text:?}");
        assert!(text.contains("Hello world"), "{text:?}");
        assert!(
            rows.iter()
                .any(|(_, alignment)| *alignment == bromine_core::Alignment::Center)
        );
    }

    #[gpui::test]
    fn an_html_image_draws_on_its_own_row(cx: &mut TestAppContext) {
        let document = MarkdownDocument::from_str(
            Path::new("fixtures/html.md"),
            "<p align=\"center\">\n  <img src=\"diagram.svg\" width=\"64\" alt=\"Diagram\">\n</p>\n",
        );
        let (view, cx) = cx.add_window_view(|_window, cx| {
            MarkdownView::new(PathBuf::from("fixtures/html.md"), document, cx)
        });
        cx.update(|_window, app| crate::text_actions::bind_text_editor_keys(app));
        let handle = cx.update(|_window, app| view.read(app).focus_handle().clone());
        cx.update(|window, _app| handle.focus(window));

        draw(cx);

        let image = cx.update(|_window, app| {
            view.read(app)
                .document()
                .rows()
                .iter()
                .find_map(|row| row.image.clone())
        });
        let image = image.expect("the HTML image becomes an image row");
        assert_eq!(image.width, Some(64));
        assert!(matches!(
            image.location,
            bromine_core::ImageLocation::Local(path) if path.ends_with("fixtures/diagram.svg")
        ));
    }

    #[gpui::test]
    fn a_table_cell_edit_never_writes_over_a_pipe(cx: &mut TestAppContext) {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        let (view, cx) = view(cx, source);
        draw(cx);
        click(cx, "table-cell-0-1-1");

        assert_eq!(
            cx.update(|_window, app| view.read(app).cell),
            Some(CellPosition {
                table: 0,
                row: 1,
                column: 1,
            })
        );

        cx.simulate_input("x");
        let edited = text(cx, &view);
        assert!(edited.contains("| c | dx |"), "{edited}");
        assert_eq!(edited.matches('|').count(), source.matches('|').count());
    }

    #[gpui::test]
    fn tab_moves_to_the_next_table_cell(cx: &mut TestAppContext) {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        let (view, cx) = view(cx, source);
        draw(cx);
        click(cx, "table-cell-0-1-0");

        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.update(|_window, app| view.read(app).cell),
            Some(CellPosition {
                table: 0,
                row: 1,
                column: 1,
            })
        );

        cx.simulate_keystrokes("shift-tab");
        assert_eq!(
            cx.update(|_window, app| view.read(app).cell),
            Some(CellPosition {
                table: 0,
                row: 1,
                column: 0,
            })
        );
    }

    #[gpui::test]
    fn the_table_toolbar_adds_and_removes_rows_and_columns(cx: &mut TestAppContext) {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        let (view, cx) = view(cx, source);
        draw(cx);
        click(cx, "table-cell-0-1-0");
        draw(cx);

        click(cx, "table-add-column");
        draw(cx);
        let edited = text(cx, &view);
        assert_eq!(edited.lines().next().unwrap().matches('|').count(), 4);

        click(cx, "table-remove-column");
        draw(cx);
        assert_eq!(
            text(cx, &view).lines().next().unwrap().matches('|').count(),
            3
        );

        click(cx, "table-add-row");
        draw(cx);
        assert_eq!(text(cx, &view).lines().count(), 4);

        click(cx, "table-remove-row");
        assert_eq!(text(cx, &view).lines().count(), 3);
    }

    #[gpui::test]
    fn the_alignment_buttons_rewrite_the_delimiter_row(cx: &mut TestAppContext) {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        let (view, cx) = view(cx, source);
        draw(cx);
        click(cx, "table-cell-0-1-0");
        draw(cx);

        click(cx, "table-align-center");

        let edited = text(cx, &view);
        assert!(edited.contains(":-:"), "{edited}");
    }

    #[gpui::test]
    fn bold_wraps_the_selection_and_leaves_it_selected(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "word\n");
        put_caret(cx, &view, 4);
        cx.simulate_keystrokes("shift-left shift-left");

        cx.simulate_keystrokes("cmd-b");

        assert_eq!(text(cx, &view), "wo**rd**\n");
        assert_eq!(cx.update(|_window, app| view.read(app).selection()), 4..6);
    }

    #[gpui::test]
    fn a_document_over_the_row_limit_is_read_only(cx: &mut TestAppContext) {
        let source = "a\n".repeat(MAX_ROWS + 1);
        let (view, cx) = view(cx, &source);

        assert!(cx.update(|_window, app| view.read(app).is_read_only()));
        draw(cx);

        put_caret(cx, &view, 0);
        cx.simulate_input("x");
        assert_eq!(text(cx, &view), source);
    }

    #[gpui::test]
    fn save_writes_the_file_and_clears_dirty(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("bromine-markdown-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("notes.md");
        std::fs::write(&path, "# Title\n").expect("the file is written");

        let document = MarkdownDocument::load(&path).expect("the file loads");
        let (view, cx) =
            cx.add_window_view(|_window, cx| MarkdownView::new(path.clone(), document, cx));
        cx.update(|_window, app| crate::text_actions::bind_text_editor_keys(app));
        let handle = cx.update(|_window, app| view.read(app).focus_handle().clone());
        cx.update(|window, _app| handle.focus(window));

        cx.simulate_input("X");
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));

        cx.update(|_window, app| view.update(app, |view, cx| view.save(cx)));

        assert!(!cx.update(|_window, app| view.read(app).is_dirty()));
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file reads back"),
            "X# Title\n"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }
}
