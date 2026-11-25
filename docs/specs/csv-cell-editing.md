# Spec: editing CSV cells

Status: proposed. Nothing here is implemented yet.

## Goal

Let the user type into a cell of the CSV grid, with a real caret, selection,
undo, and the same edit-then-save rule every other writable view follows.

Today a cell can be marked and copied but not changed. The grid's only edits are
structural (`+ row`, `- row`, `+ col`, `- col` in the status strip,
`crates/bromine-ui/src/csv_grid.rs:562-565`), and they have no undo. The shared
editing foundation (`crates/bromine-ui/src/editor.rs`) already exists and is used
by the text and Markdown views, and `README.md:229` names cell typing as the next
thing missing.

## Decisions already taken

1. **Typing with no edit open still filters rows.** The grid keeps its existing
   type-to-search behavior (`crates/bromine-ui/src/csv_grid.rs:461-495`, the
   "type to filter rows" placeholder, and the test at
   `crates/bromine-ui/src/csv_grid.rs:1445`). Cell editing is entered
   explicitly, so no documented affordance is taken away.
2. **One undo history covers cell edits and structural edits.** Two histories
   would let Cmd+Z appear to do nothing when the last action was a `+ row`. This
   also closes the gap in `README.md:157` ("Structural CSV edits do not yet
   [have undo]").

## Scope

In this increment:

- Editing one data cell at a time (the marked cell).
- A caret, a selection inside the cell, typing, deletion, clipboard, and undo.
- Moving between cells with Enter, Tab, Shift+Tab, and Up/Down while editing.
- Undo and redo for cell edits and for the four structural edits.
- Truthful status: the unsaved state, refused input, and refused saves.

Out of scope, explicitly:

- Header cells. Column names stay sort targets and are repaired at load; naming
  a column is not this increment.
- Multi-cell selection, fill, and block paste from another spreadsheet. A paste
  containing a tab or a line break is refused whole (see "Refused input").
- Cells that display more than one line. `ROW_HEIGHT` is fixed and the grid
  renders its own visible slice, so a multi-line value would have no layout that
  is both honest and uniform.
- Cell-level find/replace, and a real focusable search field.

## Interaction model

### Entering an edit

Two ways, both deliberate:

- **Double-click a cell** (`MouseDownEvent.click_count == 2` on the row, which
  already carries the click count) starts an edit of that cell with the caret at
  the character under the pointer, the way a spreadsheet does.
- **Enter** on a marked cell starts an edit with the caret at the end of the
  value. Enter is unbound while the grid is not in the editor context, so the
  grid's own `on_key_down` sees it.

A session records the cell's absolute table position (the view position is not
stable across a sort), its column, an `EditorState` whose offsets index the
cell's text, the value the cell had when the session opened, and the history
head at that moment (`opened_at`). Opening a session calls
`TableEdits::break_group` first, so nothing typed in the session can merge into
a step from before it and Escape can always discard exactly the session's own
steps.

The edited cell is also the mark (`Selection::Cell`). There is still exactly one
mark, so Cmd+C, the status strip, and the row/column edits keep meaning what
`DESIGN.md` says they mean.

### While a cell is being edited

| Key | Result |
| :-- | :----- |
| character, IME, paste | insert into the cell at the caret |
| Backspace / Delete | delete the selection, or one grapheme either side |
| Left / Right, Shift+Left / Shift+Right | move or extend the caret inside the value |
| Alt+Left / Alt+Right, Shift+Alt+Left / Shift+Alt+Right | word moves |
| Home / End, Shift+Home / Shift+End | start or end of the value |
| Shift+Up / Shift+Down | nothing: a one-line value cannot be selected vertically |
| Cmd+A | select the whole value, not the document |
| Cmd+C / Cmd+X / Cmd+V | copy, cut, paste inside the cell |
| Cmd+Z / Shift+Cmd+Z | undo and redo the cell's own steps |
| **Enter** | commit, then mark the cell one row down in the same column |
| **Tab / Shift+Tab** | commit, then mark the next or previous column |
| **Up / Down** | commit, then mark one row up or down (Excel's behaviour) |
| **Escape** | put the original value back and close the session |
| click inside the same cell | move the caret there; keep editing |
| click in another cell | commit, then mark the clicked cell |
| Cmd+S | commit, then save |

Vertical keys leave the cell because a one-line value has nowhere for a caret to
go vertically. Horizontal keys never leave it.

### Committing

A commit is not a separate data operation: the cell already holds the typed
value, because the buffer *is* the table cell (see "Where the text lives"). A
commit therefore only:

1. Closes the undo group (`TableEdits::break_group`), so the next keystroke in
   that cell starts a new step.
2. Re-measures that one column (`measure_column`) and updates its width, so the
   column grows to fit what was committed. Widths are not re-measured per
   keystroke: a column that resized as you typed would make the table jump.
3. Rebuilds the `RowView`, which may re-sort or re-filter. The mark follows the
   row to its new position via `RowView::position_of`; if the new value no longer
   matches the active filter, the row disappears and the mark is dropped. That is
   the honest outcome: the grid does not keep a filtered-out row on screen.
4. Moves the mark as the table above says.
5. Clears `save_error` and `notice`, the same way `after_table_change` clears
   the save error today: both describe the previous action, not this one.

Everything that changes what the grid is showing commits first: sorting,
filtering, the four structural buttons, the search query, and save. Switching
tabs does not commit: like the text views, the session stays open on the entity
and the caret is simply not drawn while the tab is not focused.

Escape calls `TableEdits::discard_since(opened_at)`, which undoes the session's
steps, restores the value the cell had when the session opened, and clears redo.
The next Cmd+Z therefore reaches the edit *before* the session rather than
resurrecting the text the user cancelled. No `Table::set` on this path: the
history already holds the value to put back.

### Refused input

The grid's cells are single-line, so `\n`, `\r`, and `\t` cannot be shown
faithfully. Text containing any of them is refused whole, not silent-stripped:
stripping would quietly write different bytes than the user pasted. The status
strip says `a cell cannot hold a line break or a tab`, in `theme::warning()`,
and the paste does nothing. Typing cannot produce these characters anyway;
only paste and an IME can.

A file decoded lossily stays editable. Editing it is harmless, the strip already
says the decode was lossy, and `save` refuses with its existing explanation
(`crates/bromine-ui/src/csv_grid.rs:334-352`). Structural edits already behave
this way; making cells the one exception would be a second rule to explain.

## Where the text lives

The editable text is the table cell itself, not a view-side buffer. That keeps
one source of truth, makes `cell_at` in tests read the live value, makes undo and
redo uniform with the text views, and means a save cannot write something other
than what is on screen.

`Table::set` (`crates/bromine-core/src/table.rs:124-146`) already exists for
exactly this. It must not be called directly by the grid any more: every
mutation goes through the history so undo is complete (see below).

## Core changes (`crates/bromine-core`)

Undo lives in core for the same reason every other behaviour does: it is the
part worth trusting, and `cargo test` reaches it headlessly.

### 1. Exact inverses for the structural edits (`table.rs`)

Undoing an insert or a removal must restore the table byte for byte, including
ragged rows. The current methods cannot support that on their own:

- `insert_column` clamps per row (`crates/bromine-core/src/table.rs:172-184`),
  so a row shorter than the index gets its cell appended, and undoing with
  `remove_column(index)` would leave that cell behind.
- `remove_column` skips short rows (`crates/bromine-core/src/table.rs:186-205`),
  so undoing it needs to know which rows actually had a cell.

Add reporting so the inverse is derivable without duplicating the clamp rule:

```rust
/// Where a new column's cells landed, row by row. A row shorter than `index`
/// gets its cell appended to its own end, so undo has to remove each cell from
/// the slot it actually went to.
pub struct ColumnInsertion {
    pub index: usize,
    pub slots: Vec<usize>,
}

/// A removed column, and which rows had a cell at that index.
pub struct ColumnRemoval {
    pub index: usize,
    pub name: String,
    pub cells: Vec<Option<String>>,
}

impl Table {
    // Signature change: these return what undo needs instead of a bare index.
    pub fn insert_column(&mut self, index: usize, name: String) -> ColumnInsertion;
    pub fn remove_column(&mut self, index: usize) -> Result<ColumnRemoval, CoreError>;

    // The inverses, so the history never reaches into the table's shape itself.
    pub fn undo_column_insert(&mut self, insertion: &ColumnInsertion);
    pub fn restore_column(&mut self, removal: ColumnRemoval) -> Result<(), CoreError>;
    pub fn restore_row(&mut self, index: usize, cells: Vec<String>) -> Result<(), CoreError>;
}
```

`insert_row` and `remove_row` already suffice in one direction each: undoing an
insert is `remove_row(index)`, and undoing a removal is `restore_row(index,
cells)` with the `Vec<String>` that `remove_row` returns.

Every inverse gets a core test that compares the whole table before and after a
round trip, on a ragged table.

### 2. A history over a table (`crates/bromine-core/src/edits.rs`, new)

```rust
use crate::table::{ColumnInsertion, ColumnRemoval};
use crate::{CoreError, EditKind, Table};

/// What undo or redo restored, so the view can put the mark and the caret back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    /// The absolute table row, when a step can name one.
    pub row: Option<usize>,
    /// The column the edit touched.
    pub column: Option<usize>,
    /// Where the caret belongs inside the cell, for a cell step.
    pub caret: Option<usize>,
}

/// A stable identity for the step at the head of the history.
///
/// The grid compares its saved head against this instead of a bool, so undoing
/// back to the saved state clears the dirty dot and a new edit after an undo
/// makes it dirty again even if the depth looks the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepId(u64);

/// The undo history for one table.
///
/// The only way a `Table` should be mutated by a view. Steps are cell values
/// and the four structural edits, so Cmd+Z means the same thing after a
/// keystroke and after `+ row`.
#[derive(Debug, Default)]
pub struct TableEdits {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The cell and kind currently open for coalescing.
    group: Option<(usize, usize, EditKind)>,
    next_id: u64,
    head: Option<StepId>,
}

impl TableEdits {
    pub fn set_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
        value: &str,
        caret: CaretMove,
        kind: EditKind,
    ) -> Result<(), CoreError>;

    pub fn insert_row(&mut self, table: &mut Table, index: usize) -> usize;
    pub fn remove_row(&mut self, table: &mut Table, index: usize) -> Result<(), CoreError>;
    pub fn insert_column(&mut self, table: &mut Table, index: usize, name: String) -> usize;
    pub fn remove_column(&mut self, table: &mut Table, index: usize) -> Result<(), CoreError>;

    /// Undo the head step. `None` when there is nothing to undo.
    pub fn undo(&mut self, table: &mut Table) -> Option<EditOutcome>;
    pub fn redo(&mut self, table: &mut Table) -> Option<EditOutcome>;

    /// Undo the head step only if it is a cell step in this exact cell.
    ///
    /// An open edit session must not undo a structural edit behind its own
    /// back; when the cell has no steps left the keystroke does nothing.
    pub fn undo_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
    ) -> Option<EditOutcome>;

    /// Redo, but only when the step being redone lands in this exact cell, for
    /// the same reason as [`Self::undo_cell`].
    pub fn redo_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
    ) -> Option<EditOutcome>;

    pub fn can_undo(&self) -> bool;
    pub fn can_redo(&self) -> bool;
    /// Close the current undo group, so the next edit starts a new step.
    pub fn break_group(&mut self);
    /// Undo every step after `head` and drop them, clearing redo.
    ///
    /// This is what cancelling an edit session needs: the steps the session
    /// added disappear, and nothing the user cancelled can be redone.
    pub fn discard_since(&mut self, table: &mut Table, head: Option<StepId>);
    /// The identity of the step at the head, which "saved" is compared against.
    pub fn head(&self) -> Option<StepId>;
}
```

Coalescing mirrors `TextDocument` (`crates/bromine-core/src/text.rs:29-40`,
`280-397`): a `Typing` edit merges into an open `Typing` group in the same cell,
a `Backspace` merges with `Backspace`, and `EditKind::Other` never merges. A
merged group keeps the id and the `before` value of its first step, plus the
latest `after` and caret, so undo restores the value and caret the cell had
before the group started. `next_id` is only bumped when a new step is created,
never when a step merges.

Each `Step` is one of:

```rust
enum Step {
    Cell { row: usize, column: usize, before: String, after: String,
           caret_before: usize, caret_after: usize, kind: EditKind },
    RowInserted { index: usize },
    RowRemoved { index: usize, cells: Vec<String> },
    ColumnInserted(ColumnInsertion),
    ColumnRemoved(ColumnRemoval),
}
```

Pushing a new step clears `redo`. Undo moves the head back to the previous
step's id (or `None`) and redo moves it forward, so `head()` alone decides
dirtiness.

Export `EditOutcome`, `StepId`, `TableEdits`, `ColumnInsertion`, and
`ColumnRemoval` from `crates/bromine-core/src/lib.rs` next to the existing
`EditKind` / `UndoOutcome` re-exports.

Note that `CellEdit` is already taken by the Markdown model; the new names must
not collide.

## View changes (`crates/bromine-ui/src/csv_grid.rs`)

### State

`CsvGrid` (currently `crates/bromine-ui/src/csv_grid.rs:92-124`) gains and loses:

```rust
/// An open cell edit: which cell, where the caret is, and the value to put
/// back if the user cancels.
struct CellSession {
    /// The absolute table row, not the position: sorting does not move it.
    row: usize,
    column: usize,
    editor: EditorState,
    original: String,
    /// The history head when the session opened, so cancelling can discard
    /// exactly the steps this session added.
    opened_at: Option<StepId>,
}

// on CsvGrid
edits: TableEdits,
/// The history head that was last written to disk.
saved_head: Option<StepId>,
session: Option<CellSession>,
/// Why the last keystroke or paste was refused, shown in the status strip.
notice: Option<SharedString>,

// remove
// dirty: bool,
```

`is_dirty()` becomes `self.edits.head() != self.saved_head`. Every existing
caller of `dirty` (the status strip at
`crates/bromine-ui/src/csv_grid.rs:579-594` and the workspace's close guard)
keeps its meaning, and undoing back to the saved state now clears the dot.

### The buffer adapter

The shared helpers in `editor.rs` take `&mut impl TextBuffer`. Add a scoped
adapter in `csv_grid.rs` that points them at one cell:

```rust
/// The cell being edited, borrowed from the table so the shared editor helpers
/// (typing, deletion, navigation, clipboard) work unchanged.
struct CellBuffer<'a> {
    table: &'a mut Table,
    edits: &'a mut TableEdits,
    row: usize,
    column: usize,
}
```

- `buffer_text` / `buffer_len` / `buffer_slice` read the cell.
- `buffer_replace` calls `edits.set_cell(...)` with the current `EditKind` and
  returns the caret.
- `buffer_undo` / `buffer_redo` call `edits.undo_cell(...)` / a matching
  `redo_cell`, reporting an `UndoOutcome` whose range is the whole value, so
  `editor::undo` puts the caret where the step says.
- `buffer_line_of` is always 0, `buffer_line_start` is `Some(0)`, and
  `buffer_line_range` is `Some(0..len)`: a cell is one line, so Home and End do
  the right thing for free.

`CellBuffer` is constructed with split borrows (`&mut self.document.table`,
`&mut self.edits`, `&mut session.editor`) inside a single method; no new
ownership model is needed.

### Input handling

`CsvGrid` implements `gpui::EntityInputHandler`, modeled on `TextView`
(`crates/bromine-ui/src/text_view.rs:857-987`): `text_for_range`,
`selected_text_range`, `marked_text_range`, `unmark_text`,
`replace_text_in_range`, `replace_and_mark_text_in_range`, `bounds_for_range`,
and `character_index_for_point`, all translated through the UTF-16 helpers in
`editor.rs` and clamped to the cell's text.

Two rules from the guide apply directly:

- Characters are inserted **only** in the input handler, never in `on_key_down`
  and never twice.
- The platform input handler is registered only while a session is open, by
  painting an input-bridge element inside the edited cell. Move
  `InputBridgeElement` out of `text_view.rs` into `editor.rs` and use it from
  all three views instead of copying it a third time.

The grid enters the `"TextEditor"` key context **only while editing**:

```rust
.when(self.session.is_some(), |el| el.key_context("TextEditor"))
```

That is what makes the shared bindings (`crates/bromine-ui/src/text_actions.rs`)
fire for Backspace, Delete, arrow keys, Cmd+A/C/X/V, and Cmd+Z exactly while a
cell is open, and keeps them out of the grid at every other moment. Because the
bindings are context-scoped, the grid's own Cmd+C listener still owns copying a
marked cell, row, or column when no session is open, and the menu bar still has
no global Copy item.

The grid registers `.on_action(...)` handlers for the actions it needs while
editing: `Backspace`, `Delete`, `Left`, `Right`, `Home`, `End`, the word and
selection moves, `SelectAll`, `Enter`, `Tab`, `Outdent`, `Copy`, `Cut`, `Paste`,
`Undo`, and `Redo`. The `Up`/`Down` actions commit and move the mark. Handlers
for actions the grid does not use stay out of the grid.

`on_key_down` keeps owning what is not bound anywhere: Escape (close the session
when one is open, otherwise the existing clear-query-and-mark behaviour), and
the unbound typing path that still filters rows when no session is open.

**`on_key_down` must return immediately while a session is open, except for
Escape.** Its last arm appends any `key_char` to the search query
(`crates/bromine-ui/src/csv_grid.rs:487-493`). With the input handler
registered, the same character is delivered to the cell through
`replace_text_in_range` as well, so leaving that arm reachable is exactly the
double-insertion trap the GPUI notes warn about: every character would land in
the query and in the cell.

### Rendering the edited cell

In `row()` (`crates/bromine-ui/src/csv_grid.rs:1148-1216`) the edited cell is
built differently:

- The cell keeps `selection_strong`, so the mark still reads.
- The value is drawn without `.truncate()`, inside a `.relative()
  .overflow_hidden()` box, translated horizontally by a per-frame offset that
  keeps the caret visible. The offset is computed only for the edited cell, from
  `text_width` (`crates/bromine-ui/src/csv_grid.rs:1283`), never per cell.
- The selection inside the value is a `StyledText` run with
  `background_color: theme::selection()`. One run is enough: the value is plain
  text, unlike Markdown's styled rows.
- The caret is a 1 px `theme::accent()` bar at the caret's measured x, the same
  steady (non-blinking) bar the text view uses (`DESIGN.md`, "The text view").
- The input-bridge element is a child of this cell, so `window.handle_input`
  has the cell's bounds for the IME candidate window.
- No new colour: the caret is `accent`, the selection is `selection`, the cell
  wash stays `selection_strong`.

The status strip grows one item while a session is open: the cell's coordinates,
e.g. `D7`, built from `column_label` (`crates/bromine-ui/src/csv_grid.rs:1337`)
and the row number, in `theme::text_muted()`. With the value being edited, the
reader can still say exactly where they are. This is the one chrome item that
appears on demand, the same way the Markdown table toolbar does.

### Every mutation goes through the history

- `insert_row`, `delete_row`, `insert_column`, `delete_column` call
  `self.edits.*` instead of `self.document.table.*`, then rebuild as they do
  today (`after_table_change`). They commit an open session first.
- Undo and redo of a structural step rebuild the `RowView` and re-measure widths
  (`after_table_change` minus the mark clearing), then mark the affected row or
  column when it still exists and drop the mark otherwise.
- `save()` commits the open session, keeps the lossy refusal, and records
  `self.saved_head = self.edits.head()` only when the write succeeded.
- `after_table_change` no longer sets a flag: dirtiness is derived from the
  head, so an undo that returns to the saved state says "saved" again.

### Performance

Per keystroke: one string allocation, one prefix measurement for the caret
position, and no full-table work. No `RowView::rebuild`, no
`measure_columns`, no width change until commit. A commit costs one column scan
and one `RowView::rebuild`, which is what a structural edit already costs today,
so a 200,000-row file pays a commit exactly what it pays a `+ row` now. The
validation below measures a commit on `large.csv` to confirm this is acceptable
rather than assumed.

## Tests

### Core (`crates/bromine-core/src/edits.rs`, plus `table.rs` and `csv.rs`)

- `setting_a_cell_records_one_step_and_undo_restores_the_value_and_caret`
- `a_run_of_typing_in_one_cell_is_one_undo_step`
- `a_navigation_between_keystrokes_splits_the_undo_step`
- `typing_in_another_cell_starts_a_new_step`
- `undo_followed_by_an_edit_drops_the_redo`
- `undo_cell_refuses_to_undo_a_structural_step`
- `undo_returns_the_head_to_the_step_before_and_a_new_edit_moves_it_on`
- `discarding_a_session_leaves_the_history_where_it_started`
- `undoing_a_row_insert_removes_the_same_row`
- `undoing_a_row_removal_restores_its_cells_exactly`
- `undoing_a_column_insert_removes_each_cell_from_the_slot_it_landed_in`
- `undoing_a_column_removal_restores_the_name_and_only_the_rows_that_had_a_cell`
- `an_out_of_range_cell_edit_is_reported_and_not_recorded`
- `structural_round_trips_leave_a_ragged_table_identical` (compare full tables)
- `values_with_delimiters_quotes_and_spaces_round_trip_through_a_save`

### View (`crates/bromine-ui/src/csv_grid.rs` tests)

Follow the existing helpers (`click`, `click_at`, `mark`, `visible_rows`,
`cell_at`, `focus`, `draw`).

- `double_clicking_a_cell_opens_an_edit_with_the_caret_under_the_pointer`
- `enter_on_a_marked_cell_opens_an_edit_with_the_caret_at_the_end`
- `typing_with_no_edit_open_still_filters_rows` (the existing test at
  `crates/bromine-ui/src/csv_grid.rs:1445` stays green unchanged)
- `typing_into_an_open_cell_changes_that_cell_and_nothing_else`
- `backspace_and_delete_edit_the_cell_rather_than_the_query`
- `enter_commits_and_marks_the_cell_one_row_down`
- `tab_commits_and_marks_the_next_column`
- `escape_restores_the_original_value_and_leaves_no_undo_step`
- `escape_after_commit_undoes_the_previous_edit_not_the_cancelled_one`
- `undoing_back_to_the_saved_state_clears_the_unsaved_state`
- `clicking_another_cell_commits_the_open_edit`
- `sorting_and_filtering_commit_before_they_act`
- `cmd_c_inside_a_cell_copies_the_selection`
- `cmd_c_with_no_edit_open_still_copies_the_mark`
- `a_paste_with_a_line_break_or_tab_is_refused_and_explained`
- `undo_and_redo_of_a_cell_edit_survive_switching_cells`
- `undo_after_a_row_insert_removes_that_row_and_restores_the_mark`
- `save_commits_the_open_cell_and_clears_the_unsaved_state`
- `a_committed_value_widens_only_its_own_column`
- `a_value_that_stops_matching_the_filter_drops_the_mark_on_commit`
- `a_lossy_file_can_be_edited_but_still_refuses_to_save`
- `the_caret_and_the_input_bridge_are_drawn_for_the_open_cell` (assert on
  `debug_bounds` presence; there is no rendered-text reader)
- `a_clear_cell_edit_session_leaves_the_tab_not_dirty` (close guard still reads
  clean when a session opened and closed with no change)

## Documentation to update

- `README.md:48-49`: the "Enough editing" bullet now includes typing into a
  cell, undo, and redo.
- `README.md:157`: drop "Structural CSV edits do not yet [have undo]"; undo now
  covers both kinds of edit.
- `README.md:229`: remove "Typing into a CSV cell" from the status list, and the
  filter section gains a sentence that Enter or a double-click edits a cell while
  typing still filters.
- `DESIGN.md`, "Marking, copying, and editing": replace the paragraph saying
  structural edits come before cell editing with the model above (how an edit is
  entered, what commits it, what Escape does, single-line cells).
- `AGENTS.md`: if the conditional `key_context("TextEditor")` turns out to work
  as expected, add that under the GPUI notes; it is the kind of fact that costs
  a compile cycle to rediscover. If it does not, record what it actually does
  instead.

## Verification

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p bromine-core`
4. `cargo test -p bromine-ui`
5. `cargo test --workspace`
6. Launch the app and use it: `cargo run -p bromine -- fixtures/people.csv`.
   Double-click a cell, type, Enter, Tab, Escape, click away, Cmd+Z twice, sort a
   column while a cell is open, save, reopen the file. Check the caret, the
   selection wash, and the `D7` coordinate in the strip by eye.
7. Large file: `./scripts/make-large-fixture.sh 200000`, open `large.csv`,
   double-click a cell in the first screenful, type a few characters, press
   Enter, and note the latency of a commit against the latency of `+ row`. Both
   should be a single visible beat; if a commit is not, record the number in the
   pull request rather than shipping it silently.
8. Paste a multi-line value and a tab-separated value from a spreadsheet and
   confirm both are refused with the strip's message and change nothing.
9. Open a file with commas, quotes, and leading spaces in its values, edit one,
   save, and confirm `git diff` (or a reload) shows exactly the edit.

## Risks and open points

- **Gestures inside a cell.** Drag-to-select inside the edited value reuses the
  `EditorState::dragging` approach from `TextView` (`on_mouse_down`,
  `on_mouse_move` while `dragging()`, `on_mouse_up`), clamped to the value, with
  no autoscroll. If the row's existing mouse handlers make that fight with
  marking, the fallback is click-to-place-the-caret plus Shift to extend, and
  the spec should be amended rather than the tests relaxed.
- **Up/Down committing** is Excel's behaviour but means a cell cannot use them
  for anything else. If it reads as surprising in the launched app, the
  alternative is that they do nothing until Enter or Escape, and that is a
  one-line change.
- **Undo while editing stops at the session's own steps.** Undoing past the
  start of the cell edit does not exit the editor the way Excel's does; it does
  nothing. Exiting on undo is a later refinement.
- **The grid does not scroll to follow the mark.** `move_selection` has never
  revealed its target, and Enter/Tab/Up/Down while editing inherit that: the new
  mark can be off screen and the caret with it. This increment keeps the
  behaviour consistent rather than fixing it in one path only; following the
  mark is its own change.
- **The dirty flag is now history-based.** A save that fails leaves
  `saved_head` untouched, so a refused lossy save keeps the tab dirty, which is
  correct but worth asserting.
