//! The table grid, its toolbar, and cell editing.
//!
//! A table is one block and one grid. The delimiter row and the pipes are never
//! drawn: alignment is a property the toolbar edits, not punctuation the reader
//! should have to read. The cell holding the caret is the only editable one,
//! but every cell keeps the grid's shape while it is edited.

use bromine_core::{Alignment, BlockId, CellPosition, RunStyle, TableBlock};
use gpui::{
    AnyElement, Context, Div, IntoElement, MouseButton, MouseDownEvent, Stateful, TextAlign, div,
    prelude::*, px,
};

use crate::markdown_row::{BaseStyle, ShapedTextElement, TextKey, build_runs};
use crate::markdown_view::MarkdownView;
use crate::theme;

/// Render a table block as a grid.
pub(crate) fn table_element(
    view: &MarkdownView,
    entity: gpui::Entity<MarkdownView>,
    block: BlockId,
    cx: &Context<MarkdownView>,
) -> AnyElement {
    let Some(table_index) = view.document.block(block).and_then(|block| block.table) else {
        return div().into_any_element();
    };
    let Some(table) = view.document.tables().get(table_index) else {
        return div().into_any_element();
    };

    let active = view.active_block == Some(block);
    let mut rows = Vec::new();

    for row in 0..table.row_count() {
        let mut cells = Vec::new();
        for column in 0..table.column_count() {
            cells.push(cell_element(
                view,
                entity.clone(),
                table_index,
                table,
                row,
                column,
                cx,
            ));
        }
        rows.push(
            div()
                .flex()
                .flex_row()
                .w_full()
                .when(row == 0, |element| element.bg(theme::surface()))
                .children(cells),
        );
    }

    let mut container = div().w_full().my_3().flex().flex_col().gap_2();
    if active {
        container = container.child(toolbar(view, table_index, table, cx));
    }
    container
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .border_1()
                .border_color(theme::border())
                .rounded_sm()
                .overflow_hidden()
                .children(rows),
        )
        .into_any_element()
}

fn cell_element(
    view: &MarkdownView,
    entity: gpui::Entity<MarkdownView>,
    table_index: usize,
    table: &TableBlock,
    row: usize,
    column: usize,
    cx: &Context<MarkdownView>,
) -> AnyElement {
    let header = row == 0;
    let alignment = table
        .alignments
        .get(column)
        .copied()
        .unwrap_or(Alignment::None);
    let cell = table.cell(row, column);
    let text = cell
        .map(|cell| cell.text(view.document.source()).to_string())
        .unwrap_or_default();
    let cell_range = cell.map(|cell| cell.source.clone()).unwrap_or(0..0);
    let position = CellPosition {
        table: table_index,
        row,
        column,
    };
    let active_cell = view.cell == Some(position);
    let active_block = view.active_block == Some(view.document.rows()[table.lines.start].block);

    let (selection, marked, caret) = if active_cell && active_block {
        let selection = view.editor.selection();
        let start = (selection.start.saturating_sub(cell_range.start)).min(text.len());
        let end = (selection.end.saturating_sub(cell_range.start)).min(text.len());
        let marked = view.editor.marked.as_ref().map(|range| {
            (range.start.saturating_sub(cell_range.start)).min(text.len())
                ..(range.end.saturating_sub(cell_range.start)).min(text.len())
        });
        (
            start..end,
            marked,
            Some((view.editor.caret.saturating_sub(cell_range.start)).min(text.len())),
        )
    } else {
        (0..0, None, None)
    };

    let base = BaseStyle::table(header);
    let runs = build_runs(
        &[(text.len(), RunStyle::default())],
        selection,
        marked,
        base,
    );
    let shaped = ShapedTextElement::new(
        entity,
        TextKey::Cell {
            table: table_index,
            row,
            column,
        },
        text.clone().into(),
        runs,
        base.font_size,
        base.line_height(),
        caret,
        if active_cell {
            TextAlign::Left
        } else {
            match alignment {
                Alignment::Center => TextAlign::Center,
                Alignment::Right => TextAlign::Right,
                _ => TextAlign::Left,
            }
        },
    );

    let justification = match alignment {
        Alignment::Center => div().justify_center(),
        Alignment::Right => div().justify_end(),
        _ => div().justify_start(),
    };

    div()
        .id(gpui::SharedString::from(format!(
            "table-cell-{table_index}-{row}-{column}"
        )))
        .debug_selector({
            let selector = format!("table-cell-{table_index}-{row}-{column}");
            move || selector
        })
        .flex_1()
        .min_w(px(0.0))
        .border_r_1()
        .border_b_1()
        .border_color(theme::border_subtle())
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .child(justification.w_full().child(shaped))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.on_cell_mouse_down(table_index, row, column, event, window, cx);
            }),
        )
        .into_any_element()
}

fn toolbar(
    view: &MarkdownView,
    table: usize,
    block: &TableBlock,
    cx: &Context<MarkdownView>,
) -> Div {
    let column = view.cell.map(|cell| cell.column).unwrap_or(0);
    let alignment = block
        .alignments
        .get(column)
        .copied()
        .unwrap_or(Alignment::None);

    let button = |id: &'static str, label: &'static str, active: bool| -> Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .text_xs()
            .text_color(if active {
                theme::accent()
            } else {
                theme::text_muted()
            })
            .when(active, |element| element.bg(theme::selection()))
            .child(label)
    };

    div()
        .debug_selector(|| "table-toolbar".to_string())
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w_full()
        .child(
            button("table-add-row", "+ row", false).on_click(
                cx.listener(move |this, _event, _window, cx| this.table_add_row(table, cx)),
            ),
        )
        .child(button("table-remove-row", "\u{2212} row", false).on_click(
            cx.listener(move |this, _event, _window, cx| this.table_remove_row(table, cx)),
        ))
        .child(button("table-add-column", "+ column", false).on_click(
            cx.listener(move |this, _event, _window, cx| this.table_add_column(table, cx)),
        ))
        .child(
            button("table-remove-column", "\u{2212} column", false).on_click(
                cx.listener(move |this, _event, _window, cx| this.table_remove_column(table, cx)),
            ),
        )
        .child(div().flex_1())
        .child(
            button("table-align-left", "left", alignment == Alignment::Left).on_click(cx.listener(
                move |this, _event, _window, cx| this.table_align(table, Alignment::Left, cx),
            )),
        )
        .child(
            button(
                "table-align-center",
                "center",
                alignment == Alignment::Center,
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.table_align(table, Alignment::Center, cx)
            })),
        )
        .child(
            button("table-align-right", "right", alignment == Alignment::Right).on_click(
                cx.listener(move |this, _event, _window, cx| {
                    this.table_align(table, Alignment::Right, cx)
                }),
            ),
        )
        .child(
            button("table-align-none", "none", alignment == Alignment::None).on_click(cx.listener(
                move |this, _event, _window, cx| this.table_align(table, Alignment::None, cx),
            )),
        )
}
