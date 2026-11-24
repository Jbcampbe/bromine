use std::cmp::Ordering;

use crate::Table;

/// Which way a column is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

impl Direction {
    pub fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

/// The column a view is ordered by, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub column: usize,
    pub direction: Direction,
}

/// A derived ordering and filter over a [`Table`].
///
/// The view holds row *indices*, not rows. Sorting and searching therefore
/// never move data, so a cell edit always targets a stable absolute row index
/// and a re-sort cannot lose an in-flight edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowView {
    indices: Vec<usize>,
    sort: Option<SortKey>,
    query: String,
}

impl RowView {
    /// A view of every row, in file order.
    pub fn new(table: &Table) -> Self {
        let mut view = Self::default();
        view.rebuild(table);
        view
    }

    /// Recompute the visible indices from the table's current contents.
    ///
    /// Call this after any mutation that could change ordering or which rows
    /// match the query. It is cheap enough for files with tens of thousands of
    /// rows; a larger file wants an incremental index instead.
    pub fn rebuild(&mut self, table: &Table) {
        let needle = self.query.trim().to_lowercase();

        self.indices = (0..table.row_count())
            .filter(|&row| needle.is_empty() || row_matches(table, row, &needle))
            .collect();

        if let Some(key) = self.sort {
            let column = key.column;
            self.indices.sort_by(|&left, &right| {
                let ordering = compare_values(
                    table.get(left, column).unwrap_or(""),
                    table.get(right, column).unwrap_or(""),
                );
                match key.direction {
                    Direction::Ascending => ordering,
                    Direction::Descending => ordering.reverse(),
                }
            });
        }
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Visible row indices, in display order.
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// Translate a display position into an absolute table row.
    pub fn table_row(&self, position: usize) -> Option<usize> {
        self.indices.get(position).copied()
    }

    /// Translate an absolute table row into a display position, if visible.
    pub fn position_of(&self, table_row: usize) -> Option<usize> {
        self.indices.iter().position(|&row| row == table_row)
    }

    pub fn sort(&self) -> Option<SortKey> {
        self.sort
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_sort(&mut self, table: &Table, sort: Option<SortKey>) {
        self.sort = sort;
        self.rebuild(table);
    }

    /// Sort by `column`, or flip the direction when it is already the sort key.
    /// This is the behaviour a spreadsheet header click expects.
    pub fn toggle_sort(&mut self, table: &Table, column: usize) {
        let direction = match self.sort {
            Some(key) if key.column == column => key.direction.toggled(),
            _ => Direction::Ascending,
        };
        self.set_sort(table, Some(SortKey { column, direction }));
    }

    pub fn set_query(&mut self, table: &Table, query: impl Into<String>) {
        self.query = query.into();
        self.rebuild(table);
    }

    pub fn clear_query(&mut self, table: &Table) {
        self.set_query(table, String::new());
    }
}

fn row_matches(table: &Table, row: usize, lowercase_needle: &str) -> bool {
    let Some(cells) = table.row(row) else {
        return false;
    };
    cells
        .iter()
        .any(|cell| cell.to_lowercase().contains(lowercase_needle))
}

/// Compare two cells the way a spreadsheet would.
///
/// Two numeric values compare as numbers, so `9` sorts before `10`. Anything
/// else compares case-insensitively as text. Empty cells are ordinary empty
/// strings and therefore sort before any non-empty text.
pub fn compare_values(left: &str, right: &str) -> Ordering {
    match (parse_number(left), parse_number(right)) {
        (Some(left_number), Some(right_number)) => left_number
            .partial_cmp(&right_number)
            .unwrap_or(Ordering::Equal),
        _ => left.to_lowercase().cmp(&right.to_lowercase()),
    }
}

fn parse_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f64>().ok().filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table::new(
            vec!["name".into(), "score".into()],
            vec![
                vec!["carol".into(), "10".into()],
                vec!["alice".into(), "9".into()],
                vec!["bob".into(), "".into()],
                vec!["dave".into(), "100".into()],
            ],
        )
    }

    #[test]
    fn an_unsorted_view_is_file_order() {
        let table = table();
        let view = RowView::new(&table);
        assert_eq!(view.indices(), &[0, 1, 2, 3]);
        assert_eq!(view.table_row(2), Some(2));
        assert_eq!(view.position_of(2), Some(2));
    }

    #[test]
    fn numeric_columns_sort_by_value_not_by_text() {
        let mut table = table();
        let mut view = RowView::new(&table);
        view.toggle_sort(&table, 1);

        // 9, 10, 100 numerically; the empty cell sorts first.
        assert_eq!(view.indices(), &[2, 1, 0, 3]);

        table.set(2, 1, "50").unwrap();
        view.rebuild(&table);
        assert_eq!(view.indices(), &[1, 0, 2, 3]);
    }

    #[test]
    fn toggling_the_same_column_reverses_it() {
        let table = table();
        let mut view = RowView::new(&table);

        view.toggle_sort(&table, 0);
        assert_eq!(view.indices(), &[1, 2, 0, 3]);

        view.toggle_sort(&table, 0);
        assert_eq!(view.indices(), &[3, 0, 2, 1]);

        view.toggle_sort(&table, 1);
        assert_eq!(
            view.sort(),
            Some(SortKey {
                column: 1,
                direction: Direction::Ascending
            })
        );
    }

    #[test]
    fn sorting_is_stable_for_equal_values() {
        let table = Table::new(
            vec!["k".into(), "v".into()],
            vec![
                vec!["same".into(), "first".into()],
                vec!["same".into(), "second".into()],
                vec!["same".into(), "third".into()],
            ],
        );
        let mut view = RowView::new(&table);
        view.toggle_sort(&table, 0);

        assert_eq!(view.indices(), &[0, 1, 2]);
    }

    #[test]
    fn query_filters_across_every_cell_case_insensitively() {
        let table = table();
        let mut view = RowView::new(&table);

        view.set_query(&table, "AL");
        assert_eq!(view.indices(), &[1]);

        // "10" matches carol's score, and "100" matches dave's.
        view.set_query(&table, "10");
        assert_eq!(view.indices(), &[0, 3]);

        view.clear_query(&table);
        assert_eq!(view.len(), 4);
    }

    #[test]
    fn sort_and_query_compose() {
        let table = table();
        let mut view = RowView::new(&table);

        view.set_query(&table, "o");
        view.toggle_sort(&table, 0);
        // Only bob and carol contain an "o"; ordering them by name gives bob
        // first, at display position 0, while carol's file row 0 is at 1.
        assert_eq!(view.indices(), &[2, 0]);
        assert_eq!(view.len(), 2);
        assert_eq!(view.position_of(0), Some(1));
        assert_eq!(view.position_of(3), None);
    }

    #[test]
    fn compare_values_prefers_numbers() {
        assert_eq!(compare_values("9", "10"), Ordering::Less);
        assert_eq!(compare_values("9", "9.0"), Ordering::Equal);
        assert_eq!(compare_values(" 42 ", "7"), Ordering::Greater);
        assert_eq!(compare_values("Alpha", "alpha"), Ordering::Equal);
    }

    #[test]
    fn compare_values_falls_back_to_case_insensitive_text() {
        // "10x" is not a number, so this is a text comparison and '1' < '9'.
        assert_eq!(compare_values("10x", "9"), Ordering::Less);
        assert_eq!(compare_values("abc", "abd"), Ordering::Less);
        // An empty cell is ordinary empty text, so it sorts before "0".
        assert_eq!(compare_values("", "0"), Ordering::Less);
        // "NaN" and "inf" are not usable numbers either.
        assert_eq!(compare_values("NaN", "inf"), Ordering::Greater);
    }
}
