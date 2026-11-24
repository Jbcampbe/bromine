# Bromine

A desktop viewer for the data formats developers keep opening in a terminal.

Built in Rust on [GPUI](https://gpui.rs), the UI framework behind Zed.

## Status

Early, and honest about it. Two formats are wired end to end; the third is
recognised but not implemented.

**Works today**

- Opens a CSV or TSV file, detecting the delimiter.
- Keeps several files open at once as browser-style tabs. Cmd+O opens files from
  a native panel, Cmd+W closes the visible tab, and reopening a path you already
  have focuses the tab that holds it instead of duplicating it.
- Shows each file in a virtualized grid that stays responsive on large files,
  with Excel's chrome: row numbers down the left edge that do not scroll
  sideways, and column letters along the top from `A` to `Z` to `AA`.
- Sorts by any column name on click. Numeric columns sort as numbers, so `9`
  comes before `10`, and clicking the same header again reverses the order.
- Filters as you type (backspace deletes, escape clears). Each tab keeps its own
  sort and search.
- Marks one cell, row, or column: click a cell, a row number, or a column letter,
  or move the mark with the arrow keys. The mark is highlighted, and the row
  number or letter beside it lights up so a wide row still says where you are.
- Copies the mark with Cmd+C: a cell as its value, a row as tab-separated values,
  a column as one value per line.
- Adds and removes rows and columns from the status strip, then writes the file
  back with Cmd+S. The strip says "unsaved changes" until it has. A file that
  had to be decoded lossily is never written back, because the replacement
  characters would overwrite bytes the viewer could not read.
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
- Opens a JSON file as a read-only, virtualized view with line numbers, and
  switches between the document formatted at two-space indentation and the same
  document on a single line. Formatting keeps the key order the file was written
  in; a minifier that sorted keys would be showing a different document.
- Refuses to put a very large document on one line (over 400 KB minified) and
  says so, because shaping a single line of millions of characters would freeze
  the window. The formatted view is always available.
- Reports a JSON parse error as `path:line:column: message`, so a malformed file
  says where to look instead of only that it failed.

**Does not work yet**

- Editing the text of a cell. Rows and columns can be added and removed, and an
  edit can be saved, but a new row is empty and stays empty. Typing goes to the
  search box, which is the one field the app has.
- Markdown. Opening a `.md` file reports why in the window.
- Editing JSON, or collapsing it into a tree. The JSON view is read-only.
- Undo. A removed row is gone once you save, so the grid is blunt about saying
  what is unsaved, and closing a tab with unsaved edits asks twice.
- Resizing a column by dragging its edge. Widths are measured from content, and
  a value longer than the widest column is truncated with an ellipsis.
- Dragging a scrollbar. The bars show the position and the proportions of the
  view; the wheel, trackpad, and keyboard are the only ways to move.
- A real search field with a caret. Search is driven by raw keystrokes, which
  means it has no cursor, no selection, and no undo.
- Searching inside a JSON document.
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
cargo test --workspace        # 115 tests: 56 core, 10 fixture contracts, 49 views
cargo clippy --workspace --all-targets -- -D warnings
```

`bromine-core` has no GPUI dependency, so parsing, sorting, searching, and
writing are tested headlessly. The forty-nine view tests drive real frames with
GPUI's test harness: they type into the window, click column headers and status
buttons, click tabs, send Cmd+C, Cmd+S, and Cmd+W through the real keymap,
dispatch wheel events, and assert on laid-out bounds to check that columns get
the width they were measured at, that a diagonal gesture moves one axis only,
that the row numbers follow the rows and not the columns, that only the rows and
lines in view are drawn, that a copied row comes back tab-separated, that a
lossily decoded file is never written over, and where the scrollbars and status
strips sit.

## Layout

| Path | What lives there |
| :--- | :--------------- |
| `crates/bromine-core` | Format engines. Parsing, the table model, sort, search, write-back. No UI. |
| `crates/bromine-ui` | GPUI views. The CSV grid, the JSON view, the workspace, and the theme tokens. |
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

The one performance figure there is a measurement rather than a claim: opening
the generated 26 MB, 200,000-record JSON fixture costs about 1.2 s of work
before the window has content, in a dev build (648 ms reading and parsing,
179 ms formatting it into 40 MB of text, 303 ms splitting that into 2.4 M lines,
77 ms scanning for the widest line), after which only the visible lines are ever
drawn. The 200,000-row CSV fixture measures the same way: 231 ms to parse and
22 ms to measure every column's width.

One code path has no automated cover at all: the native open panel. GPUI's test
platform stubs `prompt_for_paths` with `unimplemented!()`, so Cmd+O is only ever
exercised by hand. Everything downstream of it (the paths arriving, the file
loading, the tab appearing) is tested through `Workspace::open` directly.
