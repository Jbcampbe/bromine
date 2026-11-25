//! The workspace: one tab per open document.
//!
//! The workspace owns the list of open documents and which one is visible. Each
//! document is an entity for its own kind of view, so a tab keeps its own sort,
//! search, or scroll state; switching tabs never reloads a file.

use std::path::{Path, PathBuf};

use bromine_core::{Document, load_document};
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, MouseButton, MouseDownEvent,
    PathPromptOptions, Render, SharedString, Window, actions, div, prelude::*, px,
};

use crate::csv_grid::CsvGrid;
use crate::json_view::JsonView;
use crate::markdown_view::MarkdownView;
use crate::text_view::TextView;
use crate::theme;

actions!(bromine, [OpenFile, CloseTab, Quit, Save]);

/// One open document, whichever viewer it needs.
///
/// The tab strip and the focus handling only ever need a path, a focus handle,
/// and a dirty flag, so they ask the tab for those rather than matching on the
/// variant themselves. [`Workspace::open`] is the only place that has to know
/// how a [`Document`] becomes a view.
pub(crate) enum TabView {
    Csv(Entity<CsvGrid>),
    Json(Entity<JsonView>),
    Text(Entity<TextView>),
    Markdown(Entity<MarkdownView>),
}

impl TabView {
    /// The file this tab shows.
    fn path(&self, cx: &App) -> PathBuf {
        match self {
            Self::Csv(grid) => grid.read(cx).path().to_path_buf(),
            Self::Json(view) => view.read(cx).path().to_path_buf(),
            Self::Text(view) => view.read(cx).path().to_path_buf(),
            Self::Markdown(view) => view.read(cx).path().to_path_buf(),
        }
    }

    /// The handle that makes this tab's view the target of key events.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Csv(grid) => grid.read(cx).focus_handle().clone(),
            Self::Json(view) => view.read(cx).focus_handle().clone(),
            Self::Text(view) => view.read(cx).focus_handle().clone(),
            Self::Markdown(view) => view.read(cx).focus_handle().clone(),
        }
    }

    fn focus(&self, window: &mut Window, cx: &App) {
        match self {
            Self::Csv(grid) => grid.read(cx).focus(window),
            Self::Json(view) => view.read(cx).focus(window),
            Self::Text(view) => view.read(cx).focus(window),
            Self::Markdown(view) => view.read(cx).focus(window),
        }
    }

    /// Whether this tab has edits that are not on disk yet.
    fn is_dirty(&self, cx: &App) -> bool {
        match self {
            Self::Csv(grid) => grid.read(cx).is_dirty(),
            Self::Json(view) => view.read(cx).is_dirty(),
            Self::Text(view) => view.read(cx).is_dirty(),
            Self::Markdown(view) => view.read(cx).is_dirty(),
        }
    }

    /// Write this tab back to its file. Read-only views do nothing.
    fn save(&self, cx: &mut App) {
        match self {
            Self::Csv(grid) => grid.update(cx, |grid, cx| grid.save(cx)),
            Self::Json(_) => {}
            Self::Text(view) => view.update(cx, |view, cx| view.save(cx)),
            Self::Markdown(view) => view.update(cx, |view, cx| view.save(cx)),
        }
    }
}

/// The set of open documents and the visible one.
pub struct Workspace {
    tabs: Vec<TabView>,
    /// Index of the visible tab. Meaningless when `tabs` is empty, in which case
    /// it is kept at zero so indexing never needs a special case.
    active: usize,
    /// The last thing that went wrong, cleared by the next successful open.
    error: Option<SharedString>,
    /// True while the open panel is up. macOS hands out a single shared
    /// `NSOpenPanel`, so a second prompt would reconfigure the one on screen.
    prompting: bool,
    /// The tab the user asked to close while it had unsaved edits. Asking twice
    /// discards them, because there is no undo behind this: one keystroke should
    /// not be able to throw away work it never mentioned.
    pending_close: Option<usize>,
    /// Whether the user has already been warned that quitting would lose
    /// edits. A second Quit goes through; any change in a tab clears this, so
    /// an edit made after the warning makes the next Quit ask again.
    pending_quit: bool,
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
            pending_close: None,
            pending_quit: false,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Follow a new tab's notifications, so the tab strip redraws when the view
    /// behind it changes: a title's dirty dot is read during the workspace's own
    /// render, and a child's `cx.notify()` does not redraw its parent.
    fn track<T: 'static>(&mut self, entity: &Entity<T>, cx: &mut Context<Self>) {
        cx.observe(entity, |workspace: &mut Self, _, cx| {
            // The view changed under the user, so any half-given close or quit
            // instruction is stale. In particular, saving a tab must disarm the
            // quit warning that a save was offered for.
            workspace.pending_close = None;
            workspace.pending_quit = false;
            cx.notify();
        })
        .detach();
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
    pub(crate) fn active_tab(&self) -> Option<&TabView> {
        self.tabs.get(self.active)
    }

    /// The last open failure, formatted for display.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().map(|error| &**error)
    }

    /// The focus handle of the visible document, if there is one.
    pub fn active_focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        self.tabs.get(self.active).map(|tab| tab.focus_handle(cx))
    }

    /// Give keyboard focus to the visible document, or to the workspace when
    /// nothing is open.
    pub fn focus_active(&self, window: &mut Window, cx: &App) {
        match self.active_tab() {
            Some(tab) => tab.focus(window, cx),
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
            Ok(Document::Csv(document)) => {
                let grid = cx.new(|cx| CsvGrid::new(path, document, cx));
                self.track(&grid, cx);
                self.tabs.push(TabView::Csv(grid));
            }
            Ok(Document::Json(document)) => {
                let view = cx.new(|cx| JsonView::new(path, document, cx));
                self.track(&view, cx);
                self.tabs.push(TabView::Json(view));
            }
            Ok(Document::Text(document)) => {
                let view = cx.new(|cx| TextView::new(path, document, cx));
                self.track(&view, cx);
                self.tabs.push(TabView::Text(view));
            }
            Ok(Document::Markdown(document)) => {
                let view = cx.new(|cx| MarkdownView::new(path, document, cx));
                self.track(&view, cx);
                self.tabs.push(TabView::Markdown(view));
            }
            Err(error) => {
                self.error = Some(SharedString::from(error.to_string()));
                cx.notify();
                return false;
            }
        }

        self.active = self.tabs.len() - 1;
        self.error = None;
        cx.notify();
        true
    }

    /// Show the tab at `index` and move keyboard focus into it.
    pub fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        // Moving away from a tab withdraws a close that was only warned about.
        self.pending_close = None;

        if index != self.active {
            self.active = index;
            cx.notify();
        }
        self.focus_active(window, cx);
    }

    /// Close the tab at `index` and show a neighbour.
    ///
    /// A tab with unsaved edits asks first: the request is recorded and answered
    /// with a notice, and asking again for the same tab drops the edits. Nothing
    /// here can undo a discard, so one keystroke must not be enough.
    pub fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }

        if self.tabs[index].is_dirty(cx) && self.pending_close != Some(index) {
            self.pending_close = Some(index);
            self.error = Some(SharedString::from(
                "this tab has unsaved changes: Cmd+S keeps them, closing it again discards them",
            ));
            cx.notify();
            return;
        }

        // The notice was ours, so it goes with the tab that raised it.
        if self.pending_close.take().is_some() {
            self.error = None;
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

    /// Quit, refusing once when a tab has edits that are not on disk.
    ///
    /// The rule is the one closing a dirty tab already uses: the first request
    /// explains itself in the error strip, and a second one goes through. There
    /// is no undo for a discarded edit, so one keystroke must not be able to
    /// end the process and take the work with it.
    ///
    /// Returns whether quitting was allowed, which is how the tests can tell
    /// the two requests apart on a platform where `App::quit` does nothing.
    pub fn request_quit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs.iter().any(|tab| tab.is_dirty(cx)) && !self.pending_quit {
            self.pending_quit = true;
            self.error = Some(SharedString::from(
                "unsaved changes: Cmd+S keeps them, Cmd+Q again quits without saving",
            ));
            cx.notify();
            return false;
        }

        cx.quit();
        true
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

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        self.request_quit(cx);
    }

    /// Save the visible document.
    ///
    /// This is the one place a tab's unsaved edits become bytes on disk, so the
    /// menu item, Cmd+S, and any future affordance all share it.
    fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            tab.save(cx);
        }
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
            .position(|tab| canonical(&tab.path(cx)) == wanted)
    }

    fn tab_bar(&self, cx: &Context<Self>) -> Div {
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (index, tab) in self.tabs.iter().enumerate() {
            tabs.push(self.tab(index, tab, cx));
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

    fn tab(&self, index: usize, tab: &TabView, cx: &Context<Self>) -> Div {
        let visible = index == self.active;
        // A dot marks edits that are not on disk yet. Only a CSV grid can be
        // dirty today, so a JSON title never changes.
        let title = if tab.is_dirty(cx) {
            format!("{} \u{2022}", title_of(&tab.path(cx)))
        } else {
            title_of(&tab.path(cx))
        };

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
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(format!("Bromine {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .id("empty-open")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(theme::accent())
                    .child("Open a file")
                    .on_click(
                        cx.listener(|this, _event, window, cx| this.request_open(window, cx)),
                    ),
            )
    }

    fn body(&self, cx: &Context<Self>) -> Div {
        let content: Option<AnyElement> = match self.active_tab() {
            Some(TabView::Csv(grid)) => Some(grid.clone().into_any_element()),
            Some(TabView::Json(view)) => Some(view.clone().into_any_element()),
            Some(TabView::Text(view)) => Some(view.clone().into_any_element()),
            Some(TabView::Markdown(view)) => Some(view.clone().into_any_element()),
            None => None,
        };

        match content {
            Some(content) => div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(content),
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
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::save))
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
    use bromine_core::parse_json;
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
        cx.update(|_window, app| match workspace.read(app).tabs.get(index) {
            Some(TabView::Csv(grid)) => Some(grid.read(app).query().to_string()),
            _ => None,
        })
    }

    /// How many lines the JSON tab at `index` draws, or `None` when that tab is
    /// not a JSON view.
    fn json_line_count(
        cx: &mut VisualTestContext,
        workspace: &Entity<Workspace>,
        index: usize,
    ) -> Option<usize> {
        cx.update(|_window, app| match workspace.read(app).tabs.get(index) {
            Some(TabView::Json(view)) => Some(view.read(app).line_count()),
            _ => None,
        })
    }

    fn bind_shortcuts(cx: &mut VisualTestContext, workspace: &Entity<Workspace>) {
        cx.update(|window, app| {
            app.bind_keys([
                KeyBinding::new("cmd-o", OpenFile, None),
                KeyBinding::new("cmd-w", CloseTab, None),
                KeyBinding::new("cmd-s", Save, None),
            ]);
            workspace.read(app).focus_active(window, app);
        });
    }

    #[gpui::test]
    fn opening_a_markdown_file_creates_a_markdown_tab(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        let opened = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.open(fixture("notes.md"), cx))
        });

        assert!(opened);
        assert_eq!(tab_count(cx, &workspace), 1);
        assert!(
            cx.update(|_window, app| workspace.read(app).tabs.first().is_some_and(
                |tab| matches!(tab, TabView::Markdown(view) if view.read(app).path().ends_with("notes.md"))
            )),
            "the Markdown file should open in the Markdown view"
        );
    }

    #[gpui::test]
    fn cmd_s_saves_the_visible_markdown_tab(cx: &mut TestAppContext) {
        let directory =
            std::env::temp_dir().join(format!("bromine-workspace-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join("notes.md");
        std::fs::write(&path, "# Title\n").expect("the file is written");

        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));
        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.open(path.clone(), cx))
        });
        bind_shortcuts(cx, &workspace);
        cx.update(|window, app| window.draw(app).clear());

        cx.simulate_input("X");
        cx.simulate_keystrokes("cmd-s");

        assert_eq!(
            std::fs::read_to_string(&path).expect("the file reads back"),
            "X# Title\n"
        );
        assert!(
            cx.update(|_window, app| workspace
                .read(app)
                .active_tab()
                .is_some_and(|tab| !tab.is_dirty(app))),
            "saving clears the dirty flag"
        );

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
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
    fn closing_a_tab_with_unsaved_edits_asks_before_it_discards_them(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx)
            });
        });

        // Give the grid an edit the way a user does: click its add-row button.
        let button = cx
            .debug_bounds("edit-insert-row")
            .expect("the grid's add-row button was laid out");
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::none());

        assert!(
            cx.update(|_window, app| workspace
                .read(app)
                .active_tab()
                .is_some_and(|tab| tab.is_dirty(app))),
            "the click should have left the tab dirty"
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| workspace.close(0, window, cx))
        });
        assert_eq!(
            tab_count(cx, &workspace),
            1,
            "one close must not discard edits"
        );
        assert!(
            cx.update(|_window, app| workspace.read(app).error().is_some()),
            "the refusal has to say what happened"
        );

        cx.update(|window, app| {
            workspace.update(app, |workspace, cx| workspace.close(0, window, cx))
        });
        assert_eq!(tab_count(cx, &workspace), 0, "asking twice discards them");
        assert!(
            cx.update(|_window, app| workspace.read(app).error().is_none()),
            "the notice belonged to the tab that has now gone"
        );
    }

    #[gpui::test]
    fn quitting_with_clean_tabs_does_not_ask(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        let allowed = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx);
                workspace.request_quit(cx)
            })
        });

        assert!(allowed, "a workspace with no unsaved edits quits at once");
    }

    #[gpui::test]
    fn quitting_with_unsaved_edits_asks_before_it_discards_them(cx: &mut TestAppContext) {
        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));

        cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| {
                workspace.open(fixture("people.csv"), cx)
            });
        });

        // Make the visible tab dirty the way a user does.
        let button = cx
            .debug_bounds("edit-insert-row")
            .expect("the grid's add-row button was laid out");
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::none());

        let first = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.request_quit(cx))
        });
        assert!(!first, "one Quit must not discard edits");
        let error = cx
            .update(|_window, app| workspace.read(app).error().map(str::to_string))
            .expect("the refusal has to say what happened");
        assert!(
            error.contains("unsaved changes"),
            "unexpected message: {error}"
        );

        // A change in a tab withdraws the warning, so an edit made after it
        // makes the next Quit ask again rather than quitting silently.
        let button = cx
            .debug_bounds("edit-insert-row")
            .expect("the button is still laid out");
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::none());

        let after_edit = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.request_quit(cx))
        });
        assert!(
            !after_edit,
            "an edit after the warning must re-arm the warning"
        );

        let second = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.request_quit(cx))
        });
        assert!(second, "asking twice quits without saving");
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
            workspace.update(app, |workspace, cx| workspace.open("notes.yaml", cx))
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
                workspace.open("notes.yaml", cx);
                workspace.open(fixture("people.csv"), cx);
            })
        });

        assert_eq!(tab_count(cx, &workspace), 1);
        assert_eq!(
            cx.update(|_window, app| workspace.read(app).error().map(str::to_string)),
            None
        );
    }

    /// Write `source` to a temporary `.json` file and return the path.
    ///
    /// `fixtures/` is the contract between the engines, the views, and QA, so a
    /// JSON document a test invents lives in the temporary directory instead.
    fn temporary_json(name: &str, source: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bromine-workspace-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = directory.join(format!("{name}.json"));
        std::fs::write(&path, source).expect("the JSON file is written");
        path
    }

    #[gpui::test]
    fn opening_a_json_file_creates_one_json_tab_showing_the_pretty_lines(cx: &mut TestAppContext) {
        let source = r#"{"name": "ada", "roles": ["admin", "author"]}"#;
        let path = temporary_json("json-tab", source);
        // The same parser the workspace will use, so the expectation cannot
        // drift from what the view draws.
        let expected = parse_json(&path, source.to_string())
            .expect("the inline JSON parses")
            .line_count();

        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));
        let opened = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.open(path.clone(), cx))
        });

        std::fs::remove_dir_all(path.parent().expect("the path has a directory"))
            .expect("the temporary directory is removed");

        assert!(opened);
        assert_eq!(tab_count(cx, &workspace), 1);
        assert_eq!(
            json_line_count(cx, &workspace, 0),
            Some(expected),
            "the tab should be a JSON view drawing the formatted document"
        );
        // The CSV search box belongs to the grid, so a JSON tab has no query.
        assert_eq!(query_of(cx, &workspace, 0), None);
    }

    #[gpui::test]
    fn a_malformed_json_file_reports_where_it_broke_and_adds_no_tab(cx: &mut TestAppContext) {
        let source = "{\"name\": }";
        let path = temporary_json("malformed-json", source);
        let expected = parse_json(&path, source.to_string())
            .expect_err("the source is malformed")
            .to_string();

        let (workspace, cx) = cx.add_window_view(|_window, cx| Workspace::new(cx));
        let opened = cx.update(|_window, app| {
            workspace.update(app, |workspace, cx| workspace.open(path.clone(), cx))
        });
        let error = cx
            .update(|_window, app| workspace.read(app).error().map(str::to_string))
            .expect("a parse failure must be reported");

        std::fs::remove_dir_all(path.parent().expect("the path has a directory"))
            .expect("the temporary directory is removed");

        assert!(!opened);
        assert_eq!(tab_count(cx, &workspace), 0);
        assert_eq!(error, expected);
        // `CoreError::Json` prints `path:line:column`, which is the position a
        // reader needs to jump to the offending byte.
        assert!(
            error.contains(&format!("{}:1:", path.display())),
            "the reason should carry the broken position: {error}"
        );
    }
}
