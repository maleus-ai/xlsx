/// <reference types="node" />

import type { Readable, Transform } from "node:stream";

/**
 * Whether a sheet shows on the tab bar — `ST_SheetState`. `hidden` can be
 * unhidden from the spreadsheet's interface; `veryHidden` only by a macro or an
 * edit of the file.
 */
export type SheetState = "visible" | "hidden" | "veryHidden";

/** A sheet, as the workbook declares it — `CT_Sheet`. */
export interface SheetInfo {
  /** Sheet name, as it appears on the tab. */
  name: string;
  /**
   * The identifier the workbook gives the sheet. Stable when sheets are
   * reordered, and unrelated to the sheet's position.
   */
  sheetId: number;
  /** Whether the sheet shows on the tab bar. */
  state: SheetState;
}

/**
 * A cell value.
 *
 * A date arrives as an ISO 8601 string in UTC — `2024-03-25T00:00:00.000Z` —
 * never as the underlying serial and never through a locale. An error cell
 * arrives as its sheet spelling: `#DIV/0!`, `#N/A`. A blank arrives as `null`.
 */
export type CellValue = string | number | boolean | null;

/** One row of a sheet. */
export interface Row {
  /**
   * 0-based position of the row in the sheet. Rows the file does not carry are
   * not emitted, so positions can skip; show `row + 1` to a person.
   */
  row: number;
  /** Values at their 0-based column index. */
  values: CellValue[];
}

/**
 * The stable discriminant carried by every {@link XlsxError}.
 *
 * Open: a minor version may add a code, so a `switch` over it keeps a default
 * branch.
 */
export type XlsxErrorCode =
  | "NOT_AN_ARCHIVE"
  | "SHEET_NOT_FOUND"
  | "DECOMPRESSED_BUDGET_EXCEEDED"
  | "TOO_MANY_ENTRIES"
  | "ROW_BUDGET_EXCEEDED"
  | "CELL_BUDGET_EXCEEDED"
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

export interface ReadRowsOptions extends BudgetOptions {
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
 * A worksheet as a stream of {@link Row}s, in object mode.
 *
 * A cell missing in the middle of a row leaves `null` at its position rather
 * than shifting its neighbours left, and rows are padded to the width of the
 * widest row seen. Rows absent from the sheet are not materialised; each row
 * says where it sits.
 */
export declare class XlsxRows extends Readable {
  /**
   * The sheets the workbook declares.
   *
   * Prefer this over a separate {@link peekSheets} call when you are going to
   * read rows anyway: it answers off the open the read was about to pay for,
   * rather than adding a second pass over the file.
   */
  sheets(): Promise<SheetInfo[]>;
  [Symbol.asyncIterator](): AsyncIterableIterator<Row>;
}

/**
 * Peek at the sheets of a workbook.
 *
 * Reads two parts and no rows: the package relationships, to find where the
 * workbook lives, and the workbook itself. Both are inflated through a counter
 * and charged against the budget — which is why this takes one, and why its
 * cost does not depend on the size of the sheets.
 *
 * A part it never opens is not its problem: a shared-string bomb does not stop
 * a peek, and does stop the read that follows.
 */
export declare function peekSheets(
  path: string,
  options: BudgetOptions,
): Promise<SheetInfo[]>;

/**
 * A colour — `CT_Color`, attributes as declared. A theme colour is not
 * resolved to RGB: that needs the theme, and is a rendering decision.
 */
export interface Color {
  /** `AARRGGBB`. */
  rgb?: string;
  /** Index into the theme's colour scheme. */
  theme?: number;
  /** Index into the legacy indexed palette. */
  indexed?: number;
  /** The application's automatic colour. */
  auto: boolean;
  /** Lightening (positive) or darkening (negative), from -1 to 1. */
  tint?: number;
}

/** `ST_PatternType`. */
export type PatternType =
  | "none"
  | "solid"
  | "mediumGray"
  | "darkGray"
  | "lightGray"
  | "darkHorizontal"
  | "darkVertical"
  | "darkDown"
  | "darkUp"
  | "darkGrid"
  | "darkTrellis"
  | "lightHorizontal"
  | "lightVertical"
  | "lightDown"
  | "lightUp"
  | "lightGrid"
  | "lightTrellis"
  | "gray125"
  | "gray0625";

/** `ST_HorizontalAlignment`. */
export type HorizontalAlignment =
  | "general"
  | "left"
  | "center"
  | "right"
  | "fill"
  | "justify"
  | "centerContinuous"
  | "distributed";

/** `ST_VerticalAlignment`. */
export type VerticalAlignment =
  | "top"
  | "center"
  | "bottom"
  | "justify"
  | "distributed";

/** A number format — `CT_NumFmt`. */
export interface NumFmt {
  numFmtId: number;
  /** Resolved for a built-in identifier; absent for one whose code depends on the locale. */
  formatCode?: string;
}

/**
 * A font — `CT_Font`, its one- and two-letter elements spelled out (`b` is
 * `bold`). An enumerated value outside the format's own list is dropped
 * rather than passed on, here as in {@link Fill} and {@link CellStyle}.
 */
export interface Font {
  name?: string;
  /** Points. */
  size?: number;
  bold: boolean;
  italic: boolean;
  strike: boolean;
  underline?:
    | "single"
    | "double"
    | "singleAccounting"
    | "doubleAccounting"
    | "none";
  vertAlign?: "baseline" | "superscript" | "subscript";
  color?: Color;
}

/** A fill — `CT_Fill`: a pattern or a gradient. */
export interface Fill {
  patternFill?: {
    patternType?: PatternType;
    fgColor?: Color;
    bgColor?: Color;
  };
  gradientFill?: {
    type: "linear" | "path";
    /** Angle of a linear gradient, in degrees. */
    degree?: number;
    stops: Array<{ position: number; color: Color }>;
  };
}

/**
 * The style of a cell — an entry of the workbook's cell formats. It refers to
 * its number format, font and fill by reference: each is held once in its
 * table of the {@link WorkbookInspection}, whatever the number of styles
 * sharing it.
 *
 * A `*Ref` is a 0-based index into that table, not an identifier the file
 * declares: the `numFmtId` a cell format declares is on the entry of
 * `numFmts` that `numFmtRef` points to.
 */
export interface CellStyle {
  /** Index in `numFmts`. */
  numFmtRef: number;
  /** Index in `fonts`; absent when the style refers to a font the table does not declare, which leaves the default font. */
  fontRef?: number;
  /** Index in `fills`; absent when the style refers to a fill the table does not declare, which leaves no fill. */
  fillRef?: number;
  /** `CT_CellAlignment`. */
  alignment: {
    horizontal?: HorizontalAlignment;
    vertical?: VerticalAlignment;
    wrapText: boolean;
    shrinkToFit: boolean;
    indent?: number;
    /** Degrees, or `255` for vertical text. */
    textRotation?: number;
  };
}

/** A defined name — `CT_DefinedName`. */
export interface DefinedName {
  name: string;
  /** The formula, as written: a broken reference keeps its `#REF!`. */
  formula: string;
  /** 0-based position of the sheet the name is scoped to; absent when scoped to the workbook. */
  localSheetId?: number;
  hidden: boolean;
  comment?: string;
}

/** Everything a workbook declares about itself, apart from its sheets' contents. */
export interface WorkbookInspection {
  sheets: SheetInfo[];
  definedNames: DefinedName[];
  /** Core (`dc:`, `cp:`, `dcterms:`) and extended properties, as written. */
  properties: {
    creator?: string;
    lastModifiedBy?: string;
    /** W3C date-time, as written. */
    created?: string;
    modified?: string;
    title?: string;
    subject?: string;
    description?: string;
    keywords?: string;
    category?: string;
    /** The program that wrote the file, as it names itself. */
    application?: string;
    appVersion?: string;
    company?: string;
  };
  /** Whether dates count from 1904 rather than 1900. */
  date1904: boolean;
  /** Which kinds of parts the package carries, besides sheets. */
  parts: {
    vbaProject: boolean;
    pivotTables: boolean;
    externalLinks: boolean;
    drawings: boolean;
  };
  /**
   * The `uri` of each extension the workbook declares in its own extension
   * list (`CT_Workbook/extLst/ext`), in order — the mark an application leaves
   * of features the format does not define, such as
   * `GoogleSheetsCustomDataVersion2` for a Google Sheets export.
   */
  extensions: string[];
  /** The number formats the workbook declares and the built-in ones its styles refer to, one per `numFmtId`, by ascending `numFmtId`; indexed by the `numFmtRef` of a {@link CellStyle}. */
  numFmts: NumFmt[];
  /** The font table, indexed by the `fontRef` of a {@link CellStyle}. */
  fonts: Font[];
  /** The fill table, indexed by the `fillRef` of a {@link CellStyle}. */
  fills: Fill[];
  /** Cell styles, indexed by the `styleId` of a {@link Cell}. */
  styles: CellStyle[];
}

/**
 * Inspect a workbook: sheets, defined names, properties, date system, parts,
 * and the cell styles with the number formats, fonts and fills they refer to.
 * Reads no sheet; every part it does read is charged against
 * `maxDecompressedBytes`.
 *
 * ```ts
 * const workbook = await inspectWorkbook(path, { maxDecompressedBytes });
 * for await (const cell of readCells(path, { maxDecompressedBytes, maxCells })) {
 *   const { fontRef } = workbook.styles[cell.styleId];
 *   if (fontRef !== undefined && workbook.fonts[fontRef].bold) headers.push(cell);
 * }
 * ```
 */
export declare function inspectWorkbook(
  path: string,
  options: BudgetOptions,
): Promise<WorkbookInspection>;

/** `ST_DataValidationType`. */
export type DataValidationType =
  | "none"
  | "whole"
  | "decimal"
  | "list"
  | "date"
  | "time"
  | "textLength"
  | "custom";

/** `ST_DataValidationOperator`. */
export type DataValidationOperator =
  | "between"
  | "notBetween"
  | "equal"
  | "notEqual"
  | "lessThan"
  | "lessThanOrEqual"
  | "greaterThan"
  | "greaterThanOrEqual";

/** `ST_CfType`. */
export type CfType =
  | "expression"
  | "cellIs"
  | "colorScale"
  | "dataBar"
  | "iconSet"
  | "top10"
  | "uniqueValues"
  | "duplicateValues"
  | "containsText"
  | "notContainsText"
  | "beginsWith"
  | "endsWith"
  | "containsBlanks"
  | "notContainsBlanks"
  | "containsErrors"
  | "notContainsErrors"
  | "timePeriod"
  | "aboveAverage";

/** `ST_ConditionalFormattingOperator`. */
export type ConditionalFormattingOperator =
  | "lessThan"
  | "lessThanOrEqual"
  | "equal"
  | "notEqual"
  | "greaterThanOrEqual"
  | "greaterThan"
  | "between"
  | "notBetween"
  | "containsText"
  | "notContains"
  | "beginsWith"
  | "endsWith";

/** `ST_TimePeriod`. */
export type TimePeriod =
  | "today"
  | "yesterday"
  | "tomorrow"
  | "last7Days"
  | "thisMonth"
  | "lastMonth"
  | "nextMonth"
  | "thisWeek"
  | "lastWeek"
  | "nextWeek";

/** `ST_TotalsRowFunction`. */
export type TotalsRowFunction =
  | "none"
  | "sum"
  | "min"
  | "max"
  | "average"
  | "count"
  | "countNums"
  | "stdDev"
  | "var"
  | "custom";

/**
 * Sheet protection — the flags of `CT_SheetProtection`, with the defaults the
 * format gives them. For an action, `true` means it is *prevented* while the
 * sheet is protected. Whether a password is set is reported; its hash never is.
 */
export interface SheetProtection {
  passwordProtected: boolean;
  sheet: boolean;
  objects: boolean;
  scenarios: boolean;
  formatCells: boolean;
  formatColumns: boolean;
  formatRows: boolean;
  insertColumns: boolean;
  insertRows: boolean;
  insertHyperlinks: boolean;
  deleteColumns: boolean;
  deleteRows: boolean;
  selectLockedCells: boolean;
  sort: boolean;
  autoFilter: boolean;
  pivotTables: boolean;
  selectUnlockedCells: boolean;
}

/**
 * What a sheet declares besides its cells. References are in A1 notation as
 * declared; positions the format numbers from 1 are given 0-based.
 */
export interface SheetInspection {
  /** The range the sheet declares it uses, written by the application that saved it and not checked against the cells. */
  dimension?: string;
  tabColor?: Color;
  /** Frozen or split pane of the first view — `CT_Pane`. */
  pane?: {
    xSplit?: number;
    ySplit?: number;
    topLeftCell?: string;
    state: "split" | "frozen" | "frozenSplit";
  };
  /** `CT_Col`: `firstCol` and `lastCol` are 0-based, `min` and `max` in the file. */
  cols: Array<{
    firstCol: number;
    lastCol: number;
    width?: number;
    hidden: boolean;
    outlineLevel?: number;
  }>;
  /** 0-based positions of the hidden rows. */
  hiddenRows: number[];
  mergeCells: string[];
  /** `target` is resolved through the sheet's relationships. */
  hyperlinks: Array<{
    ref: string;
    target?: string;
    location?: string;
    tooltip?: string;
    display?: string;
  }>;
  tables: Array<{
    name: string;
    displayName: string;
    ref: string;
    headerRowCount: number;
    totalsRowCount: number;
    tableColumns: Array<{
      name: string;
      totalsRowFunction?: TotalsRowFunction;
      totalsRowLabel?: string;
      totalsRowFormula?: string;
      calculatedColumnFormula?: string;
    }>;
  }>;
  /**
   * Standard and `x14` validations in one list, in document order. A list
   * drawn from another sheet is written in the `x14` extension.
   */
  dataValidations: Array<{
    sqref: string[];
    extension: boolean;
    type?: DataValidationType;
    operator?: DataValidationOperator;
    allowBlank: boolean;
    /** As the format defines it: `true` *hides* the in-cell dropdown. */
    showDropDown: boolean;
    showInputMessage: boolean;
    showErrorMessage: boolean;
    errorStyle?: "stop" | "warning" | "information";
    errorTitle?: string;
    error?: string;
    promptTitle?: string;
    prompt?: string;
    formula1?: string;
    formula2?: string;
  }>;
  /**
   * Standard and `x14` conditional formats in one list, in document order. A
   * rule drawing on another sheet, or a custom icon set, is written in the
   * `x14` extension; a rule of the extension that extends a standard one — the
   * extra properties of a data bar — is not restored, the standard rule stands
   * for it. A rule whose type the format does not define is dropped.
   */
  conditionalFormatting: Array<{
    sqref: string[];
    extension: boolean;
    rules: Array<{
      type: CfType;
      priority: number;
      operator?: ConditionalFormattingOperator;
      stopIfTrue: boolean;
      text?: string;
      timePeriod?: TimePeriod;
      formulas: string[];
      /**
       * Index of the differential format applied; differential formats are not
       * read. Absent for a rule of the `x14` extension, which writes its
       * format inline.
       */
      dxfId?: number;
    }>;
  }>;
  sheetProtection?: SheetProtection;
  autoFilter?: { ref: string };
  /** Authors of the comments, each held once, as the comments part lists them. */
  commentAuthors: string[];
  /**
   * Comments of the classic comments part, runs concatenated; threaded
   * comments always have a copy there. `authorRef` is an index into
   * `commentAuthors`, absent when the comment names an author the part does
   * not list.
   */
  comments: Array<{ ref: string; authorRef?: number; text: string }>;
  /** Pictures in the sheet's drawing. */
  pictureCount: number;
}

/**
 * Inspect one sheet: everything it declares besides its cells.
 *
 * The sheet part is streamed, cells skipped, and every part read — the ones it
 * links to included — is charged against `maxDecompressedBytes`, as are the copy
 * of its target each hyperlink receives and the memory every record built from
 * those parts takes beyond its text.
 *
 * ```ts
 * const sheet = await inspectSheet(path, { sheet: "Data", maxDecompressedBytes });
 * for (const validation of sheet.dataValidations) {
 *   if (validation.type === "list") lists.set(validation.sqref.join(" "), validation.formula1);
 * }
 * ```
 */
export declare function inspectSheet(
  path: string,
  options: BudgetOptions & { sheet: string },
): Promise<SheetInspection>;

/**
 * Read one worksheet as a stream of rows.
 *
 * The path must be a file on disk, not a stream: a ZIP is read through its
 * central directory, which sits at the end of the archive, so a correct read
 * needs to seek.
 *
 * ```ts
 * for await (const { row, values } of readRows(path, { maxDecompressedBytes, maxRows })) {
 *   if (values[0] === null) throw new Error(`row ${row + 1}: the client is missing`);
 * }
 * ```
 */
export declare function readRows(
  path: string,
  options: ReadRowsOptions,
): XlsxRows;

/**
 * What a cell holds, as told by its decoded value. `date` and `duration` are
 * numbers formatted as such; `empty` is a cell declared without a value, such
 * as one that only carries a style.
 *
 * Open: a minor version may add a kind, so a `switch` over it keeps a default
 * branch.
 */
export type CellType =
  | "string"
  | "number"
  | "boolean"
  | "date"
  | "duration"
  | "error"
  | "empty";

/** A cell's formula — `CT_CellFormula`. */
export interface Formula {
  /** The formula as written, without the leading `=`; a cell sharing a formula gets it expanded for its own position. */
  text: string;
  /** How the formula is shared — `ST_CellFormulaType`. */
  type: "normal" | "shared" | "array" | "dataTable";
  /** The range a shared or array formula covers, on the cell that declares it, in A1 notation. */
  ref?: string;
}

/** One cell, as the sheet declares it. */
export interface Cell {
  /** 0-based row. */
  row: number;
  /** 0-based column. */
  col: number;
  /** What the value is. */
  type: CellType;
  /** The value, or the cached result of the formula — never recalculated. */
  value: CellValue;
  /** The formula, when the cell has one. */
  formula?: Formula;
  /** Index of the cell's style in the workbook's cell formats; `0` when the cell declares none. */
  styleId: number;
}

export interface ReadCellsOptions extends BudgetOptions {
  /** Sheet to read. Defaults to the first one in the workbook. */
  sheet?: string;
  /**
   * Cells the sheet may yield. Raised on the cell that crosses the budget.
   * Required, for the same reason as `maxDecompressedBytes`.
   */
  maxCells: number;
  /** Cells per FFI round trip. Defaults to 1000. */
  batchSize?: number;
}

/** A worksheet as a stream of {@link Cell}s, in object mode and document order. */
export declare class XlsxCells extends Readable {
  /** The sheets the workbook declares, off the open the read pays for anyway. */
  sheets(): Promise<SheetInfo[]>;
  [Symbol.asyncIterator](): AsyncIterableIterator<Cell>;
}

/**
 * Read one worksheet as a stream of the cells it declares, for an inspection
 * rather than an import.
 *
 * The sheet is read twice — once for the values and formulas, once for the
 * style index and the formula attributes — and the second reading is charged
 * against `maxDecompressedBytes` like the first. So is the text a cell copies
 * rather than inflates — a shared string, a shared formula expanded for its
 * position — since a few bytes of XML can ask for it again on every cell.
 *
 * ```ts
 * for await (const cell of readCells(path, { maxDecompressedBytes, maxCells: 1_000_000 })) {
 *   if (cell.formula) console.log(toA1(cell), `=${cell.formula.text}`);
 * }
 * ```
 */
export declare function readCells(
  path: string,
  options: ReadCellsOptions,
): XlsxCells;

/** 0-based position of a cell. */
export interface CellPosition {
  row: number;
  col: number;
}

/**
 * Read an A1 reference into 0-based positions: `parseRef("B3:C10")` is
 * `{ start: { row: 2, col: 1 }, end: { row: 9, col: 2 } }`, and a single cell is
 * its own start and end.
 *
 * The `$` of an absolute reference is ignored. A whole column (`A:A`), a whole
 * row, a sheet-qualified reference and a list of ranges are refused with
 * `INVALID_VALUE`, as is a reference outside `A1:XFD1048576`.
 */
export declare function parseRef(ref: string): {
  start: CellPosition;
  end: CellPosition;
};

/**
 * Spell a 0-based position as an A1 reference: `toA1({ row: 2, col: 1 })` is
 * `"B3"`. Takes the shape {@link parseRef} returns, so
 * `toA1(parseRef("$B$3").start)` is `"B3"`. Refused with `INVALID_VALUE`
 * outside the grid of a worksheet.
 */
export declare function toA1(position: CellPosition): string;

/** A value that can be written to a cell. */
export type WritableCellValue =
  | string
  | number
  | boolean
  | Date
  | null
  | undefined;

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
  values: WritableRow;
}

/** What may be written to an {@link XlsxWriteStream}. */
export type WritableInput = WritableRow | SheetRow;

/** Columns of one sheet. */
export interface SheetDefinition {
  /** Columns, in order: their headers and which of them hold dates. */
  columns?: ColumnDefinition[];
}

/** How the workbook is set up. */
export interface WriteWorkbookOptions {
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
   * stream fails with `IO` when it cannot open one.
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
  constructor(options?: WriteWorkbookOptions);
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
 *   writeWorkbook({
 *     sheet: "Export",
 *     columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
 *   }),
 *   createWriteStream("export.xlsx"),
 * );
 * ```
 */
export declare function writeWorkbook(
  options?: WriteWorkbookOptions,
): XlsxWriteStream;
