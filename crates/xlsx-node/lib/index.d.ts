/// <reference types="node" />

import type { Readable } from "node:stream";

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
  | "CLOSED";

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
