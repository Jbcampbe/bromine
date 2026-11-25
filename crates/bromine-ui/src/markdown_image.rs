//! Inline images for the Markdown view.
//!
//! Local destinations resolve to paths and remote `http(s)` destinations go
//! through GPUI's own image loader, which uses the app's HTTP client and image
//! cache. Anything else becomes a placeholder: showing the alt text and the
//! destination is more useful than a blank hole, and a missing file must not
//! take the document with it.

use bromine_core::{ImageInfo, ImageLocation};
use gpui::{AnyElement, IntoElement, ObjectFit, StyledImage, div, img, prelude::*, px};

use crate::markdown_view::COLUMN_MAX_WIDTH;
use crate::theme;

const IMAGE_MAX_HEIGHT: f32 = 320.0;

/// Render one image with its caption, or a placeholder describing why not.
pub(crate) fn image_element(info: &ImageInfo) -> AnyElement {
    let destination = match &info.location {
        ImageLocation::Local(path) => path.to_string_lossy().into_owned(),
        ImageLocation::Remote(url) => url.clone(),
        ImageLocation::Unresolved(url) => {
            return placeholder(&info.alt, url, "this image cannot be loaded");
        }
    };

    let alt = info.alt.clone();
    let fallback_destination = destination.clone();
    let fallback_alt = alt.clone();
    let loading = || {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(48.0))
            .w_full()
            .text_xs()
            .text_color(theme::text_muted())
            .child("loading image\u{2026}")
            .into_any_element()
    };
    let fallback = move || {
        placeholder(
            &fallback_alt,
            &fallback_destination,
            "this image could not be loaded",
        )
    };

    let picture = img(destination)
        .max_w(px(COLUMN_MAX_WIDTH - 64.0))
        .max_h(px(IMAGE_MAX_HEIGHT))
        .object_fit(ObjectFit::Contain)
        .with_loading(loading)
        .with_fallback(fallback);

    let mut container = div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .py_2()
        .child(picture);
    if !info.alt.is_empty() {
        container = container.child(
            div()
                .mt_1()
                .text_xs()
                .text_color(theme::text_muted())
                .child(info.alt.clone()),
        );
    }
    container.into_any_element()
}

fn placeholder(alt: &str, destination: &str, explanation: &str) -> AnyElement {
    let mut box_element = div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(theme::border())
        .bg(theme::image_placeholder())
        .p_4()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child(if alt.is_empty() {
                    "image".to_string()
                } else {
                    alt.to_string()
                }),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::warning())
                .child(explanation.to_string()),
        );
    if !destination.is_empty() {
        box_element = box_element.child(
            div()
                .text_xs()
                .font_family(theme::monospace())
                .text_color(theme::text_muted())
                .child(destination.to_string()),
        );
    }
    box_element.into_any_element()
}
