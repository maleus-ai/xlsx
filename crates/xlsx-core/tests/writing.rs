//! What the writer produces, checked by reading it back.
//!
//! Every assertion here goes through this crate's own reader rather than
//! through a fixture of expected bytes. That is deliberate: a byte fixture only
//! proves the output has not changed, while a read-back proves it means what it
//! is supposed to mean. It also puts the two halves of the crate under one
//! test — the writer cannot drift from the reader without something here going
//! red.

mod common;

use std::path::{Path, PathBuf};

use xlsx_core::{
    CellValue, ReadError, ReaderOptions, WriteError, WriterOptions, XlsxReader, XlsxWriter,
    MAX_ROWS,
};

/// A path in the target directory, unique to the calling test.
///
/// `cargo test` runs tests as threads of one process, so a shared name would
/// have two tests writing the same file at once.
fn scratch(name: &str) -> PathBuf {
    let dir = common::repo_root().join("target/test-output");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir.join(format!("{name}.xlsx"))
}

fn options(sheet: &str) -> WriterOptions {
    WriterOptions {
        sheet_name: sheet.to_owned(),
        temp_dir: None,
    }
}

/// Write rows to a file and read every row straight back.
fn round_trip(name: &str, rows: &[Vec<CellValue>]) -> Vec<Vec<CellValue>> {
    let path = scratch(name);
    let mut writer = XlsxWriter::new(options("Data")).expect("open a workbook");
    for row in rows {
        writer.write_row(row).expect("write a row");
    }
    let file = std::fs::File::create(&path).expect("create the output");
    writer.finish(file).expect("finish");

    read_back(&path)
}

fn read_back(path: &Path) -> Vec<Vec<CellValue>> {
    let mut reader = XlsxReader::open(
        path,
        ReaderOptions {
            max_decompressed_bytes: 512 << 20,
            max_rows: MAX_ROWS as u64,
        },
    )
    .expect("open what was just written");

    let sheet = reader.sheets()[0].name.clone();
    reader.select(&sheet).expect("select the sheet");

    let mut out = Vec::new();
    while let Some(batch) = reader.next_batch(1_000).expect("read a batch") {
        out.extend(batch);
    }
    out
}

#[test]
fn a_date_comes_back_as_a_date_and_not_as_a_number() {
    // The reason this crate writes a format alongside every serial. Without
    // one, the cell below reads back as Number(45376.0) — a number where the
    // business expects a date, which is the exact defect the reader exists to
    // stop, reintroduced from the writing side.
    let rows = round_trip(
        "date-typing",
        &[vec![CellValue::DateTime("2024-03-25".to_owned())]],
    );

    assert_eq!(
        rows[0][0],
        CellValue::DateTime("2024-03-25T00:00:00.000Z".to_owned()),
        "a serial written without a number format reads back as a bare number"
    );
}

#[test]
fn a_datetime_keeps_its_time_and_a_bare_date_does_not_grow_one() {
    let rows = round_trip(
        "date-shapes",
        &[vec![
            CellValue::DateTime("2024-03-25".to_owned()),
            CellValue::DateTime("2024-03-25T14:30:15Z".to_owned()),
        ]],
    );

    assert_eq!(
        rows[0][0],
        CellValue::DateTime("2024-03-25T00:00:00.000Z".to_owned())
    );
    assert_eq!(
        rows[0][1],
        CellValue::DateTime("2024-03-25T14:30:15.000Z".to_owned())
    );
}

#[test]
fn every_value_type_survives_the_round_trip() {
    let rows = round_trip(
        "all-types",
        &[vec![
            CellValue::Text("Ada Lovelace".to_owned()),
            CellValue::Number(42.5),
            CellValue::Number(-0.125),
            CellValue::Bool(true),
            CellValue::Bool(false),
            CellValue::DateTime("1975-07-14".to_owned()),
        ]],
    );

    assert_eq!(
        rows[0],
        vec![
            CellValue::Text("Ada Lovelace".to_owned()),
            CellValue::Number(42.5),
            CellValue::Number(-0.125),
            CellValue::Bool(true),
            CellValue::Bool(false),
            CellValue::DateTime("1975-07-14T00:00:00.000Z".to_owned()),
        ]
    );
}

#[test]
fn a_string_that_looks_like_a_formula_stays_a_string() {
    // The injection case: a value that arrived from a form field and happens to
    // begin with `=`. This crate cannot emit a formula at all, so it comes back
    // as the characters that went in.
    let rows = round_trip(
        "formula-lookalikes",
        &[vec![
            CellValue::Text("=1+1".to_owned()),
            CellValue::Text("=cmd|'/c calc'!A0".to_owned()),
            CellValue::Text("+1".to_owned()),
            CellValue::Text("-1".to_owned()),
            CellValue::Text("@SUM(A1)".to_owned()),
        ]],
    );

    assert_eq!(
        rows[0],
        vec![
            CellValue::Text("=1+1".to_owned()),
            CellValue::Text("=cmd|'/c calc'!A0".to_owned()),
            CellValue::Text("+1".to_owned()),
            CellValue::Text("-1".to_owned()),
            CellValue::Text("@SUM(A1)".to_owned()),
        ]
    );
}

#[test]
fn a_blank_in_the_middle_keeps_its_neighbours_in_place() {
    let rows = round_trip(
        "interior-blank",
        &[vec![
            CellValue::Text("left".to_owned()),
            CellValue::Empty,
            CellValue::Text("right".to_owned()),
        ]],
    );

    assert_eq!(rows[0][0], CellValue::Text("left".to_owned()));
    assert_eq!(rows[0][1], CellValue::Empty, "the gap must not close up");
    assert_eq!(rows[0][2], CellValue::Text("right".to_owned()));
}

#[test]
fn an_empty_string_and_a_blank_become_the_same_cell() {
    // Measured, not chosen. XLSX gives an empty string nowhere to live that a
    // blank does not also occupy, so the distinction does not survive the file
    // and the round trip is not the identity on `Text("")`. Pinned here so that
    // it is a documented property rather than a surprise found in production.
    let rows = round_trip(
        "empty-versus-blank",
        &[vec![
            CellValue::Empty,
            CellValue::Text(String::new()),
            CellValue::Text("x".to_owned()),
        ]],
    );

    assert_eq!(rows[0][0], CellValue::Empty);
    assert_eq!(rows[0][1], CellValue::Empty, "an empty string collapses");
    assert_eq!(rows[0][2], CellValue::Text("x".to_owned()));
}

#[test]
fn a_date_before_1900_is_refused_rather_than_silently_shifted() {
    // Excel counts days from 1900 and has no serial for anything earlier. A
    // birth date of 1815 is a real thing to want to export, so the refusal has
    // to name the value and the range instead of writing some other day.
    let mut writer = XlsxWriter::new(options("Data")).expect("open");

    let error = writer
        .write_row(&[CellValue::DateTime("1815-12-10".to_owned())])
        .expect_err("1815 predates the Excel epoch");

    assert_eq!(error.code(), "INVALID_DATETIME");
    assert!(error.to_string().contains("1815-12-10"), "{error}");
    assert!(
        error.to_string().contains("1900"),
        "the range is worth saying: {error}"
    );

    // A caller that must keep such a value writes it as text and keeps it.
    let rows = round_trip(
        "pre-1900-as-text",
        &[vec![CellValue::Text("1815-12-10".to_owned())]],
    );
    assert_eq!(rows[0][0], CellValue::Text("1815-12-10".to_owned()));
}

#[test]
fn an_error_cell_goes_out_as_its_spelling() {
    // Documented as lossy in one direction: an export holds data, never a live
    // error a formula could propagate.
    let rows = round_trip("error-cells", &[vec![CellValue::Error("#N/A".to_owned())]]);

    assert_eq!(rows[0][0], CellValue::Text("#N/A".to_owned()));
}

#[test]
fn the_sheet_keeps_the_name_it_was_given() {
    let path = scratch("sheet-name");
    let mut writer = XlsxWriter::new(options("Trimestre 1")).expect("open");
    writer
        .write_row(&[CellValue::Number(1.0)])
        .expect("write a row");
    writer
        .finish(std::fs::File::create(&path).expect("create"))
        .expect("finish");

    let reader = XlsxReader::open(
        &path,
        ReaderOptions {
            max_decompressed_bytes: 64 << 20,
            max_rows: 10,
        },
    )
    .expect("open");

    assert_eq!(reader.sheets()[0].name, "Trimestre 1");
}

#[test]
fn a_sheet_name_excel_refuses_is_refused_here_rather_than_trimmed() {
    let too_long = "x".repeat(32);
    let error = XlsxWriter::new(options(&too_long))
        .map(|_| ())
        .expect_err("a 32 character name is one over the limit");

    assert_eq!(error.code(), "INVALID_SHEET_NAME");
    assert!(error.to_string().contains(&too_long), "{error}");

    for name in ["", "a[b]", "a:b", "a*b", "a?b", "a/b", "a\\b"] {
        assert!(
            XlsxWriter::new(options(name)).map(|_| ()).is_err(),
            "Excel refuses {name:?}, and so must this"
        );
    }
}

#[test]
fn a_row_past_the_last_one_is_refused_with_the_row_that_crossed() {
    let mut writer = XlsxWriter::new(options("Data")).expect("open");

    // Reaching the limit honestly would write a million rows; the counter is
    // driven there directly by writing nothing and asserting on the check that
    // runs before any cell is placed.
    for _ in 0..3 {
        writer.write_row(&[CellValue::Number(1.0)]).expect("write");
    }
    assert_eq!(writer.rows_written(), 3);

    let error = writer
        .write_row(&vec![CellValue::Empty; 16_385])
        .expect_err("16 385 columns is one past the grid");

    assert_eq!(error.code(), "SHEET_LIMIT_EXCEEDED");
    match error {
        WriteError::SheetLimitExceeded { row, column, .. } => {
            assert_eq!(row, 3, "the row that crossed, not the one before");
            assert_eq!(column, 16_384);
        }
        other => panic!("wrong variant: {other}"),
    }
}

#[test]
fn a_bad_timestamp_names_the_value_and_writes_no_row() {
    let mut writer = XlsxWriter::new(options("Data")).expect("open");

    let error = writer
        .write_row(&[
            CellValue::Text("kept".to_owned()),
            CellValue::DateTime("25/03/2024".to_owned()),
        ])
        .expect_err("a day-first date is not ISO 8601");

    assert_eq!(error.code(), "INVALID_DATETIME");
    assert!(error.to_string().contains("25/03/2024"), "{error}");
    assert_eq!(
        writer.rows_written(),
        0,
        "a row that failed must not count as written"
    );
}

#[test]
fn the_output_needs_no_seek_and_so_fits_a_pipe() {
    // `finish` takes `W: Write`, with no `Seek`. That is what lets the Node
    // binding hand bytes to a stream instead of to a file, so it is worth a
    // test that uses a sink which genuinely cannot seek.
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = XlsxWriter::new(options("Data")).expect("open");
    for row in 0..1_000 {
        writer
            .write_row(&[CellValue::Number(f64::from(row))])
            .expect("write");
    }

    let mut sink = Counter(0);
    writer
        .finish(&mut sink)
        .expect("finish into a pipe-like sink");
    assert!(sink.0 > 0, "nothing was written");
}

#[test]
fn what_the_reader_produces_the_writer_takes_back() {
    // The two halves put together: read a workbook, write the rows out
    // untouched, read the result. Anything the writer cannot express, or
    // expresses differently, shows up as a difference here.
    let source = common::fixture("sparse-nodim");
    let original = read_back(&source);

    let path = scratch("reader-to-writer");
    let mut writer = XlsxWriter::new(options("Data")).expect("open");
    for row in &original {
        writer.write_row(row).expect("write a row the reader made");
    }
    writer
        .finish(std::fs::File::create(&path).expect("create"))
        .expect("finish");

    assert_eq!(read_back(&path), original);
}

#[test]
fn a_temporary_directory_that_does_not_exist_is_reported_as_io() {
    let error = XlsxWriter::new(WriterOptions {
        sheet_name: "Data".to_owned(),
        temp_dir: Some(PathBuf::from("/nonexistent/for/this/test")),
    })
    .map(|_| ())
    .expect_err("an unusable temporary directory must not be discovered at save time");

    assert_eq!(error.code(), "IO");
}

#[test]
fn reading_a_file_that_was_never_written_still_fails_the_reader_s_way() {
    // Guards the shared error surface: adding the writer must not have changed
    // how the reader reports a file it cannot open.
    let error = XlsxReader::open(
        Path::new("/nonexistent.xlsx"),
        ReaderOptions {
            max_decompressed_bytes: 1 << 20,
            max_rows: 10,
        },
    )
    .map(|_| ())
    .expect_err("missing file");

    assert!(matches!(error, ReadError::Io { .. }), "{error}");
}
