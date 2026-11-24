# Bromine

A desktop viewer for the data formats developers keep opening in a terminal.

Built in Rust on [GPUI](https://gpui.rs), the UI framework behind Zed.

## Status

Early, and honest about it. One format is wired end to end; the other two are
recognised but not implemented.

**Works today**

- Opens a CSV or TSV file, detecting the delimiter.
- Keeps several files open at once as browser-style tabs. Cmd+O opens files from
  a native panel, Cmd+W closes the visible tab, and reopening a path you already
  have focuses the tab that holds it instead of duplicating it.
- Shows each file in a virtualized grid that stays responsive on large files.
- Sorts by any column on click. Numeric columns sort as numbers, so `9` comes
  before `10`, and clicking the same header again reverses the order.
- Filters as you type (backspace deletes, escape clears). Each tab keeps its own
  sort and search.
- Reports row and column counts, and says so when the parser had to repair
  ragged input or decode a file that was not valid UTF-8.

**Does not work yet**

- Cell editing and saving. The grid is read-only and labels itself that way.
- JSON and Markdown. Opening one of those files reports why in the window.
- Per-column widths and horizontal scrolling. Columns share the width equally,
  and long values are truncated with an ellipsis.
- A real search field with a caret. Search is driven by raw keystrokes, which
  means it has no cursor, no selection, and no undo.
- Reordering tabs, and closing the window when the last tab closes.

## Requirements

- macOS. GPUI is cross-platform, but this has only been built and run on macOS.
- Rust 1.90 or newer.
- Xcode Command Line Tools. Full Xcode is not required: the workspace enables
  GPUI's `runtime_shaders` feature so shaders compile at first launch instead of
  at build time. See `AGENTS.md` for the details and how to switch back.

## Build and run

```bash
cargo run -p bromine -- fixtures/people.csv fixtures/quirky.csv   # two tabs
cargo run -p bromine                                              # empty window
```

The first build takes several minutes (measured: about 6 minutes and 3 GB of
artifacts) because the GPUI dependency tree is compiled optimized. Later builds
of our own code take seconds. `cargo check` gives fast type feedback without
paying that cost.

## Tests

```bash
cargo test --workspace        # 59 tests: 38 core, 8 fixture contracts, 13 views
cargo clippy --workspace --all-targets -- -D warnings
```

`bromine-core` has no GPUI dependency, so parsing, sorting, searching, and
writing are tested headlessly. The thirteen view tests drive real frames with
GPUI's test harness: they type into the window, click column headers, click
tabs, and send Cmd+W through the real keymap.

## Layout

| Path | What lives there |
| :--- | :--------------- |
| `crates/bromine-core` | Format engines. Parsing, the table model, sort, search, write-back. No UI. |
| `crates/bromine-ui` | GPUI views. The CSV grid and the theme tokens. |
| `crates/bromine` | The binary: arguments, loading, window. |
| `fixtures/` | Sample data shared by tests, development, and QA runs. |
| `scripts/` | Developer helpers, such as the large-fixture generator. |

`AGENTS.md` documents the hard rules and the GPUI 0.2.2 API notes that cost a
compile cycle to learn. `DESIGN.md` records the visual brief.

## Verified, and not verified

The test suite and a launch check are run on every change: the app is started on
a fixture and confirmed to register as a foreground macOS application. What
cannot be checked on this machine is the pixels, because `screencapture` is
blocked for the terminal. If you change the layout, look at the window yourself.

One code path has no automated cover at all: the native open panel. GPUI's test
platform stubs `prompt_for_paths` with `unimplemented!()`, so Cmd+O is only ever
exercised by hand. Everything downstream of it (the paths arriving, the file
loading, the tab appearing) is tested through `Workspace::open` directly.
