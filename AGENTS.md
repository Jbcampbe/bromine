# Repository guide: Bromine

Bromine is a desktop viewer and editor for the data formats developers meet in
the wild: CSV, JSON, and Markdown. It is built in Rust on GPUI (Zed's UI
framework).

## Commands

- Build the workspace: `cargo build`
- Run the app on a fixture: `cargo run -p bromine -- fixtures/people.csv`
- Test the format engines: `cargo test -p bromine-core`
- Test the views headlessly: `cargo test -p bromine-ui`
- Test everything: `cargo test --workspace`
- Format: `cargo fmt --all`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Generate large fixtures for grid and JSON work: `./scripts/make-large-fixture.sh 200000`
  (writes `large.csv` and a compact one-record-per-line `large.json`)
- Regenerate the app icon: `./scripts/make-icon.sh` (needs `rsvg-convert`)
- Build a local `.app` and dmg: `./scripts/bundle-macos.sh <target-triple> <version> <arch>`

### Build times are long, and the wait is silent

A cold build of the dependency tree takes **several minutes** here: measured at
over 5 minutes and 3.0 GB of artifacts for a test build, with roughly 450 crates
compiled at `opt-level = 3` (see Conventions). `gpui` itself is the slowest
single crate. This is normal. It is not a hang.

Two things make it *look* like a hang, and both have already caused a build to be
cancelled by mistake:

1. Cargo buffers compiler output when it is piped, so a `| tail` can show nothing
   at all for minutes.
2. Cargo holds an exclusive lock on `target/`. Any second cargo command prints
   `Blocking waiting for file lock on build directory` and nothing else until the
   first one finishes.

So: use `cargo check` for fast feedback on types, run at most one real build at a
time, and if you must watch a long build, send its output to a file and read the
file rather than running a second cargo command (which queues behind the lock).

Incremental builds after an edit to our own code take seconds; only the first
build, or a change to the dependency set or profile, pays the full cost.

## Layout

- `crates/bromine-core/` — format engines. Parsing, the table model, sorting,
  searching, writing, and the Markdown/plain-text document models with their
  undo history. **No GPUI dependency.**
- `crates/bromine-ui/` — GPUI views. Depends on `bromine-core` and `gpui`.
- `crates/bromine/` — the binary. Argument parsing, menu bar, file loading, window setup.
- `assets/` — `icon.svg` (the icon source) and the committed `AppIcon.icns`.
- `packaging/` — the `Info.plist` template the bundle script stamps.
- `.github/workflows/` — the tag-driven release workflow.
- `fixtures/` — sample data shared by core tests, UI development, and QA.
- `scripts/` — developer helpers.

## Hard rules

1. **`bromine-core` must not depend on GPUI.** Every behaviour worth trusting
   belongs in core, where `cargo test` can reach it headlessly. GPUI is pre-1.0
   and its API moves between releases; keeping it behind one crate boundary is
   what keeps a future migration small.
2. **`gpui` is pinned to `=0.2.2` and is not upgraded as a side effect.** Before
   using an API you are unsure of, read the pinned source rather than guessing:
   `~/.cargo/registry/src/index.crates.io-*/gpui-0.2.2/src/`. The crate ships 32
   runnable examples in its `examples/` directory, which is the most reliable
   API reference available for this version.
3. **Never edit `fixtures/` to make a test pass.** Fixtures are the contract
   between core tests, the views, and QA runs. A `PreToolUse` hook blocks this;
   adding a *new* fixture file is allowed.
4. **Do not change the `runtime_shaders` feature flag casually.** See
   "Environment" below.

## Environment

This machine has Xcode **Command Line Tools only** — no full Xcode, so the
`metal` and `metallib` compilers are not present. GPUI normally precompiles its
Metal shaders at build time and would fail here.

The workspace therefore enables GPUI's `runtime_shaders` feature, which emits
stitched shader source compiled on first launch instead. Consequences:

- The app can be built and run on this machine with no further setup.
- First launch pays a small shader-compilation cost.
- If you ever install full Xcode and want precompiled shaders for release
  builds, remove `runtime_shaders` from the workspace `Cargo.toml` and run
  `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`.
  Verify with `xcrun -f metal` before believing it worked.

## Packaging and releases

- `scripts/make-icon.sh` renders `assets/icon.svg` into `assets/icon-1024.png`,
  `assets/AppIcon.iconset/`, and `assets/AppIcon.icns`. Only the SVG and the
  `.icns` are in git. Rendering needs `rsvg-convert`; committing the `.icns`
  means CI and a clean checkout do not.
- `scripts/bundle-macos.sh <triple> <version> <arch>` wraps
  `target/<triple>/release/bromine` in `dist/Bromine.app`, stamps
  `packaging/Info.plist`, signs ad-hoc by default, and writes
  `dist/Bromine-<version>-<arch>.dmg` containing the app and an `/Applications`
  symlink. It never invokes cargo, and it refuses when the binary or the icon is
  missing.
- `SIGN_IDENTITY` overrides the ad-hoc signature. Any identity other than `-`
  also turns on the hardened runtime and a secure timestamp, which notarization
  requires.
- `.github/workflows/release.yml` runs on a `v*` tag. `macos-15-intel` builds
  x86_64 and `macos-15` builds aarch64, both natively. `fail-fast: false` keeps
  one architecture's result visible when the other fails. The `publish` job runs
  on `ubuntu-latest`, collects both dmgs, writes one `SHA256SUMS.txt`, and uses
  the preinstalled `gh` CLI to create or update the release. It has no checkout,
  so every `gh` command passes `--repo "$GITHUB_REPOSITORY"`; without that gh
  fails with `failed to run git: not a git repository`.
- Secrets, all optional:
  - `MACOS_CERT_P12` — base64 of a `.p12` holding a Developer ID Application
    certificate. Without it the build is ad-hoc signed.
  - `MACOS_CERT_PASSWORD` — the password for that `.p12`.
  - `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` — the notarytool
    credentials. All three, plus the certificate, run
    `notarytool submit --wait` and then `stapler staple`.
  With none set the workflow produces ad-hoc builds and does not fail.
- The version is the tag without its leading `v`. `bundle-macos.sh` rejects a
  version that is not numeric and dotted, so a tag such as `v0.1.0-rc1` has to be
  renamed or the script taught about pre-release versions first.

## GPUI 0.2.2 notes

Each of these cost a compile cycle to discover. They are true of `=0.2.2` and
may change in the next release, so check the pinned source before relying on
them after an upgrade.

- **`.id(...)` changes the type.** `div()` is a `Div`; `div().id(...)` is a
  `Stateful<Div>`. A render helper that adds an id must return `Stateful<Div>`,
  not `Div`. Every `StatefulInteractiveElement` method (`on_click`,
  `overflow_scroll`, `overflow_y_scroll`, `track_scroll`) requires the id first.
- **`SharedString` cannot borrow.** Its only conversion is
  `impl<T: Into<ArcCow<'static, str>>> From<T>`, so it accepts an owned `String`
  or a `&'static str` and nothing else. A `&str` borrowed from a table must be
  owned first. `String` implements `IntoElement` directly, so cell text does not
  need a `SharedString` at all.
- **Flex items do not shrink on their own.** With `min-width: auto` a cell will
  overflow instead of truncating. Put `.min_w(px(0.0))` next to `.flex_1()` and
  the `.truncate()` will actually apply. The reverse bites too: a child with an
  explicit `.w(px(..))` still has `flex-shrink: 1`, so it gets squeezed below its
  measured width unless it also carries `.flex_shrink_0()`.
- **`shape_line` lives on `WindowTextSystem`, not `TextSystem`.** Text shaping
  needs a window, so it is unavailable from `Context::text_system()`, which
  returns a plain `Arc<TextSystem>`. That type does expose `resolve_font`,
  `advance`, and `ch_width`, which is enough to measure text without a window:
  resolve the font once, then sum per-character advances. The grid sizes its
  columns that way at load.
- **`uniform_list` measures the first row.** Every row must be exactly the same
  height (`ROW_HEIGHT`), and the list handles its own wheel scrolling, so
  `.track_scroll()` is only needed for programmatic scrolling. It also owns that
  scrolling outright: it cannot be told to leave an axis alone, which is why the
  grid renders its own visible row slice instead.
- **There is no scrollbar widget.** `Style::scrollbar_width` only reserves layout
  space; nothing is painted. A visible scrollbar has to be drawn, from a
  `ScrollHandle`'s `offset()`, `bounds()`, and `max_offset()`.
- **`.overflow_hidden().track_scroll(&handle)` is the manual scroll container.**
  A tracked handle makes the element clip and translate its children and fills in
  `bounds()` and `max_offset()`, but the element's own wheel listener zeroes every
  delta because its overflow is not `Scroll`. That is the combination to use when
  the view has to own the scroll position itself.
- **`on_scroll_wheel` is bubble-only, and bubble runs deepest first.** There is no
  capture-phase scroll hook, so an ancestor cannot preempt a scroller inside it.
  With two nested scrollers, each takes the axis it owns from the same diagonal
  event. `allow_concurrent_scroll` and `restrict_scroll_to_axis` only arbitrate
  within one scroller, so they cannot stop two of them from both moving.
  `Hitbox::should_handle_scroll` does accept an ancestor, which is what lets one
  handler cover the whole grid.
- **A `flex_1` child of a container with no height during layout takes its
  content's height.** Marking it `.min_h(px(0.0))` does not help. When the
  geometry has to be exact regardless, position the child with
  `.absolute().top(..).bottom(..).left(..).right(..)` instead.
- **There is no text field widget.** A real editable field needs
  `EntityInputHandler` plus `window.handle_input(...)` during paint. Until that
  exists, the search box is driven by raw `KeyDownEvent.keystroke.key_char`.
- **Keyboard events follow focus.** Nothing in `on_key_down` fires unless the
  element carrying `.track_focus(&handle)` has been focused. The *child* has to
  call `track_focus`; a parent cannot do it on the child's behalf. Focusing a
  handle whose element has not been painted yet falls back to the root node for
  one frame.
- **Actions run before key listeners.** `dispatch_key_event` matches bindings
  first and dispatches the action; `on_key_down` only runs when no binding
  produced a listener. A `KeyBinding` built with a `None` context is global.
  Dispatch stops at the deepest listener unless it calls `cx.propagate()`.
- **`key_context` lives on `InteractiveElement`.** `.key_context("CsvGrid")` next
  to `.track_focus(..)` is what makes a context-scoped `KeyBinding` match. A
  binding that finds a listener suppresses `on_key_down` for that keystroke, so a
  shortcut the grid wants for itself (Cmd+C, Cmd+S) must not be bound anywhere.
- **Clipboard reads and writes are on `App`.** `cx.write_to_clipboard(
  ClipboardItem::new_string(text))` and `cx.read_from_clipboard()`, which returns
  `Option<ClipboardItem>` (call `.text()` for the string). There is no
  `ClipboardItem::new`, and `new_string` takes an owned `String`. In tests the
  same two methods are on `TestAppContext`, which `VisualTestContext`
  dereferences to, so a copy can be asserted on directly.
- **`ClickEvent` is an enum, not a struct.** It is either
  `Mouse(MouseClickEvent { down, up })` or `Keyboard(..)`, so reach through
  `event.position()` (or `event.down.position`) rather than `event.position`.
  `on_mouse_down` and `on_mouse_up` hand over a `MouseDownEvent`/`MouseUpEvent`
  with `position`, `modifiers`, and `click_count` directly, which is simpler when
  only the position is wanted.
- **Only `on_click` and `on_drag` need an id.** They are registered on
  `StatefulInteractiveElement` and do nothing without element state.
  `on_mouse_down`, `on_mouse_up`, and `on_mouse_move` are on
  `InteractiveElement` and work on a plain `Div`, so a row that has to react to a
  click can carry one listener and derive the column from the pointer's x instead
  of giving every cell a hitbox.
- **`on_mouse_move` only fires while the pointer is over the element.**
  `MouseMoveEvent::dragging()` reports whether the left button is down, but moves
  stop arriving the moment the pointer leaves, so anything that tracks a drag
  past an edge needs `on_drag` plus `on_drag_move` (capture phase, no hover
  check, and a 2 px threshold before the drag starts).
- **`simulate_click` hardcodes a click count of one.** A double click has to be
  dispatched as a `MouseDownEvent`/`MouseUpEvent` pair with `click_count: 2`
  through `simulate_event`.
- **`ScrollDelta::Pixels` reports `precise() == true`.** A test wheel event built
  from pixels therefore looks like a trackpad gesture, and the grid latches the
  axis on the first one and keeps it, because a zero-delta `Ended` event returns
  before the latch is cleared. Give each test gesture a `TouchPhase::Started` to
  start a new one.
- **`prompt_for_paths` is `unimplemented!()` in the test platform.** Any test that
  reaches the open dialog panics, so keep the dialog in its own handler and test
  the code that handler calls.
- **`App::set_menus` is a no-op in the test platform.** A menu item and the key
  equivalent printed beside it can only be checked by launching the bundled app;
  the action behind the item is tested through `Workspace::request_quit`.
- **`PathPromptOptions` has no `Default` and no builder methods**, and cannot
  filter by file type. The panel lets the user pick anything, so the loader has
  to reject what it cannot show.
- **`WeakEntity::update_in`** is how to get `(&mut View, &mut Window)` back after
  an async step such as the open dialog resolving.
- **`SharedString::as_deref()` yields `&ArcCow<str>`, not `&str`.** Convert with
  `.map(|value| &**value)`.
- **`on_open_urls` is on `Application`, not `App`.** A double-clicked file
  arrives as an `application:openURLs:` event, never as a command-line
  argument, so Finder launches would otherwise open an empty window. Register
  the handler before `run`; it receives no context, so hold an `AsyncApp` from
  the launch callback and use that to open the paths once a window exists.
- **GPUI installs no menu bar unless the app calls `App::set_menus`.** Without
  one there is no Quit item and Cmd+Q does nothing. The key equivalents shown
  in the menu come from the keymap, so `cx.bind_keys` has to run before
  `cx.set_menus`. There is deliberately no Edit menu: Copy and Cut are bound
  only in the `TextEditor` key context, so they do not suppress the grid's own
  copy handler, and a menu item needs a global action.

- **`TextRun` carries no font size.** `StyledText::new(text).with_runs(runs)`
  takes `TextRun { len, font, color, background_color, underline, strikethrough }`,
  and the size comes from the shaped line's `font_size`. Inline code is
  therefore distinguished by its face and background, not its size.
- **`TextSystem::shape_text` needs a window.** `Window::text_system()` gives the
  `WindowTextSystem`; `shape_text(text, font_size, &runs, wrap_width, line_clamp)`
  returns `SmallVec<[WrappedLine; 1]>`. `WrappedLine` derefs to
  `WrappedLineLayout`, whose `index_for_position`, `closest_index_for_position`,
  and `position_for_index` map between a point and a byte offset. That is the
  only reliable way to turn a click into a caret in proportional, wrapped
  prose, and it is why each Markdown row is a custom element that records its
  layout during paint.
- **`request_measured_layout(style, measure)` takes no element list.** The
  closure receives `(known_size, available_space, window, cx)` and returns a
  `Size<Pixels>`. Call it from `Element::request_layout`; the shaped line is
  cached in a `RefCell` for `prepaint` to take.
- **There is still no text field widget.** A view becomes a real editor by
  implementing `EntityInputHandler` and calling
  `window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx)`
  during paint. `TextInputBridge` in `editor.rs` does exactly that.
- **`dispatch_keystroke` delivers `key_char` only when the key event
  propagates.** It sends `KeyDown` first; if a binding or `on_key_down` handles
  it, the platform input handler never sees the character. Insert characters in
  the input handler and nowhere else, or every keystroke arrives twice.
- **Actions are namespaced by their declaration site, not by the string in
  `actions!`.** `actions!(text_editor, [Backspace])` creates a `Backspace` type
  in the current module; import it as `crate::text_actions::Backspace`.
- **`ImageSource` accepts a path or a URI.** `img(path_buf)` loads a local file
  and `img("https://…")` loads a remote one through the app's image loader. A
  `String` that is not a URI becomes `Resource::Embedded`, so a filesystem path
  must stay a `PathBuf` or it will be looked for inside the app bundle.
  `Img` implements `Styled` (so `max_w`/`max_h` work) and `StyledImage`
  (`with_loading`, `with_fallback`, `object_fit`). Loading and fallback closures
  return `AnyElement`.
- **`uniform_list` needs a `UniformListScrollHandle`.** Its `track_scroll` will
  not take a plain `ScrollHandle`, so the read-only Markdown fallback lets the
  list own its scrolling rather than trying to drive it.

## Testing views

`#[gpui::test]` needs no feature flag: `gpui::test`, `TestAppContext`, and
`VisualTestContext` are all ungated in 0.2.2. What *is* gated is the test
instrumentation — `debug_selector` compiles to a no-op and `debug_bounds` returns
`None` unless gpui is built with its `test-support` feature. That is why
`crates/bromine-ui/Cargo.toml` lists gpui twice: normally, and again under
`[dev-dependencies]` with `test-support`.

Two more details worth knowing:

- `VisualTestContext::simulate_mouse_down` and `simulate_mouse_up` take a
  `Modifiers` third argument, not just a position and a button.
- `VisualTestContext::debug_bounds` requires a `&'static str`, so selectors are
  formatted at construction (`format!("header-{column}")`) and looked up with a
  literal (`"header-1"`).
- **`debug_bounds` accumulates across frames.** `Frame::clear` does not reset the
  map, so a key once seen stays even after the element stops being drawn. Never
  assert that a selector is absent after the first frame; assert that something
  new appeared instead.
- **`VisualTestContext::simulate_event` dispatches any `InputEvent`, including
  `ScrollWheelEvent`.** Hover the element with `simulate_mouse_move` first,
  because hit testing uses the window's mouse position rather than the event's.

There is no rendered-text reader. Assert on view state (`cell_at`,
`visible_row_count`) or on `debug_bounds`, never on what "looks right".

## Multi-document structure

`Workspace` is the window's root view. It owns a `Vec<TabView>`, one per open
document, plus the index of the visible one. `TabView` is an enum over
`Entity<CsvGrid>`, `Entity<JsonView>`, `Entity<TextView>`, and
`Entity<MarkdownView>`, so a new format adds a variant and one arm per accessor
(`path`, `focus_handle`, `focus`, `is_dirty`, `save`) instead of a second
parallel list of tabs. `Workspace::open` is the only place that has to
know how a core `Document` becomes a view. Each tab is a live entity, so its
sort, search, or scroll position survives switching away and back.

Rules that keep this working:

- **Exactly one element holds the keyboard at a time: the active tab's view.**
  `Workspace::focus_active` is called after anything that changes the visible
  tab. It falls back to the workspace's own handle when no tab is open, because
  without a focused element the window-level shortcuts never fire.
- **The workspace observes every tab it creates.** A child's `cx.notify()` does
  not redraw its parent, so a tab title that shows a dirty dot would otherwise
  stay stale until the next unrelated render. `Workspace::track` observes each
  view and notifies the workspace, which is also a convenient place to withdraw a
  half-given close request.
- **Closing a dirty tab asks twice.** The first request records
  `pending_close` and explains itself in the error strip; a second request for
  the same tab drops the edits. There is no undo, so a single keystroke must not
  be able to throw work away, and a modal dialog is a bigger feature than the
  guard is worth.
- **Opening a path that is already open focuses its tab** rather than adding a
  second one. Paths are canonicalized first, so a relative command-line path and
  an absolute dialog path are recognised as the same file.
- **A failed open is reported in the error strip and changes nothing else.** It
  never ends the process, so one bad command-line argument cannot hide the rest.
- **Shortcuts are actions, not key matching.** `OpenFile`, `CloseTab`, `Quit`,
  and `Save` are declared in `bromine-ui/src/workspace.rs`, bound with a `None`
  context in the binary, and handled by `.on_action` on the workspace root.
  `Save` asks the active tab to write itself, which is the single place an edit
  becomes bytes on disk. Do not move these into a view's `on_key_down`: once a
  binding exists, that listener never sees the keystroke.
- **The open panel is shared and cannot be reconfigured while it is up.** A
  `prompting` flag stops a second Cmd+O from mutating the panel already on
  screen.

## Conventions

- Errors: `CoreError` in `bromine-core` is the one error type. Add a variant
  rather than returning a bare string.
- Views never mutate a `Table` directly during render. State lives on the view
  entity and is changed in event handlers, which then call `cx.notify()`.
- Sorting and searching never move data. `RowView` holds row *indices*, so an
  edit always targets a stable table index. Keep it that way.
- JSON is shown as lines, not a tree, and formatting never reorders keys:
  `serde_json` is built with `preserve_order` on purpose. Minifying refuses a
  document over 400 KB because one shaped line of millions of characters stalls
  the window; the formatted view still shows every byte.
- Our own crates are built with `opt-level = 1` in dev and all dependencies at
  `opt-level = 3`, so a GUI frame is usable without a release build. That setting
  is also why cold builds are slow; do not lower it to speed up the first build
  without measuring the frame rate first.
- All text editing goes through `crates/bromine-ui/src/editor.rs` and its
  `TextBuffer` trait, which both `TextDocument` and the Markdown model
  implement. Views supply layout, click mapping, and the few operations that
  need their own structure (table cells, list continuation); typing, deletion,
  navigation, clipboard, and undo are shared.
- Match the surrounding style. Prefer a doc comment explaining *why* over one
  restating *what*.

## Verification

Before calling work finished:

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. For any UI change, actually launch it: `cargo run -p bromine -- fixtures/people.csv`
5. For a packaging change, build a bundle and check it:
   `./scripts/bundle-macos.sh <triple> <version> <arch>`, then `codesign -dv`
   on `dist/Bromine.app` (ad-hoc reports `flags=0x2(adhoc)`) and launch the app
   from the mounted dmg.

A passing test suite does not show that a window renders. If you cannot launch
and look at the app, say so plainly instead of implying you did.

## Notes on hooks

`.factory/hooks.json` registers a `PostToolUse` hook that runs `cargo fmt` after
any file write. It deliberately does **not** run clippy: clippy over the GPUI
dependency tree takes minutes, and a hook that slow would gate every edit. Run
clippy yourself via the commands above, or use the `check` skill.
