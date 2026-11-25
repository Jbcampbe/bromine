use crate::CoreError;

/// A rectangular block of text cells with named columns.
///
/// Real-world CSV is ragged: rows can be shorter or longer than the header
/// line. A `Table` always presents a rectangle, so short rows are padded with
/// empty cells and columns that appear only in data get a generated header
/// name. [`Table::load_repairs`] reports how much repair happened, so callers
/// can tell the user rather than silently rewriting their file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// What had to be changed to make parsed records rectangular.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Repairs {
    /// Longest record seen, which becomes the column count.
    pub columns: usize,
    /// Rows that were shorter than the column count and got padded.
    pub padded_rows: usize,
    /// Columns beyond the header line that needed a generated name.
    pub unnamed_columns: usize,
}

/// Where a new column's cells landed, row by row.
///
/// A row shorter than the insertion index gets its cell appended to its own
/// end rather than padded out to the index, so undo has to remove each cell
/// from the slot it actually went to. The name is kept so redo can insert the
/// same column again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInsertion {
    pub index: usize,
    pub name: String,
    pub slots: Vec<usize>,
}

/// A removed column, and which rows had a cell at that index.
///
/// Rows shorter than the index are left alone by a removal, so `cells` holds
/// `None` for them; a restore has to skip those rows too or the table stops
/// being the shape it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRemoval {
    pub index: usize,
    pub name: String,
    pub cells: Vec<Option<String>>,
}

impl Table {
    /// Build a table from headers and rows, trimming or padding every row to
    /// match `headers`.
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        let width = headers.len();
        let rows = rows
            .into_iter()
            .map(|mut row| {
                row.resize(width, String::new());
                row
            })
            .collect();
        Self { headers, rows }
    }

    /// Build a rectangular table from ragged records, repairing as needed.
    pub fn load_repairs(headers: &[String], records: Vec<Vec<String>>) -> (Self, Repairs) {
        let mut columns = headers.len();
        for record in &records {
            columns = columns.max(record.len());
        }

        let mut repaired_headers = headers.to_vec();
        let unnamed_columns = columns.saturating_sub(repaired_headers.len());
        for _ in 0..unnamed_columns {
            // 1-based and offset past the real headers, so two unnamed columns
            // after a 2-column header become `column_3` and `column_4`.
            repaired_headers.push(format!("column_{}", repaired_headers.len() + 1));
        }

        let mut padded_rows = 0;
        let mut rows = Vec::with_capacity(records.len());
        for mut record in records {
            if record.len() < columns {
                padded_rows += 1;
                record.resize(columns, String::new());
            } else if record.len() > columns {
                record.truncate(columns);
            }
            rows.push(record);
        }

        (
            Self {
                headers: repaired_headers,
                rows,
            },
            Repairs {
                columns,
                padded_rows,
                unnamed_columns,
            },
        )
    }

    pub fn headers(&self) -> &[String] {
        &self.headers
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.headers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.headers.is_empty()
    }

    /// Header text for a column, or `None` when the index is out of range.
    pub fn header(&self, column: usize) -> Option<&str> {
        self.headers.get(column).map(String::as_str)
    }

    pub fn row(&self, row: usize) -> Option<&[String]> {
        self.rows.get(row).map(Vec::as_slice)
    }

    /// Borrow a single cell. Out-of-range coordinates return `None` so views
    /// can render a placeholder instead of panicking.
    pub fn get(&self, row: usize, column: usize) -> Option<&str> {
        self.rows.get(row)?.get(column).map(String::as_str)
    }

    /// Replace one cell. This is the only mutation that a cell editor needs.
    pub fn set(
        &mut self,
        row: usize,
        column: usize,
        value: impl Into<String>,
    ) -> Result<(), CoreError> {
        let rows = self.rows.len();
        let target = self
            .rows
            .get_mut(row)
            .ok_or(CoreError::NoSuchRow { row, rows })?;
        let columns = target.len();
        let cell = target
            .get_mut(column)
            .ok_or(CoreError::NoSuchColumn { column, columns })?;
        *cell = value.into();
        Ok(())
    }

    /// Append an empty row, extending every existing row if it is wider.
    pub fn push_empty_row(&mut self) -> usize {
        let width = self.column_count();
        self.rows.push(vec![String::new(); width]);
        self.rows.len() - 1
    }

    /// Insert an empty row at `index`, clamped to the end of the table.
    ///
    /// Returns the index the row actually landed at, because a caller that
    /// asked for a position past the end still needs to know where to select
    /// or edit the row it just created.
    pub fn insert_row(&mut self, index: usize) -> usize {
        let index = index.min(self.rows.len());
        let width = self.column_count();
        self.rows.insert(index, vec![String::new(); width]);
        index
    }

    pub fn remove_row(&mut self, row: usize) -> Result<Vec<String>, CoreError> {
        if row >= self.rows.len() {
            return Err(CoreError::NoSuchRow {
                row,
                rows: self.rows.len(),
            });
        }
        Ok(self.rows.remove(row))
    }

    /// Insert a named column at `index`, clamped to the existing width.
    ///
    /// Every row gains a cell at the same position, including rows that are
    /// still shorter than the requested index: a ragged row is clamped to its
    /// own end instead of being padded, so one bad record cannot abort the
    /// edit for the whole table. The returned [`ColumnInsertion`] records the
    /// slot each cell landed in, which is what [`Self::undo_column_insert`]
    /// needs to put the table back exactly.
    pub fn insert_column(&mut self, index: usize, name: String) -> ColumnInsertion {
        let index = index.min(self.headers.len());
        self.headers.insert(index, name.clone());
        let mut slots = Vec::with_capacity(self.rows.len());
        for row in &mut self.rows {
            let at = index.min(row.len());
            row.insert(at, String::new());
            slots.push(at);
        }
        ColumnInsertion { index, name, slots }
    }

    /// Remove a column and return its header name and the cells it held.
    ///
    /// Rows shorter than the column have nothing at that index, so they are
    /// left alone rather than being padded just to be cut again.
    pub fn remove_column(&mut self, index: usize) -> Result<ColumnRemoval, CoreError> {
        let columns = self.column_count();
        if index >= columns {
            return Err(CoreError::NoSuchColumn {
                column: index,
                columns,
            });
        }
        let mut cells = Vec::with_capacity(self.rows.len());
        for row in &mut self.rows {
            cells.push(if index < row.len() {
                Some(row.remove(index))
            } else {
                None
            });
        }
        Ok(ColumnRemoval {
            index,
            name: self.headers.remove(index),
            cells,
        })
    }

    /// The inverse of [`Self::insert_column`].
    pub fn undo_column_insert(&mut self, insertion: &ColumnInsertion) {
        if insertion.index < self.headers.len() {
            self.headers.remove(insertion.index);
        }
        for (row, slot) in self.rows.iter_mut().zip(&insertion.slots) {
            if *slot < row.len() {
                row.remove(*slot);
            }
        }
    }

    /// The inverse of [`Self::remove_column`].
    ///
    /// Refuses when the table no longer has the rows the removal was taken
    /// from: replaying it onto a different shape would put a value in the wrong
    /// cell, which is worse than reporting that the edit is stale.
    pub fn restore_column(&mut self, removal: ColumnRemoval) -> Result<(), CoreError> {
        if removal.cells.len() != self.rows.len() {
            return Err(CoreError::StaleEdit {
                expected: removal.cells.len(),
                found: self.rows.len(),
            });
        }
        self.headers.insert(removal.index, removal.name);
        for (row, cell) in self.rows.iter_mut().zip(removal.cells) {
            if let Some(value) = cell {
                let at = removal.index.min(row.len());
                row.insert(at, value);
            }
        }
        Ok(())
    }

    /// The inverse of [`Self::remove_row`]: put a removed row back.
    ///
    /// Returns the index the row landed at, which is the requested index
    /// clamped to the table, because a caller that replayed an edit still needs
    /// to know where it went.
    pub fn restore_row(&mut self, index: usize, cells: Vec<String>) -> usize {
        let index = index.min(self.rows.len());
        self.rows.insert(index, cells);
        index
    }

    /// True when every cell in the row is empty after trimming.
    pub fn row_is_blank(&self, row: usize) -> bool {
        self.rows
            .get(row)
            .is_some_and(|cells| cells.iter().all(|cell| cell.trim().is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> Vec<String> {
        vec!["name".into(), "role".into()]
    }

    #[test]
    fn new_pads_short_rows_to_header_width() {
        let table = Table::new(
            headers(),
            vec![vec!["ada".into()], vec!["grace".into(), "admiral".into()]],
        );
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.get(0, 1), Some(""));
        assert_eq!(table.get(1, 1), Some("admiral"));
    }

    #[test]
    fn new_truncates_rows_wider_than_headers() {
        let table = Table::new(headers(), vec![vec!["a".into(), "b".into(), "c".into()]]);
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.row(0).unwrap().len(), 2);
    }

    #[test]
    fn load_repairs_names_extra_columns_and_counts_padding() {
        let records = vec![
            vec!["ada".into(), "engineer".into(), "1843".into()],
            vec!["grace".into()],
        ];
        let (table, repairs) = Table::load_repairs(&headers(), records);

        assert_eq!(table.column_count(), 3);
        assert_eq!(table.header(2), Some("column_3"));
        assert_eq!(repairs.columns, 3);
        assert_eq!(repairs.unnamed_columns, 1);
        assert_eq!(repairs.padded_rows, 1);
        assert_eq!(table.get(1, 2), Some(""));
    }

    #[test]
    fn load_repairs_is_a_no_op_for_clean_records() {
        let records = vec![vec!["ada".into(), "engineer".into()]];
        let (table, repairs) = Table::load_repairs(&headers(), records);

        assert_eq!(
            repairs,
            Repairs {
                columns: 2,
                padded_rows: 0,
                unnamed_columns: 0
            }
        );
        assert_eq!(table.header(0), Some("name"));
        assert_eq!(table.row_count(), 1);
    }

    #[test]
    fn set_reports_out_of_range_coordinates() {
        let mut table = Table::new(headers(), vec![vec!["ada".into(), "engineer".into()]]);

        assert!(table.set(0, 0, "Ada Lovelace").is_ok());
        assert_eq!(table.get(0, 0), Some("Ada Lovelace"));

        assert!(matches!(
            table.set(9, 0, "x"),
            Err(CoreError::NoSuchRow { row: 9, rows: 1 })
        ));
        assert!(matches!(
            table.set(0, 9, "x"),
            Err(CoreError::NoSuchColumn {
                column: 9,
                columns: 2
            })
        ));
    }

    #[test]
    fn remove_row_shifts_later_rows_down() {
        let mut table = Table::new(
            headers(),
            vec![
                vec!["ada".into(), "engineer".into()],
                vec!["grace".into(), "admiral".into()],
            ],
        );

        let removed = table.remove_row(0).unwrap();
        assert_eq!(removed, vec!["ada".to_string(), "engineer".to_string()]);
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.get(0, 0), Some("grace"));
        assert!(table.remove_row(5).is_err());
    }

    #[test]
    fn blank_rows_are_detected_by_trimmed_content() {
        let table = Table::new(
            headers(),
            vec![vec!["  ".into(), "".into()], vec!["ada".into(), "".into()]],
        );

        assert!(table.row_is_blank(0));
        assert!(!table.row_is_blank(1));
        assert!(!table.row_is_blank(7));
    }

    #[test]
    fn insert_row_works_at_start_middle_end_and_past_the_end() {
        let mut table = Table::new(
            headers(),
            vec![
                vec!["ada".into(), "engineer".into()],
                vec!["grace".into(), "admiral".into()],
            ],
        );

        assert_eq!(table.insert_row(0), 0);
        assert_eq!(table.get(0, 0), Some(""));
        assert_eq!(table.get(1, 0), Some("ada"));

        assert_eq!(table.insert_row(2), 2);
        assert_eq!(table.get(2, 0), Some(""));
        assert_eq!(table.get(3, 0), Some("grace"));

        assert_eq!(table.insert_row(4), 4);
        assert_eq!(table.row_count(), 5);

        // Past the end lands at the end and reports where it landed.
        assert_eq!(table.insert_row(99), 5);
        assert_eq!(table.row_count(), 6);
        assert_eq!(
            table.row(5).unwrap().to_vec(),
            vec![String::new(), String::new()]
        );
    }

    #[test]
    fn insert_column_into_a_ragged_table_pads_every_row() {
        // The public constructors always make a rectangle, but a later edit or
        // a short row can leave rows of different widths, so insertion has to
        // cope with that directly.
        let mut table = Table {
            headers: headers(),
            rows: vec![
                vec!["ada".into()],
                vec!["grace".into(), "admiral".into(), "extra".into()],
            ],
        };

        assert_eq!(table.insert_column(1, "rank".into()).index, 1);
        assert_eq!(table.header(1), Some("rank"));
        assert_eq!(table.column_count(), 3);

        assert_eq!(
            table.row(0).unwrap().to_vec(),
            vec!["ada".to_string(), String::new()]
        );
        assert_eq!(
            table.row(1).unwrap().to_vec(),
            vec![
                "grace".to_string(),
                String::new(),
                "admiral".to_string(),
                "extra".to_string()
            ]
        );
        assert_eq!(table.get(1, 2), Some("admiral"));
    }

    #[test]
    fn remove_column_keeps_later_cells_aligned() {
        let mut table = Table::new(
            vec!["name".into(), "role".into(), "city".into()],
            vec![
                vec!["ada".into(), "engineer".into(), "london".into()],
                vec!["grace".into(), "admiral".into(), "arlington".into()],
            ],
        );

        let removed = table.remove_column(1).unwrap();
        assert_eq!(removed.name, "role");
        assert_eq!(removed.index, 1);
        assert_eq!(
            removed.cells,
            vec![Some("engineer".to_string()), Some("admiral".to_string())]
        );
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.header(1), Some("city"));
        // The neighbour after the hole must still hold the value that was
        // there before, not the cell that shifted into its place.
        assert_eq!(table.get(0, 0), Some("ada"));
        assert_eq!(table.get(0, 1), Some("london"));
        assert_eq!(table.get(1, 1), Some("arlington"));

        assert!(matches!(
            table.remove_column(9),
            Err(CoreError::NoSuchColumn {
                column: 9,
                columns: 2
            })
        ));
    }

    #[test]
    fn remove_column_skips_rows_shorter_than_the_index() {
        let mut table = Table {
            headers: headers(),
            rows: vec![vec!["ada".into()], vec!["grace".into(), "admiral".into()]],
        };

        let removed = table.remove_column(1).unwrap();
        assert_eq!(removed.name, "role");
        assert_eq!(removed.cells, vec![None, Some("admiral".to_string())]);
        // The short row never had the column, so it stays one cell wide.
        assert_eq!(table.row(0).unwrap().to_vec(), vec!["ada".to_string()]);
        assert_eq!(table.row(1).unwrap().to_vec(), vec!["grace".to_string()]);
    }

    #[test]
    fn remove_row_returns_the_row_and_errors_out_of_range() {
        let mut table = Table::new(
            headers(),
            vec![
                vec!["ada".into(), "engineer".into()],
                vec!["grace".into(), "admiral".into()],
                vec!["alan".into(), "cryptanalyst".into()],
            ],
        );

        let removed = table.remove_row(1).unwrap();
        assert_eq!(removed, vec!["grace".to_string(), "admiral".to_string()]);
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.get(1, 0), Some("alan"));
        assert!(matches!(
            table.remove_row(2),
            Err(CoreError::NoSuchRow { row: 2, rows: 2 })
        ));
    }

    #[test]
    fn insert_row_on_an_empty_table_makes_a_zero_width_row() {
        let mut table = Table::default();
        assert!(table.is_empty());

        assert_eq!(table.insert_row(0), 0);
        assert_eq!(table.row_count(), 1);
        assert_eq!(table.column_count(), 0);
        assert!(table.row(0).unwrap().is_empty());

        assert_eq!(table.insert_row(99), 1);
        assert_eq!(table.row_count(), 2);
    }

    #[test]
    fn insert_column_on_a_zero_column_table_gives_every_row_one_cell() {
        let mut table = Table::default();
        assert_eq!(table.insert_column(99, "name".into()).index, 0);
        assert_eq!(table.column_count(), 1);
        assert_eq!(table.header(0), Some("name"));
        assert_eq!(table.row_count(), 0);

        // Rows added afterwards inherit the new width.
        table.insert_row(0);
        assert_eq!(table.row(0).unwrap().len(), 1);

        // A table whose rows predate its first column gets the cell too.
        let mut rows_first = Table::default();
        rows_first.insert_row(0);
        rows_first.insert_row(1);
        assert_eq!(rows_first.insert_column(0, "name".into()).index, 0);
        assert_eq!(rows_first.row(0).unwrap().len(), 1);
        assert_eq!(rows_first.row(1).unwrap().len(), 1);
    }

    /// A table the public constructors cannot make: rows of different widths,
    /// which is what a column insertion into ragged input leaves behind.
    fn ragged() -> Table {
        Table {
            headers: vec!["name".into(), "role".into()],
            rows: vec![
                vec!["ada".into()],
                vec!["grace".into(), "admiral".into(), "extra".into()],
                vec![],
            ],
        }
    }

    #[test]
    fn a_column_insertion_records_the_slot_each_cell_landed_in() {
        let mut table = ragged();
        let insertion = table.insert_column(1, "rank".into());

        assert_eq!(insertion.index, 1);
        assert_eq!(insertion.name, "rank");
        // Row 0 is one wide, so its cell went to slot 1; row 1 had room at 1;
        // row 2 is empty, so its cell went to the only slot there is.
        assert_eq!(insertion.slots, vec![1, 1, 0]);
    }

    #[test]
    fn undoing_a_column_insert_restores_a_ragged_table_exactly() {
        let mut table = ragged();
        let before = table.clone();

        let insertion = table.insert_column(1, "rank".into());
        table.undo_column_insert(&insertion);

        assert_eq!(table, before);
    }

    #[test]
    fn undoing_a_column_insert_at_the_end_restores_a_ragged_table_exactly() {
        let mut table = ragged();
        let before = table.clone();

        let insertion = table.insert_column(99, "rank".into());
        table.undo_column_insert(&insertion);

        assert_eq!(table, before);
    }

    #[test]
    fn restoring_a_removed_column_puts_back_only_the_rows_that_had_a_cell() {
        let mut short = ragged();
        let before = short.clone();

        let removal = short.remove_column(1).unwrap();
        assert_eq!(removal.cells, vec![None, Some("admiral".to_string()), None]);
        short.restore_column(removal).unwrap();

        assert_eq!(short, before);
    }

    #[test]
    fn restoring_a_removed_row_puts_its_cells_back_where_they_were() {
        let mut table = ragged();
        let before = table.clone();

        let cells = table.remove_row(1).unwrap();
        table.restore_row(1, cells);

        assert_eq!(table, before);
    }

    #[test]
    fn restoring_a_column_onto_a_different_shape_is_refused() {
        let mut table = ragged();
        let removal = table.remove_column(0).unwrap();
        table.remove_row(1).unwrap();

        assert!(matches!(
            table.restore_column(removal),
            Err(CoreError::StaleEdit {
                expected: 3,
                found: 2
            })
        ));
        // A refusal must not half-apply the edit.
        assert_eq!(table.column_count(), 1);
    }
}
