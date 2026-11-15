//! Node binding.
//!
//! Two rules shape everything here.
//!
//! **Nothing runs on the main thread.** Opening the archive walks it to charge
//! the byte budget, and reading a sheet parses XML; both are measured in
//! hundreds of milliseconds on a large workbook. So every call that touches the
//! file returns an `AsyncTask`, which napi-rs runs on the libuv threadpool. The
//! event loop of a server importing a 600 000 row workbook keeps its cadence.
//!
//! **The consumer pulls.** The binding is a cursor, not a callback: JavaScript
//! asks for the next batch and Rust answers with at most that many rows. Pulling
//! rather than pushing means no `ThreadsafeFunction`, and backpressure falls out
//! on its own — a `Readable` in object mode only calls `_read()` when its buffer
//! has drained.
//!
//! Rows cross the boundary by the batch. One FFI call per row over 600 000 rows
//! would cost more than the parsing does.

#![deny(clippy::all)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};

use napi::bindgen_prelude::{
    AsyncTask, Buffer, Either4, Env, Null, Result, Task, Uint32Array, Uint8Array,
};
use napi::Error;
use napi_derive::napi;

use xlsx_core::{
    inspect_sheet, inspect_workbook, validate_sheet_name, CellReaderOptions, CellStyle, CellType,
    CellValue, Color, Fill, Font, NumFmt, ReadError, ReaderOptions, SheetInfo, SheetInspection,
    WorkbookInspection, WriteError, WriterOptions, XlsxCells, XlsxReader, XlsxWriter,
};

/// One sheet, as the workbook declares it.
#[napi(object)]
pub struct JsSheetInfo {
    /// Sheet name, as it appears on the tab.
    pub name: String,
    /// The identifier the workbook gives the sheet, unrelated to its position.
    pub sheet_id: u32,
    /// Whether the sheet shows on the tab bar.
    #[napi(ts_type = "'visible' | 'hidden' | 'veryHidden'")]
    pub state: String,
}

impl From<&SheetInfo> for JsSheetInfo {
    fn from(sheet: &SheetInfo) -> Self {
        Self {
            name: sheet.name.clone(),
            sheet_id: sheet.sheet_id,
            state: sheet.state.as_str().to_owned(),
        }
    }
}

/// Reader options. Both budgets are required — see the crate's decision on why
/// there is no permissive default.
#[napi(object)]
pub struct JsReaderOptions {
    /// Sheet to read. Defaults to the first one in the workbook.
    pub sheet: Option<String>,
    /// Bytes the archive may expand to, every entry counted.
    pub max_decompressed_bytes: f64,
    /// Rows the sheet may yield, header row included.
    pub max_rows: f64,
}

/// Options for a peek, which reads no row and so needs no row budget.
#[napi(object)]
pub struct JsPeekOptions {
    /// Bytes the archive may expand to.
    pub max_decompressed_bytes: f64,
}

/// A cell as JavaScript sees it: text, number, boolean, or `null` for a blank.
///
/// Dates arrive as ISO 8601 strings in UTC, and error cells as their sheet
/// spelling (`#DIV/0!`). Wrapping every cell in an object to carry its type
/// would allocate six million objects on a 600 000 row workbook to describe
/// what the column already says.
type JsCell = Either4<String, f64, bool, Null>;

/// A batch of rows: their values, and beside them their positions in the sheet.
///
/// Two parallel arrays rather than one object per row: the facade assembles the
/// `{ row, values }` objects on its side, and the crossing carries one integer
/// per row on top of the values it already carried.
#[napi(object)]
pub struct JsRowBatch {
    /// 0-based position in the sheet of each row, in order.
    pub positions: Vec<u32>,
    /// Values of each row, at their column index.
    #[napi(ts_type = "Array<Array<string | number | boolean | null>>")]
    pub rows: Vec<Vec<JsCell>>,
}

/// A batch of rows, or `null` once the sheet is finished.
type JsBatch = Option<JsRowBatch>;

/// What a cursor reads through: rows, or cells.
pub trait Source: Sized + Send {
    type Options: Copy + Send;
    fn open(path: &Path, options: Self::Options) -> std::result::Result<Self, ReadError>;
    fn select(&mut self, name: &str) -> std::result::Result<(), ReadError>;
    fn sheets(&self) -> &[SheetInfo];
}

impl Source for XlsxReader {
    type Options = ReaderOptions;
    fn open(path: &Path, options: ReaderOptions) -> std::result::Result<Self, ReadError> {
        XlsxReader::open(path, options)
    }
    fn select(&mut self, name: &str) -> std::result::Result<(), ReadError> {
        XlsxReader::select(self, name)
    }
    fn sheets(&self) -> &[SheetInfo] {
        XlsxReader::sheets(self)
    }
}

impl Source for XlsxCells {
    type Options = CellReaderOptions;
    fn open(path: &Path, options: CellReaderOptions) -> std::result::Result<Self, ReadError> {
        XlsxCells::open(path, options)
    }
    fn select(&mut self, name: &str) -> std::result::Result<(), ReadError> {
        XlsxCells::select(self, name)
    }
    fn sheets(&self) -> &[SheetInfo] {
        XlsxCells::sheets(self)
    }
}

struct State<R: Source> {
    path: PathBuf,
    options: R::Options,
    sheet: Option<String>,
    reader: Option<R>,
}

impl<R: Source> State<R> {
    /// The reader, opened on first use.
    ///
    /// Opening is deferred so that the constructor — which JavaScript calls on
    /// the main thread — does no I/O at all. The first pull pays for the open,
    /// on the threadpool, where it belongs.
    fn reader(&mut self) -> Result<&mut R> {
        if self.reader.is_none() {
            let mut reader = R::open(&self.path, self.options).map_err(to_js_error)?;
            if let Some(name) = &self.sheet {
                reader.select(name).map_err(to_js_error)?;
            }
            self.reader = Some(reader);
        }

        self.reader.as_mut().ok_or_else(closed_error)
    }
}

/// What the tasks and the main thread share.
///
/// The flag is deliberately outside the mutex. A read in flight holds that lock
/// for as long as it takes to walk the archive — the better part of a second on
/// an ordinary workbook, longer on one sized just under the byte budget — and
/// `close()` runs on the main thread, called by `_destroy` the moment a consumer
/// abandons a stream. Taking the lock there would hand any client the ability to
/// stall the event loop by starting an upload and cutting it: exactly the
/// invariant this binding exists to hold.
struct Shared<R: Source> {
    state: Mutex<State<R>>,
    closed: AtomicBool,
}

impl<R: Source> Shared<R> {
    fn new(path: String, options: R::Options, sheet: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                path: PathBuf::from(path),
                options,
                sheet,
                reader: None,
            }),
            closed: AtomicBool::new(false),
        })
    }

    /// Mark the cursor closed and release the archive if no read holds it.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);

        // Never blocks. If a read is in flight it holds the lock, sees the flag
        // on its way out, and releases the archive itself.
        if let Ok(mut state) = self.state.try_lock() {
            state.reader = None;
        }
    }

    /// Run `work` against the reader, opening it if this is the first pull.
    fn with_reader<T>(&self, work: impl FnOnce(&mut R) -> Result<T>) -> Result<T> {
        if self.closed.load(Ordering::Acquire) {
            return Err(closed_error());
        }

        let mut state = lock(&self.state);
        let outcome = state.reader().and_then(work);

        // `close()` may have arrived while the archive was being walked, in
        // which case it could not take the lock and left the archive to us.
        if self.closed.load(Ordering::Acquire) {
            state.reader = None;
            return Err(closed_error());
        }

        outcome
    }
}

fn closed_error() -> Error {
    Error::from_reason("CLOSED: this reader has been closed")
}

/// Carry the typed error's discriminant across a boundary that has no room for
/// one. `napi` builds its JS `code` from a fixed `Status` enum, so the code
/// travels at the head of the message and the JavaScript facade lifts it back
/// out into `error.code`.
fn to_js_error(error: ReadError) -> Error {
    Error::from_reason(format!("{}: {error}", error.code()))
}

/// A poisoned lock means a previous read panicked. The cursor's state is a
/// position in a file, not an invariant that a panic can leave half-written, so
/// the guard is taken anyway rather than turning one failure into every
/// subsequent call failing.
fn lock<R: Source>(state: &Mutex<State<R>>) -> MutexGuard<'_, State<R>> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A cursor over one worksheet.
///
/// Single-consumer: the JavaScript facade is what drives it, one pull at a time.
/// Two overlapping `nextBatch` calls are serialised rather than interleaved, but
/// which of the two gets which batch is not defined.
#[napi]
pub struct XlsxCursor {
    shared: Arc<Shared<XlsxReader>>,
}

#[napi]
impl XlsxCursor {
    /// Build a cursor. Does no I/O: the file is opened on the first pull.
    #[napi(constructor)]
    pub fn new(path: String, options: JsReaderOptions) -> Result<Self> {
        let max_decompressed_bytes =
            budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;
        let max_rows = budget(options.max_rows, "maxRows")?;

        Ok(Self {
            shared: Shared::new(
                path,
                ReaderOptions {
                    max_decompressed_bytes,
                    max_rows,
                },
                options.sheet,
            ),
        })
    }

    /// The sheets the workbook declares.
    #[napi(ts_return_type = "Promise<Array<JsSheetInfo>>")]
    pub fn sheets(&self) -> AsyncTask<SheetsTask<XlsxReader>> {
        AsyncTask::new(SheetsTask {
            shared: Arc::clone(&self.shared),
        })
    }

    /// Pull at most `size` rows. Resolves to `null` once the sheet is finished.
    #[napi(ts_return_type = "Promise<JsRowBatch | null>")]
    pub fn next_batch(&self, size: u32) -> AsyncTask<NextBatchTask> {
        AsyncTask::new(NextBatchTask {
            shared: Arc::clone(&self.shared),
            size: size as usize,
        })
    }

    /// Release the archive now rather than at the next garbage collection.
    ///
    /// A consumer that stops half way through an import — a validation failure
    /// on row three — would otherwise hold two descriptors open for as long as
    /// the object lives.
    #[napi]
    pub fn close(&self) {
        self.shared.close();
    }
}

/// Cell reader options. Both budgets are required.
#[napi(object)]
pub struct JsCellReaderOptions {
    /// Sheet to read. Defaults to the first one in the workbook.
    pub sheet: Option<String>,
    /// Bytes the archive may expand to, the sheet's second reading included.
    pub max_decompressed_bytes: f64,
    /// Cells the sheet may yield.
    pub max_cells: f64,
}

/// A cell's formula — `CT_CellFormula`.
#[napi(object)]
pub struct JsFormula {
    /// The formula as written, without the leading `=`.
    pub text: String,
    /// How the formula is shared — `ST_CellFormulaType`.
    #[napi(
        js_name = "type",
        ts_type = "'normal' | 'shared' | 'array' | 'dataTable'"
    )]
    pub formula_type: String,
    /// The range a shared or array formula covers, on the cell declaring it.
    #[napi(js_name = "ref")]
    pub reference: Option<String>,
}

/// A batch of cells, as parallel arrays the facade assembles into objects.
///
/// Numbers travel in typed arrays and the cell type as a one-byte code, and
/// formulas only for the cells that have one: a batch of a thousand cells is a
/// handful of allocations on the JavaScript side rather than several thousand.
#[napi(object)]
pub struct JsCellBatch {
    /// 0-based row of each cell.
    pub rows: Uint32Array,
    /// 0-based column of each cell.
    pub cols: Uint32Array,
    /// What each value is, as an index into [`CELL_TYPES`].
    pub types: Uint8Array,
    /// Each cell's value.
    #[napi(ts_type = "Array<string | number | boolean | null>")]
    pub values: Vec<JsCell>,
    /// Each cell's index in the workbook's cell formats.
    pub style_ids: Uint32Array,
    /// Position in the batch of each cell that has a formula, in order.
    pub formula_cells: Uint32Array,
    /// The formulas of those cells, in the same order.
    pub formulas: Vec<JsFormula>,
}

/// The cell types, in the order of their codes in [`JsCellBatch::types`]. The
/// facade holds the same list.
const CELL_TYPES: [CellType; 7] = [
    CellType::String,
    CellType::Number,
    CellType::Boolean,
    CellType::Date,
    CellType::Duration,
    CellType::Error,
    CellType::Empty,
];

fn cell_type_code(cell_type: CellType) -> u8 {
    CELL_TYPES
        .iter()
        .position(|candidate| *candidate == cell_type)
        .map_or(u8::MAX, |code| code as u8)
}

/// A cursor over the cells of one worksheet. Same protocol as [`XlsxCursor`].
#[napi]
pub struct XlsxCellCursor {
    shared: Arc<Shared<XlsxCells>>,
}

#[napi]
impl XlsxCellCursor {
    /// Build a cursor. Does no I/O: the file is opened on the first pull.
    #[napi(constructor)]
    pub fn new(path: String, options: JsCellReaderOptions) -> Result<Self> {
        let max_decompressed_bytes =
            budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;
        let max_cells = budget(options.max_cells, "maxCells")?;

        Ok(Self {
            shared: Shared::new(
                path,
                CellReaderOptions {
                    max_decompressed_bytes,
                    max_cells,
                },
                options.sheet,
            ),
        })
    }

    /// The sheets the workbook declares.
    #[napi(ts_return_type = "Promise<Array<JsSheetInfo>>")]
    pub fn sheets(&self) -> AsyncTask<SheetsTask<XlsxCells>> {
        AsyncTask::new(SheetsTask {
            shared: Arc::clone(&self.shared),
        })
    }

    /// Pull at most `size` cells. Resolves to `null` once the sheet is finished.
    #[napi(ts_return_type = "Promise<JsCellBatch | null>")]
    pub fn next_batch(&self, size: u32) -> AsyncTask<NextCellBatchTask> {
        AsyncTask::new(NextCellBatchTask {
            shared: Arc::clone(&self.shared),
            size: size as usize,
        })
    }

    /// Release the archive now rather than at the next garbage collection.
    #[napi]
    pub fn close(&self) {
        self.shared.close();
    }
}

pub struct NextCellBatchTask {
    shared: Arc<Shared<XlsxCells>>,
    size: usize,
}

impl Task for NextCellBatchTask {
    type Output = Option<JsCellBatch>;
    type JsValue = Option<JsCellBatch>;

    fn compute(&mut self) -> Result<Self::Output> {
        let size = self.size;

        self.shared.with_reader(|reader| {
            let Some(cells) = reader.next_batch(size).map_err(to_js_error)? else {
                return Ok(None);
            };

            let mut rows = Vec::with_capacity(cells.len());
            let mut cols = Vec::with_capacity(cells.len());
            let mut types = Vec::with_capacity(cells.len());
            let mut values = Vec::with_capacity(cells.len());
            let mut style_ids = Vec::with_capacity(cells.len());
            let mut formula_cells = Vec::new();
            let mut formulas = Vec::new();
            for (index, cell) in cells.into_iter().enumerate() {
                rows.push(cell.row);
                cols.push(cell.col);
                types.push(cell_type_code(cell.cell_type));
                values.push(to_js_cell(cell.value));
                style_ids.push(cell.style_id);
                if let Some(formula) = cell.formula {
                    formula_cells.push(index as u32);
                    formulas.push(JsFormula {
                        text: formula.text,
                        formula_type: formula.formula_type.as_str().to_owned(),
                        reference: formula.reference,
                    });
                }
            }
            Ok(Some(JsCellBatch {
                rows: Uint32Array::new(rows),
                cols: Uint32Array::new(cols),
                types: Uint8Array::new(types),
                values,
                style_ids: Uint32Array::new(style_ids),
                formula_cells: Uint32Array::new(formula_cells),
                formulas,
            }))
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

fn budget(value: f64, name: &str) -> Result<u64> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::from_reason(format!(
            "INVALID_OPTION: {name} must be a non-negative number, got {value}"
        )));
    }
    Ok(value as u64)
}

/// Peek at the sheets a workbook declares, without opening it to read rows.
///
/// Separate from [`XlsxCursor::sheets`] because it takes a different path
/// through the file: two small parts inflated and counted, rather than a walk of
/// the whole archive. The bound still applies — it is applied to what gets read.
#[napi(js_name = "peekSheets", ts_return_type = "Promise<Array<JsSheetInfo>>")]
pub fn peek_sheets(path: String, options: JsPeekOptions) -> Result<AsyncTask<PeekSheetsTask>> {
    let max_decompressed_bytes = budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;

    Ok(AsyncTask::new(PeekSheetsTask {
        path: PathBuf::from(path),
        max_decompressed_bytes,
    }))
}

pub struct PeekSheetsTask {
    path: PathBuf,
    max_decompressed_bytes: u64,
}

impl Task for PeekSheetsTask {
    type Output = Vec<JsSheetInfo>;
    type JsValue = Vec<JsSheetInfo>;

    fn compute(&mut self) -> Result<Self::Output> {
        let sheets = XlsxReader::peek_sheets(&self.path, self.max_decompressed_bytes)
            .map_err(to_js_error)?;

        Ok(sheets.iter().map(JsSheetInfo::from).collect())
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

// ------------------------------------------------------------- inspection --

/// `CT_Color`, as declared.
#[napi(object)]
pub struct JsColor {
    pub rgb: Option<String>,
    pub theme: Option<u32>,
    pub indexed: Option<u32>,
    pub auto: bool,
    pub tint: Option<f64>,
}

impl From<Color> for JsColor {
    fn from(color: Color) -> Self {
        Self {
            rgb: color.rgb,
            theme: color.theme,
            indexed: color.indexed,
            auto: color.auto,
            tint: color.tint,
        }
    }
}

#[napi(object)]
pub struct JsNumFmt {
    pub num_fmt_id: u32,
    pub format_code: Option<String>,
}

#[napi(object)]
pub struct JsFont {
    pub name: Option<String>,
    pub size: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    #[napi(ts_type = "'single' | 'double' | 'singleAccounting' | 'doubleAccounting' | 'none'")]
    pub underline: Option<String>,
    #[napi(ts_type = "'baseline' | 'superscript' | 'subscript'")]
    pub vert_align: Option<String>,
    pub color: Option<JsColor>,
}

#[napi(object)]
pub struct JsPatternFill {
    pub pattern_type: Option<String>,
    pub fg_color: Option<JsColor>,
    pub bg_color: Option<JsColor>,
}

#[napi(object)]
pub struct JsGradientStop {
    pub position: f64,
    pub color: JsColor,
}

#[napi(object)]
pub struct JsGradientFill {
    #[napi(js_name = "type", ts_type = "'linear' | 'path'")]
    pub gradient_type: String,
    pub degree: Option<f64>,
    pub stops: Vec<JsGradientStop>,
}

#[napi(object)]
pub struct JsFill {
    pub pattern_fill: Option<JsPatternFill>,
    pub gradient_fill: Option<JsGradientFill>,
}

#[napi(object)]
pub struct JsAlignment {
    pub horizontal: Option<String>,
    pub vertical: Option<String>,
    pub wrap_text: bool,
    pub shrink_to_fit: bool,
    pub indent: Option<u32>,
    pub text_rotation: Option<u32>,
}

#[napi(object)]
pub struct JsCellStyle {
    pub num_fmt_ref: u32,
    pub font_ref: Option<u32>,
    pub fill_ref: Option<u32>,
    pub alignment: JsAlignment,
}

impl From<NumFmt> for JsNumFmt {
    fn from(num_fmt: NumFmt) -> Self {
        Self {
            num_fmt_id: num_fmt.num_fmt_id,
            format_code: num_fmt.format_code,
        }
    }
}

impl From<Font> for JsFont {
    fn from(font: Font) -> Self {
        Self {
            name: font.name,
            size: font.size,
            bold: font.bold,
            italic: font.italic,
            strike: font.strike,
            underline: font.underline,
            vert_align: font.vert_align,
            color: font.color.map(JsColor::from),
        }
    }
}

impl From<Fill> for JsFill {
    fn from(fill: Fill) -> Self {
        Self {
            pattern_fill: fill.pattern_fill.map(|pattern| JsPatternFill {
                pattern_type: pattern.pattern_type,
                fg_color: pattern.fg_color.map(JsColor::from),
                bg_color: pattern.bg_color.map(JsColor::from),
            }),
            gradient_fill: fill.gradient_fill.map(|gradient| JsGradientFill {
                gradient_type: gradient.gradient_type,
                degree: gradient.degree,
                stops: gradient
                    .stops
                    .into_iter()
                    .map(|stop| JsGradientStop {
                        position: stop.position,
                        color: stop.color.into(),
                    })
                    .collect(),
            }),
        }
    }
}

impl From<CellStyle> for JsCellStyle {
    fn from(style: CellStyle) -> Self {
        Self {
            num_fmt_ref: style.num_fmt_ref,
            font_ref: style.font_ref,
            fill_ref: style.fill_ref,
            alignment: JsAlignment {
                horizontal: style.alignment.horizontal,
                vertical: style.alignment.vertical,
                wrap_text: style.alignment.wrap_text,
                shrink_to_fit: style.alignment.shrink_to_fit,
                indent: style.alignment.indent,
                text_rotation: style.alignment.text_rotation,
            },
        }
    }
}

#[napi(object)]
pub struct JsDefinedName {
    pub name: String,
    pub formula: String,
    pub local_sheet_id: Option<u32>,
    pub hidden: bool,
    pub comment: Option<String>,
}

#[napi(object)]
pub struct JsDocumentProperties {
    pub creator: Option<String>,
    pub last_modified_by: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub title: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub keywords: Option<String>,
    pub category: Option<String>,
    pub application: Option<String>,
    pub app_version: Option<String>,
    pub company: Option<String>,
}

#[napi(object)]
pub struct JsWorkbookParts {
    pub vba_project: bool,
    pub pivot_tables: bool,
    pub external_links: bool,
    pub drawings: bool,
}

#[napi(object)]
pub struct JsWorkbookInspection {
    pub sheets: Vec<JsSheetInfo>,
    pub defined_names: Vec<JsDefinedName>,
    pub properties: JsDocumentProperties,
    pub date1904: bool,
    pub parts: JsWorkbookParts,
    pub num_fmts: Vec<JsNumFmt>,
    pub fonts: Vec<JsFont>,
    pub fills: Vec<JsFill>,
    pub styles: Vec<JsCellStyle>,
}

impl From<WorkbookInspection> for JsWorkbookInspection {
    fn from(inspection: WorkbookInspection) -> Self {
        let p = inspection.properties;
        Self {
            sheets: inspection.sheets.iter().map(JsSheetInfo::from).collect(),
            defined_names: inspection
                .defined_names
                .into_iter()
                .map(|name| JsDefinedName {
                    name: name.name,
                    formula: name.formula,
                    local_sheet_id: name.local_sheet_id,
                    hidden: name.hidden,
                    comment: name.comment,
                })
                .collect(),
            properties: JsDocumentProperties {
                creator: p.creator,
                last_modified_by: p.last_modified_by,
                created: p.created,
                modified: p.modified,
                title: p.title,
                subject: p.subject,
                description: p.description,
                keywords: p.keywords,
                category: p.category,
                application: p.application,
                app_version: p.app_version,
                company: p.company,
            },
            date1904: inspection.date1904,
            parts: JsWorkbookParts {
                vba_project: inspection.parts.vba_project,
                pivot_tables: inspection.parts.pivot_tables,
                external_links: inspection.parts.external_links,
                drawings: inspection.parts.drawings,
            },
            num_fmts: inspection
                .num_fmts
                .into_iter()
                .map(JsNumFmt::from)
                .collect(),
            fonts: inspection.fonts.into_iter().map(JsFont::from).collect(),
            fills: inspection.fills.into_iter().map(JsFill::from).collect(),
            styles: inspection
                .styles
                .into_iter()
                .map(JsCellStyle::from)
                .collect(),
        }
    }
}

/// Inspect a workbook: properties, defined names, parts, styles. Reads no sheet.
#[napi(
    js_name = "inspectWorkbook",
    ts_return_type = "Promise<JsWorkbookInspection>"
)]
pub fn js_inspect_workbook(
    path: String,
    options: JsPeekOptions,
) -> Result<AsyncTask<InspectWorkbookTask>> {
    let max_decompressed_bytes = budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;
    Ok(AsyncTask::new(InspectWorkbookTask {
        path: PathBuf::from(path),
        max_decompressed_bytes,
    }))
}

pub struct InspectWorkbookTask {
    path: PathBuf,
    max_decompressed_bytes: u64,
}

impl Task for InspectWorkbookTask {
    type Output = JsWorkbookInspection;
    type JsValue = JsWorkbookInspection;

    fn compute(&mut self) -> Result<Self::Output> {
        inspect_workbook(&self.path, self.max_decompressed_bytes)
            .map(JsWorkbookInspection::from)
            .map_err(to_js_error)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi(object)]
pub struct JsPane {
    pub x_split: Option<f64>,
    pub y_split: Option<f64>,
    pub top_left_cell: Option<String>,
    #[napi(ts_type = "'split' | 'frozen' | 'frozenSplit'")]
    pub state: String,
}

#[napi(object)]
pub struct JsColumnRange {
    pub first_col: u32,
    pub last_col: u32,
    pub width: Option<f64>,
    pub hidden: bool,
    pub outline_level: Option<u32>,
}

#[napi(object)]
pub struct JsHyperlink {
    #[napi(js_name = "ref")]
    pub reference: String,
    pub target: Option<String>,
    pub location: Option<String>,
    pub tooltip: Option<String>,
    pub display: Option<String>,
}

#[napi(object)]
pub struct JsTableColumn {
    pub name: String,
    pub totals_row_function: Option<String>,
    pub totals_row_label: Option<String>,
    pub totals_row_formula: Option<String>,
    pub calculated_column_formula: Option<String>,
}

#[napi(object)]
pub struct JsTable {
    pub name: String,
    pub display_name: String,
    #[napi(js_name = "ref")]
    pub reference: String,
    pub header_row_count: u32,
    pub totals_row_count: u32,
    pub table_columns: Vec<JsTableColumn>,
}

#[napi(object)]
pub struct JsDataValidation {
    pub sqref: Vec<String>,
    pub extension: bool,
    #[napi(js_name = "type")]
    pub validation_type: Option<String>,
    pub operator: Option<String>,
    pub allow_blank: bool,
    pub show_drop_down: bool,
    pub show_input_message: bool,
    pub show_error_message: bool,
    pub error_style: Option<String>,
    pub error_title: Option<String>,
    pub error: Option<String>,
    pub prompt_title: Option<String>,
    pub prompt: Option<String>,
    pub formula1: Option<String>,
    pub formula2: Option<String>,
}

#[napi(object)]
pub struct JsCfRule {
    #[napi(js_name = "type")]
    pub rule_type: String,
    pub priority: u32,
    pub operator: Option<String>,
    pub stop_if_true: bool,
    pub text: Option<String>,
    pub time_period: Option<String>,
    pub formulas: Vec<String>,
    pub dxf_id: Option<u32>,
}

#[napi(object)]
pub struct JsConditionalFormatting {
    pub sqref: Vec<String>,
    pub extension: bool,
    pub rules: Vec<JsCfRule>,
}

#[napi(object)]
pub struct JsSheetProtection {
    pub password_protected: bool,
    pub sheet: bool,
    pub objects: bool,
    pub scenarios: bool,
    pub format_cells: bool,
    pub format_columns: bool,
    pub format_rows: bool,
    pub insert_columns: bool,
    pub insert_rows: bool,
    pub insert_hyperlinks: bool,
    pub delete_columns: bool,
    pub delete_rows: bool,
    pub select_locked_cells: bool,
    pub sort: bool,
    pub auto_filter: bool,
    pub pivot_tables: bool,
    pub select_unlocked_cells: bool,
}

#[napi(object)]
pub struct JsAutoFilter {
    #[napi(js_name = "ref")]
    pub reference: String,
}

#[napi(object)]
pub struct JsComment {
    #[napi(js_name = "ref")]
    pub reference: String,
    pub author_ref: Option<u32>,
    pub text: String,
}

#[napi(object)]
pub struct JsSheetInspection {
    pub dimension: Option<String>,
    pub tab_color: Option<JsColor>,
    pub pane: Option<JsPane>,
    pub cols: Vec<JsColumnRange>,
    pub hidden_rows: Vec<u32>,
    pub merge_cells: Vec<String>,
    pub hyperlinks: Vec<JsHyperlink>,
    pub tables: Vec<JsTable>,
    pub data_validations: Vec<JsDataValidation>,
    pub conditional_formatting: Vec<JsConditionalFormatting>,
    pub sheet_protection: Option<JsSheetProtection>,
    pub auto_filter: Option<JsAutoFilter>,
    pub comment_authors: Vec<String>,
    pub comments: Vec<JsComment>,
    pub picture_count: u32,
}

impl From<SheetInspection> for JsSheetInspection {
    fn from(s: SheetInspection) -> Self {
        Self {
            dimension: s.dimension,
            tab_color: s.tab_color.map(JsColor::from),
            pane: s.pane.map(|p| JsPane {
                x_split: p.x_split,
                y_split: p.y_split,
                top_left_cell: p.top_left_cell,
                state: p.state,
            }),
            cols: s
                .cols
                .into_iter()
                .map(|c| JsColumnRange {
                    first_col: c.first_col,
                    last_col: c.last_col,
                    width: c.width,
                    hidden: c.hidden,
                    outline_level: c.outline_level,
                })
                .collect(),
            hidden_rows: s.hidden_rows,
            merge_cells: s.merge_cells,
            hyperlinks: s
                .hyperlinks
                .into_iter()
                .map(|h| JsHyperlink {
                    reference: h.reference,
                    target: h.target,
                    location: h.location,
                    tooltip: h.tooltip,
                    display: h.display,
                })
                .collect(),
            tables: s
                .tables
                .into_iter()
                .map(|t| JsTable {
                    name: t.name,
                    display_name: t.display_name,
                    reference: t.reference,
                    header_row_count: t.header_row_count,
                    totals_row_count: t.totals_row_count,
                    table_columns: t
                        .columns
                        .into_iter()
                        .map(|c| JsTableColumn {
                            name: c.name,
                            totals_row_function: c.totals_row_function,
                            totals_row_label: c.totals_row_label,
                            totals_row_formula: c.totals_row_formula,
                            calculated_column_formula: c.calculated_column_formula,
                        })
                        .collect(),
                })
                .collect(),
            data_validations: s
                .data_validations
                .into_iter()
                .map(|v| JsDataValidation {
                    sqref: v.sqref,
                    extension: v.extension,
                    validation_type: v.validation_type,
                    operator: v.operator,
                    allow_blank: v.allow_blank,
                    show_drop_down: v.show_drop_down,
                    show_input_message: v.show_input_message,
                    show_error_message: v.show_error_message,
                    error_style: v.error_style,
                    error_title: v.error_title,
                    error: v.error,
                    prompt_title: v.prompt_title,
                    prompt: v.prompt,
                    formula1: v.formula1,
                    formula2: v.formula2,
                })
                .collect(),
            conditional_formatting: s
                .conditional_formatting
                .into_iter()
                .map(|f| JsConditionalFormatting {
                    sqref: f.sqref,
                    extension: f.extension,
                    rules: f
                        .rules
                        .into_iter()
                        .map(|r| JsCfRule {
                            rule_type: r.rule_type,
                            priority: r.priority,
                            operator: r.operator,
                            stop_if_true: r.stop_if_true,
                            text: r.text,
                            time_period: r.time_period,
                            formulas: r.formulas,
                            dxf_id: r.dxf_id,
                        })
                        .collect(),
                })
                .collect(),
            sheet_protection: s.sheet_protection.map(|p| JsSheetProtection {
                password_protected: p.password_protected,
                sheet: p.sheet,
                objects: p.objects,
                scenarios: p.scenarios,
                format_cells: p.format_cells,
                format_columns: p.format_columns,
                format_rows: p.format_rows,
                insert_columns: p.insert_columns,
                insert_rows: p.insert_rows,
                insert_hyperlinks: p.insert_hyperlinks,
                delete_columns: p.delete_columns,
                delete_rows: p.delete_rows,
                select_locked_cells: p.select_locked_cells,
                sort: p.sort,
                auto_filter: p.auto_filter,
                pivot_tables: p.pivot_tables,
                select_unlocked_cells: p.select_unlocked_cells,
            }),
            auto_filter: s.auto_filter.map(|reference| JsAutoFilter { reference }),
            comment_authors: s.comment_authors,
            comments: s
                .comments
                .into_iter()
                .map(|c| JsComment {
                    reference: c.reference,
                    author_ref: c.author_ref,
                    text: c.text,
                })
                .collect(),
            picture_count: s.picture_count,
        }
    }
}

/// Options for a sheet inspection.
#[napi(object)]
pub struct JsInspectSheetOptions {
    /// Sheet to inspect.
    pub sheet: String,
    /// Bytes the parts read may expand to, the sheet part included.
    pub max_decompressed_bytes: f64,
}

/// Inspect a sheet: everything it declares besides its cells.
#[napi(
    js_name = "inspectSheet",
    ts_return_type = "Promise<JsSheetInspection>"
)]
pub fn js_inspect_sheet(
    path: String,
    options: JsInspectSheetOptions,
) -> Result<AsyncTask<InspectSheetTask>> {
    let max_decompressed_bytes = budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;
    Ok(AsyncTask::new(InspectSheetTask {
        path: PathBuf::from(path),
        sheet: options.sheet,
        max_decompressed_bytes,
    }))
}

pub struct InspectSheetTask {
    path: PathBuf,
    sheet: String,
    max_decompressed_bytes: u64,
}

impl Task for InspectSheetTask {
    type Output = JsSheetInspection;
    type JsValue = JsSheetInspection;

    fn compute(&mut self) -> Result<Self::Output> {
        inspect_sheet(&self.path, &self.sheet, self.max_decompressed_bytes)
            .map(JsSheetInspection::from)
            .map_err(to_js_error)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct SheetsTask<R: Source> {
    shared: Arc<Shared<R>>,
}

impl<R: Source> Task for SheetsTask<R> {
    type Output = Vec<JsSheetInfo>;
    type JsValue = Vec<JsSheetInfo>;

    fn compute(&mut self) -> Result<Self::Output> {
        self.shared
            .with_reader(|reader| Ok(reader.sheets().iter().map(JsSheetInfo::from).collect()))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct NextBatchTask {
    shared: Arc<Shared<XlsxReader>>,
    size: usize,
}

impl Task for NextBatchTask {
    type Output = JsBatch;
    type JsValue = JsBatch;

    fn compute(&mut self) -> Result<Self::Output> {
        let size = self.size;

        self.shared.with_reader(|reader| {
            let Some(rows) = reader.next_batch(size).map_err(to_js_error)? else {
                return Ok(None);
            };

            // Shaped into its JavaScript form here, on the threadpool. All the
            // main thread has left to do is build the arrays themselves.
            let mut positions = Vec::with_capacity(rows.len());
            let mut values = Vec::with_capacity(rows.len());
            for row in rows {
                positions.push(row.row);
                values.push(row.values.into_iter().map(to_js_cell).collect());
            }
            Ok(Some(JsRowBatch {
                positions,
                rows: values,
            }))
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

fn to_js_cell(value: CellValue) -> JsCell {
    match value {
        CellValue::Empty => Either4::D(Null),
        CellValue::Text(text) => Either4::A(text),
        CellValue::Number(number) => Either4::B(number),
        CellValue::Bool(flag) => Either4::C(flag),
        CellValue::DateTime(iso) => Either4::A(iso),
        CellValue::Error(spelling) => Either4::A(spelling),
    }
}

// ---------------------------------------------------------------- writing --

/// How the workbook is set up. Mirrors [`JsReaderOptions`] in shape: plain
/// data, validated at the boundary.
#[napi(object)]
pub struct JsWriterOptions {
    /// Name on the sheet tab. At most 31 characters, free of `[ ] : * ? / \`.
    pub sheet_name: String,
    /// Column indices whose strings are ISO 8601 timestamps.
    ///
    /// Date-ness cannot be guessed from a value: `"2024-03-25"` is a perfectly
    /// good piece of text, and a reader that decided otherwise would turn a
    /// product reference into a day. It cannot be guessed from a JavaScript
    /// `Date` either without calling `toISOString` once per cell across the
    /// boundary, which is the cost this design exists to avoid. So it is
    /// declared once, for the column, and the facade is what turns a `columns`
    /// declaration into this list.
    pub date_columns: Vec<u32>,
    /// Sheets the workbook may hold. Defaults to 256.
    ///
    /// Each sheet keeps a temporary file open until the workbook is finished,
    /// so this is a ceiling on a real resource, not on the format, which has
    /// none: without it, running out of descriptors fails the sheet that
    /// cannot open one with `IO`.
    pub max_sheets: Option<u32>,

    /// Where the row spill files go. `None` uses the platform temporary
    /// directory.
    pub temp_dir: Option<String>,
}

/// Check a sheet name the way the writer will, without making a workbook.
///
/// Lets the JavaScript facade refuse a bad name in `sheets` where the caller
/// wrote it, rather than at the first row that happens to go there — and
/// without a second copy of the rules to drift from this one.
#[napi(js_name = "validateSheetName")]
pub fn js_validate_sheet_name(name: String) -> Result<()> {
    validate_sheet_name(&name).map_err(to_js_write_error)
}

/// Bytes held between the writing thread and the consumer.
///
/// The whole of the backpressure. `save_to_writer` blocks on a full channel,
/// which stops the sheet being assembled faster than it is drained; measured
/// chunks top out near 30 KB, so this is a ceiling of roughly a quarter of a
/// megabyte in flight.
const CHUNK_BACKLOG: usize = 8;

/// The writing thread's verdict. Spelled out because `Result` in this module is
/// napi's, whose error type is its own.
type WriteOutcome = std::result::Result<(), WriteError>;

enum Stage {
    /// Rows are being appended. The workbook is here.
    Filling(Box<XlsxWriter>),
    /// The workbook is being assembled on its own thread; chunks arrive here.
    Draining {
        /// Behind its own lock rather than the stage's, so that waiting for a
        /// chunk never holds the lock `close` needs.
        chunks: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
        outcome: Arc<Mutex<Option<WriteOutcome>>>,
    },
    /// Finished, failed, or closed.
    Done,
}

struct WriterShared {
    stage: Mutex<Stage>,
    closed: AtomicBool,
}

/// A worksheet being written, one batch of rows at a time.
///
/// Two phases, and the split is a property of the format rather than a choice:
/// rows go to a spill file as they arrive, and the archive is assembled from
/// those files at the end. Nothing reaches the consumer until [`Self::next_chunk`]
/// is called for the first time.
#[napi]
pub struct XlsxSink {
    shared: Arc<WriterShared>,
}

#[napi]
impl XlsxSink {
    /// Open a workbook. Does no I/O beyond preparing the spill directory.
    #[napi(constructor)]
    pub fn new(options: JsWriterOptions) -> Result<Self> {
        let writer = XlsxWriter::new(WriterOptions {
            sheet_name: options.sheet_name,
            max_sheets: options.max_sheets.map(|n| n as usize),
            temp_dir: options.temp_dir.map(PathBuf::from),
        })
        .map_err(to_js_write_error)?;

        Ok(Self {
            shared: Arc::new(WriterShared {
                stage: Mutex::new(Stage::Filling(Box::new(writer))),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Send the rows that follow to `name`, creating the sheet if it is new.
    ///
    /// Naming a sheet that already exists returns to it rather than clashing:
    /// each keeps its own row counter, so a caller can stream a source that is
    /// not sorted by sheet.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn select_sheet(&self, name: String) -> AsyncTask<SelectSheetTask> {
        AsyncTask::new(SelectSheetTask {
            shared: Arc::clone(&self.shared),
            name,
        })
    }

    /// Append rows. Resolves when they are on the spill file.
    ///
    /// Rows cross by the batch for the same reason they do coming the other
    /// way: one FFI call per row over a million rows costs more than the work.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn write_rows(
        &self,
        rows: Vec<Vec<JsCell>>,
        date_columns: Vec<u32>,
    ) -> AsyncTask<WriteRowsTask> {
        AsyncTask::new(WriteRowsTask {
            shared: Arc::clone(&self.shared),
            rows,
            date_columns,
        })
    }

    /// Pull the next piece of the file. Resolves to `null` once it is complete.
    ///
    /// The first call starts the assembly on a thread of its own — deliberately
    /// not one of the libuv pool's, which a save lasting seconds would occupy
    /// to the exclusion of every other asynchronous file operation in the
    /// process. What does briefly occupy a pool thread is the wait for each
    /// chunk, measured at about 4 ms across a full sheet.
    #[napi(ts_return_type = "Promise<Buffer | null>")]
    pub fn next_chunk(&self) -> AsyncTask<NextChunkTask> {
        AsyncTask::new(NextChunkTask {
            shared: Arc::clone(&self.shared),
        })
    }

    /// Abandon the workbook and release the spill file now.
    ///
    /// A consumer that gives up half way through an export — a broken socket on
    /// row three — would otherwise leave the writing thread assembling an
    /// archive nobody will read, and the spill file on disk until it finished.
    /// The sink the thread writes into refuses the next chunk once this is set,
    /// which unwinds the save.
    #[napi]
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);

        // Never blocks, for the reason `XlsxCursor::close` never does: this
        // runs on the main thread, called from `_destroy`, while a write may
        // hold the lock for as long as a batch takes.
        if let Ok(mut stage) = self.shared.stage.try_lock() {
            *stage = Stage::Done;
        }
    }
}

fn to_js_write_error(error: WriteError) -> Error {
    Error::from_reason(format!("{}: {error}", error.code()))
}

fn write_closed_error() -> Error {
    Error::from_reason("CLOSED: this writer has been closed")
}

pub struct SelectSheetTask {
    shared: Arc<WriterShared>,
    name: String,
}

impl Task for SelectSheetTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(write_closed_error());
        }

        let mut stage = lock_stage(&self.shared.stage);
        let Stage::Filling(writer) = &mut *stage else {
            return Err(Error::from_reason(
                "CLOSED: sheets cannot be selected once the file has started streaming",
            ));
        };

        writer.select_sheet(&self.name).map_err(to_js_write_error)
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> Result<Self::JsValue> {
        Ok(())
    }
}

pub struct WriteRowsTask {
    shared: Arc<WriterShared>,
    rows: Vec<Vec<JsCell>>,
    date_columns: Vec<u32>,
}

impl Task for WriteRowsTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(write_closed_error());
        }

        let mut stage = lock_stage(&self.shared.stage);
        let Stage::Filling(writer) = &mut *stage else {
            return Err(Error::from_reason(
                "CLOSED: rows cannot be added once the file has started streaming",
            ));
        };

        let mut row = Vec::new();
        for cells in std::mem::take(&mut self.rows) {
            row.clear();
            row.reserve(cells.len());
            for (column, cell) in cells.into_iter().enumerate() {
                row.push(to_cell_value(
                    cell,
                    self.date_columns.contains(&(column as u32)),
                ));
            }
            writer.write_row(&row).map_err(to_js_write_error)?;
        }

        Ok(())
    }

    fn resolve(&mut self, _env: Env, _output: Self::Output) -> Result<Self::JsValue> {
        Ok(())
    }
}

pub struct NextChunkTask {
    shared: Arc<WriterShared>,
}

impl Task for NextChunkTask {
    type Output = Option<Vec<u8>>;
    type JsValue = Option<Buffer>;

    fn compute(&mut self) -> Result<Self::Output> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(write_closed_error());
        }

        // Started under the lock, then the lock is dropped: the wait for a
        // chunk must not hold it, or `close` could never take it either.
        let receiver = {
            let mut stage = lock_stage(&self.shared.stage);

            if let Stage::Filling(_) = &*stage {
                let Stage::Filling(writer) = std::mem::replace(&mut *stage, Stage::Done) else {
                    unreachable!("just matched");
                };
                *stage = start_assembly(*writer, Arc::clone(&self.shared));
            }

            match &*stage {
                Stage::Draining { chunks, .. } => Arc::clone(chunks),
                Stage::Filling(_) => unreachable!("replaced above"),
                Stage::Done => return Err(write_closed_error()),
            }
        };

        // The stage lock is released above before this one is taken, and the
        // wait happens here: two overlapping pulls are serialised rather than
        // interleaved, and neither blocks `close`.
        let chunk = {
            let guard = receiver.lock().unwrap_or_else(|e| e.into_inner());
            guard.recv()
        };

        match chunk {
            Ok(bytes) => Ok(Some(bytes)),
            // The sender is gone, so the assembly is over one way or another.
            Err(_) => {
                let mut stage = lock_stage(&self.shared.stage);
                let outcome = match &*stage {
                    Stage::Draining { outcome, .. } => Arc::clone(outcome),
                    _ => return Ok(None),
                };
                *stage = Stage::Done;
                drop(stage);

                let verdict = {
                    let mut slot = outcome.lock().unwrap_or_else(|e| e.into_inner());
                    slot.take()
                };

                match verdict {
                    Some(Err(error)) => Err(to_js_write_error(error)),
                    _ => Ok(None),
                }
            }
        }
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.map(Buffer::from))
    }
}

/// Hand the workbook to a thread of its own and return the receiving end.
fn start_assembly(writer: XlsxWriter, shared: Arc<WriterShared>) -> Stage {
    let (sender, receiver) = mpsc::sync_channel(CHUNK_BACKLOG);
    let chunks = Arc::new(Mutex::new(receiver));
    let outcome: Arc<Mutex<Option<WriteOutcome>>> = Arc::new(Mutex::new(None));
    let thread_outcome = Arc::clone(&outcome);

    std::thread::spawn(move || {
        let sink = ChannelSink { sender, shared };
        let result = writer.finish(sink);
        *thread_outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        // Dropping the sink here closes the channel, which is what tells the
        // consumer there is nothing more coming.
    });

    Stage::Draining { chunks, outcome }
}

/// The `Write` end of the channel.
///
/// Blocking in `send` is the backpressure: the assembly cannot outrun the
/// consumer by more than [`CHUNK_BACKLOG`] chunks.
struct ChannelSink {
    sender: mpsc::SyncSender<Vec<u8>>,
    shared: Arc<WriterShared>,
}

impl std::io::Write for ChannelSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // A consumer that walked away stops the save rather than letting it
        // finish writing an archive nobody will read.
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(std::io::Error::other("the consumer closed the stream"));
        }

        self.sender
            .send(buf.to_vec())
            .map_err(|_| std::io::Error::other("the consumer stopped reading"))?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn lock_stage(stage: &Mutex<Stage>) -> MutexGuard<'_, Stage> {
    stage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Turn a JavaScript value into a cell.
///
/// `is_date` comes from the column declaration, never from the value: a string
/// is text unless its column was declared to hold timestamps.
fn to_cell_value(cell: JsCell, is_date: bool) -> CellValue {
    match cell {
        Either4::A(text) if is_date => CellValue::DateTime(text),
        Either4::A(text) => CellValue::Text(text),
        Either4::B(number) => CellValue::Number(number),
        Either4::C(flag) => CellValue::Bool(flag),
        Either4::D(_) => CellValue::Empty,
    }
}
