//! Bromine: open a file and look at it.
//!
//! This binary only wires things together. Loading and parsing live in
//! `bromine-core`, drawing lives in `bromine-ui`, and both can be tested without
//! starting a window.

use std::path::PathBuf;

use bromine_core::{Format, load_csv};
use bromine_ui::CsvGrid;
use gpui::{
    Application, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, prelude::*,
    px, size,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("bromine: {error}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let path = match std::env::args_os().nth(1) {
        Some(path) => PathBuf::from(path),
        None => anyhow::bail!("usage: bromine <file.csv>"),
    };

    let document = match Format::from_path(&path) {
        Some(Format::Csv) => load_csv(&path)?,
        Some(Format::Json) => {
            anyhow::bail!("{} is JSON, which Bromine cannot show yet", path.display())
        }
        Some(Format::Markdown) => anyhow::bail!(
            "{} is Markdown, which Bromine cannot show yet",
            path.display()
        ),
        None => anyhow::bail!("{} is not a format Bromine recognises", path.display()),
    };

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    Application::new().run(move |cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.0), px(720.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(format!("{name} - Bromine"))),
                ..Default::default()
            }),
            window_min_size: Some(size(px(480.0), px(320.0))),
            ..Default::default()
        };

        let grid = cx.new(|cx| CsvGrid::new(path, document, cx));

        let window = match cx.open_window(options, |window, cx| {
            // Focus before the first frame, so the grid is already the target of
            // key events when the window appears.
            grid.read(cx).focus(window);
            grid.clone()
        }) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("bromine: could not open a window: {error}");
                return;
            }
        };

        // Creating the window can move focus, so focus once more now that it
        // exists.
        if let Err(error) = window.update(cx, |grid, window, _cx| grid.focus(window)) {
            eprintln!("bromine: could not focus the grid: {error}");
        }

        cx.activate(true);
    });

    Ok(())
}
