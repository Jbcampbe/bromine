//! The CSV grid: a virtualized, sortable, searchable view over a parsed table.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bromine_core::{CsvDocument, Direction, RowView, SortKey};
use gpui::{
    ClipboardItem, Context, Div, ElementId, FocusHandle, Font, KeyDownEvent, MouseButton,
    MouseDownEvent, Pixels, Render, ScrollHandle, ScrollWheelEvent, SharedString, Stateful,
    TextSystem, TouchPhase, Window, div, font, point, prelude::*, px,
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
    /// Whether the table has edits that are not on disk yet.
    dirty: bool,
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
            dirty: false,
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
        self.dirty
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
    /// A file that had to be decoded lossily cannot be written back without
    /// destroying the bytes that would not decode, so it refuses instead of
    /// quietly damaging the original. Padded and unnamed columns are repairs the
    /// status strip already shows, and they write back as they look.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.document.lossy {
            self.save_error = Some(SharedString::from(
                "not saved: this file is not valid UTF-8, so writing it back would lose data",
            ));
            cx.notify();
            return;
        }

        match self.document.save(&self.path) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
            }
            Err(error) => self.save_error = Some(SharedString::from(error.to_string())),
        }

        cx.notify();
    }

    /// Insert an empty row below the marked one, or at the end of the file.
    fn insert_row(&mut self, cx: &mut Context<Self>) {
        let at = match self.selection {
            Selection::Cell { position, .. } | Selection::Row { position } => self
                .view
                .table_row(position + 1)
                .unwrap_or_else(|| self.total_row_count()),
            _ => self.total_row_count(),
        };

        self.document.table.insert_row(at);
        self.after_table_change(cx);
    }

    /// Remove the marked row, if one is marked.
    fn delete_row(&mut self, cx: &mut Context<Self>) {
        let position = match self.selection {
            Selection::Cell { position, .. } | Selection::Row { position } => position,
            _ => return,
        };
        let Some(row) = self.view.table_row(position) else {
            return;
        };

        if self.document.table.remove_row(row).is_ok() {
            self.after_table_change(cx);
        }
    }

    /// Insert an empty column right of the marked one, or at the right edge.
    fn insert_column(&mut self, cx: &mut Context<Self>) {
        let index = match self.selection {
            Selection::Cell { column, .. } | Selection::Column { column } => column + 1,
            _ => self.column_count(),
        };

        self.document
            .table
            .insert_column(index, format!("column {}", index + 1));
        self.after_table_change(cx);
    }

    /// Remove the marked column, if one is marked.
    fn delete_column(&mut self, cx: &mut Context<Self>) {
        let column = match self.selection {
            Selection::Cell { column, .. } | Selection::Column { column } => column,
            _ => return,
        };

        if self.document.table.remove_column(column).is_ok() {
            self.after_table_change(cx);
        }
    }

    /// Rebuild everything an edit has to keep in step.
    ///
    /// Widths are measured again because a new column has none and a removed one
    /// must not leave its width behind. The mark is dropped because positions
    /// and column indices no longer mean what they meant before the edit.
    fn after_table_change(&mut self, cx: &mut Context<Self>) {
        self.view.rebuild(&self.document.table);
        self.column_widths = measure_columns(&self.document, cx.text_system());
        self.selection = Selection::None;
        self.dirty = true;
        self.save_error = None;
        cx.notify();
    }

    /// Sort by `column`, or reverse the sort if it is already sorted by it.
    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        self.view.toggle_sort(&self.document.table, column);
        // Sorting moves rows, so a mark on a position would point at a different
        // row than it did before the click.
        self.selection = Selection::None;
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
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
        self.view.clear_query(&self.document.table);
        cx.notify();
    }

    /// Search is driven by raw keystrokes rather than a text field.
    ///
    /// A real field needs `EntityInputHandler` plus `window.handle_input` during
    /// paint, which is a larger piece of work than this increment. Typing
    /// characters, backspace, and escape cover searching, and nothing here
    /// claims to be a caret.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();

        // The editing keys go first, and never into the search box.
        if event.keystroke.modifiers.platform {
            match key {
                "c" => self.copy_selection(cx),
                "s" => self.save(cx),
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
            )
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

        if let Some(error) = self.save_error.clone() {
            bar = bar.child(div().text_xs().text_color(theme::warning()).child(error));
        }

        bar.child(
            div()
                .text_xs()
                .text_color(if self.dirty {
                    theme::warning()
                } else {
                    theme::text_muted()
                })
                .child(SharedString::from(if self.dirty {
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
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .font_family(theme::monospace())
                            .text_size(px(CELL_FONT_SIZE))
                            .truncate()
                            .child(text),
                    ),
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
            // div, and marking a cell only needs the column the click fell in.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.focus(window);
                    this.select_cell_at(position, event.position.x, cx);
                }),
            )
            .children(cells)
    }
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
            .track_focus(&self.focus_handle)
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

/// Width for every column, measured from the widest value it has to show.
///
/// The file is scanned once at load, comparing lengths only. The single widest
/// value per column is then measured, so the cost is one measurement per column
/// rather than one per cell, and a 200k-row file pays it once instead of on
/// every frame.
fn measure_columns(document: &CsvDocument, text_system: &TextSystem) -> Vec<f32> {
    let table = &document.table;
    let columns = table.column_count();

    // Seed with the header, so a column is always wide enough for its own name.
    let mut widest_chars = Vec::with_capacity(columns);
    let mut widest: Vec<&str> = Vec::with_capacity(columns);
    for column in 0..columns {
        let header = table.header(column).unwrap_or_default();
        widest_chars.push(header.chars().count());
        widest.push(header);
    }

    for row in table.rows() {
        for column in 0..columns.min(row.len()) {
            let cell = row[column].as_str();
            let chars = cell.chars().count();
            if chars > widest_chars[column] {
                widest_chars[column] = chars;
                widest[column] = cell;
            }
        }
    }

    let font = font(theme::monospace());
    let font_size = px(CELL_FONT_SIZE);
    widest
        .into_iter()
        .map(|text| column_width(text_width(text, text_system, &font, font_size)))
        .collect()
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
        assert!(cx.update(|_window, app| grid.read(app).dirty));
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
    fn an_edit_marks_the_document_dirty_and_cmd_s_writes_it_back(cx: &mut TestAppContext) {
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
        assert!(cx.update(|_window, app| grid.read(app).dirty));

        cx.simulate_keystrokes("cmd-s");

        assert!(!cx.update(|_window, app| grid.read(app).dirty));
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
        cx.simulate_keystrokes("cmd-s");

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
}
