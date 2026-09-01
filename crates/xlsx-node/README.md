# @maleus/xlsx

This package reads and writes XLSX files, and has the following design goals:

- spreadsheets submitted by users are not safe, and reading those should guard against well-crafted malicious XLSX files;
- worksheets are read or written using streams, **at bounded memory**, whatever their sizes;
- reading or writing spreadsheets should be fast, and should never block the event loop.

This NPM package relies on Rust libraries, through a [napi-rs](https://napi.rs) binding:

- [`calamine`](https://github.com/tafia/calamine) for reading
- [`rust_xlsxwriter`](https://github.com/jmcnamara/rust_xlsxwriter) for writing

```js
import { xlsxRows, xlsxWriteStream } from "@maleus/xlsx";

for await (const row of xlsxRows(upload.path, {
  maxDecompressedBytes: 512 * 1024 * 1024,
  maxRows: 1_000_000,
})) {
  // row: Array<string | number | boolean | null>
}

await pipeline(
  Readable.from(records, { objectMode: true }),
  xlsxWriteStream({
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
function xlsxRows(path: string, options: XlsxRowsOptions): XlsxRows;
function listSheets(path: string, options: BudgetOptions): Promise<SheetInfo[]>;

interface XlsxRowsOptions {
  sheet?: string; // defaults to the first sheet
  maxDecompressedBytes: number; // required, see below
  maxRows: number; // required, see below
  batchSize?: number; // defaults to 1000
}
```

`xlsxRows` returns a `Readable` stream in object mode.

Needs an existing file on the disk: XLSX files are compressed archives, and the
central directory sits at the end of the archive, so a correct read needs to
seek.

### Rows

A row is `Array<string | number | boolean | null>`, each value at **its column
index**. Holes stay holes; rows are padded to the widest row seen _so far_,
which a streaming reader cannot know in advance:

```
      A     B     C     D
  1   1                                  →  [1]
  2   2     3     4     5                →  [2, 3, 4, 5]
  3                                      →  (empty row, not materialised)
  4   6                                  →  [6, null, null, null]
```

Real sheets settle their width on the header row, so this mostly concerns
malformed input — but index by header position rather than by `row.length`.

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

### Protections

Reading takes two mandatory budgets:

- `maxRows` is rows the sheet may yield, header included, raised on the row that crosses it.
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
| `lying-sizes`         | 0.2 MB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 11.4 ms | 2.4 MB   |
| `bomb-entries`        | 14.4 MB | `TOO_MANY_ENTRIES`             | 0.4 ms  | 2.1 MB   |
| `bomb-wide-rows`      | 9.7 kB  | `DECOMPRESSED_BUDGET_EXCEEDED` | 13.9 ms | 10.5 MB  |
| `bomb-column-ref`     | 1.9 kB  | `CORRUPT`                      | 0.7 ms  | 2.5 MB   |
| `xxe-billion-laughs`  | 2.0 kB  | `CORRUPT`                      | 0.6 ms  | 2.5 MB   |
| `xxe-external-entity` | 2.0 kB  | `CORRUPT`                      | 0.7 ms  | 2.5 MB   |

The peak does not follow the size of the archive.

## Writing

```ts
function xlsxWriteStream(options?: XlsxWriteStreamOptions): XlsxWriteStream;

interface XlsxWriteStreamOptions {
  sheet?: string; // sheet a bare row goes to, defaults to "Sheet1"
  columns?: Array<{ header?: string; type?: "date" }>;
  sheets?: Record<string, { columns?: Array<{ header?: string; type?: "date" }> }>;
  tempDir?: string; // defaults to the platform temporary directory
  batchSize?: number; // defaults to 1000
}
```

`xlsxWriteStream` returns a `Transform`: rows in on the writable side in
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
  Readable.from([
    ["Ada", 1],                                        // the default sheet
    { sheet: "Q2", data: ["Alan", new Date()] },       // named
  ], { objectMode: true }),
  xlsxWriteStream({
    sheet: "Q1",
    columns: [{ header: "Client" }, { header: "N" }],
    sheets: { Q2: { columns: [{ header: "Client" }, { header: "Signed", type: "date" }] } },
  }),
  createWriteStream("export.xlsx"),
);
```

Nothing is implied by position. **The source does not have to be sorted by
sheet**: a sheet can be left and come back to, because each keeps its own
height, so a row lands under what *that* sheet already holds. Reordering the
producer cannot silently send rows to the wrong sheet either — there is no
current sheet to get wrong.

A sheet the stream names but the options do not is created on first sight, with
no header and no date columns. Sheet names are compared the way Excel compares
them, without regard to case.

What still holds is the order *within* a sheet: rows are appended, and nothing
goes back above one already written. That is what buys the flat memory.

### Dates are declared, never guessed

```js
xlsxWriteStream({ columns: [{ header: "Signed", type: "date" }] });
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

|                                          | Reading                                | Writing                                    |
| ---------------------------------------- | -------------------------------------- | ------------------------------------------ |
| Values, typed                            | yes                                    | yes                                        |
| Dates                                    | yes                                    | yes, with the format that makes them dates |
| Multiple sheets                          | read any by name; list all             | yes, filled one after another              |
| Formulas                                 | cached result only, never recalculated | **cannot be written at all**               |
| Macros, charts, images, embedded objects | ignored, not reported                  | not written                                |
| Cell formatting, widths, merges          | ignored                                | not written                                |
| CSV, ODS, `.xls`                         | no                                     | no                                         |
| Synchronous API                          | no                                     | no                                         |

## Error Codes

Every refusal is an `XlsxError` with a stable `code`. Branch on the code, not on
the message.

| `code`                         | Raised by | Meaning                                                                                                          |
| ------------------------------ | --------- | ---------------------------------------------------------------------------------------------------------------- |
| `NOT_AN_ARCHIVE`               | reading   | Not a ZIP, or its central directory is too damaged to read                                                       |
| `SHEET_NOT_FOUND`              | reading   | No sheet by that name; the message lists the ones the workbook declares                                          |
| `DECOMPRESSED_BUDGET_EXCEEDED` | reading   | The archive expands past `maxDecompressedBytes`                                                                  |
| `TOO_MANY_ENTRIES`             | reading   | Indexing the archive's entries alone would cost more than the budget allows                                      |
| `ROW_BUDGET_EXCEEDED`          | reading   | The sheet holds more rows than `maxRows`                                                                         |
| `CORRUPT`                      | reading   | A ZIP but not a readable workbook: a missing part, XML that does not parse, a cell reference that makes no sense |
| `INVALID_DATETIME`             | writing   | A timestamp that cannot be placed: not ISO 8601, a UTC offset, or before 1900                                    |
| `INVALID_VALUE`                | writing   | A cell that cannot be written: a `Date` in an undeclared column, a non-finite number, a type with no cell for it |
| `SHEET_LIMIT_EXCEEDED`         | writing   | Past Excel's grid of 1 048 576 rows by 16 384 columns                                                            |
| `INVALID_SHEET_NAME`           | writing   | A sheet name Excel refuses                                                                                       |
| `WRITE_FAILED`                 | writing   | The workbook could not be assembled                                                                              |
| `IO`                           | both      | The file could not be read or written, or a temporary file could not be made                                     |
| `INVALID_OPTION`               | both      | An option is missing or malformed                                                                                |
| `CLOSED`                       | both      | The stream was closed, or rows were added after the file started streaming                                       |
| `UNSUPPORTED_PLATFORM`         | both      | No native binary was built for this platform; raised when the module loads                                       |

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

| Rows      | Time   | Output  | Peak RSS |
| --------- | ------ | ------- | -------- |
| 100 000   | 1.0 s  | 1.9 MB  | 78 MB    |
| 600 000   | 7.3 s  | 11.3 MB | 102 MB   |
| 1 048 575 | 12.5 s | 19.8 MB | 135 MB   |

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

## Contributing

Building, testing, fixtures and releases are in
[CONTRIBUTING.md](https://github.com/maleus-ai/xlsx/blob/main/CONTRIBUTING.md).

## Licence

MIT.
