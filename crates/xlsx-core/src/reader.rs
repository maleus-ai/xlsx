//! The reader itself: open a workbook, pick a sheet, pull rows by the batch.

use std::path::Path;

use calamine::{open_workbook, Reader, SheetVisible, Xlsx, XlsxError};

use crate::budget::{enforce_decompressed_budget, map_zip_error};
use crate::cursor::SheetCursor;
use crate::error::ReadError;
use crate::value::CellValue;

/// Columns a worksheet can hold, `XFD` being the last. A reference past it is
/// not a cell that exists.
const MAX_COLUMNS: u32 = 16_384;

/// Rows a worksheet can hold.
const MAX_SHEET_ROWS: u32 = 1_048_576;

/// Bytes a value costs in a row vector.
const BYTES_PER_CELL: u64 = std::mem::size_of::<CellValue>() as u64;

/// Values a single batch may hold, whatever it was asked for.
///
/// Rows are as wide as the file says they are, so `next_batch(1000)` on a sheet
/// of 16 384 columns would hand back 16 million values — half a gigabyte — in
/// one go. This keeps a batch to about 32 MB, and a batch that fills up simply
/// comes back short.
const MAX_BATCH_CELLS: usize = 1 << 20;

/// The two bounds, both mandatory.
///
/// There is no `Default`, and that is the point. A permissive default is a bound
/// nobody set: it survives review because it is invisible, and it is discovered
/// in production. A caller that genuinely wants no ceiling writes `u64::MAX` and
/// leaves the trace of that decision in its own source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderOptions {
    /// Total bytes the archive may expand to.
    ///
    /// Two things are charged against it, because both are ways an archive
    /// turns small into large:
    ///
    /// * what actually leaves the inflater, every entry counted;
    /// * the blank values a sheet's own geometry implies. A cell reference far
    ///   to the right makes a row that wide and every row after it is padded to
    ///   match, so two kilobytes of XML can ask for a gigabyte of values
    ///   without one extra byte leaving the inflater.
    pub max_decompressed_bytes: u64,

    /// Rows the selected sheet may yield. Counted on rows handed back, header
    /// row included — the reader has no notion of a header.
    pub max_rows: u64,
}

/// One sheet, as the workbook declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetInfo {
    /// Sheet name, as it appears on the tab.
    pub name: String,
    /// `false` for both `hidden` and `veryHidden` sheets.
    pub visible: bool,
}

/// A bounded streaming reader over one XLSX file.
///
/// The reader takes a path, not a stream, and the reason is structural: a ZIP is
/// read through its central directory, which sits at the end of the archive, so
/// a correct read needs `Seek`. Walking a ZIP forward instead is the shortcut
/// that loses the tail of the archive — which is the failure this crate exists
/// to stop repeating.
pub struct XlsxReader {
    cursor: SheetCursor,
    sheets: Vec<SheetInfo>,
    /// Name of the sheet the cursor is on, for the errors it can raise.
    selected: String,
    options: ReaderOptions,
    decompressed_bytes: u64,

    /// Row being assembled: the sheet row index it came from, the values so far,
    /// and how many of them the file actually carried.
    pending: Option<PendingRow>,
    /// Width every row is padded to: the widest row seen so far.
    ///
    /// The sheet's declared `<dimension>` is deliberately not used. It is
    /// written by whoever produced the file, nothing forces it to match the
    /// data, and a workbook declaring `A1:XFD1048576` would have every row
    /// padded to 16 384 columns — a cheap way to turn a small archive into a
    /// large allocation. A real sheet establishes its width on its first row,
    /// which is what makes the observed maximum enough.
    width: usize,
    rows_emitted: u64,
    /// Blank values handed out so far, charged against the byte budget.
    blank_bytes: u64,
    exhausted: bool,
}

/// A row mid-assembly.
struct PendingRow {
    /// Row index in the sheet, used to tell when the row is finished.
    index: u32,
    values: Vec<CellValue>,
    /// Values the file carried. The rest of the row's width is padding, and
    /// padding is what gets charged.
    populated: u64,
}

/// The binding hands a reader to the libuv threadpool, so it has to be `Send`.
/// Asserted here rather than discovered there: the property comes from a chain
/// of fields — including the cursor's erased borrow — that a dependency bump
/// could quietly break.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<XlsxReader>();
};

impl std::fmt::Debug for XlsxReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XlsxReader")
            .field("sheets", &self.sheets)
            .field("options", &self.options)
            .field("decompressed_bytes", &self.decompressed_bytes)
            .field("rows_emitted", &self.rows_emitted)
            .finish_non_exhaustive()
    }
}

impl XlsxReader {
    /// Open a workbook, refusing it outright if it expands past the budget.
    ///
    /// The budget is charged here, before `calamine` builds a single table:
    /// that ordering is what keeps the refusal of a hostile archive flat in
    /// memory instead of merely eventual.
    pub fn open(path: &Path, options: ReaderOptions) -> Result<Self, ReadError> {
        let decompressed_bytes = enforce_decompressed_budget(path, options.max_decompressed_bytes)?;

        let workbook: Xlsx<_> = open_workbook(path).map_err(map_open_error)?;

        let sheets = workbook
            .sheets_metadata()
            .iter()
            .map(|sheet| SheetInfo {
                name: sheet.name.clone(),
                visible: matches!(sheet.visible, SheetVisible::Visible),
            })
            .collect();

        Ok(Self {
            cursor: SheetCursor::new(workbook),
            sheets,
            selected: String::new(),
            options,
            decompressed_bytes,
            pending: None,
            width: 0,
            rows_emitted: 0,
            blank_bytes: 0,
            exhausted: false,
        })
    }

    /// Sheets the workbook declares, in workbook order. Reads nothing further:
    /// the list is captured when the workbook is opened.
    pub fn sheets(&self) -> &[SheetInfo] {
        &self.sheets
    }

    /// The sheets a workbook declares, without opening it to read rows.
    ///
    /// Reads exactly two parts — the package relationships and the workbook —
    /// each inflated through a counter and charged against the same budget.
    /// [`XlsxReader::open`] charges the whole archive because it is about to
    /// read it; a listing has no such excuse, and on a large upload the
    /// difference is a walk of every byte against a walk of two small parts.
    pub fn list_sheets(
        path: &Path,
        max_decompressed_bytes: u64,
    ) -> Result<Vec<SheetInfo>, ReadError> {
        crate::list::list_sheets(path, max_decompressed_bytes)
    }

    /// Total bytes the archive expands to. Known exactly, because the budget
    /// walk inflated every entry to get it.
    pub fn decompressed_bytes(&self) -> u64 {
        self.decompressed_bytes
    }

    /// Point the reader at a sheet, from its first row.
    ///
    /// Selecting again rewinds to the top of the newly selected sheet and resets
    /// the row budget: the budget bounds one sheet's rows, not a session's.
    pub fn select(&mut self, name: &str) -> Result<(), ReadError> {
        if !self.sheets.iter().any(|sheet| sheet.name == name) {
            return Err(ReadError::SheetNotFound {
                name: name.to_owned(),
                available: self.sheets.iter().map(|s| s.name.clone()).collect(),
            });
        }

        self.cursor.open_sheet(name).map_err(map_sheet_error)?;

        self.selected = name.to_owned();
        self.pending = None;
        self.width = 0;
        self.rows_emitted = 0;
        self.blank_bytes = 0;
        self.exhausted = false;

        Ok(())
    }

    /// Pull at most `max` rows. `Ok(None)` means the sheet is finished.
    ///
    /// With no sheet selected, the first sheet is selected here — the same
    /// default the JavaScript facade documents.
    ///
    /// A row is a vector of values placed at their column index: a cell missing
    /// in the middle of a row leaves [`CellValue::Empty`] at its position rather
    /// than shifting its neighbours left, and a row shorter than the widest one
    /// seen so far is padded with blanks. Rows absent from the sheet altogether
    /// are not materialised; a blank row costs nothing in the file and must not
    /// cost anything here either.
    pub fn next_batch(&mut self, max: usize) -> Result<Option<Vec<Vec<CellValue>>>, ReadError> {
        if max == 0 {
            return Ok(Some(Vec::new()));
        }

        if self.cursor.reader_mut().is_none() {
            let Some(first) = self.sheets.first() else {
                return Err(ReadError::corrupt("workbook declares no sheet"));
            };
            let name = first.name.clone();
            self.select(&name)?;
        }

        if self.exhausted {
            return Ok(None);
        }

        let mut batch: Vec<Vec<CellValue>> = Vec::new();
        let mut cells_in_batch = 0usize;

        while batch.len() < max && cells_in_batch < MAX_BATCH_CELLS {
            let reader = self
                .cursor
                .reader_mut()
                .ok_or_else(|| ReadError::corrupt("sheet cursor closed unexpectedly"))?;

            let Some(cell) = reader.next_cell().map_err(map_sheet_error)? else {
                self.exhausted = true;
                if let Some(row) = self.pending.take() {
                    // The count is not read again: the batch ends here.
                    self.push_row(&mut batch, row)?;
                }
                break;
            };

            let (row_index, column_index) = cell.get_position();
            check_position(row_index, column_index)?;

            // `Cell` hands out its value by reference only. The clone is free
            // for shared strings, which are borrowed `&str`, and copies once for
            // inline strings.
            let value = CellValue::from_data_ref(cell.get_value().clone());

            match &mut self.pending {
                Some(pending) if pending.index == row_index => {
                    place(&mut pending.values, column_index, value);
                    pending.populated += 1;
                }
                _ => {
                    if let Some(row) = self.pending.take() {
                        cells_in_batch += self.push_row(&mut batch, row)?;
                    }
                    let mut values = Vec::new();
                    place(&mut values, column_index, value);
                    self.pending = Some(PendingRow {
                        index: row_index,
                        values,
                        populated: 1,
                    });
                }
            }
        }

        if batch.is_empty() {
            return Ok(None);
        }

        Ok(Some(batch))
    }

    /// Close a row: charge it against both budgets, pad it out, and hand it to
    /// the batch. Returns how many values it added.
    fn push_row(
        &mut self,
        batch: &mut Vec<Vec<CellValue>>,
        row: PendingRow,
    ) -> Result<usize, ReadError> {
        self.rows_emitted += 1;
        if self.rows_emitted > self.options.max_rows {
            // Raised on the row that crosses the budget: a reader that waited
            // for the end of the sheet would have already paid for reading it.
            return Err(ReadError::RowBudgetExceeded {
                limit: self.options.max_rows,
            });
        }

        let PendingRow {
            mut values,
            populated,
            ..
        } = row;

        self.width = self.width.max(values.len());

        // Everything in the row the file did not carry is padding, and padding
        // is the amplification: `<c r="XFD1"/>` is six bytes of XML asking for
        // sixteen thousand values, and every row after it is padded to match.
        // Charged against the same budget as the bytes the archive inflates,
        // because it is the same thing — an archive turning small into large.
        let blanks = (self.width as u64).saturating_sub(populated);
        self.blank_bytes = self.blank_bytes.saturating_add(blanks * BYTES_PER_CELL);

        let charged = self.decompressed_bytes.saturating_add(self.blank_bytes);
        if charged > self.options.max_decompressed_bytes {
            return Err(ReadError::DecompressedBudgetExceeded {
                limit: self.options.max_decompressed_bytes,
                entry: format!("blank cells in sheet {:?}", self.selected),
            });
        }

        values.resize(self.width, CellValue::Empty);
        let added = values.len();
        batch.push(values);

        Ok(added)
    }
}

/// Refuse a cell that claims to sit outside the format's own grid.
///
/// `calamine` builds a column index by accumulating `col * 26 + letter` with no
/// ceiling, so the index comes straight out of the file: `r="BZZZZZ1"` asks for
/// column 36 119 382. Neither budget sees it coming — a cell reference weighs
/// six bytes through the inflater, and it is one row.
fn check_position(row_index: u32, column_index: u32) -> Result<(), ReadError> {
    if column_index >= MAX_COLUMNS {
        return Err(ReadError::corrupt(format!(
            "cell reference names column {} and a worksheet holds {MAX_COLUMNS}",
            column_index.saturating_add(1)
        )));
    }

    if row_index >= MAX_SHEET_ROWS {
        return Err(ReadError::corrupt(format!(
            "cell reference names row {} and a worksheet holds {MAX_SHEET_ROWS}",
            row_index.saturating_add(1)
        )));
    }

    Ok(())
}

/// Put a value at its column index, filling any gap before it with blanks.
fn place(row: &mut Vec<CellValue>, column_index: u32, value: CellValue) {
    let index = column_index as usize;
    if row.len() <= index {
        row.resize(index + 1, CellValue::Empty);
    }
    row[index] = value;
}

fn map_open_error(error: XlsxError) -> ReadError {
    match error {
        XlsxError::Zip(error) => map_zip_error(error),
        XlsxError::Io(error) => ReadError::Io {
            detail: error.to_string(),
        },
        other => ReadError::corrupt(other.to_string()),
    }
}

fn map_sheet_error(error: XlsxError) -> ReadError {
    match error {
        XlsxError::WorksheetNotFound(name) => ReadError::SheetNotFound {
            name,
            available: Vec::new(),
        },
        XlsxError::Io(error) => ReadError::Io {
            detail: error.to_string(),
        },
        other => ReadError::corrupt(other.to_string()),
    }
}
