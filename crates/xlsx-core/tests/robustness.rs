//! The multi-sheet regression, and giving descriptors back.
//!
//! This is the failure that started the whole thing. Measured on unmodified
//! `exceljs` 4.4.0, full read, twenty runs: 20/20 at two tabs, 19/20 at four,
//! **0/20 at five**, 0/20 at eight — the archive walk stopped after the last
//! sheet, so `sharedStrings`, `styles` and `workbook` were never read, and the
//! reader threw while leaking about 180 descriptors for 30 reads.
//!
//! Reading a ZIP through its central directory, as this crate does, makes the
//! bug unexpressible. The test is here anyway: a property nobody checks is a
//! property that comes back.

mod common;

use common::fixture;
use xlsx_core::{CellValue, ReaderOptions, XlsxReader};

const RUNS: usize = 20;

fn permissive() -> ReaderOptions {
    ReaderOptions {
        max_decompressed_bytes: 1 << 30,
        max_rows: 1 << 20,
    }
}

#[test]
fn every_tab_of_every_workbook_reads_through_twenty_times_over() {
    for tabs in [2, 4, 8, 16] {
        let path = fixture(&format!("sheets-{tabs}"));

        for run in 0..RUNS {
            let mut reader = XlsxReader::open(&path, permissive()).expect("open");
            assert_eq!(reader.sheets().len(), tabs, "{tabs} tabs, run {run}");

            let names: Vec<String> = reader.sheets().iter().map(|s| s.name.clone()).collect();

            for (index, name) in names.iter().enumerate() {
                reader.select(name).expect("select");

                let mut rows = 0;
                let mut last: Option<Vec<CellValue>> = None;
                while let Some(batch) = reader.next_batch(16).expect("batch") {
                    rows += batch.len();
                    last = batch.last().cloned();
                }

                assert_eq!(rows, 31, "{tabs} tabs, run {run}, sheet {name}");
                // Each sheet stamps its own name in column A, so a read that
                // silently landed on the wrong tab would not pass here.
                assert_eq!(
                    last.expect("rows")[0],
                    CellValue::Text(format!("Sheet{}", index + 1)),
                    "{tabs} tabs, run {run}, sheet {name}"
                );
            }
        }
    }
}

#[test]
fn readers_give_their_descriptors_back() {
    // Twenty full reads of a sixteen-tab workbook, in a process of their own so
    // the count means something. `exceljs` abandoned about 180 descriptors for
    // thirty reads here.
    let report = common::measure(&[
        "fixtures/out/sheets-16.xlsx",
        "--all-sheets",
        "--repeat",
        "20",
    ]);

    assert_eq!(report.number("descriptorsLeaked"), Some(0.0), "{report:?}");
    assert_eq!(report.number("sheets"), Some(16.0), "{report:?}");
}

#[test]
fn a_reader_dropped_mid_sheet_leaves_nothing_behind() {
    // A consumer that stops half way through an import — a validation failure on
    // row three — must not hold the archive open either.
    let report = common::measure(&[
        "fixtures/out/large-200000.xlsx",
        "--repeat",
        "10",
        "--max-rows",
        "10",
    ]);

    assert_eq!(report.text("error").as_deref(), Some("ROW_BUDGET_EXCEEDED"));
    assert_eq!(report.number("descriptorsLeaked"), Some(0.0), "{report:?}");
}
