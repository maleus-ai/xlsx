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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use napi::bindgen_prelude::{AsyncTask, Either4, Env, Null, Result, Task};
use napi::Error;
use napi_derive::napi;

use xlsx_core::{CellValue, ReadError, ReaderOptions, XlsxReader};

/// One sheet, as the workbook declares it.
#[napi(object)]
pub struct JsSheetInfo {
    /// Sheet name, as it appears on the tab.
    pub name: String,
    /// `false` for both `hidden` and `veryHidden` sheets.
    pub visible: bool,
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

/// Options for a listing, which reads no row and so needs no row budget.
#[napi(object)]
pub struct JsListOptions {
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

/// A batch of rows, or `null` once the sheet is finished.
type JsBatch = Option<Vec<Vec<JsCell>>>;

struct State {
    path: PathBuf,
    options: ReaderOptions,
    sheet: Option<String>,
    reader: Option<XlsxReader>,
}

impl State {
    /// The reader, opened on first use.
    ///
    /// Opening is deferred so that the constructor — which JavaScript calls on
    /// the main thread — does no I/O at all. The first pull pays for the open,
    /// on the threadpool, where it belongs.
    fn reader(&mut self) -> Result<&mut XlsxReader> {
        if self.reader.is_none() {
            let mut reader = XlsxReader::open(&self.path, self.options).map_err(to_js_error)?;
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
struct Shared {
    state: Mutex<State>,
    closed: AtomicBool,
}

impl Shared {
    /// Run `work` against the reader, opening it if this is the first pull.
    fn with_reader<T>(&self, work: impl FnOnce(&mut XlsxReader) -> Result<T>) -> Result<T> {
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
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
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
    shared: Arc<Shared>,
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
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    path: PathBuf::from(path),
                    options: ReaderOptions {
                        max_decompressed_bytes,
                        max_rows,
                    },
                    sheet: options.sheet,
                    reader: None,
                }),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// The sheets the workbook declares.
    #[napi(ts_return_type = "Promise<Array<JsSheetInfo>>")]
    pub fn sheets(&self) -> AsyncTask<SheetsTask> {
        AsyncTask::new(SheetsTask {
            shared: Arc::clone(&self.shared),
        })
    }

    /// Pull at most `size` rows. Resolves to `null` once the sheet is finished.
    #[napi(ts_return_type = "Promise<Array<Array<string | number | boolean | null>> | null>")]
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
        self.shared.closed.store(true, Ordering::Release);

        // Never blocks. If a read is in flight it holds the lock, sees the flag
        // on its way out, and releases the archive itself.
        if let Ok(mut state) = self.shared.state.try_lock() {
            state.reader = None;
        }
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

/// List the sheets a workbook declares, without opening it to read rows.
///
/// Separate from [`XlsxCursor::sheets`] because it takes a different path
/// through the file: two small parts inflated and counted, rather than a walk of
/// the whole archive. The bound still applies — it is applied to what gets read.
#[napi(js_name = "listSheets", ts_return_type = "Promise<Array<JsSheetInfo>>")]
pub fn list_sheets(path: String, options: JsListOptions) -> Result<AsyncTask<ListSheetsTask>> {
    let max_decompressed_bytes = budget(options.max_decompressed_bytes, "maxDecompressedBytes")?;

    Ok(AsyncTask::new(ListSheetsTask {
        path: PathBuf::from(path),
        max_decompressed_bytes,
    }))
}

pub struct ListSheetsTask {
    path: PathBuf,
    max_decompressed_bytes: u64,
}

impl Task for ListSheetsTask {
    type Output = Vec<JsSheetInfo>;
    type JsValue = Vec<JsSheetInfo>;

    fn compute(&mut self) -> Result<Self::Output> {
        let sheets = XlsxReader::list_sheets(&self.path, self.max_decompressed_bytes)
            .map_err(to_js_error)?;

        Ok(sheets
            .into_iter()
            .map(|sheet| JsSheetInfo {
                name: sheet.name,
                visible: sheet.visible,
            })
            .collect())
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct SheetsTask {
    shared: Arc<Shared>,
}

impl Task for SheetsTask {
    type Output = Vec<JsSheetInfo>;
    type JsValue = Vec<JsSheetInfo>;

    fn compute(&mut self) -> Result<Self::Output> {
        self.shared.with_reader(|reader| {
            Ok(reader
                .sheets()
                .iter()
                .map(|sheet| JsSheetInfo {
                    name: sheet.name.clone(),
                    visible: sheet.visible,
                })
                .collect())
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct NextBatchTask {
    shared: Arc<Shared>,
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
            Ok(Some(
                rows.into_iter()
                    .map(|row| row.into_iter().map(to_js_cell).collect())
                    .collect(),
            ))
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
