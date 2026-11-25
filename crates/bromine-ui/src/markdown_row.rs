//! Shaped text for Markdown rows and table cells.
//!
//! GPUI measures and shapes text itself, and that measured layout is also the
//! only reliable way to turn a click back into a byte offset in proportional,
//! wrapped prose. Each row (and each table cell) is therefore a custom element
//! that shapes its text during layout, paints it, and records the result so the
//! view can map a click to a source offset on the next event.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bromine_core::{Row, RowKind, RunStyle};
use gpui::{
    App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Point, SharedString, StrikethroughStyle, Style, TextAlign,
    TextRun, UnderlineStyle, Window, WrappedLine, fill, font, relative, size,
};

use crate::markdown_view::MarkdownView;
use crate::theme;

/// Identifies the text a shaped element belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TextKey {
    Row(usize),
    Cell {
        table: usize,
        row: usize,
        column: usize,
    },
}

/// The shaped layout and paint bounds of one piece of text.
#[derive(Debug, Clone)]
pub(crate) struct TextLayoutRecord {
    pub line: WrappedLine,
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
}

/// The base typography of a run's line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BaseStyle {
    pub font_size: f32,
    pub monospace: bool,
    pub bold: bool,
    pub color: gpui::Hsla,
}

impl BaseStyle {
    pub fn body() -> Self {
        Self {
            font_size: 16.0,
            monospace: false,
            bold: false,
            color: theme::text(),
        }
    }

    pub fn heading(level: u8) -> Self {
        let font_size = match level {
            1 => 32.0,
            2 => 27.0,
            3 => 23.0,
            4 => 20.0,
            5 => 18.0,
            _ => 16.0,
        };
        Self {
            font_size,
            monospace: false,
            bold: true,
            color: theme::text(),
        }
    }

    pub fn code() -> Self {
        Self {
            font_size: 13.0,
            monospace: true,
            bold: false,
            color: theme::text(),
        }
    }

    pub fn table(header: bool) -> Self {
        Self {
            font_size: 13.0,
            monospace: true,
            bold: header,
            color: theme::text(),
        }
    }

    /// The line height for this style.
    pub fn line_height(&self) -> f32 {
        (self.font_size * 1.45).max(16.0)
    }
}

/// The base style for one row.
pub(crate) fn base_style(row: &Row) -> BaseStyle {
    match row.kind {
        RowKind::Heading(level) => BaseStyle::heading(level),
        RowKind::Code | RowKind::Fence => BaseStyle::code(),
        RowKind::TableRow { header } => BaseStyle::table(header),
        _ => BaseStyle::body(),
    }
}

/// Build the platform text runs for a piece of text.
///
/// `runs` are the display segments in order, without selection or composition
/// applied; this splits each of them again at those boundaries so the caret's
/// selection is painted as part of the same shaped line.
pub(crate) fn build_runs(
    runs: &[(usize, RunStyle)],
    selection: std::ops::Range<usize>,
    marked: Option<std::ops::Range<usize>>,
    base: BaseStyle,
) -> Vec<TextRun> {
    let mut text_runs = Vec::new();
    let mut cursor = 0;

    for (len, style) in runs {
        let run_start = cursor;
        let run_end = cursor + len;
        cursor = run_end;

        let mut boundaries = vec![run_start, run_end];
        for boundary in [
            selection.start,
            selection.end,
            marked.as_ref().map(|range| range.start).unwrap_or(0),
            marked.as_ref().map(|range| range.end).unwrap_or(0),
        ] {
            if boundary > run_start && boundary < run_end {
                boundaries.push(boundary);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        for pair in boundaries.windows(2) {
            let start = pair[0];
            let end = pair[1];
            if end == start {
                continue;
            }
            let selected =
                !selection.is_empty() && start >= selection.start && start < selection.end;
            let composing = marked
                .as_ref()
                .is_some_and(|range| start >= range.start && start < range.end);

            let mut run_font = if style.code || base.monospace {
                font(theme::monospace())
            } else {
                font(theme::prose_font())
            };
            if style.bold || base.bold {
                run_font = run_font.bold();
            }
            if style.italic {
                run_font = run_font.italic();
            }

            let color = if style.marker {
                theme::text_muted()
            } else if style.link.is_some() {
                theme::accent()
            } else {
                base.color
            };
            let font_size = if style.code && !base.monospace {
                (base.font_size - 2.0).max(12.0)
            } else {
                base.font_size
            };

            text_runs.push(TextRun {
                len: end - start,
                font: run_font,
                // `TextRun` carries no size, so inline code is the same size as
                // its line. The monospace face is what distinguishes it.
                color,
                background_color: if selected {
                    Some(theme::selection())
                } else if style.code && !style.marker {
                    Some(theme::code_surface())
                } else {
                    None
                },
                underline: if composing {
                    Some(UnderlineStyle {
                        color: Some(theme::accent()),
                        thickness: gpui::px(1.0),
                        wavy: false,
                    })
                } else {
                    None
                },
                strikethrough: style.strikethrough.then(|| StrikethroughStyle {
                    color: Some(theme::text_muted()),
                    thickness: gpui::px(1.0),
                }),
            });

            // The font size is part of the style, not the run, so a single
            // shaped line uses one size. Inline code keeps the body size; the
            // face change is enough to read it as code.
            let _ = font_size;
        }
    }

    text_runs
}

/// The display text, runs, and caret position for one row.
pub(crate) struct RowVisual {
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub font_size: f32,
    pub line_height: f32,
    pub caret: Option<usize>,
}

/// Build everything the shaped element needs for a row.
pub(crate) fn row_visual(view: &MarkdownView, row_index: usize, focused: bool) -> RowVisual {
    let row = &view.document.rows()[row_index];
    let active = view.active_block == Some(row.block);
    let base = base_style(row);

    let (text, source_runs) = if active {
        (
            view.document.source()[row.source.clone()].to_string(),
            row.active_runs(view.document.source()),
        )
    } else {
        (row.text.clone(), row.runs.clone())
    };

    let selection = view.editor.selection();
    let display_selection = if !selection.is_empty()
        && selection.start <= row.source.end
        && selection.end >= row.source.start
    {
        if active {
            let start = (selection.start.saturating_sub(row.source.start)).min(text.len());
            let end = (selection.end.saturating_sub(row.source.start)).min(text.len());
            start..end
        } else {
            row.source_to_display(selection.start)..row.source_to_display(selection.end)
        }
    } else {
        0..0
    };
    let display_selection =
        display_selection.start.min(text.len())..display_selection.end.min(text.len());

    let marked = view.editor.marked.as_ref().and_then(|range| {
        if range.start > row.source.end || range.end < row.source.start {
            return None;
        }
        Some(if active {
            (range.start.saturating_sub(row.source.start)).min(text.len())
                ..(range.end.saturating_sub(row.source.start)).min(text.len())
        } else {
            row.source_to_display(range.start)..row.source_to_display(range.end)
        })
    });
    let marked = marked.map(|range| range.start.min(text.len())..range.end.min(text.len()));

    let segments: Vec<(usize, RunStyle)> = source_runs
        .iter()
        .map(|run| (run.text.len(), run.style.clone()))
        .collect();
    let runs = build_runs(&segments, display_selection, marked, base);

    let caret_on_this_row =
        focused && view.editor.caret >= row.source.start && view.editor.caret <= row.source.end;
    let caret = caret_on_this_row.then(|| {
        if active {
            (view.editor.caret.saturating_sub(row.source.start)).min(text.len())
        } else {
            row.source_to_display(view.editor.caret).min(text.len())
        }
    });

    RowVisual {
        text: SharedString::from(text),
        runs,
        font_size: base.font_size,
        line_height: base.line_height(),
        caret,
    }
}

/// A custom element that shapes, paints, and records one line of text.
pub(crate) struct ShapedTextElement {
    view: gpui::Entity<MarkdownView>,
    key: TextKey,
    text: SharedString,
    runs: Vec<TextRun>,
    font_size: Pixels,
    line_height: Pixels,
    caret: Option<usize>,
    align: TextAlign,
    cached: Rc<RefCell<Option<WrappedLine>>>,
    shaped: Option<WrappedLine>,
}

impl ShapedTextElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        view: gpui::Entity<MarkdownView>,
        key: TextKey,
        text: SharedString,
        runs: Vec<TextRun>,
        font_size: f32,
        line_height: f32,
        caret: Option<usize>,
        align: TextAlign,
    ) -> Self {
        Self {
            view,
            key,
            text,
            runs,
            font_size: gpui::px(font_size),
            line_height: gpui::px(line_height),
            caret,
            align,
            cached: Rc::new(RefCell::new(None)),
            shaped: None,
        }
    }
}

impl IntoElement for ShapedTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ShapedTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();

        let text = self.text.clone();
        let runs = self.runs.clone();
        let font_size = self.font_size;
        let line_height = self.line_height;
        let cached = self.cached.clone();

        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let line = window
                    .text_system()
                    .shape_text(text.clone(), font_size, &runs, width, None)
                    .ok()
                    .and_then(|mut lines| lines.pop());
                if let Some(line) = &line {
                    cached.borrow_mut().replace(line.clone());
                }
                line.map(|line| line.size(line_height)).unwrap_or_default()
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.shaped = self.cached.borrow_mut().take();
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(line) = self.shaped.take() else {
            return;
        };

        let _ = line.paint_background(
            bounds.origin,
            self.line_height,
            self.align,
            Some(bounds),
            window,
            cx,
        );
        let _ = line.paint(
            bounds.origin,
            self.line_height,
            self.align,
            Some(bounds),
            window,
            cx,
        );

        if let Some(caret) = self.caret
            && let Some(position) = line.position_for_index(caret, self.line_height)
        {
            let caret_bounds = Bounds::new(
                Point::new(
                    bounds.left() + position.x,
                    bounds.top() + position.y + gpui::px(2.0),
                ),
                size(gpui::px(2.0), self.line_height - gpui::px(4.0)),
            );
            window.paint_quad(fill(caret_bounds, theme::accent()));
        }

        let key = self.key;
        let line_height = self.line_height;
        self.view.update(cx, |view, _cx| {
            view.record_text_layout(
                key,
                TextLayoutRecord {
                    line: line.clone(),
                    bounds,
                    line_height,
                },
            );
        });
    }
}

/// The layouts recorded by the last frame.
pub(crate) type RowLayouts = HashMap<usize, TextLayoutRecord>;
pub(crate) type CellLayouts = HashMap<TextKey, TextLayoutRecord>;

/// The display byte offset for a source offset in a row, and the reverse,
/// choosing the active or inactive rendering.
pub(crate) fn row_display_offset(view: &MarkdownView, row_index: usize, source: usize) -> usize {
    let row = &view.document.rows()[row_index];
    if view.active_block == Some(row.block) {
        (source.saturating_sub(row.source.start)).min(row.source.end - row.source.start)
    } else {
        row.source_to_display(source)
    }
}

/// The source offset for a display offset in a row.
pub(crate) fn row_source_offset(view: &MarkdownView, row_index: usize, display: usize) -> usize {
    let row = &view.document.rows()[row_index];
    if view.active_block == Some(row.block) {
        (row.source.start + display).min(row.source.end)
    } else {
        row.display_to_source(display)
    }
}
