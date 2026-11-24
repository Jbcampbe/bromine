//! Contract tests over the committed fixtures.
//!
//! `fixtures/` is shared by the format engines, the UI, and QA runs, so these
//! tests pin what each file is supposed to contain. If a fixture changes, the
//! failure here says which expectation moved, rather than leaving the UI to
//! discover it at runtime.

use std::path::PathBuf;

use bromine_core::{Direction, RowView, Table, load_csv, parse_csv};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn people_csv_is_a_clean_four_column_table() {
    let document = load_csv(&fixture("people.csv")).unwrap();

    assert_eq!(document.delimiter, b',');
    assert_eq!(
        document.table.headers(),
        &["name", "role", "team", "commits"]
    );
    assert_eq!(document.table.row_count(), 6);

    // Nothing needed repairing, which is what makes this the demo file.
    assert_eq!(document.repairs.padded_rows, 0);
    assert_eq!(document.repairs.unnamed_columns, 0);
    assert_eq!(document.repairs.columns, 4);
}

#[test]
fn people_csv_sorts_commits_as_numbers() {
    let document = load_csv(&fixture("people.csv")).unwrap();
    let mut view = RowView::new(&document.table);

    view.toggle_sort(&document.table, 3);

    // 57, 88, 201, 412, 1337, 9001 - not "1337" before "201".
    assert_eq!(view.indices(), &[4, 1, 3, 0, 2, 5]);
    assert_eq!(
        document.table.get(view.table_row(0).unwrap(), 3),
        Some("57")
    );
    assert_eq!(
        view.sort().map(|key| key.direction),
        Some(Direction::Ascending)
    );
}

#[test]
fn quirky_csv_exercises_quoting_and_repair() {
    let document = load_csv(&fixture("quirky.csv")).unwrap();

    assert_eq!(document.table.get(0, 1), Some("commas, inside quotes"));
    assert_eq!(document.table.get(1, 1), Some("a doubled \"quote\" inside"));
    assert_eq!(document.table.get(2, 1), Some("two\nlines"));

    // The file's sixth line has five fields, so the table widens to five
    // columns, names the extra one, and pads every shorter row.
    assert_eq!(document.table.column_count(), 5);
    assert_eq!(document.table.headers().last().unwrap(), "column_5");
    assert_eq!(document.repairs.unnamed_columns, 1);
    assert_eq!(document.repairs.padded_rows, 5);

    // The bare `5` row keeps its value in the first column...
    assert_eq!(document.table.get(4, 0), Some("5"));
    // ...and the trailing-empty row keeps all five cells.
    assert_eq!(
        document.table.row(5).unwrap(),
        &["6", "trailing empties", "7", "", ""]
    );
}

#[test]
fn scores_tsv_is_detected_as_tab_separated() {
    let document = load_csv(&fixture("scores.tsv")).unwrap();

    assert_eq!(document.delimiter, b'\t');
    assert_eq!(document.table.headers(), &["name", "role", "team"]);
    assert_eq!(document.table.row_count(), 2);
    assert_eq!(document.table.get(1, 0), Some("Grace Hopper"));
}

#[test]
fn empty_csv_loads_as_an_empty_table_instead_of_failing() {
    let document = load_csv(&fixture("empty.csv")).unwrap();

    assert!(document.table.is_empty());
    assert_eq!(document.table.row_count(), 0);
    assert_eq!(document.table.column_count(), 0);
}

#[test]
fn every_fixture_survives_a_write_then_read_cycle() {
    for name in ["people.csv", "quirky.csv", "scores.tsv", "empty.csv"] {
        let original = load_csv(&fixture(name)).unwrap();
        let written = original.to_csv_string().unwrap();
        let reparsed = parse_csv(&written).unwrap();

        assert_eq!(
            reparsed.table, original.table,
            "{name} did not survive a write/read cycle"
        );
        assert_eq!(
            reparsed.delimiter, original.delimiter,
            "{name} changed delimiter on write"
        );
    }
}

#[test]
fn search_finds_rows_across_every_column() {
    let document = load_csv(&fixture("people.csv")).unwrap();
    let mut view = RowView::new(&document.table);

    view.set_query(&document.table, "Engineer");
    assert_eq!(view.len(), 4);

    view.set_query(&document.table, "Ops");
    assert_eq!(view.len(), 2);

    view.set_query(&document.table, "no-such-value");
    assert!(view.is_empty());
}

#[test]
fn a_table_never_reports_a_cell_outside_its_bounds() {
    let document = load_csv(&fixture("people.csv")).unwrap();
    let table: &Table = &document.table;

    assert_eq!(table.get(6, 0), None);
    assert_eq!(table.get(0, 4), None);
    assert_eq!(table.get(99, 99), None);
}
