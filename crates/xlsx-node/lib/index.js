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

/** Sheet name used when the caller does not pick one. */
const DEFAULT_SHEET_NAME = "Sheet1";

/**
 * Read the `columns` declaration into the two things writing actually needs:
 * the header row, and which columns hold timestamps.
 *
 * Date-ness is declared rather than detected. `"2024-03-25"` is a valid product
 * reference as well as a valid day, and a writer that guessed would turn one
 * into the other silently — the same class of mistake the reader refuses to
 * make when it declines to invent a type for a bare number.
 */
function readColumns(columns) {
  if (columns === undefined) {
    return { header: null, dateColumns: [] };
  }
  if (!Array.isArray(columns)) {
    throw new XlsxError("INVALID_OPTION", "columns must be an array");
  }

  const dateColumns = [];
  let anyHeader = false;

  const header = columns.map((column, index) => {
    if (column === null || typeof column !== "object") {
      throw new XlsxError(
        "INVALID_OPTION",
        `columns[${index}] must be an object`,
      );
    }
    if (column.type !== undefined && column.type !== "date") {
      throw new XlsxError(
        "INVALID_OPTION",
        `columns[${index}].type must be "date" if given, got ${JSON.stringify(column.type)}`,
      );
    }
    if (column.type === "date") {
      dateColumns.push(index);
    }
    if (column.header !== undefined) {
      if (typeof column.header !== "string") {
        throw new XlsxError(
          "INVALID_OPTION",
          `columns[${index}].header must be a string`,
        );
      }
      anyHeader = true;
      return column.header;
    }
    return null;
  });

  return { header: anyHeader ? header : null, dateColumns };
}

/**
 * Turn one JavaScript value into something the binding accepts.
 *
 * A `Date` becomes the ISO 8601 spelling the reader also uses, and only in a
 * column declared to hold dates. Elsewhere it is refused rather than written as
 * a number or as text: a `Date` in an undeclared column is a mistake with a
 * one-line fix, and a serial with no format is exactly the wrong-value-in-the-
 * database failure this package exists to prevent.
 */
function toCell(value, row, column, isDateColumn) {
  if (value === null || value === undefined) {
    return null;
  }

  const type = typeof value;
  if (type === "string" || type === "number" || type === "boolean") {
    if (type === "number" && !Number.isFinite(value)) {
      throw new XlsxError(
        "INVALID_VALUE",
        `row ${row}, column ${column}: ${String(value)} is not a finite number, ` +
          "and a spreadsheet has no cell for it",
      );
    }
    return value;
  }

  if (value instanceof Date) {
    if (!isDateColumn) {
      throw new XlsxError(
        "INVALID_VALUE",
        `row ${row}, column ${column}: a Date needs its column declared as ` +
          `{ type: "date" }. Without that the cell carries a serial and no ` +
          "number format, which reads back as a plain number rather than a date",
      );
    }
    if (Number.isNaN(value.getTime())) {
      throw new XlsxError(
        "INVALID_VALUE",
        `row ${row}, column ${column}: an Invalid Date`,
      );
    }
    return value.toISOString();
  }

  throw new XlsxError(
    "INVALID_VALUE",
    `row ${row}, column ${column}: ${type} is not a cell value; ` +
      "pass a string, a finite number, a boolean, a Date, or null",
  );
}

/**
 * A worksheet being written, as a stream.
 *
 * Rows go in on the writable side and the `.xlsx` comes out in pieces on the
 * readable side, so it drops into a `pipeline` between whatever produces the
 * rows and wherever the file is going.
 *
 * **Bytes arrive only once the rows are in.** Each row is flushed to a spill
 * file as the next one is written — that is what keeps memory flat — and the
 * archive is assembled from those files when the writable side ends. So this is
 * a stream, but not a transform of rows into bytes as they arrive: nothing is
 * readable until `end()`. Saying otherwise would set up a caller to wait for a
 * first chunk that cannot come.
 */
class XlsxWriteStream extends Transform {
  #sink;
  #dateColumns;
  #batchSize;
  #pending = [];
  #header = null;
  #row = 0;
  #finished = false;

  constructor(options) {
    const batchSize = requireWriteBatchSize(options);

    super({
      writableObjectMode: true,
      readableObjectMode: false,
      writableHighWaterMark: batchSize,
    });

    const { header, dateColumns } = readColumns(options?.columns);
    this.#dateColumns = dateColumns;
    this.#batchSize = batchSize;

    try {
      this.#sink = new XlsxSink({
        sheetName: options?.sheet ?? DEFAULT_SHEET_NAME,
        dateColumns,
        tempDir: options?.tempDir ?? undefined,
      });
    } catch (error) {
      // A rejected sheet name or an unusable temporary directory arrives here
      // as a native error, and has to come out with its code on it like any
      // other — the caller branches on `code`, not on where it was raised.
      throw lift(error);
    }

    // Held apart from the rows rather than pushed in with them: a header is
    // text in every column, including the ones declared to hold dates, and a
    // column called "Signed" is not a timestamp.
    this.#header = header === null ? null : header.map((name) => name ?? null);
  }

  _transform(row, _encoding, callback) {
    if (!Array.isArray(row)) {
      callback(
        new XlsxError(
          "INVALID_VALUE",
          `row ${this.#row} is ${row === null ? "null" : typeof row}; ` +
            "each row must be an array of cell values",
        ),
      );
      return;
    }

    let cells;
    try {
      cells = row.map((value, column) =>
        toCell(value, this.#row, column, this.#dateColumns.includes(column)),
      );
    } catch (error) {
      callback(error);
      return;
    }

    this.#row += 1;
    this.#pending.push(cells);

    if (this.#pending.length < this.#batchSize) {
      callback();
      return;
    }

    this.#flushRows().then(() => callback(), callback);
  }

  _flush(callback) {
    // Everything still in hand goes down, then the archive is assembled and
    // pulled through in pieces.
    this.#flushRows()
      .then(() => this.#drain())
      .then(() => callback(), callback);
  }

  async #flushRows() {
    // The header goes down first and with no date columns declared, so its
    // labels are written as the text they are.
    if (this.#header !== null) {
      const header = this.#header;
      this.#header = null;
      try {
        await this.#sink.writeRows([header], []);
      } catch (error) {
        throw lift(error);
      }
    }

    if (this.#pending.length === 0) {
      return;
    }
    const batch = this.#pending;
    this.#pending = [];

    try {
      await this.#sink.writeRows(batch, this.#dateColumns);
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
    // nor the spill file on disk until it has finished doing so.
    if (!this.#finished) {
      this.#sink.close();
    }
    callback(error);
  }
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
 * Write a worksheet as a stream of rows.
 *
 * Rows are arrays of values placed at their column index; `null` leaves a cell
 * blank without shifting its neighbours. Declare `columns` to give the sheet a
 * header row and to say which columns hold dates.
 *
 * ```js
 * await pipeline(
 *   Readable.from(records, { objectMode: true }),
 *   xlsxWriteStream({
 *     sheet: "Export",
 *     columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
 *   }),
 *   createWriteStream("export.xlsx"),
 * );
 * ```
 *
 * Peak memory stays near 4 MB across a full sheet, because rows spill to a
 * temporary file rather than being held. That spill is real disk — roughly
 * 178 MB for a sheet of 1 048 576 rows by four columns — so on a host whose
 * temporary directory is memory-backed, point `tempDir` at one that is not.
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

