//! What the cell reader must get right: every declared cell, typed, with its
//! formula and its style index, at its own position.

mod common;

use common::fixture;
use xlsx_core::{Cell, CellReaderOptions, CellType, CellValue, Formula, FormulaType, XlsxCells};

fn permissive() -> CellReaderOptions {
    CellReaderOptions {
        max_decompressed_bytes: 1 << 30,
        max_cells: 1 << 20,
    }
}

fn read_cells(name: &str) -> Vec<Cell> {
    let mut reader = XlsxCells::open(&fixture(name), permissive()).expect("open");
    let mut cells = Vec::new();
    while let Some(batch) = reader.next_batch(5).expect("batch") {
        cells.extend(batch);
    }
    cells
}

fn at(cells: &[Cell], row: u32, col: u32) -> &Cell {
    cells
        .iter()
        .find(|cell| cell.row == row && cell.col == col)
        .unwrap_or_else(|| panic!("no cell at ({row}, {col})"))
}

#[test]
fn emits_every_cell_the_file_declares_with_its_position() {
    let positions: Vec<(u32, u32)> = read_cells("formulas")
        .iter()
        .map(|cell| (cell.row, cell.col))
        .collect();

    assert_eq!(
        positions,
        vec![
            (0, 0),
            (0, 1),
            (0, 2),
            (1, 0),
            (1, 1),
            (1, 2),
            (2, 0),
            (2, 1),
            (2, 2),
            (3, 3),
            (4, 3),
            (5, 4),
            (5, 5),
            (5, 6),
            // Row 7 declares no `r` on its cells: they follow one another.
            (6, 0),
            (6, 1),
            (6, 2),
        ]
    );
}

#[test]
fn emits_a_cell_without_a_value_that_only_carries_a_style_as_empty() {
    let cells = read_cells("formulas");

    let styled = at(&cells, 5, 4);
    assert_eq!(styled.cell_type, CellType::Empty);
    assert_eq!(styled.value, CellValue::Empty);
    assert_eq!(styled.style_id, 1);

    let bare = at(&cells, 5, 5);
    assert_eq!(bare.cell_type, CellType::Empty);
    assert_eq!(bare.style_id, 0);
}

#[test]
fn tells_apart_a_date_an_error_and_a_string_the_value_alone_would_confuse() {
    let cells = read_cells("types");
    let types: Vec<CellType> = cells.iter().map(|cell| cell.cell_type).collect();

    assert_eq!(
        types,
        vec![
            CellType::String,
            CellType::String,
            CellType::Number,
            CellType::Number,
            CellType::Boolean,
            CellType::Boolean,
            CellType::Error,
            CellType::Date,
            CellType::Date,
            CellType::Number,
            CellType::Duration,
            CellType::Empty,
        ]
    );
    // All three arrive as text; only the type tells them apart.
    assert_eq!(cells[6].value, CellValue::Error("#DIV/0!".into()));
    assert_eq!(
        cells[7].value,
        CellValue::DateTime("2024-03-25T00:00:00.000Z".into())
    );
}

#[test]
fn restores_the_text_of_a_formula_without_the_equals_sign() {
    let cells = read_cells("types");

    assert_eq!(
        cells[9].formula,
        Some(Formula {
            text: "C1*2".into(),
            formula_type: FormulaType::Normal,
            reference: None,
        })
    );
}

#[test]
fn expands_a_shared_formula_on_every_derived_cell_and_keeps_its_shared_type() {
    let cells = read_cells("formulas");

    assert_eq!(
        at(&cells, 0, 2).formula,
        Some(Formula {
            text: "A1*B1".into(),
            formula_type: FormulaType::Shared,
            reference: Some("C1:C3".into()),
        })
    );
    for (row, text) in [(1, "A2*B2"), (2, "A3*B3")] {
        assert_eq!(
            at(&cells, row, 2).formula,
            Some(Formula {
                text: text.into(),
                formula_type: FormulaType::Shared,
                reference: None,
            }),
            "row {row}"
        );
    }
}

#[test]
fn restores_the_range_of_an_array_formula_on_its_master_cell() {
    let cells = read_cells("formulas");

    assert_eq!(
        at(&cells, 3, 3).formula,
        Some(Formula {
            text: "A1:A2*2".into(),
            formula_type: FormulaType::Array,
            reference: Some("D4:D5".into()),
        })
    );
    // The other cells of the array carry a value and no formula of their own.
    assert_eq!(at(&cells, 4, 3).formula, None);
    assert_eq!(at(&cells, 4, 3).value, CellValue::Number(8.0));
}

#[test]
fn restores_the_cached_value_of_a_formula_without_recalculating_it() {
    let cells = read_cells("formulas");

    assert_eq!(at(&cells, 2, 2).value, CellValue::Number(42.0));

    let uncached = &read_cells("types")[11];
    assert_eq!(uncached.cell_type, CellType::Empty);
    assert!(uncached.formula.is_some());
}

#[test]
fn every_cell_carries_its_style_index_zero_when_it_declares_none() {
    let cells = read_cells("formulas");

    assert_eq!(at(&cells, 0, 0).style_id, 0);
    assert_eq!(at(&cells, 5, 6).style_id, 2);
    assert_eq!(at(&cells, 5, 6).cell_type, CellType::Date);
}

#[test]
fn the_style_index_stays_on_its_cell_when_cells_do_not_say_where_they_sit() {
    let cells = read_cells("formulas");

    assert_eq!(at(&cells, 6, 0).style_id, 0);
    assert_eq!(at(&cells, 6, 1).style_id, 1);
    assert_eq!(at(&cells, 6, 1).cell_type, CellType::Empty);
    assert_eq!(at(&cells, 6, 2).value, CellValue::Number(3.0));
}

#[test]
fn selects_a_sheet_by_name_and_reads_it_from_the_top() {
    let mut reader = XlsxCells::open(&fixture("sheets-4"), permissive()).expect("open");

    reader.select("Sheet3").expect("select");
    let first = reader.next_batch(3).expect("batch").expect("cells");
    reader.select("Sheet3").expect("select again");
    let again = reader.next_batch(3).expect("batch").expect("cells");

    assert_eq!(first, again);
    assert_eq!((first[0].row, first[0].col), (0, 0));
}

#[test]
fn finds_the_sheet_parts_when_entries_use_backslashes_and_another_case() {
    // Both readings of the sheet — calamine's and the attribute pass — have to
    // find `xl\worksheets\sheet2.xml` from the target `worksheets/sheet2.xml`.
    let mut reader = XlsxCells::open(&fixture("backslash-entries"), permissive()).expect("open");

    reader.select("Notes").expect("select");
    let cells = reader.next_batch(100).expect("batch").expect("cells");

    assert_eq!(
        cells.len(),
        8,
        "a header row and three data rows, two cells each"
    );
    assert_eq!(cells[2].value, CellValue::Text("Sheet2".into()));
}
