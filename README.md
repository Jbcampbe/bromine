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
- Sizes each column to its content: the widest value it holds, or its header if
  that is wider, clamped to 56–420 px. A table wider than the window scrolls
  sideways instead of squeezing columns.
- Scrolls in one direction at a time: a swipe or wheel gesture picks the axis it
  starts on, so a diagonal trackpad movement does not drag the grid diagonally.
  Thin scrollbars along the right and bottom edges show where in the file you
  are.
- Reports row and column counts along the bottom of the window, and says so when
  the parser had to repair ragged input or decode a file that was not valid
  UTF-8.

**Does not work yet**

- Cell editing and saving. The grid is read-only and labels itself that way.
- JSON and Markdown. Opening one of those files reports why in the window.
- Resizing a column by dragging its edge. Widths are measured from content, and
  a value longer than the widest column is truncated with an ellipsis.
- Dragging a scrollbar. The bars show the position and the proportions of the
  view; the wheel, trackpad, and keyboard are the only ways to move.
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
cargo test --workspace        # 74 tests: 38 core, 8 fixture contracts, 28 views
cargo clippy --workspace --all-targets -- -D warnings
```

`bromine-core` has no GPUI dependency, so parsing, sorting, searching, and
writing are tested headlessly. The twenty-eight view tests drive real frames with
GPUI's test harness: they type into the window, click column headers, click tabs,
send Cmd+W through the real keymap, dispatch wheel events, and assert on laid-out
bounds to check that columns get the width they were measured at, that a diagonal
gesture moves one axis only, that only the rows in view are drawn, and where the
scrollbars and status strip sit.

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
