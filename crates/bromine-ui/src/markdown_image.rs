//! Inline images for the Markdown view.
//!
//! Local destinations resolve to paths and remote `http(s)` destinations go
//! through GPUI's own image loader, which uses the app's HTTP client and image
//! cache. Anything else becomes a placeholder: showing the alt text and the
//! destination is more useful than a blank hole, and a missing file must not
//! take the document with it.

use bromine_core::{ImageInfo, ImageLocation};
use gpui::{
    AnyElement, ImageSource, IntoElement, ObjectFit, StyledImage, div, img, prelude::*, px,
};

use crate::markdown_view::COLUMN_MAX_WIDTH;
use crate::theme;

const IMAGE_MAX_HEIGHT: f32 = 320.0;

/// Render one image with its caption, or a placeholder describing why not.
pub(crate) fn image_element(info: &ImageInfo) -> AnyElement {
    let Some((source, destination)) = image_source(info) else {
        return placeholder(&info.alt, &info.destination, "this image cannot be loaded");
    };

    let alt = info.alt.clone();
    let fallback_destination = destination;
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

    let mut picture = img(source).object_fit(ObjectFit::Contain);
    picture = match (info.width, info.height) {
        (Some(width), Some(height)) => picture.w(px(width as f32)).h(px(height as f32)),
        (Some(width), None) => picture.w(px(width as f32)).max_h(px(IMAGE_MAX_HEIGHT)),
        (None, Some(height)) => picture
            .max_w(px(COLUMN_MAX_WIDTH - 64.0))
            .h(px(height as f32)),
        (None, None) => picture
            .max_w(px(COLUMN_MAX_WIDTH - 64.0))
            .max_h(px(IMAGE_MAX_HEIGHT)),
    };
    let picture = picture.with_loading(loading).with_fallback(fallback);

    let mut container = div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .py_2()
        .child(picture);
    if info.caption && !info.alt.is_empty() {
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

/// The source GPUI should load, plus a label for the fallback.
///
/// A local path has to stay a `PathBuf`: GPUI treats a `String` that is not a
/// URI as an embedded resource name, so a filesystem path passed as a string
/// would never reach the disk.
fn image_source(info: &ImageInfo) -> Option<(ImageSource, String)> {
    match &info.location {
        ImageLocation::Local(path) => {
            Some((path.clone().into(), path.to_string_lossy().into_owned()))
        }
        ImageLocation::Remote(url) => Some((url.clone().into(), url.clone())),
        ImageLocation::Unresolved(_) => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Resource;
    use std::path::PathBuf;

    fn info(location: ImageLocation) -> ImageInfo {
        ImageInfo {
            alt: String::new(),
            destination: String::new(),
            location,
            width: None,
            height: None,
            caption: false,
        }
    }

    #[test]
    fn a_local_image_keeps_its_filesystem_path() {
        let info = info(ImageLocation::Local(PathBuf::from("/tmp/diagram.png")));
        let (source, _) = image_source(&info).expect("a local path has a source");

        match source {
            ImageSource::Resource(Resource::Path(path)) => {
                assert_eq!(&*path, std::path::Path::new("/tmp/diagram.png"));
            }
            _ => panic!("a local image must be a filesystem path, not an embedded resource"),
        }
    }

    #[test]
    fn a_remote_image_is_a_uri() {
        let info = info(ImageLocation::Remote(
            "https://example.com/badge.png".into(),
        ));
        let (source, _) = image_source(&info).expect("a URL has a source");

        assert!(matches!(source, ImageSource::Resource(Resource::Uri(_))));
    }

    #[test]
    fn an_unresolved_image_has_no_source() {
        assert!(image_source(&info(ImageLocation::Unresolved("data:x".into()))).is_none());
    }
}
