//! The plain-text view: monospace, line numbers, and a real caret.
//!
//! The row window is the JSON view's construction applied to an editable
//! buffer. Every row is a fixed height and only the rows in view are built,
//! which is what lets a large log open without creating a hundred thousand
//! elements. Long lines are reached by scrolling sideways, never by wrapping:
//! a line number has to describe the line beside it.

use std::path::{Path, PathBuf};

use bromine_core::TextDocument;
use gpui::{
    Bounds, Context, Div, ElementId, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Point, Render, ScrollHandle, ScrollWheelEvent, SharedString, Stateful,
    StyledText, TextRun, TextSystem, UTF16Selection, UnderlineStyle, Window, div, font, point,
    prelude::*, px,
};

use crate::editor::{self, EditorState};
use crate::text_actions::{
    Backspace, Copy, Cut, Delete, DocumentEnd, DocumentStart, Down, End, Enter, Home, Left,
    Outdent, Paste, Redo, Right, SelectAll, SelectDown, SelectEnd, SelectHome, SelectLeft,
    SelectRight, SelectUp, SelectWordLeft, SelectWordRight, Tab, Undo, Up, WordLeft, WordRight,
};
use crate::theme;

const ROW_HEIGHT: f32 = 20.0;
const FONT_SIZE: f32 = 13.0;
const GUTTER_WIDTH: f32 = 52.0;
const LINE_PADDING: f32 = 8.0;
const TAB_COLUMNS: usize = 4;
const SCROLLBAR_THICKNESS: f32 = 8.0;
const MIN_SCROLLBAR_THUMB: f32 = 24.0;

/// A display line: the source line with tabs expanded, plus the source offset
/// for every byte boundary of the displayed text.
struct DisplayLine {
    text: String,
    map: Vec<usize>,
}

impl DisplayLine {
    fn new(source: &str, line_start: usize) -> Self {
        let mut text = String::with_capacity(source.len());
        // `map[d]` is the source offset for display byte `d`. The first entry
        // is the boundary before the line, and each character appends the
        // boundaries after itself, so the two lengths always agree.
        let mut map = vec![line_start];

        for (offset, character) in source.char_indices() {
            let source_offset = line_start + offset;
            if character == '\t' {
                for column in 0..TAB_COLUMNS {
                    text.push(' ');
                    map.push(if column + 1 == TAB_COLUMNS {
                        source_offset + 1
                    } else {
                        source_offset
                    });
                }
            } else {
                text.push(character);
                for _ in 1..character.len_utf8() {
                    map.push(source_offset);
                }
                map.push(source_offset + character.len_utf8());
            }
        }

        Self { text, map }
    }

    fn source_at_column(&self, column: usize) -> usize {
        match self.text.char_indices().nth(column) {
            Some((byte, _)) => self.map[byte],
            None => *self.map.last().unwrap_or(&0),
        }
    }

    fn column_for_source(&self, source: usize) -> usize {
        let mut column = 0;
        for (byte, _) in self.text.char_indices() {
            if self.map[byte] >= source {
                return column;
            }
            column += 1;
        }
        column
    }
}

/// An editable plain-text document.
pub struct TextView {
    path: PathBuf,
    document: TextDocument,
    editor: EditorState,
    body_scroll: ScrollHandle,
    gutter_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    advance: f32,
    content_width: f32,
    save_error: Option<SharedString>,
}

impl TextView {
    pub fn new(path: PathBuf, document: TextDocument, cx: &mut Context<Self>) -> Self {
        let advance = measure_advance(cx.text_system());
        let content_width = widest_line_width(document.text(), advance);
        Self {
            path,
            document,
            editor: EditorState::default(),
            body_scroll: ScrollHandle::new(),
            gutter_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            advance,
            content_width,
            save_error: None,
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

    pub fn document(&self) -> &TextDocument {
        &self.document
    }

    pub fn caret(&self) -> usize {
        self.editor.caret
    }

    pub fn selection(&self) -> std::ops::Range<usize> {
        self.editor.selection()
    }

    pub fn line_count(&self) -> usize {
        self.document.line_count()
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

    fn display_line(&self, index: usize) -> DisplayLine {
        let source = self.document.line_text(index);
        DisplayLine::new(source, self.line_start(index))
    }

    fn line_start(&self, index: usize) -> usize {
        self.document.line_start(index).unwrap_or(0)
    }

    fn content_changed(&mut self, cx: &mut Context<Self>) {
        self.content_width = widest_line_width(self.document.text(), self.advance);
        self.reveal_caret();
        cx.notify();
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        self.reveal_caret();
        cx.notify();
    }

    /// Keep the caret inside the viewport, on both axes.
    fn reveal_caret(&mut self) {
        let viewport = self.body_scroll.bounds().size;
        let height = f32::from(viewport.height);
        let width = f32::from(viewport.width);
        if height <= 0.0 {
            return;
        }

        let line = self.document.line_of(self.editor.caret);
        let display = self.display_line(line);
        let column = display.column_for_source(self.editor.caret);
        let caret_y = line as f32 * ROW_HEIGHT;
        let caret_x = LINE_PADDING + column as f32 * self.advance;

        let current = self.body_scroll.offset();
        let max = self.body_scroll.max_offset();
        let mut x = f32::from(current.x);
        let mut y = f32::from(current.y);

        let top = -y;
        if caret_y < top {
            y = -caret_y;
        } else if caret_y + ROW_HEIGHT > top + height {
            y = -(caret_y + ROW_HEIGHT - height);
        }

        let left = -x;
        let usable = (width - LINE_PADDING).max(self.advance);
        if caret_x < left {
            x = -caret_x;
        } else if caret_x + self.advance > left + usable {
            x = -(caret_x + self.advance - usable);
        }

        let x = x.clamp(-f32::from(max.width), 0.0);
        let y = y.clamp(-f32::from(max.height), 0.0);
        self.body_scroll.set_offset(point(px(x), px(y)));
        self.gutter_scroll.set_offset(point(px(0.0), px(y)));
    }

    fn line_window(&self) -> (usize, usize) {
        let lines = self.document.line_count();
        if lines == 0 {
            return (0, 0);
        }
        let measured = f32::from(self.body_scroll.bounds().size.height);
        let viewport = if measured > 0.0 {
            measured
        } else {
            ROW_HEIGHT * 40.0
        };
        let offset = f32::from(self.body_scroll.offset().y);
        let first = (((-offset) / ROW_HEIGHT).floor().max(0.0) as usize).min(lines - 1);
        let drawn = (viewport / ROW_HEIGHT).ceil() as usize + 1;
        (first, (first + drawn).min(lines))
    }

    fn runs_for(&self, display: &DisplayLine) -> Vec<TextRun> {
        let selection = self.editor.selection();
        let marked = self.editor.marked.clone();
        let mut runs = Vec::new();
        let mut current: Option<(bool, bool, usize)> = None;

        for (byte, character) in display.text.char_indices() {
            let source = display.map[byte];
            let selected =
                !selection.is_empty() && source >= selection.start && source < selection.end;
            let composing = marked
                .as_ref()
                .is_some_and(|range| source >= range.start && source < range.end);
            let len = character.len_utf8();

            match &mut current {
                Some((run_selected, run_composing, run_len))
                    if *run_selected == selected && *run_composing == composing =>
                {
                    *run_len += len;
                }
                _ => {
                    if let Some((run_selected, run_composing, run_len)) = current.take() {
                        runs.push(self.text_run(run_len, run_selected, run_composing));
                    }
                    current = Some((selected, composing, len));
                }
            }
        }

        if let Some((selected, composing, len)) = current {
            runs.push(self.text_run(len, selected, composing));
        }
        runs
    }

    fn text_run(&self, len: usize, selected: bool, composing: bool) -> TextRun {
        TextRun {
            len,
            font: font(theme::monospace()),
            color: theme::text(),
            background_color: selected.then(theme::selection),
            underline: composing.then(|| UnderlineStyle {
                color: Some(theme::accent()),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        }
    }

    fn visible_lines(&self, focused: bool) -> Vec<Stateful<Div>> {
        let (first, last) = self.line_window();
        (first..last)
            .map(|index| self.line(index, focused))
            .collect()
    }

    fn line(&self, index: usize, focused: bool) -> Stateful<Div> {
        let display = self.display_line(index);
        let runs = self.runs_for(&display);
        let caret_line = self.document.line_of(self.editor.caret) == index;
        let caret_column = if caret_line {
            display.column_for_source(self.editor.caret)
        } else {
            0
        };

        let mut row = div()
            .id(ElementId::named_usize("text-line", index))
            .debug_selector({
                let selector = format!("text-line-{index}");
                move || selector
            })
            .absolute()
            .top(px(index as f32 * ROW_HEIGHT))
            .left(px(0.0))
            .h(px(ROW_HEIGHT))
            .pl(px(LINE_PADDING))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .font_family(theme::monospace())
            .text_size(px(FONT_SIZE))
            .text_color(theme::text())
            .child(StyledText::new(display.text.clone()).with_runs(runs));

        if caret_line && focused {
            row = row.child(
                div()
                    .debug_selector(|| "text-caret".to_string())
                    .absolute()
                    .top(px(2.0))
                    .left(px(LINE_PADDING + caret_column as f32 * self.advance))
                    .w(px(2.0))
                    .h(px(ROW_HEIGHT - 4.0))
                    .bg(theme::accent()),
            );
        }

        row
    }

    fn gutter(&self) -> Stateful<Div> {
        let (first, last) = self.line_window();
        let numbers: Vec<Stateful<Div>> = (first..last)
            .map(|index| {
                div()
                    .id(ElementId::named_usize("text-line-number", index))
                    .debug_selector({
                        let selector = format!("text-line-number-{index}");
                        move || selector
                    })
                    .absolute()
                    .top(px(index as f32 * ROW_HEIGHT))
                    .left(px(0.0))
                    .w(px(GUTTER_WIDTH))
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_2()
                    .font_family(theme::monospace())
                    .text_size(px(FONT_SIZE))
                    .text_color(theme::text_muted())
                    .child(SharedString::from((index + 1).to_string()))
            })
            .collect();

        div()
            .id("text-gutter")
            .debug_selector(|| "text-gutter".to_string())
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .w(px(GUTTER_WIDTH))
            .overflow_hidden()
            .bg(theme::background())
            .border_r_1()
            .border_color(theme::border())
            .track_scroll(&self.gutter_scroll)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(GUTTER_WIDTH))
                    .h(px(self.document.line_count() as f32 * ROW_HEIGHT))
                    .children(numbers),
            )
    }

    fn body(&self, focused: bool, cx: &Context<Self>) -> Stateful<Div> {
        div()
            .id("text-body")
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.gutter())
            .child(
                div()
                    .id("text-lines")
                    .debug_selector(|| "text-lines".to_string())
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(GUTTER_WIDTH))
                    .right(px(0.0))
                    .overflow_hidden()
                    .track_scroll(&self.body_scroll)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .w(px(self.content_width.max(1.0)))
                            .h(px(self.document.line_count() as f32 * ROW_HEIGHT))
                            .children(self.visible_lines(focused)),
                    ),
            )
            .children(self.scrollbars())
    }

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
                    .debug_selector(|| "text-scrollbar-vertical".to_string())
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
                    .debug_selector(|| "text-scrollbar-horizontal".to_string())
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

    fn status_strip(&self) -> Div {
        let line = self.document.line_of(self.editor.caret) + 1;
        let column = self.document.column_of(self.editor.caret) + 1;

        let mut strip = div()
            .debug_selector(|| "text-status".to_string())
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
                        self.document.line_count()
                    ))),
            )
            .child(div().flex_1());

        if let Some(error) = self.save_error.clone() {
            strip = strip.child(div().text_xs().text_color(theme::warning()).child(error));
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
                    .text_color(if self.document.is_dirty() {
                        theme::warning()
                    } else {
                        theme::text_muted()
                    })
                    .child(if self.document.is_dirty() {
                        "unsaved changes"
                    } else {
                        "saved"
                    }),
            );
        }

        strip
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        let current = self.body_scroll.offset();
        let max = self.body_scroll.max_offset();
        let next = point(
            (current.x + delta.x).clamp(-max.width, px(0.0)),
            (current.y + delta.y).clamp(-max.height, px(0.0)),
        );
        if next == current {
            return;
        }
        self.gutter_scroll.set_offset(point(px(0.0), next.y));
        self.body_scroll.set_offset(next);
        cx.notify();
    }

    fn offset_for_position(&self, position: Point<gpui::Pixels>) -> usize {
        let bounds = self.body_scroll.bounds();
        let offset = self.body_scroll.offset();
        let local_y = position.y - bounds.top() - offset.y;
        let local_x = position.x - bounds.left() - offset.x - px(LINE_PADDING);
        let line = ((f32::from(local_y) / ROW_HEIGHT).floor().max(0.0) as usize)
            .min(self.document.line_count().saturating_sub(1));
        let display = self.display_line(line);
        let column = (f32::from(local_x) / self.advance).round().max(0.0) as usize;
        display.source_at_column(column)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus(window);
        self.document.break_undo_group();
        let offset = self.offset_for_position(event.position);
        self.editor.dragging = true;
        if event.modifiers.shift {
            self.editor.select_to(offset);
        } else {
            self.editor.move_to(offset);
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editor.dragging || !event.dragging() {
            return;
        }
        let offset = self.offset_for_position(event.position);
        self.editor.select_to(offset);
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

    fn act_left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::left(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::right(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::left(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::right(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::word_left(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::word_right(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::word_left(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_select_word_right(
        &mut self,
        _: &SelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.document.break_undo_group();
        editor::word_right(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::home(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::end(&self.document, &mut self.editor, false);
        self.after_move(cx);
    }

    fn act_select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::home(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::end(&self.document, &mut self.editor, true);
        self.after_move(cx);
    }

    fn act_document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::document_start(&mut self.editor, false);
        self.after_move(cx);
    }

    fn act_document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::document_end(&self.document, &mut self.editor, false);
        self.after_move(cx);
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

    fn vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let desired = match self.editor.desired_x {
            Some(x) => x,
            None => {
                let line = self.document.line_of(self.editor.caret);
                let display = self.display_line(line);
                let column = display.column_for_source(self.editor.caret);
                let x = column as f32 * self.advance;
                self.editor.desired_x = Some(x);
                x
            }
        };

        let current = self.document.line_of(self.editor.caret) as isize;
        let target_line = (current + delta)
            .clamp(0, self.document.line_count().saturating_sub(1) as isize)
            as usize;
        let display = self.display_line(target_line);
        let column = (desired / self.advance).round().max(0.0) as usize;
        let target = display.source_at_column(column);

        self.document.break_undo_group();
        if extend {
            self.editor.select_to(target);
        } else {
            self.editor.move_to(target);
        }
        self.editor.desired_x = Some(desired);
        self.reveal_caret();
        cx.notify();
    }

    fn act_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        editor::backspace(&mut self.document, &mut self.editor);
        self.content_changed(cx);
    }

    fn act_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        editor::delete(&mut self.document, &mut self.editor);
        self.content_changed(cx);
    }

    fn act_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        editor::insert(&mut self.document, &mut self.editor, "\n");
        self.content_changed(cx);
    }

    fn act_tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        editor::insert(&mut self.document, &mut self.editor, "\t");
        self.content_changed(cx);
    }

    fn act_outdent(&mut self, _: &Outdent, _: &mut Window, _cx: &mut Context<Self>) {}

    fn act_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.document.break_undo_group();
        editor::select_all(&self.document, &mut self.editor);
        cx.notify();
    }

    fn act_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        editor::copy(&self.document, &self.editor, cx);
    }

    fn act_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        editor::cut(&mut self.document, &mut self.editor, cx);
        self.content_changed(cx);
    }

    fn act_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        editor::paste(&mut self.document, &mut self.editor, cx);
        self.content_changed(cx);
    }

    fn act_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        editor::undo(&mut self.document, &mut self.editor);
        self.content_changed(cx);
    }

    fn act_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        editor::redo(&mut self.document, &mut self.editor);
        self.content_changed(cx);
    }
}

impl Render for TextView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        div()
            .id("text-view")
            .debug_selector(|| "text-view".to_string())
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .text_sm()
            .key_context("TextEditor")
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
            .child(editor::InputBridgeElement::new(
                cx.entity(),
                self.focus_handle.clone(),
            ))
            .child(self.body(focused, cx))
            .child(self.status_strip())
    }
}

impl gpui::EntityInputHandler for TextView {
    fn text_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        actual_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = editor::range_from_utf16(self.document.text(), &range_utf16);
        let range = range.start.min(self.document.len())..range.end.min(self.document.len());
        actual_range.replace(editor::range_to_utf16(self.document.text(), &range));
        Some(self.document.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(editor::utf16_selection(self.document.text(), &self.editor))
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.editor
            .marked
            .as_ref()
            .map(|range| editor::range_to_utf16(self.document.text(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.editor.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| editor::range_from_utf16(self.document.text(), range))
            .or_else(|| self.editor.marked.clone())
            .unwrap_or_else(|| self.editor.selection());
        let before = self.editor.caret;
        let caret = self.document.replace(
            range.clone(),
            text,
            before,
            range.start + text.len(),
            bromine_core::EditKind::Typing,
        );
        self.editor.marked = None;
        self.editor.move_to(caret);
        self.content_changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| editor::range_from_utf16(self.document.text(), range))
            .or_else(|| self.editor.marked.clone())
            .unwrap_or_else(|| self.editor.selection());
        let before = self.editor.caret;
        let start = range.start;
        let caret = self.document.replace(
            range,
            new_text,
            before,
            start + new_text.len(),
            bromine_core::EditKind::Typing,
        );
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
        self.content_changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        let offset = editor::offset_from_utf16(self.document.text(), range_utf16.start);
        let line = self.document.line_of(offset);
        let display = self.display_line(line);
        let column = display.column_for_source(offset);
        let scroll = self.body_scroll.offset();
        let origin = point(
            element_bounds.left()
                + px(GUTTER_WIDTH + LINE_PADDING)
                + scroll.x
                + px(column as f32 * self.advance),
            element_bounds.top() + scroll.y + px(line as f32 * ROW_HEIGHT),
        );
        Some(Bounds::new(origin, gpui::size(px(2.0), px(ROW_HEIGHT))))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_position(position);
        Some(editor::offset_to_utf16(self.document.text(), offset))
    }
}

fn measure_advance(text_system: &TextSystem) -> f32 {
    let font = font(theme::monospace());
    let font_id = text_system.resolve_font(&font);
    text_system
        .advance(font_id, px(FONT_SIZE), 'M')
        .map(|size| f32::from(size.width))
        .unwrap_or(8.0)
}

fn widest_line_width(text: &str, advance: f32) -> f32 {
    let widest = text
        .lines()
        .map(|line| {
            line.chars()
                .map(|character| if character == '\t' { TAB_COLUMNS } else { 1 })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    (widest as f32 * advance + LINE_PADDING * 2.0).max(1.0)
}

fn thumb_length(track: f32, overflow: f32) -> f32 {
    let visible = track / (track + overflow);
    (track * visible).max(MIN_SCROLLBAR_THUMB).min(track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bromine_core::TextDocument;
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};
    use std::path::PathBuf;

    fn view<'a>(
        cx: &'a mut TestAppContext,
        source: &str,
    ) -> (Entity<TextView>, &'a mut VisualTestContext) {
        let document = TextDocument::from_str(Path::new("notes.txt"), source);
        let (view, cx) = cx
            .add_window_view(|_window, cx| TextView::new(PathBuf::from("notes.txt"), document, cx));
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

    fn text(cx: &mut VisualTestContext, view: &Entity<TextView>) -> String {
        cx.update(|_window, app| view.read(app).document().text().to_string())
    }

    fn caret(cx: &mut VisualTestContext, view: &Entity<TextView>) -> usize {
        cx.update(|_window, app| view.read(app).caret())
    }

    #[gpui::test]
    fn typing_inserts_text_moves_the_caret_and_marks_the_document_dirty(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "hello");

        cx.simulate_input("abc");

        assert_eq!(text(cx, &view), "abchello");
        assert_eq!(caret(cx, &view), 3);
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));
    }

    #[gpui::test]
    fn backspace_coalesces_and_undo_returns_the_word(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "");
        cx.simulate_input("abc");
        cx.simulate_keystrokes("backspace backspace");
        assert_eq!(text(cx, &view), "a");

        cx.simulate_keystrokes("cmd-z");
        assert_eq!(text(cx, &view), "abc", "two backspaces are one undo step");
        assert_eq!(caret(cx, &view), 3);
    }

    #[gpui::test]
    fn a_click_places_the_caret_at_the_clicked_column(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abcdef\nsecond\n");
        draw(cx);
        let line = cx
            .debug_bounds("text-line-0")
            .expect("the first line is drawn");
        let advance = cx.update(|_window, app| view.read(app).advance);
        let position = point(
            line.left() + px(LINE_PADDING + advance * 3.0 + 1.0),
            line.top() + px(ROW_HEIGHT / 2.0),
        );
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());

        assert_eq!(caret(cx, &view), 3);
    }

    #[gpui::test]
    fn shift_arrow_selects_and_copy_writes_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abcdef");
        cx.simulate_keystrokes("shift-right shift-right shift-right");

        assert_eq!(cx.update(|_window, app| view.read(app).selection()), 0..3);

        cx.simulate_keystrokes("cmd-c");
        let copied = cx
            .update(|_window, app| app.read_from_clipboard())
            .and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("abc"));
    }

    #[gpui::test]
    fn enter_adds_a_line_and_up_and_down_keep_the_column(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abcd\nwxyz\n");
        cx.simulate_keystrokes("right right");
        cx.simulate_keystrokes("down");
        assert_eq!(
            cx.update(|_window, app| view.read(app).document().line_of(view.read(app).caret())),
            1
        );
        assert_eq!(caret(cx, &view), 7, "down keeps column two");

        cx.simulate_keystrokes("enter");
        assert_eq!(text(cx, &view), "abcd\nwx\nyz\n");
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));
    }

    #[gpui::test]
    fn save_writes_the_file_and_clears_dirty(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("bromine-text-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("notes.txt");
        std::fs::write(&path, "one\ntwo\n").expect("the file is written");

        let document = TextDocument::load(&path).expect("the file loads");
        let (view, cx) =
            cx.add_window_view(|_window, cx| TextView::new(path.clone(), document, cx));
        cx.update(|_window, app| crate::text_actions::bind_text_editor_keys(app));
        let handle = cx.update(|_window, app| view.read(app).focus_handle().clone());
        cx.update(|window, _app| handle.focus(window));

        cx.simulate_input("X");
        assert!(cx.update(|_window, app| view.read(app).is_dirty()));

        cx.update(|_window, app| view.update(app, |view, cx| view.save(cx)));

        assert!(!cx.update(|_window, app| view.read(app).is_dirty()));
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file reads back"),
            "Xone\ntwo\n"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    #[gpui::test]
    fn a_lossy_document_says_why_it_cannot_save(cx: &mut TestAppContext) {
        let document =
            TextDocument::from_bytes(Path::new("latin1.txt"), vec![b'c', b'a', b'f', 0xE9])
                .expect("the lossy file loads");
        let (view, cx) = cx.add_window_view(|_window, cx| {
            TextView::new(PathBuf::from("latin1.txt"), document, cx)
        });

        cx.update(|_window, app| view.update(app, |view, cx| view.save(cx)));

        assert!(
            cx.update(|_window, app| view.read(app).save_error().is_some()),
            "a lossy document must explain the refusal"
        );
        assert!(cx.update(|_window, app| view.read(app).document().is_lossy()));
    }

    #[test]
    fn a_tab_expands_to_four_display_columns_and_maps_back() {
        let line = DisplayLine::new("a\tb", 10);
        assert_eq!(line.text, "a    b");
        assert_eq!(line.source_at_column(0), 10);
        assert_eq!(line.source_at_column(1), 11);
        assert_eq!(line.source_at_column(4), 11, "inside the expansion");
        assert_eq!(line.source_at_column(5), 12, "after the tab");
        assert_eq!(line.column_for_source(12), 5);
    }
}
