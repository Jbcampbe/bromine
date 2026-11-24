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

Add a token rather than a one-off colour. If a colour appears twice in views and
is not a token, it should be one.

## Type scale

Three sizes is enough: a small size for metadata, a base size for cells and
labels, and one larger size for a file name or heading. Resist adding a fourth.

## Layout

- A tab strip: one tab per open file, named after the file, with its close
  affordance on the tab itself and an "open" action at the far right. The visible
  tab is distinguished by background, not by an underline or a heavier border.
  Tabs shrink and truncate rather than scrolling; a tab bar that scrolls is a
  later problem.
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
- A status strip along the bottom carrying the row and column counts, the read
  only notice, and any repair warning. The file name lives in the tab, so it is
  not repeated here, and the status strip sits below the grid so the eye finds
  nothing between the tabs and the data.
- An error strip, under the tab strip, only when opening a file failed. It is
  dismissable and never covers the grid.
- Column widths are measured from the content a column holds: the widest value
  in it, or its header if that is wider. Widths are computed once when a file
  opens, never per frame, and clamped to 56–420 px so a single pathological
  value cannot crowd out every other column. A table wider than the window
  scrolls sideways rather than squeezing columns below their measured width.

## The Markdown goal

The eventual Markdown experience should feel like Bear: distraction-free
writing, one accent colour, and typography that makes prose pleasant rather than
a WYSIWYG toolbar. That is a different register from the CSV grid, so it should
be a different view rather than a mode of the grid view.
