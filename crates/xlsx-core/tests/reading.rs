//! What the reader must get right about the format itself.

mod common;

use common::fixture;
use xlsx_core::{CellValue, ReadError, ReaderOptions, XlsxReader};

/// Generous bounds: these tests are about reading, not about refusing.
fn permissive() -> ReaderOptions {
    ReaderOptions {
        max_decompressed_bytes: 1 << 30,
        max_rows: 1 << 20,
    }
}

fn read_all(reader: &mut XlsxReader) -> Vec<Vec<CellValue>> {
    let mut rows = Vec::new();
    while let Some(batch) = reader.next_batch(128).expect("batch") {
        rows.extend(batch);
    }
    rows
}

#[test]
fn lists_sheets_with_their_visibility() {
    let reader = XlsxReader::open(&fixture("hidden-sheets"), permissive()).expect("open");

    let sheets: Vec<(&str, bool)> = reader
        .sheets()
        .iter()
        .map(|s| (s.name.as_str(), s.visible))
        .collect();

    assert_eq!(
        sheets,
        vec![
            ("Data", true),
            ("Paramètres", false),
            ("Notes", false), // veryHidden is hidden too
        ]
    );
}

#[test]
fn listing_sheets_agrees_with_opening_the_workbook() {
    // Listing reads two small parts; opening walks the whole archive. Two paths
    // through the same format is two chances to disagree about what a sheet is,
    // so they are held to the same answer on every workbook in the repository.
    for name in [
        "sheets-2",
        "sheets-4",
        "sheets-8",
        "sheets-16",
        "hidden-sheets",
        "types",
        "sparse",
        "dates-1904",
        "large-200000",
    ] {
        let path = fixture(name);

        let opened = XlsxReader::open(&path, permissive()).expect("open");
        let listed =
            XlsxReader::list_sheets(&path, permissive().max_decompressed_bytes).expect("list");

        assert_eq!(listed, opened.sheets(), "{name}");
    }
}

#[test]
fn listing_sheets_is_bounded_like_everything_else() {
    for name in ["bomb-workbook", "bomb-package-rels"] {
        let error =
            XlsxReader::list_sheets(&fixture(name), 8 * 1024 * 1024).expect_err("must refuse");
        assert_eq!(error.code(), "DECOMPRESSED_BUDGET_EXCEEDED", "{name}");
    }

    let error = XlsxReader::list_sheets(&fixture("bomb-entries"), 8 * 1024 * 1024)
        .expect_err("must refuse");
    assert_eq!(error.code(), "TOO_MANY_ENTRIES");

    let error =
        XlsxReader::list_sheets(&fixture("not-an-archive"), 1 << 20).expect_err("must refuse");
    assert_eq!(error.code(), "NOT_AN_ARCHIVE");
}

#[test]
fn listing_sheets_does_not_read_the_sheets() {
    // The point of the separate path, stated as a duration: a listing must not
    // cost a walk of the archive. Opening the same workbook inflates 204 MB.
    let path = fixture("large-600000");

    // Warm the page cache so this measures inflating, not reading from disk.
    let _ = std::fs::read(&path);

    let started = std::time::Instant::now();
    let sheets = XlsxReader::list_sheets(&path, 1 << 30).expect("list");
    let elapsed = started.elapsed();

    assert_eq!(sheets.len(), 1);
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "listing took {elapsed:?}, which means it read the sheets"
    );
}

#[test]
fn selects_a_sheet_by_name() {
    let mut reader = XlsxReader::open(&fixture("sheets-8"), permissive()).expect("open");

    reader.select("Sheet6").expect("select");
    let rows = read_all(&mut reader);

    assert_eq!(rows.len(), 31); // one header, thirty data rows
    assert_eq!(rows[1][0], CellValue::Text("Sheet6".into()));
}

#[test]
fn an_unknown_sheet_names_the_alternatives() {
    let mut reader = XlsxReader::open(&fixture("sheets-2"), permissive()).expect("open");

    let error = reader
        .select("Feuille inexistante")
        .expect_err("must refuse");

    assert_eq!(
        error,
        ReadError::SheetNotFound {
            name: "Feuille inexistante".into(),
            available: vec!["Sheet1".into(), "Sheet2".into()],
        }
    );
    assert_eq!(error.code(), "SHEET_NOT_FOUND");
}

#[test]
fn without_a_selection_the_first_sheet_is_read() {
    let mut reader = XlsxReader::open(&fixture("sheets-4"), permissive()).expect("open");

    let rows = read_all(&mut reader);

    assert_eq!(rows[1][0], CellValue::Text("Sheet1".into()));
}

#[test]
fn types_every_kind_of_cell() {
    let mut reader = XlsxReader::open(&fixture("types"), permissive()).expect("open");
    let rows = read_all(&mut reader);

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0],
        vec![
            CellValue::Text("shared text".into()),
            CellValue::Text("inline & escaped".into()),
            CellValue::Number(42.0),
            CellValue::Number(3.5),
            CellValue::Bool(true),
            CellValue::Bool(false),
            CellValue::Error("#DIV/0!".into()),
            // Same serial as column C would hold, told apart only by its numFmt.
            CellValue::DateTime("2024-03-25T00:00:00.000Z".into()),
            CellValue::DateTime("2024-03-25T12:00:00.000Z".into()),
            // A formula cell yields its cached result, not its formula.
            CellValue::Number(84.0),
            // `[h]:mm:ss` is an elapsed time, not a point in one. Reporting it
            // as a datetime would put it on some arbitrary day, so it comes back
            // as an ISO 8601 duration — and hours are not wrapped at 24.
            CellValue::Text("PT30H0M0S".into()),
            // A formula with no cached result. Formulas are never recalculated,
            // so there is nothing to hand back.
            CellValue::Empty,
        ]
    );
}

#[test]
fn the_1904_date_system_lands_on_the_same_day() {
    let mut reader = XlsxReader::open(&fixture("dates-1904"), permissive()).expect("open");
    let rows = read_all(&mut reader);

    assert_eq!(
        rows[0],
        vec![
            CellValue::DateTime("2024-03-25T00:00:00.000Z".into()),
            CellValue::DateTime("2024-03-25T12:00:00.000Z".into()),
        ]
    );
}

#[test]
fn sparse_rows_keep_their_column_index() {
    for name in ["sparse", "sparse-nodim"] {
        let mut reader = XlsxReader::open(&fixture(name), permissive()).expect("open");
        let rows = read_all(&mut reader);

        // Row 4 is absent from the sheet and is not materialised.
        assert_eq!(rows.len(), 4, "{name}");

        // A hole in the middle leaves a blank where the cell would have been.
        assert_eq!(
            rows[1],
            vec![
                CellValue::Number(5.0),
                CellValue::Empty,
                CellValue::Number(7.0),
                CellValue::Number(8.0),
            ],
            "{name}"
        );

        // Holes at the end are padded out to the widest row seen, not truncated.
        assert_eq!(
            rows[2],
            vec![
                CellValue::Number(9.0),
                CellValue::Number(10.0),
                CellValue::Empty,
                CellValue::Empty,
            ],
            "{name}"
        );

        // A row starting mid-sheet is padded on both sides.
        assert_eq!(
            rows[3],
            vec![
                CellValue::Empty,
                CellValue::Empty,
                CellValue::Number(11.0),
                CellValue::Empty,
            ],
            "{name}"
        );
    }
}

#[test]
fn a_row_closed_before_the_sheet_widens_keeps_the_width_it_had() {
    // The documented rule, pinned: rows are padded to the widest row seen *so
    // far*, and a streaming reader cannot go back and widen one it has already
    // handed over. Row 1 carries only A1 and comes out length 1; row 2 carries
    // A to D and sets the width from there on.
    let mut reader = XlsxReader::open(&fixture("narrow-first-row"), permissive()).expect("open");
    let rows = read_all(&mut reader);

    assert_eq!(rows[0], vec![CellValue::Number(1.0)]);
    assert_eq!(
        rows[1],
        vec![
            CellValue::Number(2.0),
            CellValue::Number(3.0),
            CellValue::Number(4.0),
            CellValue::Number(5.0),
        ]
    );
    assert_eq!(
        rows[2],
        vec![
            CellValue::Number(6.0),
            CellValue::Empty,
            CellValue::Empty,
            CellValue::Empty,
        ]
    );
}

#[test]
fn batches_never_exceed_the_size_asked_for() {
    let mut reader = XlsxReader::open(&fixture("sheets-2"), permissive()).expect("open");

    let mut batches = Vec::new();
    while let Some(batch) = reader.next_batch(7).expect("batch") {
        assert!(batch.len() <= 7);
        batches.push(batch.len());
    }

    // 31 rows in batches of 7: four full ones and a remainder.
    assert_eq!(batches, vec![7, 7, 7, 7, 3]);
}

#[test]
fn selecting_again_rewinds_to_the_top() {
    let mut reader = XlsxReader::open(&fixture("sheets-4"), permissive()).expect("open");

    reader.select("Sheet2").expect("select");
    let first = reader.next_batch(4).expect("batch").expect("rows");

    reader.select("Sheet2").expect("re-select");
    let again = reader.next_batch(4).expect("batch").expect("rows");

    assert_eq!(first, again);
}

#[test]
fn a_file_that_is_not_an_archive_is_refused_as_such() {
    let error =
        XlsxReader::open(&fixture("not-an-archive"), permissive()).expect_err("must refuse");

    assert_eq!(error, ReadError::NotAnArchive);
    assert_eq!(error.code(), "NOT_AN_ARCHIVE");
}

#[test]
fn a_missing_file_is_not_reported_as_a_bad_upload() {
    let error = XlsxReader::open(
        &common::repo_root().join("fixtures/out/il-n-y-a-rien-ici.xlsx"),
        permissive(),
    )
    .expect_err("must refuse");

    assert_eq!(error.code(), "IO");
}
