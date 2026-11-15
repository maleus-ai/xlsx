# @maleus/xlsx

This package reads and writes XLSX files, and has the following design goals:

- spreadsheets submitted by users are not safe, and reading those should guard against well-crafted malicious XLSX files;
- worksheets are read or written using streams, **at bounded memory**, whatever their sizes;
- reading or writing spreadsheets should be fast, and should never block the event loop.

This NPM package relies on Rust libraries, through a [napi-rs](https://napi.rs) binding:

- [`calamine`](https://github.com/tafia/calamine) for reading
- [`rust_xlsxwriter`](https://github.com/jmcnamara/rust_xlsxwriter) for writing

```js
import { readRows, writeWorkbook } from "@maleus/xlsx";

for await (const { row, values } of readRows(upload.path, {
  maxDecompressedBytes: 512 * 1024 * 1024,
  maxRows: 1_000_000,
})) {
  // row: 0-based position in the sheet
  // values: Array<string | number | boolean | null>
}

await pipeline(
  Readable.from(records, { objectMode: true }),
  writeWorkbook({
    sheet: "Export",
    columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
  }),
  createWriteStream("export.xlsx"),
);
```

## Install

```sh
npm install @maleus/xlsx
```

Prebuilt binaries ship as one package per platform in `optionalDependencies`,
constrained by `os`, `cpu` and `libc`: `linux-x64-gnu`, `linux-arm64-gnu`,
`linux-x64-musl`, `linux-arm64-musl`, `darwin-arm64`. A platform nobody built
for installs nothing and fails when the module loads — at boot, not part way
through an import.

## Reading

```ts
function readRows(path: string, options: ReadRowsOptions): XlsxRows;
function peekSheets(path: string, options: BudgetOptions): Promise<SheetInfo[]>;

interface ReadRowsOptions {
  sheet?: string; // defaults to the first sheet
  maxDecompressedBytes: number; // required, see below
  maxRows: number; // required, see below
  batchSize?: number; // defaults to 1000
}

interface SheetInfo {
  name: string;
  sheetId: number; // as the workbook declares it, unrelated to the tab order
  state: "visible" | "hidden" | "veryHidden";
}
```

`readRows` returns a `Readable` stream in object mode. `peekSheets` reads the
workbook's sheet list and nothing else, so its cost does not depend on the size
of the sheets; `readRows(...).sheets()` answers the same question off the open a
read pays for anyway.

Needs an existing file on the disk: XLSX files are compressed archives, and the
central directory sits at the end of the archive, so a correct read needs to
seek.

### Positions

A number is a 0-based position; a string is the A1 notation the file uses.
Column A is `0`, the first row is `0`: show `row + 1` to a person. Two pure
helpers go from one to the other:

```js
parseRef("B3:C10"); // { start: { row: 2, col: 1 }, end: { row: 9, col: 2 } }
toA1({ row: 2, col: 1 }); // "B3"
```

### Rows

A row is `{ row, values }`. `row` is its 0-based position in the sheet, and
`values` an `Array<string | number | boolean | null>`, each value at **its
column index**. Rows the file does not carry are not emitted, so `row` can
skip — it is what an import reports when a line is wrong. Holes stay holes;
rows are padded to the widest row seen _so far_, which a streaming reader cannot
know in advance:

```
      A     B     C     D
  1   1                                  →  { row: 0, values: [1] }
  2   2     3     4     5                →  { row: 1, values: [2, 3, 4, 5] }
  3                                      →  (empty row, not materialised)
  4   6                                  →  { row: 3, values: [6, null, null, null] }
```

Real sheets settle their width on the header row, so this mostly concerns
malformed input — but index by header position rather than by
`values.length`.

### Values

| Cell                             | Yields                                               |
| -------------------------------- | ---------------------------------------------------- |
| shared or inline string          | `string`                                             |
| number                           | `number`                                             |
| boolean                          | `boolean`                                            |
| date, time or datetime           | `string`, ISO 8601 UTC: `"2024-03-25T00:00:00.000Z"` |
| elapsed time (`[h]:mm:ss`)       | `string`, ISO 8601 duration: `"PT30H0M0S"`           |
| error                            | `string`, the sheet spelling: `"#DIV/0!"`            |
| formula                          | its cached result, typed as any of the above         |
| empty, absent, or past the width | `null`                                               |

XLSX carries no timezone, so the stored wall clock is reported verbatim with a
`Z`, whatever `TZ` the process runs under. An elapsed time is a duration, not a
point in time, and its hours do not wrap at 24.

### Cells

```ts
function readCells(path: string, options: ReadCellsOptions): XlsxCells;

interface ReadCellsOptions {
  sheet?: string; // defaults to the first sheet
  maxDecompressedBytes: number; // required
  maxCells: number; // required
  batchSize?: number; // defaults to 1000
}

interface Cell {
  row: number; // 0-based
  col: number; // 0-based
  type:
    | "string"
    | "number"
    | "boolean"
    | "date"
    | "duration"
    | "error"
    | "empty";
  value: string | number | boolean | null;
  formula?: {
    text: string;
    type: "normal" | "shared" | "array" | "dataTable";
    ref?: string;
  };
  styleId: number; // index in the workbook's cell formats, 0 when none
}
```

Where `readRows` serves an import, `readCells` serves an inspection: one
object per cell the sheet declares, in document order — a cell that only
carries a style included, as `type: "empty"`. `type` tells apart what the value
alone confuses: a date, an error and a string all arrive as strings.

A formula comes as written, without the `=`, alongside its cached result; it is
never recalculated. A shared formula is expanded for every cell that uses it,
and `ref` gives the range on the cell that declares it.

The sheet is read twice — once for values and formulas, once for the style
index and the formula attributes — and the second reading is charged against
`maxDecompressedBytes` too. So is the text a cell copies rather than inflates —
a shared string, a shared formula expanded for its position — because a few
bytes of XML can ask for it again on every cell. Measured on 200 000 rows by 10
columns: 2.7 s for 2 000 010 cells, against 1.4 s for the same sheet through
`readRows`.

### Inspecting a workbook

```ts
function inspectWorkbook(
  path: string,
  options: BudgetOptions,
): Promise<WorkbookInspection>;
```

Everything a workbook declares about itself, apart from its sheets' contents:
its sheets, its defined names (`#REF!` kept as written), its core and extended
document properties, its date system, whether it carries a VBA project, pivot
tables, links to other workbooks or drawings, the `uri` of each extension it
declares (`GoogleSheetsCustomDataVersion2` for a Google Sheets export), and its
cell styles, indexed by the `styleId` that `readCells` reports. No sheet is
read.

A style carries its own alignment and refers to its number format, font and
fill through `numFmtRef`, `fontRef` and `fillRef`. The number formats, fonts
and fills are each restored once, in `numFmts`, `fonts` and `fills`, however
many styles share them:

```ts
const workbook = await inspectWorkbook(path, { maxDecompressedBytes });
const style = workbook.styles[cell.styleId];
const { numFmtId, formatCode } = workbook.numFmts[style.numFmtRef];
const font =
  style.fontRef === undefined ? undefined : workbook.fonts[style.fontRef];
```

A `*Ref` is a 0-based index into its table of the inspection, not an
identifier the file declares: the `numFmtId` of the format is on the entry
`numFmtRef` points to. `numFmts` holds one entry per `numFmtId`, by ascending
`numFmtId`. A `fontRef` or `fillRef` is absent when the style refers to an
entry its table does not declare.

Types mirror ECMA-376: attribute names as the format spells them, except the
one- and two-letter font elements, spelled out (`b` is `bold`). A colour keeps
its attributes as declared — a theme colour stays a theme index with its tint,
since resolving it is a rendering decision. A built-in number format comes with
the code the specification fixes for it, or none when that code depends on the
locale. An enumerated value outside the format's own list is dropped. Borders,
named styles and differential formats are not read.

### Inspecting a sheet

```ts
function inspectSheet(
  path: string,
  options: BudgetOptions & { sheet: string },
): Promise<SheetInspection>;
```

Everything a sheet declares besides its cells: its dimension, tab colour and
frozen panes, its column definitions and hidden rows, merged ranges, hyperlinks
(targets resolved through the sheet's relationships), tables, data validations
and conditional formats — the `x14` extension included, where Excel writes a
list drawn from another sheet, a format rule drawing on another sheet or a
custom icon set, flagged `extension: true` — protection (whether
a password is set, never its hash), autofilter, comments, and how many pictures
its drawing holds.

References stay in A1 notation as declared, `sqref` split into its ranges.
Positions the format numbers from 1 come 0-based under a name of their own:
`col@min` and `col@max` are `firstCol` and `lastCol`. The sheet part is
streamed and its cells skipped; every part read, including those it links to,
is charged against `maxDecompressedBytes`, and a relationship pointing at a part
the archive does not hold, or outside it, is `CORRUPT`.

### Protections

Reading takes two mandatory budgets:

- `maxRows` (or `maxCells` for `readCells`) is what the sheet may yield, header included, raised on the row or cell that crosses it.
- `maxDecompressedBytes` is what the archive may expand to, and guards against zip/sheet bombs

Only the five predefined XML entities are resolved; any entity a document
declares is an error, not an expansion.

Every hostile workbook in the `fixtures/` test directory is refused before the
thing it was built to cause. On an 8 MiB memory budget, one process per figure:

| Archive               | Upload  | Refused with                   | Time    | Peak RSS |
| --------------------- | ------- | ------------------------------ | ------- | -------- |
| `bomb-sharedstrings`  | 7.6 MB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 3.8 ms  | 2.4 MB   |
| `bomb-styles`         | 0.8 MB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 2.4 ms  | 2.4 MB   |
| `bomb-rels`           | 11.7 MB | `DECOMPRESSED_BUDGET_EXCEEDED` | 2.5 ms  | 2.4 MB   |
| `bomb-package-rels`   | 11.2 MB | `DECOMPRESSED_BUDGET_EXCEEDED` | 11.0 ms | 2.4 MB   |
| `bomb-workbook`       | 10.0 MB | `DECOMPRESSED_BUDGET_EXCEEDED` | 5.7 ms  | 2.5 MB   |
| `bomb-inline`         | 0.3 MB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 1.2 ms  | 2.5 MB   |
| `lying-sizes`         | 0.2 MB  | `CORRUPT`                      | 0.5 ms  | 2.4 MB   |
| `bomb-entries`        | 14.4 MB | `TOO_MANY_ENTRIES`             | 0.4 ms  | 2.1 MB   |
| `bomb-wide-rows`      | 9.7 kB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 13.9 ms | 10.5 MB  |
| `bomb-column-ref`     | 1.9 kB  | `CORRUPT`                      | 0.7 ms  | 2.5 MB   |
| `xxe-billion-laughs`  | 2.0 kB  | `CORRUPT`                      | 0.6 ms  | 2.5 MB   |
| `xxe-external-entity` | 2.0 kB  | `CORRUPT`                      | 0.7 ms  | 2.5 MB   |

The peak does not follow the size of the archive.

## Writing

```ts
function writeWorkbook(options?: WriteWorkbookOptions): XlsxWriteStream;

interface WriteWorkbookOptions {
  sheet?: string; // sheet a bare row goes to, defaults to "Sheet1"
  columns?: Array<{ header?: string; type?: "date" }>;
  sheets?: Record<
    string,
    { columns?: Array<{ header?: string; type?: "date" }> }
  >;
  maxSheets?: number; // defaults to 256
  tempDir?: string; // defaults to the platform temporary directory
  batchSize?: number; // defaults to 1000
}
```

`writeWorkbook` returns a `Transform`: rows in on the writable side in
object mode, `Buffer` chunks out on the readable side.

### It buffers, then emits

Nothing comes out of the readable side until you call `end()`.

Rows written go to a temporary file, not to the output — that is what keeps
memory flat. Only at `end()` is the `.xlsx` assembled from them and emitted,
in chunks.

So time-to-first-byte is the time to write every row: an export is not a
live feed you can start sending to a client early. And do not wait on a
`data` event before calling `end()` — none will come.

### Rows

`Array<string | number | boolean | Date | null/undefined>`, at their column
index. `null` and `undefined` leaves a blank without shifting neighbours.

### Several sheets

Columns are declared in the options; a row names the sheet it goes to:

```js
await pipeline(
  Readable.from(
    [
      ["Ada", 1], // the default sheet
      { sheet: "Q2", values: ["Alan", new Date()] }, // named
    ],
    { objectMode: true },
  ),
  writeWorkbook({
    sheet: "Q1",
    columns: [{ header: "Client" }, { header: "N" }],
    sheets: {
      Q2: {
        columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
      },
    },
  }),
  createWriteStream("export.xlsx"),
);
```

A sheet the stream names but the options do not is created on first sight, with
no header and no date columns. Sheet names are compared the way Excel compares
them, without regard to case.

### Dates are declared, never guessed

```js
writeWorkbook({ columns: [{ header: "Signed", type: "date" }] });
```

In an XLSX a date is a serial _plus_ a `numFmt`. Write the serial alone and the
cell reads back as `45376` — a number where the business expects a day. So a
`Date` in an undeclared column is **refused**, and the error names the column to
declare. Inference is not on offer either: `"2024-03-25"` is a valid product
reference as much as a valid day.

Strings in a declared date column are parsed as ISO 8601, so the reader's output
feeds straight back in. Two refusals rather than silent wrong answers: a UTC
offset (XLSX stores no timezone, so honouring it would shift the value and
ignoring it would keep the wrong one), and a date before 1900 (Excel has no
serial for it — write it as text if you need to keep it).

## What is and is not supported

Reads one worksheet at a time, writes as many as you like, and creates new
files only — there is no open-change-save.

|                                                                           | Reading                                         | Writing                                    |
| ------------------------------------------------------------------------- | ----------------------------------------------- | ------------------------------------------ |
| Values, typed                                                             | yes                                             | yes                                        |
| Dates                                                                     | yes                                             | yes, with the format that makes them dates |
| Multiple sheets                                                           | read any by name; peek at all, with their state | yes, filled one after another              |
| Formulas                                                                  | text and cached result, never recalculated      | **cannot be written at all**               |
| Macros, charts, images                                                    | presence reported by `inspectWorkbook`          | not written                                |
| Number formats, fonts, fills, alignment                                   | read by `inspectWorkbook`, indexed per cell     | not written                                |
| Merges, widths, validations, tables, comments, links, conditional formats | read by `inspectSheet`                          | not written                                |
| Borders, named styles, differential formats                               | ignored                                         | not written                                |
| CSV, ODS, `.xls`                                                          | no                                              | no                                         |
| Synchronous API                                                           | no                                              | no                                         |

## Error Codes

Every refusal is an `XlsxError` with a stable `code`. Branch on the code, not on
the message.

The set of codes is **open**: a minor version may add one, so a `switch` over
`code` keeps a default branch. Every other union in the typings mirrors an
enumeration of the file format itself, and is closed.

| `code`                         | Raised by                   | Meaning                                                                                                                                                                                                 |
| ------------------------------ | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `NOT_AN_ARCHIVE`               | reading                     | Not a ZIP, or its central directory is too damaged to read                                                                                                                                              |
| `SHEET_NOT_FOUND`              | reading                     | No sheet by that name; the message lists the ones the workbook declares                                                                                                                                 |
| `DECOMPRESSED_BUDGET_EXCEEDED` | reading                     | The archive expands past `maxDecompressedBytes`                                                                                                                                                         |
| `TOO_MANY_ENTRIES`             | reading                     | Indexing the archive's entries alone would cost more than the budget allows                                                                                                                             |
| `ROW_BUDGET_EXCEEDED`          | reading                     | The sheet holds more rows than `maxRows`                                                                                                                                                                |
| `CELL_BUDGET_EXCEEDED`         | reading                     | The sheet declares more cells than `maxCells`                                                                                                                                                           |
| `CORRUPT`                      | reading                     | A ZIP but not a readable workbook: a missing part, a part that inflates past its declared size, XML that does not parse, a cell reference that makes no sense, a sheet state the format does not define |
| `INVALID_DATETIME`             | writing                     | A timestamp that cannot be placed: not ISO 8601, a UTC offset, or before 1900                                                                                                                           |
| `INVALID_VALUE`                | writing, `parseRef`, `toA1` | A cell that cannot be written: a `Date` in an undeclared column, a non-finite number, a type with no cell for it; or a reference or position outside a worksheet                                        |
| `SHEET_LIMIT_EXCEEDED`         | writing                     | Past Excel's grid of 1 048 576 rows by 16 384 columns                                                                                                                                                   |
| `INVALID_SHEET_NAME`           | writing                     | A sheet name Excel refuses                                                                                                                                                                              |
| `WRITE_FAILED`                 | writing                     | The workbook could not be assembled                                                                                                                                                                     |
| `IO`                           | both                        | The file could not be read or written, or a temporary file could not be made                                                                                                                            |
| `INVALID_OPTION`               | both                        | An option is missing or malformed                                                                                                                                                                       |
| `CLOSED`                       | both                        | The stream was closed, or rows were added after the file started streaming                                                                                                                              |
| `UNSUPPORTED_PLATFORM`         | both                        | No native binary was built for this platform; raised when the module loads                                                                                                                              |

## Measurements

```sh
node fixtures/generate.mjs   # rebuild the fixtures
pnpm run build               # release binary
node scripts/bench.mjs       # the tables below
```

One process per figure, `VmHWM` for the peak, which includes Node's own ~40 MB
baseline.

### Reading

| Workbook                  | Rows    | Time    | Peak RSS |
| ------------------------- | ------- | ------- | -------- |
| `large-200000`            | 200 001 | 1.3 s   | 66 MB    |
| `large-600000`            | 600 001 | 3.7 s   | 68 MB    |
| `large-600000`, list only | —       | 0.003 s | 49 MB    |

### Writing

Four columns of which one holds dates:

| Rows      | Time  | Output  | Peak RSS |
| --------- | ----- | ------- | -------- |
| 100 000   | 0.6 s | 1.9 MB  | 78 MB    |
| 600 000   | 3.6 s | 11.3 MB | 100 MB   |
| 1 048 575 | 6.2 s | 19.8 MB | 134 MB   |

Budget **80–140 MB for an ordinary export, and under 200 MB even when the file
itself runs to hundreds of megabytes**. Most of that is Node rather than this
package: an idle `node` is 40 MB, and pushing the row arrays through a plain
`Transform` with no addon involved accounts for most of the rest. The Rust
writer holds 4.2 MB.

Those peaks are a sawtooth, and the garbage collector is why. Output chunks are
handed to V8 and only collected in batches, so the figure climbs by roughly
64 MB and drops back, whatever the size of the export — a 432 MB file peaks no
higher than a 123 MB one. That ceiling is V8's, not ours, and it is the reason
memory does not grow with the output. Under a hard container limit, budget for
the peak rather than the average.

**Flat memory is bought with disk.** Rows spill to one unlinked temporary file —
~178 MB for a sheet at Excel's maximum — released by the kernel on close. Where
the temporary directory is memory-backed (`tmpfs`, `/dev/shm`, a Kubernetes
`emptyDir` with `medium: Memory`) that spill is RAM again _and does not appear
in RSS_. Point `tempDir` at real disk there.

## Upgrading

The package carries its own upgrade notes, so they are always those of the
version installed: `CHANGELOG.md` lists every change, and the first version of
each major ships a migration recipe under `migrations/`, both at the root of the
package (`node_modules/@maleus/xlsx/`). A recipe lists every breaking change
with how to find the code it affects, the change to make, what changes without
the compiler noticing, and how to check the result — precise enough for a
coding agent to apply on its own.

Release candidates are published under the `next` dist-tag and never become
`latest`: `npm install @maleus/xlsx` always resolves a stable version.

## Contributing

Building, testing, fixtures and releases are in
[CONTRIBUTING.md](https://github.com/maleus-ai/xlsx/blob/main/CONTRIBUTING.md).

## Licence

MIT.
