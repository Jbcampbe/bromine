# Design guidance: Bromine

Bromine is a developer tool, so the design brief is "dense, quiet, and legible
for hours", not "friendly". It sits next to a terminal all day.

## Principles

1. **The data is the interface.** Chrome should recede. No borders around
   containers that do not need them, no shadows on panels, no decoration.
2. **Density over airiness.** A CSV grid earns its screen space. Row height is
   compact and consistent, because `uniform_list` needs uniform heights anyway.
3. **Monospace for values, proportional for chrome.** Cell contents, keys, and
   paths are monospace. Labels and menus are not.
4. **Nothing moves unless it means something.** Sorting a column, filtering a
   view, and switching files are the motions. Hover states get a background
   change, not a transform.
5. **Always show the truth about the file.** Show row and column counts, show
   when ragged input was repaired, show when a file was decoded lossily. A
   viewer that silently normalizes data is lying to the person reading it.

## Tokens

Define one set of tokens in `crates/bromine-ui/src/theme.rs` and use them
everywhere. Views must not inline raw colors.

| Role | Purpose |
| :--- | :------ |
| `background` | Window background |
| `surface` | The header row and the search bar |
| `surface_active` | The header of the column currently sorted by |
| `row_alternate` | Every second data row, so wide rows stay readable across |
| `border` | Separators between regions |
| `border_subtle` | Separators between cells, quieter than a region border |
| `text` | Primary text |
| `text_muted` | Labels, counts, secondary metadata |
| `accent` | The sorted column and interactive affordances |
| `warning` | Something the user needs to know about the data itself, such as a repaired or lossily decoded file |
| `scrollbar` | The drawn scrollbar thumbs |
| `selection` | The wash behind a marked cell, row, or column |
| `selection_strong` | The wash behind the one cell that was clicked, so it stands out from the row it is in |
| `code_surface` | Inline code and fenced blocks in the Markdown view |
| `quote_bar` | The vertical rule beside a blockquote |
| `image_placeholder` | The box standing in for an image that cannot be shown |

Add a token rather than a one-off colour. If a colour appears twice in views and
is not a token, it should be one.

## The icon

The Dock icon is the app's only illustration, so it follows the same rules as
the window.

- **Vector source, one drawing at every size.** `assets/icon.svg` is the source,
  and `scripts/make-icon.sh` rasterizes every iconset entry from it. Do not
  replace it with a hand-pixeled 16 px variant: the small icon is the same
  drawing, not a different one.
- **Letterforms are paths, never `<text>`.** A `<text>` element makes the icon
  depend on fonts installed wherever it is rendered; rasterizing on a machine
  without that face would produce a different mark.
- **Palette only.** The tile is `background`, its edge is `border`, and the mark
  is `accent`. The grid lines behind the letters are the theme's slate, not a
  new colour.
- **Legible at 16 px.** The mark is chunky enough to read as "Br" in a Finder
  list. A change that only works at 512 px is not finished.
- **macOS geometry.** A 1024 canvas with an 824 px rounded tile centred in it is
  Apple's icon grid. The transparent margin is what keeps the icon the same
  weight in the Dock as the apps beside it.

## Type scale

Three sizes are enough for chrome and data: a small size for metadata, a base
size for cells and labels, and one larger size for a file name or heading.
Resist adding a fourth. The Markdown view is the exception, because a heading
level there is content rather than chrome; its six sizes are the document's
hierarchy, not a new scale for the app.

## Layout

- A tab strip: one tab per open file, named after the file, with its close
  affordance on the tab itself and an "open" action at the far right. The visible
  tab is distinguished by background, not by an underline or a heavier border.
  Tabs shrink and truncate rather than scrolling; a tab bar that scrolls is a
  later problem.
- A native menu bar: **Bromine** (Services, Quit) and **File** (Open…, Save,
  Close Tab). The key equivalent shown beside an item comes from the app's
  keymap, so the menu names the key the app actually listens for. There is
  deliberately no Edit menu yet: Copy and Cut are bound only inside the text
  editors' `TextEditor` context, and a menu item needs a global action. Save is
  global because every editable tab implements the same `save` entry point.
- Quit is guarded the way Close Tab is. With unsaved edits the first Cmd+Q
  explains itself in the error strip and the second quits; one keystroke must
  not be able to throw work away. `Workspace::request_quit` owns that rule so
  the menu item, the key equivalent, and any future dock menu share it.
- A search bar, kept minimal: one field and a match count.
- The grid, which takes all remaining space and scrolls in both directions. Its
  header row scrolls sideways with the rows and stays put vertically.
- One axis per gesture. A trackpad reports both axes in the same event, and a
  grid that moved on both feels drunk; a gesture keeps the axis it started on
  until the fingers lift. A mouse wheel picks per event, because it has no
  gesture to keep.
- Thin scrollbars along the right and bottom edges, drawn only when that axis
  actually overflows. They are indicators of position and proportion, not
  controls: a bar you can drag is a later increment.
- A status strip along the bottom carrying the row and column counts, the row
  and column edit buttons, the saved state, any repair warning, and, for a view
  that cannot write, a read only notice. The file name lives in the tab, so it is
  not repeated here, and the status strip sits below the grid so the eye finds
  nothing between the tabs and the data.
- Row numbers down the left edge and column letters along the top, in the
  convention every spreadsheet shares: numbers count from one and do not scroll
  sideways, letters read `A` to `Z` then `AA`. A reader who has used Excel or
  Numbers should not have to learn where row 12,000 is. The letters mark a
  column where the names below them sort by one, so the two affordances stay
  distinct.
- An error strip, under the tab strip, only when opening a file failed. It is
  dismissable and never covers the grid.
- Column widths are measured from the content a column holds: the widest value
  in it, or its header if that is wider. Widths are computed once when a file
  opens, never per frame, and clamped to 56–420 px so a single pathological
  value cannot crowd out every other column. A table wider than the window
  scrolls sideways rather than squeezing columns below their measured width.

## Marking, copying, and editing

- A mark is one cell, one row, or one column. Clicking a cell marks it, clicking
  a row number marks that row, clicking a letter marks that column, and the
  arrow keys move the mark one cell at a time. There is exactly one mark, never
  a multi-cell range: a range needs a modifier or a drag, and either one should
  arrive with a visible reason.
- A marked row or column is washed with `selection`. The one cell that was
  clicked gets `selection_strong`, and the letter above it and the number beside
  it turn `accent`, so a mark inside a wide row still says where it started.
- Cmd+C copies the mark as text: a cell as its value, a row as tab-separated
  values, a column as one value per line. Tab and newline are the shapes a
  spreadsheet and a text editor paste back into place.
- Sorting or filtering drops the mark, because a position stops naming the same
  row the moment the order or the filter changes.
- Structural edits come before cell editing: add and remove rows and columns
  from the status strip. A new row is empty and a new column takes its position
  as a name. Typing into a CSV cell needs the text-input foundation (caret,
  selection, undo), which now exists in the text and Markdown views, and is not
  faked with a half-working field until the grid has its own increment.
- A new row lands below the marked row, or at the end of the file when nothing
  is marked. A new column lands to the right of the marked column, or at the
  right edge. Every edit is resolved against the table through the view, so a
  row removed from a filtered view removes the row the user is looking at.
- Edit then save; never save as you type. Cmd+S writes the file back and the
  status strip says "unsaved changes" until it does.
- A file that was decoded lossily is never written back: the replacement
  characters would overwrite bytes the viewer could not read, so the grid
  refuses and says why. Repairs applied at load (padded rows, unnamed columns)
  do not block a save, because they are visible in the status strip and what gets
  written is the rectangle the user is looking at.

## The JSON view

- **A line view, not a tree.** A tree hides most of the file behind folds and
  makes the thing a developer usually wants, the line a parser complained about,
  hard to reach. One line per formatted line with numbers down the side is
  duller and more useful. A collapsible tree is a later view, not a better
  first one.
- **Format and minify are both offered** because they answer different
  questions: indented is for reading, one line is for confirming the shape of a
  record. The buttons sit in a strip above the text and the active one is the
  accented one, the same way the sorted column header is.
- **Formatting never reorders keys.** What is written is the document that was
  opened, with different whitespace.
- **A very large document is not put on one line.** Minifying shapes the whole
  document as one run of text, and a run of millions of characters freezes the
  window before anything is drawn. Past 400 KB the view stays formatted and says
  why in the status strip. An instant refusal is better than a stall, and the
  reader can still see every byte.
- **Line numbers, not scrollbar math.** The gutter is the same construction as
  the grid's row numbers: it follows the text vertically and never sideways, so
  a line number always describes the line beside it.
- **Read only, and it says so.** There is no edit path for JSON yet, so the
  status strip says "read only" rather than leaving the reader to discover it.
  When editing arrives it goes through the same edit-then-save model as the
  grid, not through a save-on-type field.

## The text view

Plain text is shown, not interpreted. One monospace size, line numbers down
the side, tabs expanded to four spaces, and no wrapping: a log line that runs
past the window scrolls sideways rather than folding into an unreadable block.
The caret is a steady accent bar rather than a blinking one, because a blinking
caret in a read-mostly window is movement that does not mean anything.

The status strip carries line and column, the line count, the saved state, and
the reason a save was refused. A file decoded lossily can be read but never
written back.

## The Markdown view

Markdown is a writing surface, and it should feel like Bear rather than like a
word processor with a toolbar.

- **One column, about 720 px wide, centred.** Long prose lines are hard to
  track; the margin is doing work, not decoration.
- **The source is the truth.** Every edit reparses the document, so row N stays
  line N and a click can always be turned back into a byte offset.
- **Markers are hidden until the block is active.** A paragraph reads as prose;
  clicking it reveals the `**` around a word and the `#` before a heading, in
  place. That is the Bear arrangement, and it keeps the reading view quiet
  without hiding how the file is written.
- **One accent colour.** Links, active controls, and the caret share `accent`.
  Inline code gets `code_surface`, a blockquote gets `quote_bar`, and nothing
  gets a border it does not need.
- **Lists are a gutter, not punctuation.** Bullets, numbers, and checkboxes sit
  in a narrow gutter so the text itself starts at the same place on every item.
  Clicking a checkbox rewrites `[ ]` or `[x]` in the source.
- **A table is a grid, and the pipes are never shown.** Alignment lives in a
  toolbar above the active table, not in the delimiter row. Cell edits are
  clamped to the cell's own source range, so typing cannot eat a pipe.
- **Images are blocks.** A local path resolves relative to the Markdown file;
  `http` and `https` go through the shared image loader. A missing or failed
  image leaves a bordered placeholder with the alt text and the destination,
  because a blank hole says less than an explanation.
- **A very large file is read only.** Past 2 MB or 20,000 rows the view falls
  back to monospace lines and says why. An instant, honest refusal is better
  than a window that freezes while it shapes the file.

The table toolbar is the one piece of chrome that appears on demand. It sits
above the table it belongs to and is absent everywhere else.
