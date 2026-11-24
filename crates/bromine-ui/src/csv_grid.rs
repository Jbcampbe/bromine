//! The CSV grid: a virtualized, sortable, searchable view over a parsed table.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bromine_core::{CsvDocument, Direction, RowView, SortKey};
use gpui::{
    Context, Div, ElementId, FocusHandle, Font, KeyDownEvent, MouseButton, MouseDownEvent, Pixels,
    Render, SharedString, Stateful, TextSystem, Window, div, font, prelude::*, px, uniform_list,
};

use crate::theme;

/// Row height in pixels.
///
/// `uniform_list` measures the first row and assumes every other row matches it,
/// so every row has to be exactly this tall.
pub const ROW_HEIGHT: f32 = 24.0;

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

    /// Sort by `column`, or reverse the sort if it is already sorted by it.
    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        self.view.toggle_sort(&self.document.table, column);
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.view.set_query(&self.document.table, query);
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
        // Leave shortcuts to the platform and to whatever else is bound.
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            return;
        }

        match event.keystroke.key.as_str() {
            "backspace" => self.delete_last_query_char(cx),
            "escape" => self.clear_query(cx),
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

    /// The counts and repair notice, along the bottom of the grid.
    ///
    /// The file name is not repeated here: the tab above the grid carries it.
    fn status_bar(&self) -> Div {
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
            // The counts stay left and the notice stays right however wide the
            // window is.
            .child(div().flex_1());

        if let Some(notice) = self.repairs_notice() {
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(theme::warning())
                    .child(SharedString::from(notice)),
            );
        }

        bar.child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child("read only"),
        )
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

    fn body(&self, cx: &Context<Self>) -> Div {
        if self.column_count() == 0 {
            return div().flex_1().flex().items_center().justify_center().child(
                div()
                    .text_color(theme::text_muted())
                    .child("this file has no columns"),
            );
        }

        let total: f32 = self.column_widths.iter().sum();

        // The header and the rows live inside one horizontally scrolling
        // container so they cannot drift apart. Only the list inside scrolls
        // vertically, which keeps the header in place.
        div().flex_1().min_h(px(0.0)).overflow_hidden().child(
            div()
                .id("grid-scroll")
                .debug_selector(|| "grid-scroll".to_string())
                .size_full()
                .overflow_x_scroll()
                .child(
                    div()
                        .w(px(total))
                        .min_w_full()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(self.header_row(cx))
                        .child(self.rows(cx)),
                ),
        )
    }

    fn header_row(&self, cx: &Context<Self>) -> Div {
        let mut cells = Vec::with_capacity(self.column_widths.len());
        for (column, width) in self.column_widths.iter().enumerate() {
            cells.push(self.header_cell(column, *width, cx));
        }

        div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .w_full()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .children(cells)
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
            .py_1()
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

    fn rows(&self, cx: &Context<Self>) -> Div {
        let visible = self.view.len();

        if visible == 0 {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(theme::text_muted()).child("no rows match"));
        }

        div().flex_1().min_h(px(0.0)).overflow_hidden().child(
            uniform_list(
                "rows",
                visible,
                cx.processor(|this: &mut CsvGrid, range: Range<usize>, _window, _cx| {
                    range
                        .map(|position| this.row(position))
                        .collect::<Vec<Stateful<Div>>>()
                }),
            )
            .size_full(),
        )
    }

    fn row(&self, position: usize) -> Stateful<Div> {
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
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(ROW_HEIGHT))
            .when(position % 2 == 1, |el| el.bg(theme::row_alternate()))
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
            .child(self.status_bar())
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

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bromine_core::parse_csv;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

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
    fn the_counts_and_read_only_notice_sit_below_the_grid(cx: &mut TestAppContext) {
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
}
