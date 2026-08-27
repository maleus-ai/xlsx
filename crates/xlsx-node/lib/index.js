"use strict";

/**
 * The public surface of `@maleus/xlsx-reader`.
 *
 * Everything below the line — the native cursor, the batch protocol, the way an
 * error code crosses the FFI boundary — is an implementation detail. What a
 * consumer gets is a `Readable` in object mode and a way to list sheets.
 *
 * What this package deliberately does not do: write XLSX, read CSV or ODS,
 * recompute formulas, or offer a synchronous API. Its scope is the bounded
 * streaming read of one `.xlsx` on disk. Anything else has to argue its way
 * past that sentence first.
 */

const { Readable } = require("node:stream");

/**
 * A read that was refused, a file that could not be read, or a platform with no
 * binary to read it with.
 *
 * `code` is the stable discriminant — `NOT_AN_ARCHIVE`, `SHEET_NOT_FOUND`,
 * `DECOMPRESSED_BUDGET_EXCEEDED`, `TOO_MANY_ENTRIES`, `ROW_BUDGET_EXCEEDED`,
 * `CORRUPT`, `IO`, `INVALID_OPTION`, `CLOSED`, `UNSUPPORTED_PLATFORM`. Branch on
 * it rather than on the message.
 */
class XlsxError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "XlsxError";
    this.code = code;
  }
}

const { XlsxCursor, listSheets: listSheetsNative } = loadNativeBinding();

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
      `@maleus/xlsx-reader has no native binding for ${platform}. ` +
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

module.exports = { listSheets, xlsxRows, XlsxError, XlsxRows };
