//! What the reader must refuse, and how fast.
//!
//! Every hostile fixture here carries a *single row of data*: what blows up is
//! the phase a reader runs before it hands back its first row. That is the whole
//! argument for putting the bounds inside the reader rather than above it.

mod common;

use common::fixture;
use xlsx_core::{ReadError, ReaderOptions, XlsxReader};

const EIGHT_MIB: u64 = 8 * 1024 * 1024;

fn budget(max_decompressed_bytes: u64, max_rows: u64) -> ReaderOptions {
    ReaderOptions {
        max_decompressed_bytes,
        max_rows,
    }
}

#[test]
fn every_hostile_archive_is_refused_at_the_part_that_blows_up() {
    // The archive member named in the error is the one being inflated when the
    // budget ran out: a caller can tell a shared-string bomb from a style bomb
    // without opening the file again.
    let cases = [
        ("bomb-sharedstrings", "xl/sharedStrings.xml"),
        ("bomb-styles", "xl/styles.xml"),
        ("bomb-rels", "xl/_rels/workbook.xml.rels"),
        ("bomb-workbook", "xl/workbook.xml"),
        ("bomb-inline", "xl/worksheets/sheet1.xml"),
    ];

    for (name, expected_entry) in cases {
        let error = XlsxReader::open(&fixture(name), budget(EIGHT_MIB, 1_000_000))
            .expect_err("must refuse");

        assert_eq!(
            error,
            ReadError::DecompressedBudgetExceeded {
                limit: EIGHT_MIB,
                entry: expected_entry.into(),
            },
            "{name}"
        );
        assert_eq!(error.code(), "DECOMPRESSED_BUDGET_EXCEEDED");
    }
}

#[test]
fn an_archive_that_lies_about_its_sizes_is_refused_on_its_real_expansion() {
    let path = fixture("lying-sizes");

    // The lie, stated: the central directory announces 64 bytes for a part that
    // deploys 64 MB. A reader bounding on declared sizes waves this through.
    let file = std::fs::File::open(&path).expect("open");
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).expect("archive");
    let declared = archive
        .by_name("xl/sharedStrings.xml")
        .expect("part")
        .size();
    assert_eq!(declared, 64);

    let error = XlsxReader::open(&path, budget(EIGHT_MIB, 1_000_000)).expect_err("must refuse");

    assert_eq!(
        error,
        ReadError::DecompressedBudgetExceeded {
            limit: EIGHT_MIB,
            entry: "xl/sharedStrings.xml".into(),
        }
    );
}

#[test]
fn a_cell_reference_outside_the_grid_is_refused() {
    // Amplification through geometry rather than through bytes. `calamine`
    // builds a column index by accumulating `col * 26 + letter` with no
    // ceiling, so `r="BZZZZZ1"` names column 36 119 382 — and neither budget
    // sees it coming: a cell reference weighs six bytes through the inflater,
    // and it is one row. Left alone, this 1 969 byte archive reached 1.13 GB.
    let mut reader =
        XlsxReader::open(&fixture("bomb-column-ref"), budget(1 << 20, 5)).expect("open");

    let error = reader.next_batch(1_000).expect_err("must refuse");

    assert_eq!(error.code(), "CORRUPT");
    assert!(
        error.to_string().contains("36119382"),
        "the error must name the column that was asked for: {error}"
    );
}

#[test]
fn rows_padded_out_to_a_far_column_are_charged_against_the_byte_budget() {
    // `XFD` is a real column, so nothing here is outside the format: a thousand
    // rows padded to sixteen thousand columns is half a gigabyte of values for
    // ten kilobytes of upload. The blanks a sheet implies are charged like the
    // bytes it inflates, because they are the same thing.
    let mut reader =
        XlsxReader::open(&fixture("bomb-wide-rows"), budget(1 << 20, 5_000)).expect("open");

    let error = reader.next_batch(1_000).expect_err("must refuse");

    assert_eq!(error.code(), "DECOMPRESSED_BUDGET_EXCEEDED");
    assert!(
        error.to_string().contains("blank cells"),
        "the error must say where the expansion came from: {error}"
    );
}

#[test]
fn a_batch_comes_back_short_rather_than_holding_a_wide_sheet_whole() {
    // Same workbook, with a budget that authorises the expansion. The rows are
    // legitimately 16 384 wide, so `next_batch(1000)` would hand back sixteen
    // million values — half a gigabyte — in one go. It comes back short instead.
    let mut reader =
        XlsxReader::open(&fixture("bomb-wide-rows"), budget(1 << 30, 5_000)).expect("open");

    let batch = reader.next_batch(1_000).expect("batch").expect("rows");

    assert!(
        batch.len() < 1_000,
        "batch was not cut: {} rows",
        batch.len()
    );
    assert!(
        batch.len() * batch[0].len() <= 1 << 20,
        "batch holds {} values",
        batch.len() * batch[0].len()
    );

    // And the rest of the sheet still comes through, batch after batch.
    let mut rows = batch.len();
    while let Some(more) = reader.next_batch(1_000).expect("batch") {
        rows += more.len();
    }
    assert_eq!(rows, 1_000);
}

#[test]
fn declared_xml_entities_are_not_expanded() {
    // Neither of these is defended against here: `quick-xml` resolves the five
    // predefined entities and nothing else, so a declared one is an unknown
    // name rather than a substitution. That makes it an inherited property, and
    // an inherited property is one a version bump can take away without saying
    // so — hence the test.
    for name in ["xxe-billion-laughs", "xxe-external-entity"] {
        let mut reader = XlsxReader::open(&fixture(name), budget(1 << 30, 100)).expect("open");

        let error = reader.next_batch(100).expect_err("must refuse");

        assert_eq!(error.code(), "CORRUPT", "{name}");
        assert!(error.to_string().contains("entity"), "{name}: {error}");
    }
}

#[test]
fn an_archive_of_countless_tiny_entries_is_refused_before_it_is_opened() {
    // Not a byte bomb but a directory bomb: 65 000 entries of one byte each cost
    // nothing to inflate and a great deal to index — and the indexing happens
    // inside the ZIP reader, before any byte budget has anything to say. Left
    // alone, this 14 MB archive reaches 57 MB of RSS without reading a row.
    let error =
        XlsxReader::open(&fixture("bomb-entries"), budget(EIGHT_MIB, 1_000)).expect_err("refuse");

    assert_eq!(
        error,
        ReadError::TooManyEntries {
            // The byte budget is what sets this: an entry costs about a
            // kilobyte to index, so eight mebibytes buys eight thousand.
            limit: EIGHT_MIB / 1024,
            count: 65_006,
        }
    );
    assert_eq!(error.code(), "TOO_MANY_ENTRIES");
}

#[test]
fn appending_junk_to_an_archive_does_not_switch_the_entry_bound_off() {
    // `zip` accepts a trailer whose comment stops short of the end of the file,
    // deliberately, for "garbage-after-comment" archives. A bound that insisted
    // on the comment ending exactly on the last byte would find no trailer, and
    // a reader that treats "no trailer" as "nothing to check" hands the attacker
    // the bound for five arbitrary bytes.
    let source = fixture("bomb-entries");
    let mut bytes = std::fs::read(&source).expect("read the fixture");
    bytes.extend_from_slice(b"JUNK!");

    let tampered = std::env::temp_dir().join("xlsx-core-bomb-entries-with-junk.xlsx");
    std::fs::write(&tampered, &bytes).expect("write the tampered copy");

    let error = XlsxReader::open(&tampered, budget(EIGHT_MIB, 1_000)).expect_err("refuse");
    assert_eq!(error.code(), "TOO_MANY_ENTRIES", "{error}");

    std::fs::remove_file(&tampered).ok();
}

#[test]
fn a_workbook_with_an_ordinary_number_of_parts_is_not_caught_by_that() {
    // The bound must not fire on a real workbook. sheets-16 carries twenty
    // parts, and the tightest budget any of these tests uses allows eight
    // thousand.
    XlsxReader::open(&fixture("sheets-16"), budget(EIGHT_MIB, 1_000)).expect("open");
}

#[test]
fn an_archive_within_budget_is_accepted_and_measured() {
    let reader = XlsxReader::open(&fixture("types"), budget(EIGHT_MIB, 1_000)).expect("open");

    assert!(reader.decompressed_bytes() > 0);
    assert!(reader.decompressed_bytes() < EIGHT_MIB);
}

#[test]
fn the_row_budget_cuts_on_the_row_that_crosses_it() {
    // sheets-2 holds exactly 31 rows: one header plus thirty.
    let mut reader = XlsxReader::open(&fixture("sheets-2"), budget(EIGHT_MIB, 31)).expect("open");
    let mut rows = 0;
    while let Some(batch) = reader.next_batch(8).expect("batch") {
        rows += batch.len();
    }
    assert_eq!(
        rows, 31,
        "a sheet of exactly max_rows rows must read through"
    );

    let mut reader = XlsxReader::open(&fixture("sheets-2"), budget(EIGHT_MIB, 30)).expect("open");
    let mut rows = 0;
    let error = loop {
        match reader.next_batch(8) {
            Ok(Some(batch)) => rows += batch.len(),
            Ok(None) => panic!("the sheet must not read through"),
            Err(error) => break error,
        }
    };

    assert_eq!(error, ReadError::RowBudgetExceeded { limit: 30 });
    assert_eq!(error.code(), "ROW_BUDGET_EXCEEDED");
    // Cut on the row that crossed the budget, not at the end of the sheet: the
    // rows already handed over are the ones that fitted.
    assert_eq!(rows, 24, "batches of 8: three full ones, then the refusal");
}

#[test]
fn the_row_budget_does_not_wait_for_the_end_of_a_large_sheet() {
    let mut reader = XlsxReader::open(&fixture("large-200000"), budget(1 << 30, 10)).expect("open");

    // Timed from the first pull, not from the open: opening charges the
    // decompressed-byte budget over the whole archive, which is its own cost.
    let started = std::time::Instant::now();
    let error = reader.next_batch(1_000).expect_err("must refuse");
    let elapsed = started.elapsed();

    assert_eq!(error, ReadError::RowBudgetExceeded { limit: 10 });

    // Reading these 200 000 rows through takes the best part of a second. The
    // refusal lands on the eleventh row, so it must cost nothing measurable.
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "refusal took {elapsed:?}, which means it read the sheet"
    );
}

#[test]
fn selecting_another_sheet_gives_that_sheet_its_own_row_budget() {
    let mut reader = XlsxReader::open(&fixture("sheets-4"), budget(EIGHT_MIB, 31)).expect("open");

    for sheet in ["Sheet1", "Sheet2", "Sheet3"] {
        reader.select(sheet).expect("select");
        let mut rows = 0;
        while let Some(batch) = reader.next_batch(64).expect("batch") {
            rows += batch.len();
        }
        assert_eq!(rows, 31, "{sheet}");
    }
}
