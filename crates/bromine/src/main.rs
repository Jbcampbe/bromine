//! Bromine: open files and look at them.
//!
//! This binary only wires things together. Loading and parsing live in
//! `bromine-core`, drawing lives in `bromine-ui`, and both can be tested without
//! starting a window.
//!
//! Every path on the command line becomes a tab. With no arguments the window
//! opens empty, and files are opened from inside it with Cmd+O.

mod cli;

use std::path::PathBuf;

use bromine_ui::{CloseTab, OpenFile, Quit, Workspace};
use gpui::{
    Application, Bounds, KeyBinding, Menu, MenuItem, SharedString, SystemMenuType, TitlebarOptions,
    WindowBounds, WindowOptions, prelude::*, px, size,
};

fn main() {
    let invocation = match cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let paths: Vec<PathBuf> = match invocation {
        cli::Invocation::Run(paths) => paths,
        cli::Invocation::Version => {
            println!("Bromine {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        cli::Invocation::Help => {
            println!("{}", cli::USAGE);
            return;
        }
    };

    Application::new().run(move |cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.0), px(720.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Bromine")),
                ..Default::default()
            }),
            window_min_size: Some(size(px(480.0), px(320.0))),
            ..Default::default()
        };

        // Global, unbound-by-context shortcuts, so they work no matter which
        // tab has focus.
        cx.bind_keys([
            KeyBinding::new("cmd-o", OpenFile, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);

        // GPUI installs no menu bar unless asked, and without one there is no
        // Quit item and Cmd+Q does nothing. The key equivalents themselves come
        // from the bindings above. Deliberately no Edit > Copy: binding `cmd-c`
        // would suppress the grid's own copy listener.
        cx.set_menus(vec![
            Menu {
                name: "Bromine".into(),
                items: vec![
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Quit Bromine", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Open\u{2026}", OpenFile),
                    MenuItem::action("Close Tab", CloseTab),
                ],
            },
        ]);

        let workspace = cx.new(Workspace::new);

        // A file that cannot be opened is reported in the window rather than
        // ending the process, so one bad argument does not hide the others.
        workspace.update(cx, |workspace, cx| {
            for path in paths {
                workspace.open(path, cx);
            }
        });

        let window = match cx.open_window(options, |_window, _cx| workspace.clone()) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("bromine: could not open a window: {error}");
                return;
            }
        };

        // Focus once the window exists: the grid's element has to have been laid
        // out before it can hold the keyboard.
        if let Err(error) = window.update(cx, |workspace, window, cx| {
            workspace.focus_active(window, cx);
        }) {
            eprintln!("bromine: could not focus the window: {error}");
        }

        cx.activate(true);
    });
}
