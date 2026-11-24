//! Bromine: open files and look at them.
//!
//! This binary only wires things together. Loading and parsing live in
//! `bromine-core`, drawing lives in `bromine-ui`, and both can be tested without
//! starting a window.
//!
//! Every path on the command line becomes a tab, and so does a file opened from
//! Finder. With no files the window opens empty, and files are opened from
//! inside it with Cmd+O.

mod cli;
mod open_urls;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use bromine_ui::{CloseTab, OpenFile, Quit, Workspace};
use gpui::{
    App, Application, AsyncApp, Bounds, Context, Entity, KeyBinding, Menu, MenuItem, SharedString,
    SystemMenuType, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, prelude::*,
    px, size,
};

/// Where a file handed over by Finder can be opened once a window exists.
#[derive(Clone)]
struct OpenTarget {
    async_cx: AsyncApp,
    workspace: Entity<Workspace>,
    /// A `Cell` because the window can be closed and replaced while the app
    /// keeps running, and the open handler only has a shared reference here.
    window: Rc<Cell<Option<WindowHandle<Workspace>>>>,
}

impl OpenTarget {
    fn open(&self, paths: Vec<PathBuf>) {
        let result = self.async_cx.update(|cx| {
            cx.activate(true);

            if let Some(window) = self.window.get()
                && window
                    .update(cx, |workspace, window, cx| {
                        show(workspace, window, cx, &paths)
                    })
                    .is_ok()
            {
                return Ok(());
            }

            // The window was closed while the app stayed alive; opening a file
            // has to make one again.
            let window =
                cx.open_window(window_options(cx), |_window, _cx| self.workspace.clone())?;
            self.window.set(Some(window));
            window.update(cx, |workspace, window, cx| {
                show(workspace, window, cx, &paths)
            })?;
            Ok(())
        });
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) | Err(error) => {
                eprintln!("bromine: could not open a file from Finder: {error}");
            }
        }
    }
}

/// Open paths as tabs, bring the window forward, and give the newest tab the
/// keyboard.
fn show(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    paths: &[PathBuf],
) {
    for path in paths {
        workspace.open(path.clone(), cx);
    }
    workspace.focus_active(window, cx);
    window.activate_window();
}

fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
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
    }
}

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

    // A double-clicked file does not arrive on the command line: macOS sends an
    // open event instead. The handler is installed before the window exists, so
    // an event that arrives during launch is queued until it can be shown.
    let pending_paths: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
    let target: Rc<RefCell<Option<OpenTarget>>> = Rc::new(RefCell::new(None));
    let queued = pending_paths.clone();
    let open_target = target.clone();

    let app = Application::new();
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|url| open_urls::path_from_url(url))
            .collect();
        if paths.is_empty() {
            return;
        }

        let target = open_target.borrow().clone();
        match target {
            Some(target) => target.open(paths),
            None => queued.borrow_mut().extend(paths),
        }
    });
    app.run(move |cx| {
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

        let window = match cx.open_window(window_options(cx), |_window, _cx| workspace.clone()) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("bromine: could not open a window: {error}");
                return;
            }
        };

        // Files handed over during launch can be shown now that the window
        // exists. Later open events use the target directly.
        let launched: Vec<PathBuf> = pending_paths.borrow_mut().drain(..).collect();
        if !launched.is_empty() {
            workspace.update(cx, |workspace, cx| {
                for path in launched {
                    workspace.open(path, cx);
                }
            });
        }
        *target.borrow_mut() = Some(OpenTarget {
            async_cx: cx.to_async(),
            workspace: workspace.clone(),
            window: Rc::new(Cell::new(Some(window))),
        });

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
