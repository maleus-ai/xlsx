//! The memory claims, measured.
//!
//! Reference points, all taken on one machine, one process per measurement,
//! RSS sampled every 5 ms, on the same 600 000 × 10 workbook:
//!
//! | reader                | 200k           | 600k            |
//! | --------------------- | -------------- | --------------- |
//! | `exceljs` 4.4.0       | 249 MB · 5.6 s | 292 MB · 18.7 s |
//! | `xlsx-stream-reader`  |  99 MB · 19.6s | 112 MB · 55.9 s |
//! | `read-excel-file`     | 146 MB · 4.9 s | `RangeError`    |
//!
//! The bar this crate is held to is 150 MB, set just under the only JavaScript
//! reader that genuinely streams.
//!
//! Every figure here is produced by a child process, because peak RSS is a
//! property of a process and `cargo test` runs its tests as threads of one.

mod common;

use common::{fixture, measure};

/// Peak RSS allowed for a full read, in kilobytes.
const RSS_CEILING_KB: f64 = 150.0 * 1024.0;

/// Peak RSS allowed for an archive that is refused. Nothing is parsed and
/// nothing is retained, so this is the process itself plus one inflate buffer.
const REFUSAL_CEILING_KB: f64 = 32.0 * 1024.0;

fn peak_kb(report: &common::Report) -> f64 {
    match report.number("peakRssKb") {
        Some(kb) => kb,
        // Only Linux exposes a high-water mark without a dependency. Elsewhere
        // the assertion has nothing to stand on and must not pretend otherwise.
        None => {
            eprintln!("peak RSS unavailable on this platform; assertion skipped");
            0.0
        }
    }
}

#[test]
fn six_hundred_thousand_rows_read_well_under_the_ceiling() {
    fixture("large-600000");
    let report = measure(&[
        "fixtures/out/large-600000.xlsx",
        "--max-bytes",
        "1073741824",
        "--max-rows",
        "1000000",
    ]);

    assert_eq!(report.number("rows"), Some(600_001.0), "{report:?}");
    assert!(
        peak_kb(&report) < RSS_CEILING_KB,
        "peak RSS {} kB over 600 000 rows: {report:?}",
        peak_kb(&report)
    );
}

#[test]
fn the_peak_does_not_follow_the_number_of_rows() {
    fixture("large-200000");
    fixture("large-600000");

    let small = measure(&[
        "fixtures/out/large-200000.xlsx",
        "--max-bytes",
        "1073741824",
        "--max-rows",
        "1000000",
    ]);
    let large = measure(&[
        "fixtures/out/large-600000.xlsx",
        "--max-bytes",
        "1073741824",
        "--max-rows",
        "1000000",
    ]);

    let (small_kb, large_kb) = (peak_kb(&small), peak_kb(&large));

    // Three times the rows. A buffered reader triples with them; a streaming one
    // does not move. The bar is deliberately loose — what is being tested is the
    // shape of the curve, not a constant.
    assert!(
        large_kb < small_kb * 3.0,
        "200k peaked at {small_kb} kB and 600k at {large_kb} kB, which follows the row count"
    );
}

#[test]
fn hostile_archives_are_refused_at_a_flat_peak() {
    let cases = [
        "bomb-sharedstrings",
        "bomb-styles",
        "bomb-rels",
        "bomb-workbook",
        "bomb-inline",
        "lying-sizes",
    ];

    let mut peaks = Vec::new();

    for name in cases {
        fixture(name);
        let report = measure(&[
            &format!("fixtures/out/{name}.xlsx"),
            "--max-bytes",
            "8388608",
            "--max-rows",
            "1000000",
        ]);

        assert_eq!(
            report.text("error").as_deref(),
            Some("DECOMPRESSED_BUDGET_EXCEEDED"),
            "{name}: {report:?}"
        );

        let peak = peak_kb(&report);
        assert!(
            peak < REFUSAL_CEILING_KB,
            "{name} peaked at {peak} kB while being refused: {report:?}"
        );
        peaks.push((name, peak));
    }

    // The archives here range from 0.16 MB to 11.4 MB and expand to hundreds.
    // If the size of the archive reached the peak, these would not agree.
    let lowest = peaks.iter().map(|(_, kb)| *kb).fold(f64::MAX, f64::min);
    let highest = peaks.iter().map(|(_, kb)| *kb).fold(0.0, f64::max);
    assert!(
        highest < lowest + 8.0 * 1024.0,
        "the peak follows the archive: {peaks:?}"
    );
}

#[test]
fn listing_sheets_stays_at_a_flat_peak() {
    fixture("large-600000");
    let report = measure(&[
        "fixtures/out/large-600000.xlsx",
        "--list",
        "--max-bytes",
        "1073741824",
    ]);

    assert_eq!(report.number("sheets"), Some(1.0), "{report:?}");
    assert_eq!(report.number("rows"), Some(0.0), "{report:?}");
    assert!(
        peak_kb(&report) < REFUSAL_CEILING_KB,
        "listing peaked at {} kB: {report:?}",
        peak_kb(&report)
    );
}
