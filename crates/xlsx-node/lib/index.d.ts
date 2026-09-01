/// <reference types="node" />

import type { Readable, Transform } from "node:stream";

/** A sheet, as the workbook declares it. */
export interface SheetInfo {
  /** Sheet name, as it appears on the tab. */
  name: string;
  /** `false` for both `hidden` and `veryHidden` sheets. */
  visible: boolean;
}

/**
 * A cell value.
 *
 * A date arrives as an ISO 8601 string in UTC — `2024-03-25T00:00:00.000Z` —
 * never as the underlying serial and never through a locale. An error cell
 * arrives as its sheet spelling: `#DIV/0!`, `#N/A`. A blank arrives as `null`.
 */
export type CellValue = string | number | boolean | null;

/** One row: values at their column index. */
export type Row = CellValue[];

/** The stable discriminant carried by every {@link XlsxError}. */
export type XlsxErrorCode =
  | "NOT_AN_ARCHIVE"
  | "SHEET_NOT_FOUND"
  | "DECOMPRESSED_BUDGET_EXCEEDED"
  | "TOO_MANY_ENTRIES"
  | "ROW_BUDGET_EXCEEDED"
  | "CORRUPT"
  | "IO"
  | "INVALID_OPTION"
  | "UNSUPPORTED_PLATFORM"
  | "CLOSED"
  | "INVALID_DATETIME"
  | "INVALID_VALUE"
  | "SHEET_LIMIT_EXCEEDED"
  | "INVALID_SHEET_NAME"
  | "TOO_MANY_SHEETS"
  | "WRITE_FAILED";

/**
 * A read that was refused, a file that could not be read, or a platform with no
 * binary to read it with.
 */
export declare class XlsxError extends Error {
  readonly name: "XlsxError";
  /** Branch on this rather than on the message. */
  readonly code: XlsxErrorCode;
}

/** The bounds, both mandatory. */
export interface BudgetOptions {
  /**
   * Bytes the archive may expand to, every entry counted, measured on what
   * actually leaves the inflater — never on the sizes the archive declares.
   *
   * It also caps how many entries the archive may hold, at roughly one per
   * kilobyte of budget: entries cost memory to index before any of them is
   * inflated, so an archive of hundreds of thousands of tiny parts is a bomb
   * that a byte count alone does not see.
   *
   * And it charges the blank values a sheet's geometry implies. A cell
   * reference far to the right makes a row that wide and every row after it is
   * padded to match, so two kilobytes of XML can ask for a gigabyte of values
   * without one extra byte leaving the inflater.
   *
   * Required. There is no permissive default: a bound nobody set survives
   * review because it is invisible, and is discovered in production. To read
   * without a ceiling, pass `Number.MAX_SAFE_INTEGER` and leave the trace of
   * that decision in your own source.
   */
  maxDecompressedBytes: number;
}

export interface XlsxRowsOptions extends BudgetOptions {
  /** Sheet to read. Defaults to the first one in the workbook. */
  sheet?: string;
  /**
   * Rows the sheet may yield, header row included — the reader has no notion of
   * a header. Raised on the row that crosses the budget, not at the end of the
   * sheet. Required, for the same reason as `maxDecompressedBytes`.
   */
  maxRows: number;
  /**
   * Rows per FFI round trip. Defaults to 1000, which is where the crossing cost
   * stops being measurable against the parsing without holding more than a few
   * megabytes.
   */
  batchSize?: number;
}

/**
 * A worksheet as a stream of rows, in object mode.
 *
 * A cell missing in the middle of a row leaves `null` at its position rather
 * than shifting its neighbours left, and rows are padded to the width of the
 * widest row seen. Rows absent from the sheet are not materialised.
 */
export declare class XlsxRows extends Readable {
  /**
   * The sheets the workbook declares.
   *
   * Prefer this over a separate {@link listSheets} call when you are going to
   * read rows anyway: it answers off the open the read was about to pay for,
   * rather than adding a second pass over the file.
   */
  sheets(): Promise<SheetInfo[]>;
}

/**
 * List the sheets of a workbook.
 *
 * Reads two parts and no rows: the package relationships, to find where the
 * workbook lives, and the workbook itself. Both are inflated through a counter
 * and charged against the budget — which is why this takes one, and why it does
 * not cost a walk of the archive.
 *
 * A part it never opens is not its problem: a shared-string bomb does not stop
 * a listing, and does stop the read that follows.
 */
export declare function listSheets(
  path: string,
  options: BudgetOptions,
): Promise<SheetInfo[]>;

/**
 * Read one worksheet as a stream of rows.
 *
 * The path must be a file on disk, not a stream: a ZIP is read through its
 * central directory, which sits at the end of the archive, so a correct read
 * needs to seek.
 */
export declare function xlsxRows(
  path: string,
  options: XlsxRowsOptions,
): XlsxRows;

/** A value that can be written to a cell. */
export type WritableCellValue = string | number | boolean | Date | null | undefined;

/** One row on its way out: values at their column index. */
export type WritableRow = WritableCellValue[];

/** One column of the sheet being written. */
export interface ColumnDefinition {
  /**
   * Label for this column, written as the first row.
   *
   * A header is text in every column, including one declared to hold dates.
   */
  header?: string;

  /**
   * Declare that this column holds timestamps.
   *
   * Required for a `Date`, and for an ISO 8601 string that should land as a
   * date rather than as text. Date-ness is never inferred from a value:
   * `"2024-03-25"` is a valid product reference as well as a valid day, and a
   * cell written without a number format reads back as the serial `45376` — a
   * number where the business expects a date.
   *
   * A `Date` in a column without this is refused rather than written wrongly.
   */
  type?: "date";
}

/**
 * A row that names the sheet it goes to.
 *
 * Nothing is implied by position, so a source that is not sorted by sheet
 * streams as it is, and reordering the producer cannot silently send rows to
 * the wrong sheet. A sheet may be left and come back to: each keeps its own
 * height.
 */
export interface SheetRow {
  /** Sheet this row belongs to. Created on first sight if it is new. */
  sheet: string;
  /** The row itself: values at their column index. */
  data: WritableRow;
}

/** What may be written to an {@link XlsxWriteStream}. */
export type WritableInput = WritableRow | SheetRow;

/** Columns of one sheet. */
export interface SheetDefinition {
  /** Columns, in order: their headers and which of them hold dates. */
  columns?: ColumnDefinition[];
}

/** How the workbook is set up. */
export interface XlsxWriteStreamOptions {
  /**
   * Name on the sheet tab. Defaults to `Sheet1`.
   *
   * At most 31 characters, not empty, and free of `[ ] : * ? / \`. A name
   * Excel would refuse is refused here rather than trimmed to fit.
   */
  sheet?: string;

  /** Columns of the default sheet, in order. */
  columns?: ColumnDefinition[];

  /**
   * Columns of the other sheets, keyed by name.
   *
   * Columns are configuration rather than data, so they are declared here
   * instead of travelling in the stream. A sheet the stream names but this does
   * not still works: it is created with no header and no date columns.
   *
   * Giving columns for the default sheet both here and in `columns` is refused.
   */
  sheets?: Record<string, SheetDefinition>;

  /**
   * Where the row spill files go. Defaults to the platform temporary directory.
   *
   * Rows are flushed to a temporary file as they arrive, which is what keeps
   * memory flat — roughly 178 MB of spill for a sheet at Excel's maximum. Set
   * this when the default directory is small, read-only, or **mounted in
   * memory** (`tmpfs`, `/dev/shm`, a Kubernetes `emptyDir` with
   * `medium: Memory`): on a memory-backed mount the spill is RAM again, and it
   * does not show up in the process's RSS.
   */
  tempDir?: string;

  /**
   * Sheets the workbook may hold. Defaults to 256.
   *
   * Each sheet keeps a temporary file open until the workbook is finished, so
   * this is a ceiling on a real resource rather than on the format, which has
   * none. It matters when sheet names come from data: without it, a source that
   * names a new sheet on every row exhausts the process's descriptors, and the
   * underlying writer panics rather than erroring when it cannot open one.
   */
  maxSheets?: number;

  /** Rows per round trip into the native writer. Defaults to 1000. */
  batchSize?: number;
}

/**
 * A worksheet being written: rows in, an `.xlsx` out in pieces.
 *
 * **Bytes arrive only once the rows are in.** Each row is flushed to a spill
 * file as the next one is written, and the archive is assembled from those
 * files when the writable side ends — so nothing is readable before `end()`.
 * It is a stream, but not a transform of rows into bytes as they arrive.
 *
 * A bare array is a row of the default sheet; a {@link SheetRow} names the
 * sheet it goes to.
 */
export declare class XlsxWriteStream extends Transform {
  constructor(options?: XlsxWriteStreamOptions);
}

/**
 * Write a worksheet as a stream of rows.
 *
 * Rows are arrays of values at their column index; `null` leaves a cell blank
 * without shifting its neighbours. Every string is written as a string, never
 * as a formula — a value beginning with `=` reaches the sheet as those
 * characters.
 *
 * ```ts
 * await pipeline(
 *   Readable.from(records, { objectMode: true }),
 *   xlsxWriteStream({
 *     sheet: "Export",
 *     columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
 *   }),
 *   createWriteStream("export.xlsx"),
 * );
 * ```
 */
export declare function xlsxWriteStream(
  options?: XlsxWriteStreamOptions,
): XlsxWriteStream;
