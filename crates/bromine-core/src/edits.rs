//! Undo for table edits: cell values and the four structural changes.
//!
//! The history is the only thing a view should mutate a [`Table`] through, so
//! Cmd+Z means the same thing after a keystroke and after `+ row`. It lives here
//! rather than in the grid because it is the part worth trusting, and a plain
//! `cargo test` can reach it without a window.
//!
//! Steps store what they replaced, not a copy of the table: undoing a 200,000
//! row file must not have paid for 200,000 cloned strings when the edit
//! happened. A column insertion records the slot each row's new cell landed in,
//! because a ragged row clamps the insertion to its own end and a plain
//! "remove at `index`" would leave that cell behind.

use crate::table::{ColumnInsertion, ColumnRemoval};
use crate::{CoreError, EditKind, Table};

/// What an undo or redo restored, so the view can put the mark and the caret
/// back where they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditOutcome {
    /// The absolute table row the step touched, when it can name one.
    pub row: Option<usize>,
    /// The column the step touched.
    pub column: Option<usize>,
    /// Where the caret belongs inside the cell, for a cell step.
    pub caret: Option<usize>,
}

/// Where the caret sat before and after a cell edit, so undo and redo can put it
/// back where the user left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaretMove {
    /// The offset the caret was at before the edit.
    pub before: usize,
    /// The offset it moved to as a result.
    pub after: usize,
}

/// A stable identity for a step in the history.
///
/// A view remembers the id at the head when it last saved and compares it with
/// the head now. A plain depth would say "three steps, so clean" after an undo
/// followed by a different edit, which is exactly when the file is not the file
/// on disk. Merged typing keeps the id of the step it merged into, so a run of
/// keystrokes cannot look like a save boundary either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepId(u64);

/// One reversible change.
#[derive(Debug, Clone)]
enum Step {
    Cell {
        row: usize,
        column: usize,
        before: String,
        after: String,
        /// Where the caret was before the step, for undo.
        caret_before: usize,
        /// Where it was after the step, for redo.
        caret_after: usize,
        kind: EditKind,
    },
    RowInserted {
        index: usize,
    },
    RowRemoved {
        index: usize,
        cells: Vec<String>,
    },
    ColumnInserted(ColumnInsertion),
    ColumnRemoved(ColumnRemoval),
}

#[derive(Debug)]
struct Recorded {
    id: StepId,
    step: Step,
}

/// The undo history for one table.
#[derive(Debug, Default)]
pub struct TableEdits {
    undo: Vec<Recorded>,
    redo: Vec<Recorded>,
    /// The cell and kind currently open for coalescing. `None` means the next
    /// edit starts a new step, which is what navigation, a click, and every
    /// structural edit leave behind.
    group: Option<(usize, usize, EditKind)>,
    next_id: u64,
}

impl TableEdits {
    /// Replace one cell and record the step.
    ///
    /// A run of typing in the same cell folds into one step, the way the text
    /// views coalesce keystrokes; the caret move is recorded so undo and redo
    /// can put the caret back where the user left it.
    pub fn set_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
        value: &str,
        caret: CaretMove,
        kind: EditKind,
    ) -> Result<(), CoreError> {
        let Some(before) = table.get(row, column).map(str::to_string) else {
            // Report the same error `set` would rather than inventing one.
            return table.set(row, column, value);
        };
        if before == value {
            return Ok(());
        }

        table.set(row, column, value)?;
        self.redo.clear();

        if self.merges(row, column, kind) {
            if let Some(Recorded {
                step: Step::Cell {
                    after, caret_after, ..
                },
                ..
            }) = self.undo.last_mut()
            {
                *after = value.to_string();
                *caret_after = caret.after;
            }
        } else {
            let id = StepId(self.next_id);
            self.next_id += 1;
            self.undo.push(Recorded {
                id,
                step: Step::Cell {
                    row,
                    column,
                    before,
                    after: value.to_string(),
                    caret_before: caret.before,
                    caret_after: caret.after,
                    kind,
                },
            });
        }

        self.group = match kind {
            EditKind::Other => None,
            kind => Some((row, column, kind)),
        };
        Ok(())
    }

    pub fn insert_row(&mut self, table: &mut Table, index: usize) -> usize {
        let index = table.insert_row(index);
        self.push(Step::RowInserted { index });
        index
    }

    pub fn remove_row(&mut self, table: &mut Table, index: usize) -> Result<(), CoreError> {
        let cells = table.remove_row(index)?;
        self.push(Step::RowRemoved { index, cells });
        Ok(())
    }

    pub fn insert_column(&mut self, table: &mut Table, index: usize, name: String) -> usize {
        let insertion = table.insert_column(index, name);
        let index = insertion.index;
        self.push(Step::ColumnInserted(insertion));
        index
    }

    pub fn remove_column(&mut self, table: &mut Table, index: usize) -> Result<(), CoreError> {
        let removal = table.remove_column(index)?;
        self.push(Step::ColumnRemoved(removal));
        Ok(())
    }

    /// Undo the step at the head. `None` when there is nothing to undo.
    pub fn undo(&mut self, table: &mut Table) -> Option<EditOutcome> {
        let recorded = self.undo.pop()?;
        // Steps are replayed in order, so an inverse always fits the table it
        // is applied to. A mismatch would be a bug in this module rather than
        // in the file, so it must not leave the table half-undone and silent.
        let applied = revert(table, &recorded.step);
        debug_assert!(applied.is_ok(), "a recorded step did not fit its table");
        let outcome = outcome(&recorded.step, false);
        self.redo.push(recorded);
        self.group = None;
        Some(outcome)
    }

    /// Redo the step that was last undone.
    pub fn redo(&mut self, table: &mut Table) -> Option<EditOutcome> {
        let recorded = self.redo.pop()?;
        let applied = apply(table, &recorded.step);
        debug_assert!(applied.is_ok(), "a replayed step did not fit its table");
        let outcome = outcome(&recorded.step, true);
        self.undo.push(recorded);
        self.group = None;
        Some(outcome)
    }

    /// Undo the head step only when it is a cell step in this exact cell.
    ///
    /// An open edit session must not undo a structural edit behind its own
    /// back: the row it would remove is not the row the user is looking at.
    pub fn undo_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
    ) -> Option<EditOutcome> {
        match self.undo.last() {
            Some(Recorded {
                step:
                    Step::Cell {
                        row: recorded_row,
                        column: recorded_column,
                        ..
                    },
                ..
            }) if *recorded_row == row && *recorded_column == column => self.undo(table),
            _ => None,
        }
    }

    /// Redo the next step only when it lands in this exact cell.
    pub fn redo_cell(
        &mut self,
        table: &mut Table,
        row: usize,
        column: usize,
    ) -> Option<EditOutcome> {
        match self.redo.last() {
            Some(Recorded {
                step:
                    Step::Cell {
                        row: recorded_row,
                        column: recorded_column,
                        ..
                    },
                ..
            }) if *recorded_row == row && *recorded_column == column => self.redo(table),
            _ => None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Close the current undo group, so the next edit starts a new step.
    ///
    /// Navigation, a click, the start of an edit session, and every structural
    /// edit call this. Without it a caret move in the middle of typing would
    /// fold the next word into the previous undo step.
    pub fn break_group(&mut self) {
        self.group = None;
    }

    /// Undo every step after `head` and drop them, clearing redo.
    ///
    /// This is what cancelling an edit session needs: the steps the session
    /// added disappear, and nothing the user cancelled can be replayed. The
    /// session's steps are always at the tail because opening a session breaks
    /// the group first.
    pub fn discard_since(&mut self, table: &mut Table, head: Option<StepId>) {
        while self.head() != head {
            if self.undo(table).is_none() {
                break;
            }
        }
        self.redo.clear();
    }

    /// The identity of the step at the head, which "saved" is compared against.
    pub fn head(&self) -> Option<StepId> {
        self.undo.last().map(|recorded| recorded.id)
    }

    fn push(&mut self, step: Step) {
        self.redo.clear();
        self.group = None;
        let id = StepId(self.next_id);
        self.next_id += 1;
        self.undo.push(Recorded { id, step });
    }

    /// Whether this edit is a continuation of the open step.
    ///
    /// The conditions are deliberately narrow, mirroring the text views: the
    /// group must still be open for this exact cell and kind, and the head step
    /// must be the one that opened it.
    fn merges(&self, row: usize, column: usize, kind: EditKind) -> bool {
        if kind == EditKind::Other || self.group != Some((row, column, kind)) {
            return false;
        }
        matches!(
            self.undo.last(),
            Some(Recorded {
                step: Step::Cell {
                    row: recorded_row,
                    column: recorded_column,
                    kind: recorded_kind,
                    ..
                },
                ..
            }) if *recorded_row == row && *recorded_column == column && *recorded_kind == kind
        )
    }
}

/// Apply a step's change again, for redo.
fn apply(table: &mut Table, step: &Step) -> Result<(), CoreError> {
    match step {
        Step::Cell {
            row, column, after, ..
        } => table.set(*row, *column, after.clone()),
        Step::RowInserted { index } => {
            table.insert_row(*index);
            Ok(())
        }
        Step::RowRemoved { index, .. } => table.remove_row(*index).map(|_| ()),
        Step::ColumnInserted(insertion) => {
            table.insert_column(insertion.index, insertion.name.clone());
            Ok(())
        }
        Step::ColumnRemoved(removal) => table.remove_column(removal.index).map(|_| ()),
    }
}

/// Put a step's change back the way it was, for undo.
fn revert(table: &mut Table, step: &Step) -> Result<(), CoreError> {
    match step {
        Step::Cell {
            row,
            column,
            before,
            ..
        } => table.set(*row, *column, before.clone()),
        Step::RowInserted { index } => table.remove_row(*index).map(|_| ()),
        Step::RowRemoved { index, cells } => {
            table.restore_row(*index, cells.clone());
            Ok(())
        }
        Step::ColumnInserted(insertion) => {
            table.undo_column_insert(insertion);
            Ok(())
        }
        Step::ColumnRemoved(removal) => table.restore_column(removal.clone()),
    }
}

/// What a step restored, from the point of view of the direction it ran in.
fn outcome(step: &Step, redone: bool) -> EditOutcome {
    match step {
        Step::Cell {
            row,
            column,
            caret_before,
            caret_after,
            ..
        } => EditOutcome {
            row: Some(*row),
            column: Some(*column),
            caret: Some(if redone { *caret_after } else { *caret_before }),
        },
        Step::RowInserted { index } | Step::RowRemoved { index, .. } => EditOutcome {
            row: Some(*index),
            column: None,
            caret: None,
        },
        Step::ColumnInserted(insertion) => EditOutcome {
            row: None,
            column: Some(insertion.index),
            caret: None,
        },
        Step::ColumnRemoved(removal) => EditOutcome {
            row: None,
            column: Some(removal.index),
            caret: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table::new(
            vec!["name".into(), "commits".into()],
            vec![
                vec!["ada".into(), "412".into()],
                vec!["grace".into(), "88".into()],
            ],
        )
    }

    fn value(table: &Table, row: usize, column: usize) -> String {
        table.get(row, column).unwrap_or_default().to_string()
    }

    #[test]
    fn setting_a_cell_records_one_step_and_undo_restores_the_value_and_caret() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "999",
                CaretMove {
                    before: 3,
                    after: 3,
                },
                EditKind::Other,
            )
            .unwrap();
        assert_eq!(value(&table, 0, 1), "999");
        assert!(edits.can_undo());

        let outcome = edits.undo(&mut table).expect("the step undoes");
        assert_eq!(value(&table, 0, 1), "412");
        assert_eq!(
            outcome,
            EditOutcome {
                row: Some(0),
                column: Some(1),
                caret: Some(3)
            }
        );

        let outcome = edits.redo(&mut table).expect("the step redoes");
        assert_eq!(value(&table, 0, 1), "999");
        assert_eq!(outcome.caret, Some(3));
    }

    #[test]
    fn a_run_of_typing_in_one_cell_is_one_undo_step() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "4123",
                CaretMove {
                    before: 3,
                    after: 4,
                },
                EditKind::Typing,
            )
            .unwrap();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "41234",
                CaretMove {
                    before: 4,
                    after: 5,
                },
                EditKind::Typing,
            )
            .unwrap();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "412345",
                CaretMove {
                    before: 5,
                    after: 6,
                },
                EditKind::Typing,
            )
            .unwrap();

        let head = edits.head();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "4123456",
                CaretMove {
                    before: 6,
                    after: 7,
                },
                EditKind::Typing,
            )
            .unwrap();
        assert_eq!(edits.head(), head, "the run stays one step");

        edits.undo(&mut table);
        assert_eq!(value(&table, 0, 1), "412", "one undo returns the whole run");
        assert!(!edits.can_undo());
    }

    #[test]
    fn a_navigation_between_keystrokes_splits_the_undo_step() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "4123",
                CaretMove {
                    before: 3,
                    after: 4,
                },
                EditKind::Typing,
            )
            .unwrap();
        edits.break_group();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "41235",
                CaretMove {
                    before: 4,
                    after: 5,
                },
                EditKind::Typing,
            )
            .unwrap();

        edits.undo(&mut table);
        assert_eq!(value(&table, 0, 1), "4123");
        edits.undo(&mut table);
        assert_eq!(value(&table, 0, 1), "412");
    }

    #[test]
    fn typing_in_another_cell_starts_a_new_step() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "412x",
                CaretMove {
                    before: 3,
                    after: 4,
                },
                EditKind::Typing,
            )
            .unwrap();
        edits
            .set_cell(
                &mut table,
                1,
                1,
                "88x",
                CaretMove {
                    before: 2,
                    after: 3,
                },
                EditKind::Typing,
            )
            .unwrap();

        edits.undo(&mut table);
        assert_eq!(value(&table, 1, 1), "88");
        assert_eq!(value(&table, 0, 1), "412x", "the other cell is untouched");
    }

    #[test]
    fn other_kind_edits_never_merge() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "412x",
                CaretMove {
                    before: 3,
                    after: 4,
                },
                EditKind::Other,
            )
            .unwrap();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "412xy",
                CaretMove {
                    before: 4,
                    after: 5,
                },
                EditKind::Other,
            )
            .unwrap();

        edits.undo(&mut table);
        assert_eq!(value(&table, 0, 1), "412x");
    }

    #[test]
    fn undo_followed_by_an_edit_drops_the_redo() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "1",
                CaretMove {
                    before: 3,
                    after: 1,
                },
                EditKind::Other,
            )
            .unwrap();
        edits.undo(&mut table);
        assert!(edits.can_redo());

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "2",
                CaretMove {
                    before: 3,
                    after: 1,
                },
                EditKind::Other,
            )
            .unwrap();
        assert!(!edits.can_redo());
    }

    #[test]
    fn undo_cell_refuses_to_undo_a_structural_step() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits.insert_row(&mut table, 0);

        assert_eq!(edits.undo_cell(&mut table, 0, 1), None);
        assert_eq!(table.row_count(), 3, "the row is still there");
        assert!(edits.can_undo(), "the structural step is still undoable");
    }

    #[test]
    fn undo_returns_the_head_to_the_step_before_and_a_new_edit_moves_it_on() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                0,
                "Ada",
                CaretMove {
                    before: 3,
                    after: 3,
                },
                EditKind::Other,
            )
            .unwrap();
        let first = edits.head();
        edits
            .set_cell(
                &mut table,
                0,
                1,
                "1",
                CaretMove {
                    before: 3,
                    after: 1,
                },
                EditKind::Other,
            )
            .unwrap();
        let second = edits.head();
        assert_ne!(first, second);

        edits.undo(&mut table);
        assert_eq!(edits.head(), first);

        edits
            .set_cell(
                &mut table,
                1,
                0,
                "Grace",
                CaretMove {
                    before: 5,
                    after: 5,
                },
                EditKind::Other,
            )
            .unwrap();
        let third = edits.head();
        assert_ne!(third, first);
        assert_ne!(third, second, "an id is never reused");
    }

    #[test]
    fn discarding_a_session_leaves_the_history_where_it_started() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                0,
                "Ada",
                CaretMove {
                    before: 3,
                    after: 3,
                },
                EditKind::Other,
            )
            .unwrap();
        let saved = edits.head();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "gone",
                CaretMove {
                    before: 3,
                    after: 4,
                },
                EditKind::Other,
            )
            .unwrap();
        edits.discard_since(&mut table, saved);

        assert_eq!(edits.head(), saved);
        assert_eq!(value(&table, 0, 1), "412");
        assert!(!edits.can_redo(), "a cancelled edit cannot be redone");
        assert!(edits.can_undo(), "the earlier edit is still there");
    }

    #[test]
    fn undoing_a_row_insert_removes_the_same_row() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits.insert_row(&mut table, 1);
        assert_eq!(table.row_count(), 3);

        let outcome = edits.undo(&mut table).expect("the insert undoes");
        assert_eq!(outcome.row, Some(1));
        assert_eq!(table.row_count(), 2);
        assert_eq!(value(&table, 1, 0), "grace");
    }

    #[test]
    fn undoing_a_row_removal_restores_its_cells_exactly() {
        let mut table = table();
        let before = table.clone();
        let mut edits = TableEdits::default();

        edits.remove_row(&mut table, 0).unwrap();
        edits.undo(&mut table);

        assert_eq!(table, before);
    }

    #[test]
    fn undoing_a_column_insert_removes_each_cell_from_the_slot_it_landed_in() {
        let mut table = Table::new(
            vec!["name".into()],
            vec![vec!["ada".into()], vec!["grace".into()]],
        );
        let before = table.clone();
        let mut edits = TableEdits::default();

        edits.insert_column(&mut table, 1, "role".into());
        assert_eq!(table.column_count(), 2);

        edits.undo(&mut table);
        assert_eq!(table, before);
    }

    #[test]
    fn undoing_a_column_removal_restores_the_name_and_only_the_rows_that_had_a_cell() {
        let mut table = Table::new(
            vec!["name".into(), "role".into()],
            vec![vec!["ada".into(), "engineer".into()]],
        );
        let before = table.clone();
        let mut edits = TableEdits::default();

        edits.remove_column(&mut table, 1).unwrap();
        let outcome = edits.undo(&mut table).expect("the removal undoes");

        assert_eq!(outcome.column, Some(1));
        assert_eq!(table, before);
    }

    #[test]
    fn structural_round_trips_leave_a_ragged_table_identical() {
        let mut table = Table::new(
            vec!["name".into(), "role".into()],
            vec![
                vec!["ada".into(), "engineer".into()],
                vec!["grace".into(), "admiral".into()],
            ],
        );
        let before = table.clone();
        let mut edits = TableEdits::default();

        edits.insert_column(&mut table, 1, "rank".into());
        edits.remove_row(&mut table, 0).unwrap();
        edits.insert_row(&mut table, 0);
        edits.remove_column(&mut table, 1).unwrap();

        while edits.can_undo() {
            edits.undo(&mut table);
        }
        assert_eq!(table, before);
    }

    #[test]
    fn an_out_of_range_cell_edit_is_reported_and_not_recorded() {
        let mut table = table();
        let mut edits = TableEdits::default();

        assert!(matches!(
            edits.set_cell(
                &mut table,
                9,
                0,
                "x",
                CaretMove {
                    before: 0,
                    after: 1,
                },
                EditKind::Typing,
            ),
            Err(CoreError::NoSuchRow { row: 9, rows: 2 })
        ));
        assert!(!edits.can_undo());
    }

    #[test]
    fn setting_a_cell_to_the_value_it_already_holds_records_nothing() {
        let mut table = table();
        let mut edits = TableEdits::default();

        edits
            .set_cell(
                &mut table,
                0,
                1,
                "412",
                CaretMove {
                    before: 3,
                    after: 3,
                },
                EditKind::Typing,
            )
            .unwrap();

        assert!(!edits.can_undo());
    }
}
