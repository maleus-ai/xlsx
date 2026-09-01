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
use std::sync::{mpsc, Arc, Mutex, MutexGuard};

use napi::bindgen_prelude::{AsyncTask, Buffer, Either4, Env, Null, Result, Task};
use napi::Error;
use napi_derive::napi;

use xlsx_core::{
    CellValue, ReadError, ReaderOptions, WriteError, WriterOptions, XlsxReader, XlsxWriter,
};

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
    /// Where the row spill files go. `None` uses the platform temporary
    /// directory.
    pub temp_dir: Option<String>,
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
