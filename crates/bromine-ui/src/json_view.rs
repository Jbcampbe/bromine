//! The JSON view: a read-only, virtualized view over a formatted document.
//!
//! JSON is shown as one line per formatted line rather than as a tree. A tree
//! would hide most of the file and make the thing a developer wants most often,
//! finding a line the parser complained about, harder. Formatting and minifying
//! are both offered because they answer different questions: one is for reading,
//! the other for confirming the file really is one record.
//!
//! The text is stored as a `Vec<SharedString>` of lines and the lines drawn are
//! only the ones in view, exactly as the CSV grid draws a row window. A 12 MB
//! document is 400k lines; creating that many elements would make the window
//! unusable.

use std::path::{Path, PathBuf};

use bromine_core::JsonDocument;
use gpui::{
    Context, Div, ElementId, FocusHandle, Font, MouseButton, MouseDownEvent, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, TextSystem, Window, div, font, point, prelude::*, px,
};

use crate::theme;

/// The height of one line of document text, in pixels. Every line is exactly
/// this tall, which is what lets the visible slice be computed from the scroll
/// offset instead of measured.
const LINE_HEIGHT: f32 = 20.0;

/// The font size of document text and line numbers. A number this module owns,
/// because the widest line is measured at this size to size the scrollable area.
const LINE_FONT_SIZE: f32 = 13.0;

/// The width of the line-number gutter, in pixels.
const GUTTER_WIDTH: f32 = 48.0;

/// Slack added to the measured width of the widest line, so the last character
/// is not clipped by the scroll area's edge.
const LINE_PADDING: f32 = 24.0;

/// How thick the drawn scrollbars are, in pixels.
const SCROLLBAR_THICKNESS: f32 = 8.0;

/// The shortest a scrollbar thumb may get, so a huge document still has a
/// handle that can be seen.
const MIN_SCROLLBAR_THUMB: f32 = 24.0;

/// The largest document this view will put on one line.
///
/// Minified mode shapes the whole document as a single line of text, and a line
/// of millions of characters costs the text system seconds and a lot of memory
/// before anything reaches the screen. Refusing is instant and honest; the
/// formatted view still shows every byte, and panning works normally.
const MAX_MINIFIED_BYTES: usize = 400_000;

/// How a JSON document is laid out for reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonMode {
    /// Two-space indentation, one line per value, for reading.
    Pretty,
    /// Everything on a single line, for confirming the record boundaries.
    Minified,
}

/// A read-only, virtualized view over a [`JsonDocument`].
///
/// The view owns the formatted text as lines rather than owning a formatted
/// `String`, so the line count and the lines that are drawn cannot disagree: both
/// come from the same vector. Switching modes rebuilds that vector.
pub struct JsonView {
    path: PathBuf,
    document: JsonDocument,
    mode: JsonMode,
    /// The whole document, one entry per line, in the current mode.
    lines: Vec<SharedString>,
    /// The width the widest line needs, so a document of long single-line
    /// records can be panned sideways. Measured once per mode, never per frame.
    line_width: f32,
    /// Where the text is scrolled to, on both axes. Used purely as storage: the
    /// element that tracks it clips its children and translates them, but the
    /// wheel handler below owns the offsets.
    body_scroll: ScrollHandle,
    /// Where the line numbers are scrolled to. They follow the text vertically
    /// and never move sideways, so they keep answering "which line is this"
    /// while the text is panned.
    gutter_scroll: ScrollHandle,
    /// Why the last mode change did not happen, shown in the status strip.
    notice: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl JsonView {
    /// Build a view over a document that has already been parsed. Starts pretty.
    pub fn new(path: PathBuf, document: JsonDocument, cx: &mut Context<Self>) -> Self {
        let text = document.pretty();
        let lines = split_lines(&text);
        let line_width = widest_line_width(&text, cx.text_system());

        Self {
            path,
            document,
            mode: JsonMode::Pretty,
            lines,
            line_width,
            body_scroll: ScrollHandle::new(),
            gutter_scroll: ScrollHandle::new(),
            notice: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The file this document came from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The handle that makes the view the target of key events.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Give the view keyboard focus.
    pub fn focus(&self, window: &mut Window) {
        self.focus_handle.focus(window);
    }

    /// How the document is currently laid out.
    pub fn mode(&self) -> JsonMode {
        self.mode
    }

    /// How many lines the current mode draws.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The text drawn on `index`, counting from zero.
    pub fn line_at(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(|line| line.as_str())
    }

    /// Switch between the formatted and the single-line rendering.
    ///
    /// The scroll offset is reset to the top because the two renderings have
    /// nothing in common: a line number means a different part of the file in
    /// each, so keeping the offset would put the reader somewhere arbitrary.
    pub fn set_mode(&mut self, mode: JsonMode, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }

        let text = match mode {
            JsonMode::Pretty => self.document.pretty(),
            JsonMode::Minified => {
                let bytes = self.document.minified().len();
                if bytes > MAX_MINIFIED_BYTES {
                    self.notice = Some(SharedString::from(format!(
                        "too large to show on one line ({bytes} bytes)"
                    )));
                    cx.notify();
                    return;
                }
                self.document.minified()
            }
        };

        self.lines = split_lines(&text);
        self.line_width = widest_line_width(&text, cx.text_system());
        self.body_scroll.set_offset(point(px(0.0), px(0.0)));
        self.gutter_scroll.set_offset(point(px(0.0), px(0.0)));
        self.notice = None;
        self.mode = mode;
        cx.notify();
    }

    /// The document is read only, so it is never dirty.
    pub fn is_dirty(&self) -> bool {
        false
    }

    /// The "JSON", the line count, and the two mode buttons.
    fn top_strip(&self, cx: &Context<Self>) -> Div {
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
                    .child("JSON"),
            )
            // The buttons stay right however wide the window is.
            .child(div().flex_1())
            .child(self.mode_button(JsonMode::Pretty, "format", "json-format", cx))
            .child(self.mode_button(JsonMode::Minified, "minify", "json-minify", cx))
    }

    /// One of the two mode buttons. The active mode is the accented one.
    fn mode_button(
        &self,
        mode: JsonMode,
        label: &'static str,
        id: &'static str,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let active = self.mode == mode;

        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .px_2()
            .cursor_pointer()
            .text_xs()
            .text_color(if active {
                theme::accent()
            } else {
                theme::text_muted()
            })
            .child(label)
            .on_click(cx.listener(move |this, _event, _window, cx| this.set_mode(mode, cx)))
    }

    /// The viewport the document scrolls in.
    ///
    /// One wheel handler covers the whole body and nothing below it scrolls
    /// natively, so exactly one thing moves the text and the gutter. The two
    /// viewports are positioned with absolute insets rather than `flex_1`: a
    /// flex item whose container has no definite height during layout takes its
    /// content's height instead of the space left over, which would make the
    /// viewport as tall as the document.
    fn body(&self, cx: &Context<Self>) -> Stateful<Div> {
        div()
            .id("json-body")
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(self.gutter_viewport())
            .child(self.lines_viewport())
            .children(self.scrollbars())
    }

    /// The line numbers, clipped to their own column.
    fn gutter_viewport(&self) -> Stateful<Div> {
        let numbers = self.visible_numbers();

        div()
            .id("json-gutter")
            .debug_selector(|| "json-gutter".to_string())
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
                // The content box is what the scroll handle measures itself
                // against, so it must keep the full height of the document.
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(GUTTER_WIDTH))
                    .h(px(self.lines.len() as f32 * LINE_HEIGHT))
                    .children(numbers),
            )
    }

    /// The document text, drawn for the part of the file that is in view.
    fn lines_viewport(&self) -> Stateful<Div> {
        let lines = self.visible_lines();

        div()
            .id("json-lines")
            .debug_selector(|| "json-lines".to_string())
            // Absolute insets rather than `flex_1`, for the reason in `body`.
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(GUTTER_WIDTH))
            .right(px(0.0))
            .overflow_hidden()
            .track_scroll(&self.body_scroll)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(self.line_width))
                    .h(px(self.lines.len() as f32 * LINE_HEIGHT))
                    .children(lines),
            )
    }

    /// The slice of line numbers that is in view.
    fn visible_numbers(&self) -> Vec<Stateful<Div>> {
        let (first, last) = self.line_window();
        (first..last).map(|index| self.line_number(index)).collect()
    }

    /// The slice of lines that is in view.
    fn visible_lines(&self) -> Vec<Stateful<Div>> {
        let (first, last) = self.line_window();
        (first..last).map(|index| self.line(index)).collect()
    }

    /// The visible slice of lines, as indices into `lines`.
    ///
    /// The numbers and the text have to agree on this window, so both ask for it
    /// here rather than each deriving it.
    fn line_window(&self) -> (usize, usize) {
        let lines = self.lines.len();
        if lines == 0 {
            return (0, 0);
        }

        // Bounds are known only after a frame has been laid out. Until then draw
        // a screenful: the next frame measures the real viewport and the count
        // settles.
        let measured = f32::from(self.body_scroll.bounds().size.height);
        let viewport_height = if measured > 0.0 {
            measured
        } else {
            LINE_HEIGHT * 40.0
        };

        let offset = f32::from(self.body_scroll.offset().y);
        let first = (((-offset) / LINE_HEIGHT).floor().max(0.0) as usize).min(lines - 1);
        let drawn = (viewport_height / LINE_HEIGHT).ceil() as usize + 1;
        (first, (first + drawn).min(lines))
    }

    /// One line number. The text counts from one; the index it is keyed by
    /// counts from zero, like the line it labels.
    fn line_number(&self, index: usize) -> Stateful<Div> {
        div()
            .id(ElementId::named_usize("json-line-number", index))
            .debug_selector({
                let selector = format!("json-line-number-{index}");
                move || selector
            })
            .absolute()
            .top(px(index as f32 * LINE_HEIGHT))
            .left(px(0.0))
            .w(px(GUTTER_WIDTH))
            .h(px(LINE_HEIGHT))
            .flex()
            .items_center()
            .justify_end()
            .pr_2()
            .font_family(theme::monospace())
            .text_size(px(LINE_FONT_SIZE))
            .text_color(theme::text_muted())
            .child(SharedString::from((index + 1).to_string()))
    }

    /// One line of document text.
    fn line(&self, index: usize) -> Stateful<Div> {
        div()
            .id(ElementId::named_usize("json-line", index))
            .debug_selector({
                let selector = format!("json-line-{index}");
                move || selector
            })
            .absolute()
            .top(px(index as f32 * LINE_HEIGHT))
            .left(px(0.0))
            .h(px(LINE_HEIGHT))
            .flex()
            .items_center()
            // No wrapping: a line is a line, and a long one is reached by
            // panning rather than by reflowing the document.
            .whitespace_nowrap()
            .font_family(theme::monospace())
            .text_size(px(LINE_FONT_SIZE))
            .text_color(theme::text())
            .child(self.lines[index].clone())
    }

    /// Thin bars along the right and bottom edges showing where in the document
    /// the view is.
    ///
    /// GPUI 0.2.2 has no scrollbar widget and a native scroll container would
    /// scroll itself, so the bars are drawn from the offsets. They show where
    /// you are; dragging them is not wired up yet.
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
                    .debug_selector(|| "json-scrollbar-vertical".to_string())
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
                    .debug_selector(|| "json-scrollbar-horizontal".to_string())
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

    /// The line count and the read-only notice along the bottom.
    fn status_strip(&self) -> Div {
        let mut strip = div()
            .debug_selector(|| "json-status".to_string())
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
                    .child(SharedString::from(format!("{} lines", self.lines.len()))),
            )
            .child(div().flex_1());

        if let Some(notice) = self.notice.clone() {
            strip = strip.child(div().text_xs().text_color(theme::warning()).child(notice));
        }

        strip.child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child("read only"),
        )
    }

    /// Scroll by one wheel event on both axes, clamped to the content.
    ///
    /// The grid latches a gesture to one axis because its rows and columns mean
    /// different things; a code column is read in both directions at once, so a
    /// diagonal swipe moving diagonally is what is expected here.
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

        // The numbers only ever follow the text vertically.
        self.gutter_scroll.set_offset(point(px(0.0), next.y));
        self.body_scroll.set_offset(next);
        cx.notify();
    }
}

impl Render for JsonView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("json-view")
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .text_sm()
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| this.focus(window)),
            )
            .child(self.top_strip(cx))
            .child(self.body(cx))
            .child(self.status_strip())
    }
}

/// Split formatted text into one entry per line.
///
/// `lines()` drops the trailing newline `to_string_pretty` never writes, so a
/// document of one line yields one entry rather than one entry and an empty
/// remainder.
fn split_lines(text: &str) -> Vec<SharedString> {
    text.lines()
        .map(|line| SharedString::from(line.to_string()))
        .collect()
}

/// How wide the widest line needs the scrollable text area to be.
///
/// The font is monospaced, so one representative advance times the longest
/// line's character count is close enough to size the area. Only lengths are
/// compared while scanning: measuring every line at load would cost one
/// measurement per line on a file with hundreds of thousands of them.
fn widest_line_width(text: &str, text_system: &TextSystem) -> f32 {
    let widest = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if widest == 0 {
        return 0.0;
    }

    let font: Font = font(theme::monospace());
    let font_id = text_system.resolve_font(&font);
    let advance = text_system
        .advance(font_id, px(LINE_FONT_SIZE), 'M')
        .map(|size| f32::from(size.width))
        .unwrap_or(0.0);
    widest as f32 * advance + LINE_PADDING
}

/// How long a scrollbar thumb should be, given the length of its track and how
/// far the content runs past it.
fn thumb_length(track: f32, overflow: f32) -> f32 {
    let visible = track / (track + overflow);
    // max-then-min rather than clamp: a track shorter than the minimum thumb
    // makes `clamp` panic, and a tiny window should not crash the view.
    (track * visible).max(MIN_SCROLLBAR_THUMB).min(track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bromine_core::parse_json;
    use gpui::{
        Entity, Modifiers, Pixels, Point, ScrollDelta, TestAppContext, TouchPhase,
        VisualTestContext,
    };

    fn parse(source: &str) -> JsonDocument {
        parse_json(Path::new("inline.json"), source.to_string()).expect("the document parses")
    }

    fn object() -> JsonDocument {
        parse(r#"{"name": "ada", "roles": ["admin", "author"], "commits": 412}"#)
    }

    /// Long enough that the formatted text is taller than any test window.
    fn long_document() -> JsonDocument {
        let values: Vec<String> = (0..5_000).map(|value| value.to_string()).collect();
        parse(&format!("[{}]", values.join(",")))
    }

    fn view(
        cx: &mut TestAppContext,
        document: JsonDocument,
    ) -> (Entity<JsonView>, &mut VisualTestContext) {
        cx.add_window_view(|_window, cx| JsonView::new(PathBuf::from("inline.json"), document, cx))
    }

    fn wheel(dx: f32, dy: f32) -> ScrollWheelEvent {
        ScrollWheelEvent {
            position: point(px(120.0), px(200.0)),
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        }
    }

    /// Point the mouse at the body, so the wheel event is dispatched to it.
    fn hover_the_body(cx: &mut VisualTestContext) {
        cx.simulate_mouse_move(point(px(120.0), px(200.0)), None, Modifiers::none());
    }

    /// Draw a frame, so bounds reflect what the last event left behind.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
    }

    fn scroll_offset(cx: &mut VisualTestContext, view: &Entity<JsonView>) -> Point<Pixels> {
        cx.update(|_window, app| view.read(app).body_scroll.offset())
    }

    fn line_count(cx: &mut VisualTestContext, view: &Entity<JsonView>) -> usize {
        cx.update(|_window, app| view.read(app).line_count())
    }

    #[gpui::test]
    fn pretty_draws_one_line_per_formatted_line(cx: &mut TestAppContext) {
        let document = object();
        let formatted = document.pretty().lines().count();
        let (view, cx) = view(cx, document);

        assert_eq!(line_count(cx, &view), formatted);
        assert_eq!(
            cx.update(|_window, app| view.read(app).line_at(0).map(str::to_string)),
            Some("{".to_string())
        );
        assert_eq!(
            cx.update(|_window, app| view.read(app).mode()),
            JsonMode::Pretty
        );
    }

    #[test]
    fn splitting_text_keeps_one_entry_per_line_and_drops_the_final_newline() {
        assert_eq!(split_lines("{\n  \"a\": 1\n}").len(), 3);
        assert_eq!(split_lines("{\"a\":1}").len(), 1);
        assert_eq!(split_lines("").len(), 0);

        let lines = split_lines("first\nsecond");
        assert_eq!(lines[0].as_str(), "first");
        assert_eq!(lines[1].as_str(), "second");
    }

    #[gpui::test]
    fn minifying_collapses_the_document_to_one_line(cx: &mut TestAppContext) {
        let document = object();
        let minified = document.minified();
        let (view, cx) = view(cx, document);

        cx.update(|_window, app| {
            view.update(app, |view, cx| view.set_mode(JsonMode::Minified, cx));
        });

        assert_eq!(line_count(cx, &view), 1);
        assert_eq!(
            cx.update(|_window, app| view.read(app).line_at(0).map(str::to_string)),
            Some(minified)
        );
        assert_eq!(
            cx.update(|_window, app| view.read(app).mode()),
            JsonMode::Minified
        );
    }

    #[gpui::test]
    fn switching_back_to_pretty_restores_the_lines_and_the_top_of_the_file(
        cx: &mut TestAppContext,
    ) {
        let document = long_document();
        let formatted = document.pretty().lines().count();
        let (view, cx) = view(cx, document);
        draw(cx);

        // Scroll well into the file, so a mode change has something to undo.
        hover_the_body(cx);
        cx.simulate_event(wheel(0.0, -400.0));
        assert!(
            scroll_offset(cx, &view).y < px(0.0),
            "the wheel did not scroll the document"
        );

        cx.update(|_window, app| {
            view.update(app, |view, cx| view.set_mode(JsonMode::Minified, cx));
        });
        assert_eq!(line_count(cx, &view), 1);
        assert_eq!(scroll_offset(cx, &view).y, px(0.0));

        cx.update(|_window, app| {
            view.update(app, |view, cx| view.set_mode(JsonMode::Pretty, cx));
        });
        assert_eq!(line_count(cx, &view), formatted);
        assert_eq!(
            scroll_offset(cx, &view).y,
            px(0.0),
            "the formatted document should open at its first line again"
        );
    }

    #[gpui::test]
    fn the_first_line_number_is_drawn_left_of_the_first_line(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, object());
        draw(cx);

        let number = cx
            .debug_bounds("json-line-number-0")
            .expect("the first line number was laid out");
        let line = cx
            .debug_bounds("json-line-0")
            .expect("the first line was laid out");

        assert!(
            number.right() <= line.left(),
            "the number {number:?} overlaps the line {line:?}"
        );
        assert_eq!(number.origin.y, line.origin.y);
        assert!(!cx.update(|_window, app| view.read(app).is_dirty()));
    }

    #[gpui::test]
    fn a_wheel_event_moves_the_offset(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, long_document());
        draw(cx);
        hover_the_body(cx);

        assert_eq!(scroll_offset(cx, &view).y, px(0.0));
        cx.simulate_event(wheel(0.0, -120.0));

        assert_eq!(scroll_offset(cx, &view).y, px(-120.0));
    }

    #[gpui::test]
    fn only_the_lines_in_view_are_drawn(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, long_document());
        draw(cx);

        // The document is 5002 lines tall; only a screenful is laid out, so a
        // line deep in the file does not exist as an element.
        assert!(cx.debug_bounds("json-line-0").is_some());
        assert!(cx.debug_bounds("json-line-4000").is_none());
        assert!(cx.update(|_window, app| view.read(app).line_count()) > 5_000);
    }

    #[gpui::test]
    fn a_document_too_long_for_one_line_is_not_minified(cx: &mut TestAppContext) {
        // Sixty thousand small records: 480 000 bytes on one line, past the
        // limit, while every formatted line stays short enough to draw.
        let record = r#"{"a":1}"#;
        let source = format!("[{}]", vec![record; 60_000].join(","));
        assert!(
            source.len() > MAX_MINIFIED_BYTES,
            "the fixture has to be past the limit to test it"
        );

        let (view, cx) = view(cx, parse(&source));
        let formatted = line_count(cx, &view);

        cx.update(|_window, app| {
            view.update(app, |view, cx| view.set_mode(JsonMode::Minified, cx));
        });

        assert_eq!(
            cx.update(|_window, app| view.read(app).mode()),
            JsonMode::Pretty,
            "the view must stay in a mode it can actually draw"
        );
        assert_eq!(line_count(cx, &view), formatted);
        assert!(
            cx.update(|_window, app| view.read(app).notice.is_some()),
            "refusing has to say why"
        );
    }
}
