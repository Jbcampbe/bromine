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

    pub fn remove_row(&mut self, row: usize) -> Result<Vec<String>, CoreError> {
        if row >= self.rows.len() {
            return Err(CoreError::NoSuchRow {
                row,
                rows: self.rows.len(),
            });
        }
        Ok(self.rows.remove(row))
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
}
