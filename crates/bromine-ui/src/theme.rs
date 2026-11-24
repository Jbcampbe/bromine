//! The palette.
//!
//! Every colour the UI draws lives here, so restyling is a one-file change and
//! the views stay about layout. Values are written as `hsla` rather than hex so
//! they can be nudged by eye without a conversion step.

use gpui::{Hsla, hsla};

/// The window behind everything else.
pub fn background() -> Hsla {
    hsla(0.611, 0.08, 0.10, 1.0)
}

/// Bars and headers that sit above the background.
pub fn surface() -> Hsla {
    hsla(0.611, 0.08, 0.145, 1.0)
}

/// A surface that is currently doing something, such as a sorted column header.
pub fn surface_active() -> Hsla {
    hsla(0.611, 0.10, 0.20, 1.0)
}

/// Every second data row, so long rows stay readable across.
pub fn row_alternate() -> Hsla {
    hsla(0.611, 0.08, 0.125, 1.0)
}

/// Lines between major regions.
pub fn border() -> Hsla {
    hsla(0.611, 0.08, 0.22, 1.0)
}

/// Lines between cells, which should be quieter than region borders.
pub fn border_subtle() -> Hsla {
    hsla(0.611, 0.08, 0.175, 1.0)
}

/// Body text.
pub fn text() -> Hsla {
    hsla(0.611, 0.10, 0.88, 1.0)
}

/// Labels and secondary counts.
pub fn text_muted() -> Hsla {
    hsla(0.611, 0.07, 0.58, 1.0)
}

/// Emphasis, such as the sorted column.
pub fn accent() -> Hsla {
    hsla(0.55, 0.70, 0.65, 1.0)
}

/// Something the user needs to know about the data itself.
pub fn warning() -> Hsla {
    hsla(0.09, 0.80, 0.62, 1.0)
}

/// The scrollbar thumbs. Quiet enough to ignore until it is looked for, but
/// present enough to find.
pub fn scrollbar() -> Hsla {
    hsla(0.611, 0.08, 0.34, 1.0)
}

/// The font for cell values and column headers.
///
/// Values are monospaced so digit columns line up and can be compared by eye.
/// Chrome stays in the system font, which is easier to read at small sizes.
/// Menlo ships with macOS; GPUI falls back to its default if it is missing.
pub fn monospace() -> &'static str {
    "Menlo"
}
