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
- Generate a large fixture for grid work: `./scripts/make-large-fixture.sh 200000`

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
  searching, and writing. **No GPUI dependency.**
- `crates/bromine-ui/` — GPUI views. Depends on `bromine-core` and `gpui`.
- `crates/bromine/` — the binary. Argument parsing, file loading, window setup.
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
  the `.truncate()` will actually apply.
- **`uniform_list` measures the first row.** Every row must be exactly the same
  height (`ROW_HEIGHT`), and the list handles its own wheel scrolling, so
  `.track_scroll()` is only needed for programmatic scrolling.
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
- **`prompt_for_paths` is `unimplemented!()` in the test platform.** Any test that
  reaches the open dialog panics, so keep the dialog in its own handler and test
  the code that handler calls.
- **`PathPromptOptions` has no `Default` and no builder methods**, and cannot
  filter by file type. The panel lets the user pick anything, so the loader has
  to reject what it cannot show.
- **`WeakEntity::update_in`** is how to get `(&mut View, &mut Window)` back after
  an async step such as the open dialog resolving.
- **`SharedString::as_deref()` yields `&ArcCow<str>`, not `&str`.** Convert with
  `.map(|value| &**value)`.

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

There is no rendered-text reader. Assert on view state (`cell_at`,
`visible_row_count`) or on `debug_bounds`, never on what "looks right".

## Multi-document structure

`Workspace` is the window's root view. It owns a `Vec<Entity<CsvGrid>>`, one per
open document, plus the index of the visible one. Each tab is a live entity, so
its sort and search survive switching away and back.

Rules that keep this working:

- **Exactly one element holds the keyboard at a time: the active grid.**
  `Workspace::focus_active` is called after anything that changes the visible
  tab. It falls back to the workspace's own handle when no tab is open, because
  without a focused element the window-level shortcuts never fire.
- **Opening a path that is already open focuses its tab** rather than adding a
  second one. Paths are canonicalized first, so a relative command-line path and
  an absolute dialog path are recognised as the same file.
- **A failed open is reported in the error strip and changes nothing else.** It
  never ends the process, so one bad command-line argument cannot hide the rest.
- **Shortcuts are actions, not key matching.** `OpenFile` and `CloseTab` are
  declared in `bromine-ui/src/workspace.rs`, bound with a `None` context in the
  binary, and handled by `.on_action` on the workspace root. Do not move them
  into `CsvGrid::on_key_down`: once a binding exists, that listener never sees
  the keystroke.
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
- Our own crates are built with `opt-level = 1` in dev and all dependencies at
  `opt-level = 3`, so a GUI frame is usable without a release build. That setting
  is also why cold builds are slow; do not lower it to speed up the first build
  without measuring the frame rate first.
- Match the surrounding style. Prefer a doc comment explaining *why* over one
  restating *what*.

## Verification

Before calling work finished:

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. For any UI change, actually launch it: `cargo run -p bromine -- fixtures/people.csv`

A passing test suite does not show that a window renders. If you cannot launch
and look at the app, say so plainly instead of implying you did.

## Notes on hooks

`.factory/hooks.json` registers a `PostToolUse` hook that runs `cargo fmt` after
any file write. It deliberately does **not** run clippy: clippy over the GPUI
dependency tree takes minutes, and a hook that slow would gate every edit. Run
clippy yourself via the commands above, or use the `check` skill.
