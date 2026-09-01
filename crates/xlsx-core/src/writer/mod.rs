//! Bounded streaming writer for XLSX worksheets.
//!
//! The reader in this crate exists because a buffered read of 200 000 rows asks
//! for 2.1 GB on a 4 GB machine. Writing has the same shape and the same
//! remedy, so it is held to the same properties — with one difference that
//! decides what belongs here and what does not.
//!
//! **The threat model is inverted.** A reader is handed a file by a stranger,
//! which is why every budget over there is mandatory: an archive can lie about
//! its own size, and a column reference can ask for a gigabyte from two
//! kilobytes. A writer is handed data by the program that owns it. Mirroring
//! the reader's compulsory budgets here would be cargo cult — a ceiling nobody
//! needs, on data nobody attacked. What is enforced instead is the grid Excel
//! actually defines, reported with the cell that crossed it.
//!
//! What is kept, because it is owed to a measurement rather than to symmetry:
//!
//! 1. **Flat memory.** Every row is flushed to a temporary file as the next one
//!    arrives, so peak RSS stays near 4 MB whether the sheet holds a thousand
//!    rows or the full 1 048 576. See [`XlsxWriter::finish`] for the disk this
//!    trades against, which is not small and is not hidden.
//! 2. **Correct date typing.** In an XLSX a date is a serial *plus* a `numFmt`.
//!    Writing the serial alone produces a cell this crate's own reader hands
//!    back as `Number(45376.0)` — the exact defect the reader was built to stop,
//!    reintroduced from the other side. So a date always carries a format, and
//!    that is not an option a caller can forget to pass.
//!
//! # What does not survive a round trip
//!
//! Read a workbook, write the rows back, read again: values return unchanged
//! except in three places, each a property of the format rather than a choice
//! made here, and each pinned by a test in `tests/writing.rs`.
//!
//! | going in | coming back | why |
//! | --- | --- | --- |
//! | `Text("")` | `Empty` | XLSX gives an empty string nowhere to live that a blank does not also occupy |
//! | `Error("#N/A")` | `Text("#N/A")` | an export holds data, never a live error a formula could propagate |
//! | `DateTime("1815-12-10")` | *refused* | Excel counts days from 1900 and has no serial for anything earlier |
//!
//! The last one is an error, not a silent substitution: a caller that must keep
//! a date from before 1900 writes it as [`CellValue::Text`] and keeps it
//! exactly, which is better than a cell showing some other day.
//!
//! A workbook holds as many sheets as you like.
//! [`XlsxWriter::select_sheet`] says where the rows that follow go, and a sheet
//! can be left and come back to: each keeps its own row counter. So a caller
//! does not have to sort its source by sheet.
//!
//! The ordering constraint is per sheet, not per workbook: rows are appended to
//! whichever sheet is selected, and nothing goes back above one already
//! written. That is what buys the flat memory.
//!
//! ```no_run
//! use xlsx_core::{CellValue, WriterOptions, XlsxWriter};
//!
//! let mut writer = XlsxWriter::new(WriterOptions {
//!     sheet_name: "Export".to_owned(),
//!     temp_dir: None,
//! })?;
//!
//! writer.write_row(&[
//!     CellValue::Text("Ada Lovelace".to_owned()),
//!     CellValue::Number(36.0),
//!     CellValue::DateTime("2024-03-25".to_owned()),
//! ])?;
//!
//! writer.finish(std::io::stdout())?;
//! # Ok::<(), xlsx_core::WriteError>(())
//! ```

mod datetime;
mod error;

use std::io::Write;
use std::path::PathBuf;

use rust_xlsxwriter::{Format, Workbook, Worksheet};

use crate::value::CellValue;
use datetime::Stamp;

pub use error::WriteError;

/// Rows in an Excel worksheet. A hard edge of the format.
pub const MAX_ROWS: u32 = 1_048_576;

/// Columns in an Excel worksheet. A hard edge of the format.
pub const MAX_COLUMNS: u32 = 16_384;

/// Sheets a workbook may hold before this crate refuses to make another.
///
/// Not a limit of the format, which has none: a limit of the machine. Every
/// sheet in constant-memory mode holds a temporary file open for the life of
/// the workbook — measured at 300 descriptors for 300 sheets — and the
/// underlying writer reaches for one with `unwrap()`, so running out is a
/// panic rather than an error. A caller whose sheet names come from its data
/// would otherwise hand that panic to whoever supplies the data.
pub const MAX_SHEETS: usize = 256;

/// How the workbook is set up.
#[derive(Debug, Clone)]
pub struct WriterOptions {
    /// Name on the sheet tab.
    ///
    /// At most 31 characters, not empty, and free of `[ ] : * ? / \`. Refused
    /// rather than silently trimmed: a truncated tab name is the sort of thing
    /// nobody notices until a downstream lookup misses.
    pub sheet_name: String,

    /// Sheets the workbook may hold. `None` uses [`MAX_SHEETS`].
    ///
    /// Worth raising only deliberately: each sheet costs an open descriptor
    /// until the workbook is finished.
    pub max_sheets: Option<usize>,

    /// Where the row spill files go.
    ///
    /// `None` uses the platform temporary directory. Worth setting explicitly
    /// when that directory is small, read-only, or **mounted in memory** — a
    /// `tmpfs`, a `/dev/shm`, a Kubernetes `emptyDir` with `medium: Memory`.
    /// On a memory-backed mount the spill is RAM again, and it will not show up
    /// in the process's RSS: the flat 4 MB reading stays flat while the machine
    /// fills up. See [`XlsxWriter::finish`] for the size of the spill.
    pub temp_dir: Option<PathBuf>,
}

/// A worksheet being written, one row at a time.
///
/// Rows must be written in order, because each one is flushed to disk when the
/// next arrives; there is no going back to a row already gone. That constraint
/// is what buys the flat memory, and it is the same shape as a `Writable` in
/// object mode, which is what the Node binding puts on top of it.
pub struct XlsxWriter {
    workbook: Workbook,
    formats: Formats,
    max_sheets: usize,
    /// Index of the sheet rows are going to.
    current: usize,
    /// Rows written to each sheet, by index.
    ///
    /// Per sheet rather than one counter, because a caller may come back to a
    /// sheet it left: the next row goes under what that sheet already holds,
    /// not under the tallest sheet in the workbook.
    rows: Vec<u32>,
    /// Sheet names in workbook order, folded to lower case.
    ///
    /// Excel compares sheet names without regard to case, so this does too —
    /// which makes `Data` and `DATA` the same sheet rather than a name clash
    /// the underlying writer would only notice when the workbook is saved.
    names: Vec<String>,
}

/// The number formats a date needs to be a date.
///
/// Registered with the workbook up front because constant-memory mode has no
/// second pass in which to collect them: a row is already on disk by the time
/// the next one is written.
struct Formats {
    date: Format,
    datetime: Format,
    time: Format,
}

impl XlsxWriter {
    /// Open a workbook with its first worksheet, in constant-memory mode.
    pub fn new(options: WriterOptions) -> Result<Self, WriteError> {
        let mut workbook = Workbook::new();

        if let Some(dir) = &options.temp_dir {
            workbook.set_tempdir(dir).map_err(|error| WriteError::Io {
                detail: format!("temporary directory {dir:?} is not usable: {error}"),
            })?;
        }

        let formats = Formats {
            date: Format::new().set_num_format("yyyy-mm-dd"),
            datetime: Format::new().set_num_format("yyyy-mm-dd hh:mm:ss"),
            time: Format::new().set_num_format("hh:mm:ss"),
        };
        workbook.register_format(&formats.date);
        workbook.register_format(&formats.datetime);
        workbook.register_format(&formats.time);

        // Validated before the sheet is made, for the reason `select_sheet`
        // does the same: a name refused afterwards would leave a worksheet in
        // the workbook that this type does not know about.
        validate_sheet_name(&options.sheet_name)?;

        let sheet = workbook.add_worksheet_with_constant_memory();
        sheet
            .set_name(&options.sheet_name)
            .map_err(|error| WriteError::InvalidSheetName {
                name: options.sheet_name.clone(),
                detail: error.to_string(),
            })?;

        Ok(Self {
            workbook,
            formats,
            max_sheets: options.max_sheets.unwrap_or(MAX_SHEETS).max(1),
            current: 0,
            rows: vec![0],
            names: vec![options.sheet_name.to_lowercase()],
        })
    }

    /// Send the rows that follow to `name`, creating the sheet if it is new.
    ///
    /// A sheet can be left and come back to: each keeps its own row counter, so
    /// the next row lands under what *that* sheet already holds. Which means a
    /// caller does not have to sort its source by sheet — a row can say where
    /// it goes and be believed.
    ///
    /// What still holds is the order within a sheet: rows are appended, and
    /// nothing goes back above one already written. That is what buys the flat
    /// memory, and no arrangement of calls here relaxes it.
    pub fn select_sheet(&mut self, name: &str) -> Result<(), WriteError> {
        let folded = name.to_lowercase();

        if let Some(index) = self.names.iter().position(|known| *known == folded) {
            self.current = index;
            return Ok(());
        }

        // Everything that can refuse this name is checked *before* a worksheet
        // exists, so a refusal leaves the workbook exactly as it was.
        //
        // Checking afterwards was a defect, not a style: the underlying writer
        // pushes the worksheet on creation and names it later, so a rejected
        // name left an unnamed sheet in the workbook that this type had no
        // record of. Every later index was then one out, and rows written to
        // the sheet a caller had asked for landed on the orphan instead.
        validate_sheet_name(name)?;

        if self.names.len() >= self.max_sheets {
            return Err(WriteError::TooManySheets {
                limit: self.max_sheets,
            });
        }

        let sheet = self.workbook.add_worksheet_with_constant_memory();
        sheet
            .set_name(name)
            .map_err(|error| WriteError::InvalidSheetName {
                name: name.to_owned(),
                detail: error.to_string(),
            })?;

        self.names.push(folded);
        self.rows.push(0);
        self.current = self.names.len() - 1;
        Ok(())
    }

    /// Sheets in the workbook.
    pub fn sheet_count(&self) -> usize {
        self.names.len()
    }

    /// Rows written to the sheet currently selected.
    pub fn rows_written(&self) -> u32 {
        self.rows[self.current]
    }

    /// Append a row.
    ///
    /// Values land at their index, so a `CellValue::Empty` in the middle leaves
    /// the cell blank rather than shifting its neighbours left — the same
    /// placement rule the reader applies coming the other way.
    pub fn write_row(&mut self, cells: &[CellValue]) -> Result<(), WriteError> {
        let row = self.rows[self.current];

        // Checked before anything is written, so a row that cannot fit does not
        // leave half of itself in the sheet.
        if row >= MAX_ROWS {
            return Err(WriteError::SheetLimitExceeded {
                max_rows: MAX_ROWS,
                max_columns: MAX_COLUMNS,
                row: u64::from(row),
                column: 0,
            });
        }
        if cells.len() as u64 > u64::from(MAX_COLUMNS) {
            return Err(WriteError::SheetLimitExceeded {
                max_rows: MAX_ROWS,
                max_columns: MAX_COLUMNS,
                row: u64::from(row),
                column: cells.len() as u64 - 1,
            });
        }

        let formats = &self.formats;
        let sheet = self
            .workbook
            .worksheet_from_index(self.current)
            .map_err(WriteError::from)?;

        for (column, cell) in cells.iter().enumerate() {
            write_cell(sheet, row, column as u16, cell, formats)?;
        }

        self.rows[self.current] += 1;
        Ok(())
    }

    /// Assemble the workbook and stream it to `sink`.
    ///
    /// Consumes the writer: the spill files are folded into the archive here,
    /// and there is nothing left to append to afterwards.
    ///
    /// **This is where the bytes appear.** Rows written before this call went to
    /// temporary files, not to `sink`; nothing reaches the sink until this runs.
    /// It is therefore not a transform of rows into bytes as they arrive, and
    /// anything built on top has to say so rather than imply otherwise.
    ///
    /// **What flat memory costs.** A full sheet of 1 048 576 rows by four
    /// columns spills roughly 178 MB of uncompressed XML to hold peak RSS at
    /// 4 MB, and deflates to about 20 MB on the way out. The spill is one
    /// unlinked file — invisible to `ls`, released by the kernel when the
    /// handle closes, including on a panic. It is disk that RAM would otherwise
    /// be, which is only a good trade where the temporary directory is really
    /// disk: see [`WriterOptions::temp_dir`].
    ///
    /// `sink` is written sequentially and never seeked, so a pipe or a socket
    /// works as well as a file.
    pub fn finish<W: Write + Send>(mut self, sink: W) -> Result<(), WriteError> {
        self.workbook.save_to_writer(sink).map_err(WriteError::from)
    }
}

/// Check a sheet name against everything that can refuse it.
///
/// `rust_xlsxwriter::check_sheet_name` covers Excel's own rules — not blank, at
/// most 31 characters, none of `[ ] : * ? / \`, no leading or trailing
/// apostrophe. It does not cover control characters, and those are written into
/// the workbook XML raw: a name holding `\u{1}` produces a file that is not
/// well-formed XML at all, which Excel and any strict parser refuse to open.
/// Measured — a `U+0001` in a sheet name lands as the byte `0x01` in
/// `xl/workbook.xml`, and expat rejects the document.
///
/// So the file is refused here, where the caller can still do something about
/// it, rather than at whoever opens it.
pub fn validate_sheet_name(name: &str) -> Result<(), WriteError> {
    rust_xlsxwriter::utility::check_sheet_name(name).map_err(|error| {
        WriteError::InvalidSheetName {
            name: name.to_owned(),
            detail: error.to_string(),
        }
    })?;

    if let Some(bad) = name.chars().find(|c| c.is_control()) {
        return Err(WriteError::InvalidSheetName {
            name: name.to_owned(),
            detail: format!(
                "it holds the control character U+{:04X}, which goes into the \
                 workbook XML unescaped and leaves a file no strict parser will open",
                bad as u32
            ),
        });
    }

    Ok(())
}

/// Write one cell.
///
/// Every string is written as a string — never as a formula, whatever it starts
/// with. A cell holding `=1+1`, or `=cmd|'/c calc'!A0` arrived from a form
/// field, ends up in the sheet as those characters and Excel shows them. This
/// crate has no way to emit a formula at all, which is the point: an export is
/// data, and the value of not being able to turn a user's string into code is
/// worth more than the convenience of computing a total in the sheet.
fn write_cell(
    sheet: &mut Worksheet,
    row: u32,
    column: u16,
    cell: &CellValue,
    formats: &Formats,
) -> Result<(), WriteError> {
    match cell {
        // Writing nothing is what leaves a blank. Writing an empty string would
        // produce a cell that exists and holds "", which the reader would then
        // hand back as Text("") rather than Empty.
        CellValue::Empty => Ok(()),

        CellValue::Text(text) => sheet.write_string(row, column, text).map(drop),
        CellValue::Number(number) => sheet.write_number(row, column, *number).map(drop),
        CellValue::Bool(flag) => sheet.write_boolean(row, column, *flag).map(drop),

        // The serial and the format travel together or the value is not a date.
        CellValue::DateTime(iso) => {
            let (stamp, kind) = datetime::parse(iso)?;
            let format = match kind {
                Stamp::Date => &formats.date,
                Stamp::DateTime => &formats.datetime,
                Stamp::Time => &formats.time,
            };
            sheet
                .write_datetime_with_format(row, column, stamp, format)
                .map(drop)
        }

        // An error cell is written as its spelling, not as a live error. Excel
        // stores `#N/A` as an error type whose value a formula can propagate;
        // reproducing that would mean putting a computed thing into an export
        // that promises to hold none. The round trip is therefore lossy in one
        // direction on purpose: Error("#N/A") goes out and comes back as
        // Text("#N/A").
        CellValue::Error(spelling) => sheet.write_string(row, column, spelling).map(drop),
    }
    .map_err(|error| match error {
        rust_xlsxwriter::XlsxError::RowColumnLimitError => WriteError::SheetLimitExceeded {
            max_rows: MAX_ROWS,
            max_columns: MAX_COLUMNS,
            row: u64::from(row),
            column: u64::from(column),
        },
        other => WriteError::from(other),
    })
}
