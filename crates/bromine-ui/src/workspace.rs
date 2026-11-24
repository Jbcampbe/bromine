//! The workspace: one tab per open document.
//!
//! The workspace owns the list of open documents and which one is visible. Each
//! document is a [`CsvGrid`] entity, so a tab keeps its own sort and search
//! state; switching tabs never reloads a file.

use std::path::{Path, PathBuf};

use bromine_core::load_document;
use gpui::{
    App, Context, Div, ElementId, Entity, FocusHandle, MouseButton, MouseDownEvent,
    PathPromptOptions, Render, SharedString, Window, actions, div, prelude::*, px,
};

use crate::csv_grid::CsvGrid;
use crate::theme;

actions!(bromine, [OpenFile, CloseTab]);

/// The set of open documents and the visible one.
pub struct Workspace {
    tabs: Vec<Entity<CsvGrid>>,
    /// Index of the visible tab. Meaningless when `tabs` is empty, in which case
    /// it is kept at zero so indexing never needs a special case.
    active: usize,
    /// The last thing that went wrong, cleared by the next successful open.
    error: Option<SharedString>,
    /// True while the open panel is up. macOS hands out a single shared
    /// `NSOpenPanel`, so a second prompt would reconfigure the one on screen.
    prompting: bool,
    /// Focused only when no tab is open, so shortcuts still have a target.
    focus_handle: FocusHandle,
}

impl Workspace {
    /// Build an empty workspace.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            error: None,
            prompting: false,
            focus_handle: cx.focus_handle(),
        }
    }

    /// How many documents are open.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Which tab is visible.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The visible document, if there is one.
    pub fn active_tab(&self) -> Option<&Entity<CsvGrid>> {
        self.tabs.get(self.active)
    }

    /// The last open failure, formatted for display.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().map(|error| &**error)
    }

    /// The focus handle of the visible document, if there is one.
    pub fn active_focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        self.tabs
            .get(self.active)
            .map(|grid| grid.read(cx).focus_handle().clone())
    }

    /// Give keyboard focus to the visible document, or to the workspace when
    /// nothing is open.
    pub fn focus_active(&self, window: &mut Window, cx: &App) {
        match self.active_focus_handle(cx) {
            Some(handle) => handle.focus(window),
            None => self.focus_handle.focus(window),
        }
    }

    /// Open `path`, or show the tab that already has it.
    ///
    /// Returns whether the document is now open. A file that is already open is
    /// focused rather than opened twice, which is what a browser does and what
    /// stops the tab bar filling with duplicates of the same file.
    pub fn open(&mut self, path: impl Into<PathBuf>, cx: &mut Context<Self>) -> bool {
        let path = path.into();

        if let Some(index) = self.index_of(&path, cx) {
            self.active = index;
            self.error = None;
            cx.notify();
            return true;
        }

        match load_document(&path) {
            Ok(document) => {
                let grid = cx.new(|cx| CsvGrid::new(path, document, cx));
                self.tabs.push(grid);
                self.active = self.tabs.len() - 1;
                self.error = None;
                cx.notify();
                true
            }
            Err(error) => {
                self.error = Some(SharedString::from(error.to_string()));
                cx.notify();
                false
            }
        }
    }

    /// Show the tab at `index` and move keyboard focus into it.
    pub fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        if index != self.active {
            self.active = index;
            cx.notify();
        }
        self.focus_active(window, cx);
    }

    /// Close the tab at `index` and show a neighbour.
    pub fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        self.tabs.remove(index);

        if self.tabs.is_empty() {
            self.active = 0;
        } else if index < self.active {
            // Everything after the closed tab shifted down by one.
            self.active -= 1;
        } else if self.active >= self.tabs.len() {
            // The visible tab was the last one, so clamp to the new end.
            self.active = self.tabs.len() - 1;
        }

        self.focus_active(window, cx);
        cx.notify();
    }

    /// Close the visible tab.
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.close(self.active, window, cx);
        }
    }

    /// Ask the user for files and open the ones they pick.
    ///
    /// The panel gives no way to filter by extension, so a user can pick any
    /// file; [`load_document`] rejects what it cannot show and the reason lands
    /// in the error strip rather than in a dialog.
    fn request_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompting {
            return;
        }
        self.prompting = true;

        // On an unbundled executable started from a terminal the app may not be
        // the active one, and the panel would open behind the window.
        cx.activate(true);

        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(SharedString::from("Open")),
        });

        cx.spawn_in(window, async move |workspace, cx| {
            let picked = prompt.await;

            let _ = workspace.update_in(cx, |workspace, window, cx| {
                workspace.prompting = false;

                // Cancelled, cancelled by the platform, or the channel closed.
                let Ok(Ok(Some(paths))) = picked else {
                    cx.notify();
                    return;
                };

                for path in paths {
                    workspace.open(path, cx);
                }
                workspace.focus_active(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.request_open(window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(window, cx);
    }

    /// The index of an already-open document with the same file behind `path`.
    ///
    /// Both sides are canonicalized, so `fixtures/people.csv` from the command
    /// line and the absolute path a file dialog returns are recognised as one
    /// file rather than two tabs.
    fn index_of(&self, path: &Path, cx: &App) -> Option<usize> {
        let wanted = canonical(path);
        self.tabs
            .iter()
            .position(|tab| canonical(tab.read(cx).path()) == wanted)
    }

    fn tab_bar(&self, cx: &Context<Self>) -> Div {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (index, grid) in self.tabs.iter().enumerate() {
            tabs.push(self.tab(index, grid, cx));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(
                div()
                    .id("open-file")
                    .debug_selector(|| "open-file".to_string())
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme::accent())
                    .child("open")
                    .on_click(
                        cx.listener(|this, _event, window, cx| this.request_open(window, cx)),
                    ),
            )
    }

    fn tab(&self, index: usize, grid: &Entity<CsvGrid>, cx: &Context<Self>) -> Div {
        let visible = index == self.active;
        let title = title_of(grid.read(cx).path());

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .border_r_1()
            .border_color(theme::border())
            .when(visible, |el| el.bg(theme::background()))
            .child(
                div()
                    .id(ElementId::named_usize("tab", index))
                    .debug_selector({
                        let selector = format!("tab-{index}");
                        move || selector
                    })
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(if visible {
                        theme::text()
                    } else {
                        theme::text_muted()
                    })
                    .child(
                        div()
                            .font_family(theme::monospace())
                            .truncate()
                            .child(SharedString::from(title)),
                    )
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.activate(index, window, cx);
                    })),
            )
            .child(
                div()
                    .id(ElementId::named_usize("tab-close", index))
                    .debug_selector({
                        let selector = format!("tab-close-{index}");
                        move || selector
                    })
                    .px_2()
                    .py_2()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("\u{d7}")
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.close(index, window, cx);
                    })),
            )
    }

    fn error_strip(&self, cx: &Context<Self>) -> Div {
        let Some(error) = self.error.clone() else {
            return div();
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_xs()
                    .text_color(theme::warning())
                    .child(error),
            )
            .child(
                div()
                    .id("dismiss-error")
                    .px_2()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("dismiss")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.error = None;
                        cx.notify();
                    })),
            )
    }

    fn empty_state(&self, cx: &Context<Self>) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(div().text_color(theme::text_muted()).child("No file open"))
            .child(
                div()
                    .id("empty-open")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(theme::accent())
                    .child("Open a CSV file")
                    .on_click(
                        cx.listener(|this, _event, window, cx| this.request_open(window, cx)),
                    ),
            )
    }

    fn body(&self, cx: &Context<Self>) -> Div {
        match self.tabs.get(self.active) {
            Some(grid) => div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(grid.clone()),
            None => self.empty_state(cx),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace")
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::background())
            .text_color(theme::text())
            .text_sm()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::close_tab))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                    // With no document open there is nothing else to hold focus,
                    // and without focus the window-level shortcuts never fire.
                    if this.tabs.is_empty() {
                        this.focus_handle.focus(window);
                    }
                }),
            )
            .child(self.tab_bar(cx))
            .child(self.error_strip(cx))
            .child(self.body(cx))
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn title_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{KeyBinding, Modifiers, TestAppContext, VisualTestContext};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }

    fn tab_count(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) -> usize {
        cx.update(|_window, app| workspace.read(app).tab_count())
    }

    fn active_index(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) -> usize {
        cx.update(|_window, app| workspace.read(app).active_index())
    }

    fn query_of(
        cx: &mut VisualTestContext,
        workspace: &Entity<Workspace>,
        index: usize,
    ) -> Option<String> {
        cx.update(|_window, app| {
            workspace
                .read(app)
                .tabs
                .get(index)
                .map(|grid| grid.read(app).query().to_string())
        })
    }

    fn bind_shortcuts(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) {
        cx.update(|window, app| {
            app.bind_keys([
                KeyBinding::new("cmd-o", OpenFile, None),
                KeyBinding::new("cmd-w", CloseTab, None),
            ]);
            workspace.read(app).focus_active(window, app);
        });
    }

    #[gpui::test]
    fn opening_two_files_creates_two_tabs_and_shows_the_newest(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        let opened = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                (
                    workspace.open(fixture("people.csv"), cx),
                    workspace.open(fixture("quirky.csv"), cx),
                )
            })
        });

        assert_eq!(opened, (true, true));
        assert_eq!(tab_count(cx, &workspace), 2);
        assert_eq!(active_index(cx, &workspace), 1);
    }

    #[gpui::test]
    fn opening_the_same_file_twice_focuses_the_tab_it_already_has(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.open(fixture("quirky.csv"), cx);
                // Relative and absolute forms of one file must not become two tabs.
                workspace.open(fixture("people.csv"), cx);
            })
        });

        assert_eq!(tab_count(cx, &workspace), 2);
        assert_eq!(active_index(cx, &workspace), 0);
    }

    #[gpui::test]
    fn clicking_a_tab_makes_it_the_visible_one(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.open(fixture("quirky.csv"), cx);
            })
        });
        assert_eq!(active_index(cx, &workspace), 1);

        let tab = cx
            .debug_bounds("tab-0")
            .expect("the first tab was laid out");
        cx.simulate_mouse_down(tab.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(tab.center(), MouseButton::Left, Modifiers::none());

        assert_eq!(active_index(cx, &workspace), 0);
    }

    #[gpui::test]
    fn the_close_button_removes_a_tab_without_closing_the_others(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.open(fixture("quirky.csv"), cx);
            })
        });

        let close = cx
            .debug_bounds("tab-close-0")
            .expect("the first tab's close button was laid out");
        cx.simulate_mouse_down(close.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(close.center(), MouseButton::Left, Modifiers::none());

        assert_eq!(tab_count(cx, &workspace), 1);
        // The closed tab was before the visible one, so the visible tab shifted down.
        assert_eq!(active_index(cx, &workspace), 0);
    }

    #[gpui::test]
    fn closing_the_last_tab_leaves_an_empty_workspace(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx)
            });
        });

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| workspace.close_active(window, cx));
        });

        assert_eq!(tab_count(cx, &workspace), 0);
        assert!(
            cx.update(|_window, app| workspace.read(app).active_tab().is_none()),
            "no tab should be visible once the last one closes"
        );
    }

    #[gpui::test]
    fn cmd_w_closes_the_visible_tab_through_the_keymap(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.open(fixture("quirky.csv"), cx);
            })
        });
        bind_shortcuts(cx, &workspace);

        assert_eq!(tab_count(cx, &workspace), 2);
        cx.simulate_keystrokes("cmd-w");
        assert_eq!(tab_count(cx, &workspace), 1);
        cx.simulate_keystrokes("cmd-w");
        assert_eq!(tab_count(cx, &workspace), 0);
    }

    #[gpui::test]
    fn typing_reaches_only_the_visible_tab(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.open(fixture("quirky.csv"), cx);
            })
        });
        bind_shortcuts(cx, &workspace);

        cx.simulate_input("ad");
        assert_eq!(
            query_of(cx, &workspace, 1).as_deref(),
            Some("ad"),
            "the visible tab should have received the keystrokes"
        );
        assert_eq!(query_of(cx, &workspace, 0).as_deref(), Some(""));

        // Switch back and type again: the first tab keeps its own search text.
        let tab = cx
            .debug_bounds("tab-0")
            .expect("the first tab was laid out");
        cx.simulate_mouse_down(tab.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(tab.center(), MouseButton::Left, Modifiers::none());

        cx.simulate_input("gr");
        assert_eq!(query_of(cx, &workspace, 0).as_deref(), Some("gr"));
        assert_eq!(query_of(cx, &workspace, 1).as_deref(), Some("ad"));
    }

    #[gpui::test]
    fn a_file_that_cannot_be_opened_reports_why_and_adds_no_tab(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        let opened = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.open("notes.txt", cx))
        });

        assert!(!opened);
        assert_eq!(tab_count(cx, &workspace), 0);
        let error = cx
            .update(|_window, app| workspace.read(app).error().map(str::to_string))
            .expect("an open failure must be reported");
        assert!(
            error.contains("is not a format Bromine can open"),
            "unexpected message: {error}"
        );
    }

    #[gpui::test]
    fn a_later_successful_open_clears_the_previous_error(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open("notes.txt", cx);
                workspace.open(fixture("people.csv"), cx);
            })
        });

        assert_eq!(tab_count(cx, &workspace), 1);
        assert_eq!(
            cx.update(|_window, app| workspace.read(app).error().map(str::to_string)),
            None
        );
    }
}
