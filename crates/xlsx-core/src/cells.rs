//! Reading a sheet cell by cell: position, type, value, formula, style index.
//!
//! Rows are what an import wants; an inspection wants every cell the file
//! declares, with what it says about itself. Two readings of the sheet run in
//! step to give it that:
//!
//! * `calamine` decodes the value — shared strings, dates by their number
//!   format, the 1904 system — and the formula, a shared one as its anchor's
//!   template or a derived cell's index;
//! * a quick-xml pass over the same part reads what `calamine` parses and does
//!   not hand out: the `s` attribute that indexes the cell's style, and the
//!   `t` and `ref` attributes of its formula.
//!
//! `calamine` expands empty elements, so it yields every `<c>` the sheet
//! declares, a self-closing one included, in document order. The second pass
//! sees the same elements in the same order, which makes the pairing one to
//! one; the positions are compared anyway, and a disagreement makes the sheet
//! `CORRUPT` rather than a cell carrying another cell's style.
//!
//! The second pass inflates the sheet part a second time, and those bytes are
//! charged against the same budget as everything else.
//!
//! Shared formulas are expanded here rather than by `calamine`, whose own
//! expansion sizes a table by the largest `si` a cell declares: one cell
//! claiming `si="1000000000"` would allocate gigabytes before any bound saw
//! it. The templates live in a map instead, which holds one entry per anchor
//! the sheet actually declares.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use calamine::{expand_shared_formula, DataRef, XlsxFormulaMetadata};
use flate2::read::DeflateDecoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader as XmlReader;
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

use crate::budget::map_zip_error;
use crate::cursor::SheetCursor;
use crate::error::ReadError;
use crate::reader::{check_position, map_sheet_error, open_checked, Opened, SheetInfo};
use crate::value::CellValue;

/// Cells a single batch may hold, whatever it was asked for.
const MAX_BATCH_CELLS: usize = 1 << 16;

/// What a cell holds, as told by its decoded value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellType {
    /// Text: a shared string, an inline string, or a cached string result.
    String,
    /// A number not formatted as a date or a duration.
    Number,
    /// `TRUE` or `FALSE`.
    Boolean,
    /// A number formatted as a date, time or datetime, or an ISO date cell.
    Date,
    /// A number formatted as an elapsed time (`[h]:mm:ss`).
    Duration,
    /// An error value (`#DIV/0!`).
    Error,
    /// A cell declared without a value — one that only carries a style, or a
    /// formula with no cached result.
    Empty,
}

impl CellType {
    /// The spelling the JavaScript facade uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Duration => "duration",
            Self::Error => "error",
            Self::Empty => "empty",
        }
    }

    fn of(value: &DataRef<'_>) -> Self {
        match value {
            DataRef::Empty => Self::Empty,
            DataRef::Int(_) | DataRef::Float(_) => Self::Number,
            DataRef::String(_) | DataRef::SharedString(_) => Self::String,
            DataRef::Bool(_) => Self::Boolean,
            DataRef::Error(_) => Self::Error,
            DataRef::DateTime(v) if v.is_duration() => Self::Duration,
            DataRef::DateTime(_) | DataRef::DateTimeIso(_) => Self::Date,
            DataRef::DurationIso(_) => Self::Duration,
        }
    }
}

/// How a formula is shared between cells — `ST_CellFormulaType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormulaType {
    /// A formula of its own.
    Normal,
    /// Part of a formula written once and shared across a range.
    Shared,
    /// The master cell of an array formula.
    Array,
    /// The master cell of a data table.
    DataTable,
}

impl FormulaType {
    /// The spelling the format uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Shared => "shared",
            Self::Array => "array",
            Self::DataTable => "dataTable",
        }
    }

    fn parse(attribute: Option<&[u8]>) -> Result<Self, ReadError> {
        match attribute {
            None | Some(b"normal") => Ok(Self::Normal),
            Some(b"shared") => Ok(Self::Shared),
            Some(b"array") => Ok(Self::Array),
            Some(b"dataTable") => Ok(Self::DataTable),
            Some(other) => Err(ReadError::corrupt(format!(
                "a formula declares type {:?}, which is not one of normal, shared, array, dataTable",
                String::from_utf8_lossy(other)
            ))),
        }
    }
}

/// A cell's formula — `CT_CellFormula`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    /// The formula as written, without the leading `=`. A cell that shares a
    /// formula gets the text expanded for its own position.
    pub text: String,
    /// How the formula is shared.
    pub formula_type: FormulaType,
    /// The range a shared or array formula covers, on the cell that declares
    /// it, in A1 notation.
    pub reference: Option<String>,
}

/// One cell, as the sheet declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// 0-based row.
    pub row: u32,
    /// 0-based column.
    pub col: u32,
    /// What the value is.
    pub cell_type: CellType,
    /// The value, or the cached result of the formula. Never recalculated.
    pub value: CellValue,
    /// The formula, when the cell has one.
    pub formula: Option<Formula>,
    /// Index of the cell's style in the workbook's cell formats; `0`, the
    /// default format, when the cell declares none.
    pub style_id: u32,
}

/// The two bounds, both mandatory, for the same reason as
/// [`crate::ReaderOptions`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellReaderOptions {
    /// Total bytes the archive may expand to — the sheet's second reading
    /// included, and the text its cells copy out of the shared strings and
    /// the shared formulas.
    pub max_decompressed_bytes: u64,
    /// Cells the selected sheet may yield.
    pub max_cells: u64,
}

/// A bounded streaming reader over the cells of one XLSX file.
pub struct XlsxCells {
    cursor: SheetCursor,
    path: PathBuf,
    sheets: Vec<SheetInfo>,
    /// Archive part of each sheet, in the order of `sheets`.
    parts: Vec<String>,
    options: CellReaderOptions,
    decompressed_bytes: u64,
    attributes: Option<AttributePass>,
    /// Shared formula templates of the selected sheet, by `si`: the text and
    /// the first cell of the range it was written for.
    shared_formulas: HashMap<usize, (String, (u32, u32))>,
    /// Text the selected sheet's cells copied rather than inflated, charged
    /// against the byte budget.
    copied_bytes: u64,
    cells_emitted: u64,
    exhausted: bool,
}

/// Handed to the libuv threadpool by the binding, like [`crate::XlsxReader`].
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<XlsxCells>();
};

impl std::fmt::Debug for XlsxCells {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XlsxCells")
            .field("sheets", &self.sheets)
            .field("options", &self.options)
            .field("cells_emitted", &self.cells_emitted)
            .finish_non_exhaustive()
    }
}

impl XlsxCells {
    /// Open a workbook, refusing it outright if it expands past the budget.
    pub fn open(path: &Path, options: CellReaderOptions) -> Result<Self, ReadError> {
        let Opened {
            decompressed_bytes,
            sheets,
            workbook,
        } = open_checked(path, options.max_decompressed_bytes)?;
        let parts = crate::peek::read_sheet_parts(path, options.max_decompressed_bytes)?;

        Ok(Self {
            cursor: SheetCursor::new(workbook),
            path: path.to_owned(),
            sheets,
            parts,
            options,
            decompressed_bytes,
            attributes: None,
            shared_formulas: HashMap::new(),
            copied_bytes: 0,
            cells_emitted: 0,
            exhausted: false,
        })
    }

    /// Sheets the workbook declares, in workbook order.
    pub fn sheets(&self) -> &[SheetInfo] {
        &self.sheets
    }

    /// Point the reader at a sheet, from its first cell. Resets the cell
    /// budget: it bounds one sheet's cells, not a session's.
    pub fn select(&mut self, name: &str) -> Result<(), ReadError> {
        let Some(index) = self.sheets.iter().position(|sheet| sheet.name == name) else {
            return Err(ReadError::SheetNotFound {
                name: name.to_owned(),
                available: self.sheets.iter().map(|s| s.name.clone()).collect(),
            });
        };

        self.attributes = None;
        self.cursor.open_sheet(name).map_err(map_sheet_error)?;
        self.attributes = Some(AttributePass::open(
            &self.path,
            &self.parts[index],
            self.options.max_decompressed_bytes,
            self.decompressed_bytes,
        )?);
        self.shared_formulas.clear();
        self.copied_bytes = 0;
        self.cells_emitted = 0;
        self.exhausted = false;
        Ok(())
    }

    /// Pull at most `max` cells, in document order. `Ok(None)` means the sheet
    /// is finished. With no sheet selected, the first one is.
    pub fn next_batch(&mut self, max: usize) -> Result<Option<Vec<Cell>>, ReadError> {
        if max == 0 {
            return Ok(Some(Vec::new()));
        }
        if self.attributes.is_none() {
            let Some(first) = self.sheets.first() else {
                return Err(ReadError::corrupt("workbook declares no sheet"));
            };
            let name = first.name.clone();
            self.select(&name)?;
        }
        if self.exhausted {
            return Ok(None);
        }

        let mut batch = Vec::new();
        while batch.len() < max.min(MAX_BATCH_CELLS) {
            let reader = self
                .cursor
                .reader_mut()
                .ok_or_else(|| ReadError::corrupt("sheet cursor closed unexpectedly"))?;
            let attributes = self
                .attributes
                .as_mut()
                .ok_or_else(|| ReadError::corrupt("sheet cursor closed unexpectedly"))?;

            let decoded = reader
                .next_cell_with_formula_metadata()
                .map_err(map_sheet_error)?;
            let declared = attributes.next()?;

            let (decoded, declared) = match (decoded, declared) {
                (None, None) => {
                    self.exhausted = true;
                    break;
                }
                (Some(decoded), Some(declared)) if decoded.pos == declared.pos => {
                    (decoded, declared)
                }
                (decoded, declared) => {
                    return Err(ReadError::corrupt(format!(
                        "the two readings of the sheet disagree: {:?} against {:?}",
                        decoded.map(|cell| cell.pos),
                        declared.map(|cell| cell.pos)
                    )));
                }
            };

            let (row, col) = decoded.pos;
            check_position(row, col)?;

            self.cells_emitted += 1;
            if self.cells_emitted > self.options.max_cells {
                return Err(ReadError::CellBudgetExceeded {
                    limit: self.options.max_cells,
                });
            }

            let cell_type = CellType::of(&decoded.value);
            let derived = matches!(
                decoded.formula,
                Some(XlsxFormulaMetadata::SharedDerived { .. })
            );
            let text = match decoded.formula {
                None => None,
                Some(XlsxFormulaMetadata::Normal { formula }) => Some(formula),
                Some(XlsxFormulaMetadata::Shared {
                    shared_index,
                    range,
                    formula,
                }) => {
                    let start = range.map_or((row, col), |range| range.start);
                    self.shared_formulas
                        .insert(shared_index, (formula.clone(), start));
                    Some(formula)
                }
                // A derived cell met before its anchor has no text to expand.
                Some(XlsxFormulaMetadata::SharedDerived { shared_index }) => self
                    .shared_formulas
                    .get(&shared_index)
                    .map(|(template, start)| expand_shared_formula(template, *start, (row, col)))
                    .transpose()
                    .map_err(map_sheet_error)?,
                Some(_) => None,
            };

            // A shared string and a derived formula are copies, not inflated
            // bytes: thirty bytes of XML each ask for the whole string or the
            // whole template, as many times as the sheet repeats them.
            let shared_string = match &decoded.value {
                DataRef::SharedString(value) => value.len(),
                _ => 0,
            };
            let expanded = if derived {
                text.as_ref().map_or(0, String::len)
            } else {
                0
            };
            self.copied_bytes = self
                .copied_bytes
                .saturating_add((shared_string + expanded) as u64);
            if attributes.inflated().saturating_add(self.copied_bytes)
                > self.options.max_decompressed_bytes
            {
                return Err(ReadError::DecompressedBudgetExceeded {
                    limit: self.options.max_decompressed_bytes,
                    entry: format!("text copied into the cells of {:?}", attributes.part()),
                });
            }

            let formula = text.map(|text| Formula {
                text,
                formula_type: declared.formula_type,
                reference: declared.formula_reference,
            });

            batch.push(Cell {
                row,
                col,
                cell_type,
                value: CellValue::from_data_ref(decoded.value),
                formula,
                style_id: declared.style_id,
            });
        }

        if batch.is_empty() {
            return Ok(None);
        }
        Ok(Some(batch))
    }
}

/// What the second pass reads off one `<c>`.
#[derive(Debug)]
struct Declared {
    pos: (u32, u32),
    style_id: u32,
    formula_type: FormulaType,
    formula_reference: Option<String>,
}

/// The second reading of a sheet part.
struct AttributePass {
    xml: XmlReader<BufReader<Charged>>,
    buf: Vec<u8>,
    row_index: u32,
    col_index: u32,
}

impl AttributePass {
    fn open(path: &Path, part: &str, limit: u64, already_used: u64) -> Result<Self, ReadError> {
        let stream = open_part_stream(path, part)?;
        let charged = Charged {
            inner: stream,
            used: already_used,
            limit,
            part: part.to_owned(),
            exceeded: false,
        };
        let mut xml = XmlReader::from_reader(BufReader::new(charged));
        // The same view of the document `calamine` takes: a self-closing
        // element reads as a start and an end, so `<c r="A1" s="3"/>` counts.
        xml.config_mut().expand_empty_elements = true;
        Ok(Self {
            xml,
            buf: Vec::new(),
            row_index: 0,
            col_index: 0,
        })
    }

    /// The next `<c>` in document order, or `None` at the end of the data.
    ///
    /// Positions follow `calamine`'s own rule for a cell without an `r`: the
    /// row from the enclosing `<row r>`, the column one past the previous cell.
    fn next(&mut self) -> Result<Option<Declared>, ReadError> {
        loop {
            self.buf.clear();
            let event = self.xml.read_event_into(&mut self.buf);
            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(self.failure(error)),
            };
            match event {
                Event::Start(element) if element.local_name().as_ref() == b"row" => {
                    if let Some(r) = raw_attribute(&element, b"r") {
                        let row: u32 = std::str::from_utf8(&r)
                            .ok()
                            .and_then(|r| r.parse().ok())
                            .filter(|row| *row >= 1)
                            .ok_or_else(|| ReadError::corrupt("a row declares no valid number"))?;
                        self.row_index = row - 1;
                    }
                }
                Event::End(element) if element.local_name().as_ref() == b"row" => {
                    self.row_index += 1;
                    self.col_index = 0;
                }
                Event::Start(element) if element.local_name().as_ref() == b"c" => {
                    let pos = match raw_attribute(&element, b"r") {
                        Some(reference) => {
                            let (row, col) = parse_cell_reference(&reference)?;
                            self.col_index = col;
                            (row, col)
                        }
                        None => (self.row_index, self.col_index),
                    };
                    let style_id = match raw_attribute(&element, b"s") {
                        Some(s) => std::str::from_utf8(&s)
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| {
                                ReadError::corrupt("a cell declares a style that is not an index")
                            })?,
                        None => 0,
                    };
                    let (formula_type, formula_reference) = self.read_cell_content()?;
                    self.col_index += 1;
                    return Ok(Some(Declared {
                        pos,
                        style_id,
                        formula_type,
                        formula_reference,
                    }));
                }
                Event::End(element) if element.local_name().as_ref() == b"sheetData" => {
                    return Ok(None);
                }
                Event::Eof => {
                    return Err(ReadError::corrupt(
                        "the sheet ends before its data is closed",
                    ))
                }
                _ => {}
            }
        }
    }

    /// Skip to the end of the current `<c>`, keeping its formula's attributes.
    fn read_cell_content(&mut self) -> Result<(FormulaType, Option<String>), ReadError> {
        let mut formula_type = FormulaType::Normal;
        let mut reference = None;
        loop {
            self.buf.clear();
            let event = match self.xml.read_event_into(&mut self.buf) {
                Ok(event) => event,
                Err(error) => return Err(self.failure(error)),
            };
            match event {
                Event::Start(element) if element.local_name().as_ref() == b"f" => {
                    formula_type = FormulaType::parse(raw_attribute(&element, b"t").as_deref())?;
                    reference = raw_attribute(&element, b"ref")
                        .map(|r| String::from_utf8_lossy(&r).into_owned());
                }
                Event::End(element) if element.local_name().as_ref() == b"c" => {
                    return Ok((formula_type, reference));
                }
                Event::Eof => return Err(ReadError::corrupt("a cell is never closed")),
                _ => {}
            }
        }
    }

    /// Bytes inflated so far, the whole archive's included.
    fn inflated(&self) -> u64 {
        self.xml.get_ref().get_ref().used
    }

    fn part(&self) -> &str {
        &self.xml.get_ref().get_ref().part
    }

    /// The error to report when the XML reader fails: the budget if that is
    /// what stopped the inflater, the XML otherwise.
    fn failure(&self, error: quick_xml::Error) -> ReadError {
        let charged = self.xml.get_ref().get_ref();
        if charged.exceeded {
            return ReadError::DecompressedBudgetExceeded {
                limit: charged.limit,
                entry: format!(
                    "{:?}, read a second time for its cell attributes",
                    charged.part
                ),
            };
        }
        ReadError::corrupt(format!("{} does not parse: {error}", charged.part))
    }
}

pub(crate) fn raw_attribute(element: &BytesStart<'_>, name: &[u8]) -> Option<Vec<u8>> {
    element
        .try_get_attribute(name)
        .ok()
        .flatten()
        .map(|attribute| attribute.value.into_owned())
}

/// `B3` → `(2, 1)`. A reference past the grid is left for
/// [`check_position`] to refuse, with the same message as the row reader.
fn parse_cell_reference(reference: &[u8]) -> Result<(u32, u32), ReadError> {
    let bad = || {
        ReadError::corrupt(format!(
            "cell reference {:?} is not in A1 notation",
            String::from_utf8_lossy(reference)
        ))
    };
    let letters = reference
        .iter()
        .take_while(|b| b.is_ascii_alphabetic())
        .count();
    if letters == 0 || letters == reference.len() {
        return Err(bad());
    }
    let mut col: u32 = 0;
    for letter in &reference[..letters] {
        col = col
            .checked_mul(26)
            .and_then(|c| c.checked_add(u32::from(letter.to_ascii_uppercase() - b'A') + 1))
            .ok_or_else(bad)?;
    }
    let row: u32 = std::str::from_utf8(&reference[letters..])
        .ok()
        .and_then(|digits| digits.parse().ok())
        .filter(|row| *row >= 1)
        .ok_or_else(bad)?;
    Ok((row - 1, col - 1))
}

/// A stream over one part's inflated bytes, owning its file handle.
///
/// The archive is used only to locate the part; the stream then reads the raw
/// data from its own handle, which is what lets it outlive the archive.
pub(crate) fn open_part_stream(path: &Path, part: &str) -> Result<Box<dyn Read + Send>, ReadError> {
    let io_error = |error: io::Error| ReadError::Io {
        detail: format!("cannot read {}: {error}", path.display()),
    };

    let (method, size, start) = {
        let file = File::open(path).map_err(io_error)?;
        let mut archive = ZipArchive::new(BufReader::new(file)).map_err(map_zip_error)?;
        let entry = archive.by_name(part).map_err(|error| match error {
            ZipError::FileNotFound => {
                ReadError::corrupt(format!("the archive has no {part:?} part"))
            }
            other => map_zip_error(other),
        })?;
        let start = entry
            .data_start()
            .ok_or_else(|| ReadError::corrupt(format!("{part:?} has no data")))?;
        (entry.compression(), entry.compressed_size(), start)
    };

    let mut file = File::open(path).map_err(io_error)?;
    file.seek(SeekFrom::Start(start)).map_err(io_error)?;
    let raw = BufReader::new(file).take(size);

    match method {
        CompressionMethod::Stored => Ok(Box::new(raw)),
        CompressionMethod::Deflated => Ok(Box::new(DeflateDecoder::new(raw))),
        other => Err(ReadError::corrupt(format!(
            "{part:?} is compressed with {other}, which a workbook does not use"
        ))),
    }
}

/// Charges every byte that leaves the inflater against the budget, and stops
/// the stream the moment it is crossed.
pub(crate) struct Charged {
    pub(crate) inner: Box<dyn Read + Send>,
    pub(crate) used: u64,
    pub(crate) limit: u64,
    pub(crate) part: String,
    pub(crate) exceeded: bool,
}

impl Read for Charged {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.used = self.used.saturating_add(read as u64);
        if self.used > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("decompressed byte budget exceeded"));
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a1_references() {
        assert_eq!(parse_cell_reference(b"A1").unwrap(), (0, 0));
        assert_eq!(parse_cell_reference(b"B3").unwrap(), (2, 1));
        assert_eq!(parse_cell_reference(b"AA10").unwrap(), (9, 26));
        assert_eq!(
            parse_cell_reference(b"XFD1048576").unwrap(),
            (1_048_575, 16_383)
        );
    }

    #[test]
    fn refuses_what_is_not_a_cell_reference() {
        for reference in [&b"A"[..], b"1", b"A0", b"", b"A1B"] {
            assert!(parse_cell_reference(reference).is_err(), "{reference:?}");
        }
    }
}
