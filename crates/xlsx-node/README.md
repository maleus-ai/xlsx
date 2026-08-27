# @maleus/xlsx-reader

Spreadsheets submitted by users are not safe, and well-crafted malicious XLSX
files can be used to take down backend servers.

This package is FAST, and it reads one worksheet of an `.xlsx` file as a stream
of typed rows, **at bounded memory**.

Written in Rust on [`calamine`](https://github.com/tafia/calamine),
published to npm as `@maleus/xlsx-reader` through a [napi-rs](https://napi.rs)
binding.

```js
import { xlsxRows } from "@maleus/xlsx-reader";

for await (const row of xlsxRows(uploadedFile.path, {
  maxDecompressedBytes: 512 * 1024 * 1024,
  maxRows: 1_000_000,
})) {
  // row: Array<string | number | boolean | null>
}
```

## Install

```sh
npm install @maleus/xlsx-reader
```

Prebuilt binaries ship as one package per platform, listed in
`optionalDependencies` and constrained by `os`, `cpu` and `libc`:
`linux-x64-gnu`, `linux-arm64-gnu`, `linux-x64-musl`, `linux-arm64-musl`,
`darwin-arm64`. A platform nobody built for installs nothing and fails when the
application loads the module — at boot, not part way through an import.

## API

```ts
function xlsxRows(path: string, options: XlsxRowsOptions): XlsxRows;
function listSheets(path: string, options: BudgetOptions): Promise<SheetInfo[]>;

interface XlsxRowsOptions {
  sheet?: string; // defaults to the first sheet
  maxDecompressedBytes: number;
  maxRows: number;
  batchSize?: number; // defaults to 1000
}
```

`xlsxRows` returns a `Readable` in object mode. The path must be a file on disk,
not a stream: a ZIP is read through its central directory, which sits at the end
of the archive, so a correct read needs to seek.

### What a row looks like

A row is `Array<string | number | boolean | null>`, each value at **its column
index**. Given a sheet whose cells are

```
      A     B     C     D
  1   1     2     3     4
  2   5           7     8
  3   9     10
  4
  5               11
```

the stream yields four rows:

```js
[1, 2, 3, 4]; // 1 — full width, and it sets the width
[5, null, 7, 8]; // 2 — a hole stays a hole; B does not shift left
[9, 10, null, null]; // 3 — padded on the right, not truncated
[null, null, 11, null]; // 5 — row 4 is empty and is not materialised
```

Rows are padded to the widest row seen **so far**, which is not the same as the
width of the sheet: a streaming reader cannot know that before the end, and
nothing goes back to widen a row already handed over. A sheet whose first row is
narrower than a later one therefore yields rows of different lengths:

```
      A     B     C     D
  1   1
  2   2     3     4     5
  3   6
```

```js
[1]; // 1 — one value, and no way to know a wider row is coming
[2, 3, 4, 5]; // 2 — the width from here on
[6, null, null, null]; // 3
```

Real sheets settle their width on the header row, so this is mostly a matter for
malformed input — but index by the header position rather than by `row.length`,
and guard the access.

Both sheets above are fixtures — `sparse` and `narrow-first-row` — and the rows
shown are what the test suites assert, not what the documentation remembers.

### What a value looks like

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

Dates carry no timezone in the format, so the stored wall clock is reported
verbatim with a `Z`: what the sheet shows is what comes out, whatever `TZ` the
process runs under. An elapsed time is not a point in time — reporting it as a
datetime would put it on an arbitrary day — so it comes back as a duration, with
hours that do not wrap at 24.

Formulas are never recalculated. A formula cell yields the result the writing
application cached in the file, and yields `null` if it cached none.

Every refusal is an `XlsxError` carrying a stable `code`: `NOT_AN_ARCHIVE`,
`SHEET_NOT_FOUND`, `DECOMPRESSED_BUDGET_EXCEEDED`, `TOO_MANY_ENTRIES`,
`ROW_BUDGET_EXCEEDED`, `CORRUPT`, `IO`, `INVALID_OPTION`, `CLOSED`, and
`UNSUPPORTED_PLATFORM` when the module loads on a platform no binary was built
for. Branch on the code, not on the message.

`listSheets` reads two parts and no rows — the package relationships, to find
where the workbook lives, and the workbook itself — each inflated through a
counter and charged against the budget. That is why it takes one, and why it
costs three milliseconds on a workbook that takes 3.7 seconds to read. A part it
never opens is not its problem: a shared-string bomb does not stop a listing, and
does stop the read that follows. When you are going to read rows anyway,
`xlsxRows(...).sheets()` answers the same question off the open you were about to
pay for.

## What it does

- **Streams one worksheet.** Rows are pulled by the batch and nothing
  accumulates: the peak moves by two megabytes between two hundred thousand
  rows and six hundred thousand.
- **Types values as the format defines them.** In an XLSX a date is a number
  plus a `numFmt` declared in `xl/styles.xml`; without that table `45376` cannot
  be told apart from a quantity. Dates come out as ISO 8601 strings in UTC,
  whatever the process timezone.
- **Reads any sheet**, by name, and lists the sheets of a workbook without
  reading any of them.
- **Applies two bounds, both mandatory**, on what the file is allowed to cost.

## What it is for

Reading a spreadsheet that came from outside the system — an upload, a file
someone was allowed to send.

The goal is one sentence: **nothing in the file decides how much memory the
process uses.** Not the number of rows, not the number of sheets, not the size of
the shared string table, not how far to the right a cell claims to sit. The
caller sets the ceiling and the file has no say in it.

That is why the bounds live inside the reader rather than in a check above it. A
bound placed above protects nothing during the phases the reader runs before it
hands back its first row, and for an XLSX those phases build four tables —
package relationships, shared strings, styles, workbook — each one sized by the
file and none of them by the number of rows. By the time a caller could look at
the first row, the allocation has already happened.

It is also why they are mandatory in the API, with no permissive default. A
bound nobody set survives review because it is invisible, and is discovered in
production. A caller who wants no ceiling writes `Number.MAX_SAFE_INTEGER` and
leaves the trace of that decision in their own source.

## The bounds

`maxRows` is the number of rows the sheet may yield, header row included — the
reader has no notion of a header. It is raised on the row that crosses the
budget, not at the end of the sheet.

`maxDecompressedBytes` is what the archive may expand to. Three expansions are
charged against it, because all three turn a small file into a large process:

- **Bytes leaving the inflater**, every entry counted, measured on the real
  expansion and never on the sizes the archive declares. `zip` verifies an
  entry's CRC but does not cap its output at the declared length, so an archive
  is free to announce 64 bytes for a part that deploys 64 MB.
- **The archive's own directory.** Entries cost memory to index before any of
  them is inflated, and that indexing happens inside the ZIP reader. The count
  is read from the archive's trailer and charged before it is opened, at roughly
  one entry per kilobyte of budget.
- **The blank values a sheet's geometry implies.** A column index is built by
  accumulating `col * 26 + letter`, so `r="BZZZZZ1"` names column 36 119 382 —
  and a row that wide costs a gigabyte without one extra byte leaving the
  inflater. References past the format's own grid (16 384 columns, 1 048 576
  rows) are refused outright, and the padding a sheet implies inside that grid
  is charged like the bytes it inflates.

Two further limits are not options because nothing sensible could be set for
them. A batch holds at most about 32 MB of values whatever it was asked for, so
`nextBatch(1000)` on a genuinely wide sheet comes back short rather than handing
over half a gigabyte at once. And declared XML entities are never expanded: the
parser resolves the five predefined ones and treats any other name as unknown,
which is what makes an entity-expansion or external-entity payload an error
rather than a read.

Every hostile workbook in `fixtures/` is refused, and refused before the thing it
was built to cause. Measured with an 8 MiB budget, one process per figure:

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

The peak does not follow the size of the archive, and the refusal does not wait
for the file to be read.

## What it does not do

It reads. It does not write XLSX, read CSV or ODS, recompute formulas, or offer
a synchronous API. Anything else has to argue its way past that sentence first.

Three things it is worth being explicit about, because the bounds above might
suggest otherwise:

- **It does not inspect content.** Macros, embedded objects and external links
  are neither examined nor reported. Formula injection in a value that gets
  re-exported somewhere else is the consumer's problem, not this reader's.
- **The bounds are on memory, not on time.** There is no wall-clock budget: a
  file of legitimate shape, inside its budget, can still take seconds. Cancelling
  a read means destroying the stream, which releases the archive but does not
  interrupt the batch already in flight.
- **It trusts `calamine` and `zip`** for memory safety, and carries the only
  `unsafe` code in the repository: two expressions in one function of
  `crates/xlsx-core/src/cursor.rs`, which erase a lifetime so that a workbook
  and a cursor borrowed from it can live in the same struct. Their invariants
  are written out above them.

## Measurements

```sh
node fixtures/generate.mjs   # rebuild the fixtures
pnpm run build               # release binary
node scripts/bench.mjs       # the tables below
```

Through the Node facade, one process per figure, `VmHWM` for the peak — which
includes Node's own ~49 MB baseline:

| Workbook                  | Rows    | Time    | Peak RSS |
| ------------------------- | ------- | ------- | -------- |
| `large-200000`            | 200 001 | 1.3 s   | 66 MB    |
| `large-600000`            | 600 001 | 3.7 s   | 68 MB    |
| `large-600000`, list only | —       | 0.003 s | 49 MB    |

Three times the rows, two megabytes apart.

Batch size, on 600 000 rows, best of two runs:

| `batchSize` | Time   | Peak RSS |
| ----------- | ------ | -------- |
| 1           | 76.8 s | 59 MB    |
| 10          | 12.2 s | 59 MB    |
| 100         | 5.7 s  | 60 MB    |
| **1000**    | 4.9 s  | 65 MB    |
| 10000       | 5.1 s  | 104 MB   |

Which is why the default is 1000: the fastest of the five, and 39 MB lighter
than the one next to it. Below 100 the FFI crossing costs more than the parsing
does — at one row per call, fifteen times more.

## Contributing

Building, testing, how the fixtures are made and how a release goes out are in
[CONTRIBUTING.md](https://github.com/maleus-ai/xlsx/blob/main/CONTRIBUTING.md).

## Licence

MIT.
