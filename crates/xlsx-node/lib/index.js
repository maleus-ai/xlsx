"use strict";

/**
 * The public surface of `@maleus/xlsx`.
 *
 * Everything below the line — the native cursor, the batch protocol, the way an
 * error code crosses the FFI boundary — is an implementation detail. What a
 * consumer gets is a `Readable` in object mode and a way to list sheets.
 *
 * What this package deliberately does not do: read CSV or ODS, recompute
 * formulas, edit a workbook that already exists, or offer a synchronous API.
 * Its scope is the bounded streaming read and write of one `.xlsx`. Anything
 * else has to argue its way past that sentence first.
 */

const { Readable, Transform } = require("node:stream");
const { once } = require("node:events");

/**
 * A read that was refused, a file that could not be read, or a platform with no
 * binary to read it with.
 *
 * `code` is the stable discriminant. Reading raises `NOT_AN_ARCHIVE`,
 * `SHEET_NOT_FOUND`, `DECOMPRESSED_BUDGET_EXCEEDED`, `TOO_MANY_ENTRIES`,
 * `ROW_BUDGET_EXCEEDED` or `CORRUPT`; writing raises `INVALID_DATETIME`,
 * `INVALID_VALUE`, `SHEET_LIMIT_EXCEEDED`, `INVALID_SHEET_NAME` or
 * `WRITE_FAILED`; either raises `IO`, `INVALID_OPTION`, `CLOSED` or
 * `UNSUPPORTED_PLATFORM`. Branch on it rather than on the message.
 */
class XlsxError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "XlsxError";
    this.code = code;
  }
}

const {
  XlsxCursor,
  XlsxSink,
  listSheets: listSheetsNative,
  validateSheetName,
} = loadNativeBinding();

/**
 * Load the native addon, and say something useful when there is none.
 *
 * The prebuilt binaries ship as one npm package per platform, listed in
 * `optionalDependencies` and constrained by `os`, `cpu` and `libc`. A package
 * manager installs the one that matches and silently skips the rest, so a
 * platform nobody built for leaves nothing behind and shows up here, at load
 * time — when the application boots, not part way through an import in
 * production. Naming the platform is what turns that into a fixable report.
 */
function loadNativeBinding() {
  try {
    return require("../binding.js");
  } catch (cause) {
    const platform = `${process.platform}-${process.arch}`;
    throw new XlsxError(
      "UNSUPPORTED_PLATFORM",
      `@maleus/xlsx has no native binding for ${platform}. ` +
        "Prebuilt binaries are published for linux x64 and arm64 (gnu and musl) " +
        "and macOS arm64. If yours is one of those, the install skipped it: " +
        "remove the lockfile and node_modules and install again.",
      { cause },
    );
  }
}

/** Rows per FFI round trip when the caller does not say. */
const DEFAULT_BATCH_SIZE = 1000;

/**
 * Lift the error code back out of the message.
 *
 * N-API builds a JavaScript error's `code` from a fixed enum with no room for
 * ours, so the Rust side puts it at the head of the message and it is taken
 * back off here. An error that does not carry one is passed through untouched:
 * it did not come from the reader.
 */
function lift(error) {
  const message = error instanceof Error ? error.message : String(error);
  const match = /^([A-Z][A-Z0-9_]*): ([\s\S]*)$/.exec(message);
  if (!match) {
    return error;
  }
  return new XlsxError(match[1], match[2]);
}

function requireBudget(options, name) {
  const value = options?.[name];
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    // Both budgets are mandatory, and this is where that is enforced. A
    // permissive default is a bound nobody set: invisible in review, found in
    // production. A caller that wants no ceiling passes Number.MAX_SAFE_INTEGER
    // and leaves the trace of that decision in its own source.
    throw new XlsxError(
      "INVALID_OPTION",
      `${name} is required and must be a non-negative number`,
    );
  }
  return value;
}

function requireBatchSize(options) {
  const value = options?.batchSize ?? DEFAULT_BATCH_SIZE;
  if (!Number.isInteger(value) || value < 1) {
    throw new XlsxError(
      "INVALID_OPTION",
      `batchSize must be a positive integer, got ${String(value)}`,
    );
  }
  return value;
}

function requirePath(path) {
  if (typeof path !== "string" || path.length === 0) {
    throw new XlsxError("INVALID_OPTION", "path must be a non-empty string");
  }
  return path;
}

/**
 * A worksheet as a stream of rows.
 *
 * One row is an array of values placed at their column index: a cell missing in
 * the middle of a row leaves `null` at its position rather than shifting its
 * neighbours left, and rows are padded to the width of the widest row seen. A
 * date arrives as an ISO 8601 string in UTC.
 */
class XlsxRows extends Readable {
  #cursor;
  #batchSize;
  #pulling = false;
  #done = false;

  constructor(path, options) {
    const batchSize = requireBatchSize(options);

    super({ objectMode: true, highWaterMark: batchSize });

    this.#batchSize = batchSize;
    this.#cursor = new XlsxCursor(requirePath(path), {
      sheet: options?.sheet,
      maxDecompressedBytes: requireBudget(options, "maxDecompressedBytes"),
      maxRows: requireBudget(options, "maxRows"),
    });
  }

  /**
   * The sheets the workbook declares.
   *
   * Reading this from the stream rather than through `listSheets` opens the
   * archive once instead of twice, which matters: opening charges the
   * decompressed-byte budget over the whole file. If `sheet` was given, it must
   * name a sheet that exists — the selection happens on the same open.
   */
  async sheets() {
    try {
      return await this.#cursor.sheets();
    } catch (error) {
      throw lift(error);
    }
  }

  _read() {
    // `_read` can be called again as soon as a row is pushed, and a second pull
    // would take rows out of order behind the first one's back.
    if (this.#pulling || this.#done) {
      return;
    }
    this.#pulling = true;

    this.#cursor.nextBatch(this.#batchSize).then(
      (batch) => {
        this.#pulling = false;

        if (batch === null) {
          this.#done = true;
          this.#cursor.close();
          this.push(null);
          return;
        }

        for (const row of batch) {
          this.push(row);
        }
      },
      (error) => {
        this.#pulling = false;
        this.#done = true;
        this.#cursor.close();
        this.destroy(lift(error));
      },
    );
  }

  _destroy(error, callback) {
    // A consumer that breaks out of its loop half way through — a validation
    // failure on row three — must not leave the archive open until the next
    // garbage collection.
    this.#done = true;
    this.#cursor.close();
    callback(error);
  }
}

/**
 * List the sheets of a workbook.
 *
 * Reads two parts and no rows: the package relationships, to find where the
 * workbook lives, and the workbook itself. Both are inflated through a counter
 * and charged against the budget — which is why this takes one, and why a
 * listing does not cost a walk of the archive.
 *
 * When you are going to read rows anyway, `xlsxRows(...).sheets()` answers the
 * same question off the open you were about to pay for.
 */
async function listSheets(path, options) {
  try {
    return await listSheetsNative(requirePath(path), {
      maxDecompressedBytes: requireBudget(options, "maxDecompressedBytes"),
    });
  } catch (error) {
    throw lift(error);
  }
}

/**
 * Read one worksheet as a stream of rows.
 *
 * The path must be a file on disk, not a stream. A ZIP is read through its
 * central directory, which sits at the end of the archive, so a correct read
 * needs to seek — walking the archive forward instead is the shortcut that
 * loses its tail. `diskStorage` already produces exactly what is needed.
 */
function xlsxRows(path, options) {
  return new XlsxRows(path, options);
}


/** Rows per FFI round trip on the writing side. */
const DEFAULT_WRITE_BATCH_SIZE = 1000;

/**
 * How many batches' worth of rows may be held across all sheets before the
 * fullest is sent down. Bounds what a source spread thinly over many sheets
 * holds, without giving up batching on any one of them.
 */
const PENDING_SHEETS_FACTOR = 4;

/** Sheet a bare row goes to when the caller does not name one. */
const DEFAULT_SHEET_NAME = "Sheet1";

/**
 * Excel compares sheet names without regard to case, so this is the key two
 * spellings of one sheet agree on.
 */
function fold(name) {
  return name.toLowerCase();
}

/**
 * Read a `columns` declaration into the two things writing needs from it: the
 * header row, and which columns hold timestamps.
 *
 * Date-ness is declared rather than detected. `"2024-03-25"` is a valid product
 * reference as well as a valid day, and a writer that guessed would turn one
 * into the other silently — the same class of mistake the reader refuses when
 * it declines to invent a type for a bare number.
 */
function readColumns(columns, where) {
  if (columns === undefined) {
    return { header: null, dateColumns: [] };
  }
  if (!Array.isArray(columns)) {
    throw new XlsxError("INVALID_OPTION", `${where} must be an array`);
  }

  const dateColumns = [];
  let anyHeader = false;

  const header = columns.map((column, index) => {
    if (column === null || typeof column !== "object") {
      throw new XlsxError("INVALID_OPTION", `${where}[${index}] must be an object`);
    }
    if (column.type !== undefined && column.type !== "date") {
      throw new XlsxError(
        "INVALID_OPTION",
        `${where}[${index}].type must be "date" if given, got ${JSON.stringify(column.type)}`,
      );
    }
    if (column.type === "date") {
      dateColumns.push(index);
    }
    if (column.header !== undefined) {
      if (typeof column.header !== "string") {
        throw new XlsxError("INVALID_OPTION", `${where}[${index}].header must be a string`);
      }
      anyHeader = true;
      return column.header;
    }
    return null;
  });

  return { header: anyHeader ? header.map((name) => name ?? null) : null, dateColumns };
}

/**
 * Turn one JavaScript value into something the binding accepts.
 *
 * A `Date` becomes the ISO 8601 spelling the reader also uses, and only in a
 * column declared to hold dates. Elsewhere it is refused rather than written as
 * a number: a serial with no number format is exactly the wrong-value-in-the-
 * database failure this package exists to prevent.
 */
function toCell(value, sheet, row, column, isDateColumn) {
  if (value === null || value === undefined) {
    return null;
  }

  const type = typeof value;
  if (type === "string" || type === "number" || type === "boolean") {
    if (type === "number" && !Number.isFinite(value)) {
      throw new XlsxError(
        "INVALID_VALUE",
        `sheet ${JSON.stringify(sheet)}, row ${row}, column ${column}: ` +
          `${String(value)} is not a finite number, and a spreadsheet has no cell for it`,
      );
    }
    return value;
  }

  if (value instanceof Date) {
    if (!isDateColumn) {
      throw new XlsxError(
        "INVALID_VALUE",
        `sheet ${JSON.stringify(sheet)}, row ${row}, column ${column}: a Date needs ` +
          `its column declared as { type: "date" }. Without that the cell carries a ` +
          "serial and no number format, which reads back as a plain number rather than a date",
      );
    }
    if (Number.isNaN(value.getTime())) {
      throw new XlsxError(
        "INVALID_VALUE",
        `sheet ${JSON.stringify(sheet)}, row ${row}, column ${column}: an Invalid Date`,
      );
    }
    return value.toISOString();
  }

  throw new XlsxError(
    "INVALID_VALUE",
    `sheet ${JSON.stringify(sheet)}, row ${row}, column ${column}: ${type} is not a ` +
      "cell value; pass a string, a finite number, a boolean, a Date, or null",
  );
}

function requireWriteBatchSize(options) {
  const value = options?.batchSize ?? DEFAULT_WRITE_BATCH_SIZE;
  if (!Number.isInteger(value) || value < 1) {
    throw new XlsxError(
      "INVALID_OPTION",
      `batchSize must be a positive integer, got ${String(value)}`,
    );
  }
  return value;
}

/**
 * A worksheet, or several, as a stream.
 *
 * Rows go in on the writable side and the `.xlsx` comes out in pieces on the
 * readable side, so it drops into a `pipeline` between whatever produces the
 * rows and wherever the file is going.
 *
 * **Bytes arrive only once the rows are in.** Each row is flushed to a spill
 * file as the next one is written — that is what keeps memory flat — and the
 * archive is assembled from those files when the writable side ends. So this is
 * a stream, but not a transform of rows into bytes as they arrive: nothing is
 * readable until `end()`.
 */
class XlsxWriteStream extends Transform {
  #sink;
  #batchSize;
  /** Folded name → `{ name, dateColumns, header, rows }`, built on first use. */
  #sheets = new Map();
  #defaultSheet;
  /**
   * Rows held per sheet, folded name → rows, waiting to make a batch.
   *
   * Per sheet rather than one queue because a row may name any sheet at any
   * time. A single queue would have to be flushed on every change, which on an
   * interleaved source is one native call per row — measured at 150 s for
   * 600 000 rows across twelve sheets, against 7 s for the same rows on one.
   */
  #pending = new Map();
  #pendingRows = 0;
  /** The sheet the native writer currently has selected. */
  #selected = null;
  #finished = false;

  constructor(options) {
    const batchSize = requireWriteBatchSize(options);

    super({
      writableObjectMode: true,
      readableObjectMode: false,
      writableHighWaterMark: batchSize,
    });

    this.#batchSize = batchSize;

    const defaultName = options?.sheet ?? DEFAULT_SHEET_NAME;
    if (typeof defaultName !== "string" || defaultName.length === 0) {
      throw new XlsxError("INVALID_OPTION", "sheet must be a non-empty string");
    }
    this.#defaultSheet = fold(defaultName);
    this.#declare(defaultName, readColumns(options?.columns, "columns"));

    // Columns for the other sheets are configuration, not data, so they are
    // declared here rather than travelling in the stream. A sheet the stream
    // names but this does not still works: it is created with no header and no
    // date columns.
    const sheets = options?.sheets;
    if (sheets !== undefined) {
      if (sheets === null || typeof sheets !== "object" || Array.isArray(sheets)) {
        throw new XlsxError(
          "INVALID_OPTION",
          "sheets must be an object mapping a sheet name to its definition",
        );
      }
      for (const [name, definition] of Object.entries(sheets)) {
        if (definition === null || typeof definition !== "object") {
          throw new XlsxError("INVALID_OPTION", `sheets[${JSON.stringify(name)}] must be an object`);
        }
        // Checked here rather than at the first row that goes there, so that
        // one bad name is one error whether or not any data reaches it.
        try {
          validateSheetName(name);
        } catch (error) {
          throw lift(error);
        }
        if (fold(name) === this.#defaultSheet && options?.columns !== undefined) {
          throw new XlsxError(
            "INVALID_OPTION",
            `columns for ${JSON.stringify(name)} are given twice, once as \`columns\` and ` +
              "once in `sheets`; keep one",
          );
        }
        this.#declare(name, readColumns(definition.columns, `sheets[${JSON.stringify(name)}].columns`));
      }
    }

    try {
      this.#sink = new XlsxSink({
        sheetName: defaultName,
        dateColumns: this.#sheets.get(this.#defaultSheet).dateColumns,
        maxSheets: options?.maxSheets ?? undefined,
        tempDir: options?.tempDir ?? undefined,
      });
    } catch (error) {
      // A rejected sheet name or an unusable temporary directory arrives here
      // as a native error, and has to come out with its code on it like any
      // other — the caller branches on `code`, not on where it was raised.
      throw lift(error);
    }
    this.#selected = this.#defaultSheet;
  }

  #declare(name, { header, dateColumns }) {
    this.#sheets.set(fold(name), { name, header, dateColumns, rows: 0, written: false });
  }

  /** The sheet record for `name`, made on first sight if it was not declared. */
  #sheetFor(name) {
    const key = fold(name);
    let record = this.#sheets.get(key);
    if (record === undefined) {
      record = { name, header: null, dateColumns: [], rows: 0, written: false };
      this.#sheets.set(key, record);
    }
    return record;
  }

  _transform(value, _encoding, callback) {
    let name;
    let data;

    if (Array.isArray(value)) {
      name = this.#sheets.get(this.#defaultSheet).name;
      data = value;
    } else if (value !== null && typeof value === "object" && Array.isArray(value.data)) {
      // A row that says where it goes. Nothing is implied by position, so a
      // source that is not sorted by sheet streams as it is, and reordering the
      // producer cannot silently send rows to the wrong sheet.
      if (typeof value.sheet !== "string" || value.sheet.length === 0) {
        callback(
          new XlsxError(
            "INVALID_VALUE",
            "a row object needs a non-empty `sheet`, as in { sheet: \"Q2\", data: [...] }",
          ),
        );
        return;
      }
      name = value.sheet;
      data = value.data;
    } else {
      callback(
        new XlsxError(
          "INVALID_VALUE",
          `${value === null ? "null" : typeof value} is not a row; pass an array of ` +
            'cell values, or { sheet: "Q2", data: [...] } to name the sheet it goes to',
        ),
      );
      return;
    }

    const record = this.#sheetFor(name);
    const key = fold(name);

    let cells;
    try {
      cells = data.map((cell, column) =>
        toCell(cell, record.name, record.rows, column, record.dateColumns.includes(column)),
      );
    } catch (error) {
      callback(error);
      return;
    }
    record.rows += 1;

    let bucket = this.#pending.get(key);
    if (bucket === undefined) {
      bucket = [];
      this.#pending.set(key, bucket);
    }
    bucket.push(cells);
    this.#pendingRows += 1;

    // A sheet with a full batch goes down. Otherwise, once enough rows are held
    // across all sheets, the fullest one goes down — which bounds what is held
    // when a source spreads thinly over many sheets, without giving up batching
    // on any of them.
    if (bucket.length >= this.#batchSize) {
      this.#flushSheet(key).then(() => callback(), callback);
      return;
    }
    if (this.#pendingRows >= this.#batchSize * PENDING_SHEETS_FACTOR) {
      this.#flushSheet(this.#fullestSheet()).then(() => callback(), callback);
      return;
    }
    callback();
  }

  _flush(callback) {
    this.#flushAll()
      .then(() => this.#finishDeclaredSheets())
      .then(() => this.#drain())
      .then(() => callback(), callback);
  }

  #fullestSheet() {
    let best = null;
    let most = -1;
    for (const [key, rows] of this.#pending) {
      if (rows.length > most) {
        most = rows.length;
        best = key;
      }
    }
    return best;
  }

  async #flushAll() {
    // Deterministic order, so a failure part way through leaves the same
    // workbook whichever sheet was fullest.
    for (const key of [...this.#pending.keys()]) {
      await this.#flushSheet(key);
    }
  }

  /**
   * Give every declared sheet its existence and its header row.
   *
   * An export that returns no rows still has columns, and a consumer that reads
   * them from the first line of the file needs that line to be there. Writing
   * the header only alongside rows meant an empty export produced an empty
   * file, and a sheet declared in `sheets` that received nothing produced no
   * sheet at all.
   */
  async #finishDeclaredSheets() {
    for (const [key, record] of this.#sheets) {
      if (record.written) {
        continue;
      }
      try {
        if (this.#selected !== key) {
          await this.#sink.selectSheet(record.name);
          this.#selected = key;
        }
        if (record.header !== null) {
          const header = record.header;
          record.header = null;
          await this.#sink.writeRows([header], []);
        }
        record.written = true;
      } catch (error) {
        throw lift(error);
      }
    }
  }

  /** Send one sheet's held rows to it. */
  async #flushSheet(key) {
    if (key === null) {
      return;
    }
    const batch = this.#pending.get(key);
    if (batch === undefined || batch.length === 0) {
      return;
    }

    const record = this.#sheets.get(key);
    this.#pending.delete(key);
    this.#pendingRows -= batch.length;

    try {
      if (this.#selected !== key) {
        await this.#sink.selectSheet(record.name);
        this.#selected = key;
      }

      // The header goes down on the sheet's first use, and with no date
      // columns declared: a label is text in every column, and a column called
      // "Signed" is not a timestamp.
      if (record.header !== null) {
        const header = record.header;
        record.header = null;
        await this.#sink.writeRows([header], []);
      }

      await this.#sink.writeRows(batch, record.dateColumns);
      record.written = true;
    } catch (error) {
      throw lift(error);
    }
  }

  /**
   * Pull the file through, one piece at a time.
   *
   * `push` returning false means the consumer's buffer is full, and the next
   * pull simply does not happen until it drains — which stops the Rust side
   * assembling ahead, because the channel it writes into is bounded and it
   * blocks on a full one. The backpressure is the same mechanism at both ends.
   */
  async #drain() {
    for (;;) {
      let chunk;
      try {
        chunk = await this.#sink.nextChunk();
      } catch (error) {
        throw lift(error);
      }

      if (chunk === null) {
        this.#finished = true;
        this.#sink.close();
        return;
      }

      if (!this.push(chunk)) {
        await once(this, "drain");
      }
    }
  }

  _destroy(error, callback) {
    // A consumer that walks away — a broken socket half way through an export —
    // must not leave the writing thread assembling an archive nobody will read,
    // nor the spill files on disk until it has finished doing so.
    if (!this.#finished) {
      this.#sink.close();
    }
    callback(error);
  }
}

/**
 * Write one or more worksheets as a stream of rows.
 *
 * A bare array is a row of the sheet named by `sheet`; `{ sheet, data }` names
 * the sheet it goes to, so a source that is not sorted by sheet streams as it
 * is. Values sit at their column index and `null` leaves a blank without
 * shifting its neighbours.
 *
 * ```js
 * await pipeline(
 *   Readable.from(records, { objectMode: true }),
 *   xlsxWriteStream({
 *     sheet: "Q1",
 *     columns: [{ header: "Client" }],
 *     sheets: { Q2: { columns: [{ header: "Signed", type: "date" }] } },
 *   }),
 *   createWriteStream("export.xlsx"),
 * );
 * ```
 *
 * Peak memory stays flat across the export, because rows spill to temporary
 * files rather than being held. That spill is real disk — see `tempDir` if the
 * platform temporary directory is memory-backed.
 */
function xlsxWriteStream(options) {
  return new XlsxWriteStream(options);
}

module.exports = {
  listSheets,
  xlsxRows,
  xlsxWriteStream,
  XlsxError,
  XlsxRows,
  XlsxWriteStream,
};

