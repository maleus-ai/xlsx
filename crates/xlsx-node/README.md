# @maleus/xlsx

Spreadsheets submitted by users are not safe, and well-crafted malicious XLSX
files can be used to take down backend servers.

This package is FAST, and it reads one worksheet of an `.xlsx` file as a stream
of typed rows, **at bounded memory**. It writes one the same way: rows in, an
`.xlsx` out in pieces.

Rust, through a [napi-rs](https://napi.rs) binding, on
[`calamine`](https://github.com/tafia/calamine) for reading and
[`rust_xlsxwriter`](https://github.com/jmcnamara/rust_xlsxwriter) for writing.

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
  maxDecompressedBytes: number; // required, see The bounds
  maxRows: number; // required
  batchSize?: number; // defaults to 1000
}
```

`xlsxRows` returns a `Readable` in object mode. The path must be a file on
disk, not a stream: a ZIP is read through its central directory, which sits at
the end of the archive, so a correct read needs to seek.

`listSheets` reads two small parts and no rows, so it costs 3 ms on a workbook
that takes 3.7 s to read. When you are going to read rows anyway,
`xlsxRows(...).sheets()` answers the same question off the open you already pay
for.

### Rows

A row is `Array<string | number | boolean | null>`, each value at **its column
index**. Holes stay holes; rows are padded to the widest row seen *so far*,
which a streaming reader cannot know in advance:

```
      A     B     C     D
  1   1                                  →  [1]
  2   2     3     4     5                →  [2, 3, 4, 5]
  3   6                                  →  [6, null, null, null]
  4                                      →  (empty row, not materialised)
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

## Writing

```ts
function xlsxWriteStream(options?: XlsxWriteStreamOptions): XlsxWriteStream;

interface XlsxWriteStreamOptions {
  sheet?: string; // defaults to "Sheet1"
  columns?: Array<{ header?: string; type?: "date" }>;
  tempDir?: string; // defaults to the platform temporary directory
  batchSize?: number; // defaults to 1000
}
```

`xlsxWriteStream` returns a `Transform`: rows in on the writable side in object
mode, `Buffer` chunks out on the readable side. It drops into a `pipeline`
between whatever produces the rows and wherever the file is going — a file, a
socket, an HTTP response.

**Bytes arrive only once the rows are in.** Each row is flushed to a spill file
as the next one is written, and the archive is assembled from those files at
`end()`. Nothing is readable before then: it is a stream, but not a transform of
rows into bytes as they arrive.

### Rows

`Array<string | number | boolean | Date | null>`, at their column index. `null`
leaves a blank without shifting neighbours.

### Dates are declared, never guessed

```js
xlsxWriteStream({ columns: [{ header: "Signed", type: "date" }] });
```

In an XLSX a date is a serial *plus* a `numFmt`. Write the serial alone and the
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

Reads one worksheet, writes one worksheet, and creates new files only — there is
no open-change-save.

| | Reading | Writing |
| --- | --- | --- |
| Values, typed | yes | yes |
| Dates | yes | yes, with the format that makes them dates |
| Multiple sheets | read any by name; list all | one sheet per file |
| Formulas | cached result only, never recalculated | **cannot be written at all** |
| Macros, charts, images, embedded objects | ignored, not reported | not written |
| Cell formatting, widths, merges | ignored | not written |
| CSV, ODS, `.xls` | no | no |
| Synchronous API | no | no |

Two consequences worth stating outright.

**No formula can be emitted.** A value beginning with `=`, `+`, `-` or `@` —
`=cmd|'/c calc'!A0` typed into a form field, say — reaches the sheet as those
characters. This is a capability the package does not have rather than a default
it applies: there is no option to turn it off.

**Macros and embedded objects in a file being read are neither examined nor
reported.** This package bounds what a file costs, not what it contains. If a
value you read is re-exported somewhere that evaluates it, that is yours to
handle.

If you need charts, styling or multi-sheet output, `rust_xlsxwriter` underneath
does all of it — this package deliberately does not expose it.

## The bounds

Reading takes two mandatory budgets. They are mandatory because a bound nobody
set is invisible in review and discovered in production; a caller who wants no
ceiling writes `Number.MAX_SAFE_INTEGER` and leaves that decision in their own
source.

They live *inside* the reader because a check placed above it protects nothing:
an XLSX builds four tables — package relationships, shared strings, styles,
workbook — before the first row exists, each sized by the file and none by the
row count.

`maxRows` is rows the sheet may yield, header included, raised on the row that
crosses it.

`maxDecompressedBytes` is what the archive may expand to. Three expansions are
charged against it:

- **Bytes leaving the inflater**, measured, never the declared sizes: `zip`
  verifies an entry's CRC but does not cap its output, so an archive may
  announce 64 bytes for a part that deploys 64 MB.
- **The archive's directory.** Entries cost memory to index before any is
  inflated, so the count is read from the trailer and charged before opening, at
  roughly one entry per kilobyte of budget.
- **Blank values a sheet's geometry implies.** Column indices accumulate as
  `col * 26 + letter`, so `r="BZZZZZ1"` names column 36 119 382 — a gigabyte
  without one extra byte inflating. References past the format's grid are
  refused; padding inside it is charged like inflated bytes.

Two further limits are not options: a batch holds at most ~32 MB of values
whatever was asked for, and declared XML entities are never expanded, which
makes entity-expansion and external-entity payloads errors rather than reads.

Writing takes no budgets. The data comes from the program that owns it, not from
a stranger, so a mandatory ceiling there would guard nothing. What is enforced is
Excel's grid — 1 048 576 rows by 16 384 columns — reported with the cell that
crossed it.

Every hostile workbook in `fixtures/` is refused before the thing it was built
to cause. An 8 MiB budget, one process per figure:

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

## Errors

Every refusal is an `XlsxError` with a stable `code`. Branch on the code, not on
the message.

Reading: `NOT_AN_ARCHIVE`, `SHEET_NOT_FOUND`, `DECOMPRESSED_BUDGET_EXCEEDED`,
`TOO_MANY_ENTRIES`, `ROW_BUDGET_EXCEEDED`, `CORRUPT`.
Writing: `INVALID_DATETIME`, `INVALID_VALUE`, `SHEET_LIMIT_EXCEEDED`,
`INVALID_SHEET_NAME`, `WRITE_FAILED`.
Either: `IO`, `INVALID_OPTION`, `CLOSED`, `UNSUPPORTED_PLATFORM`.

## Measurements

```sh
node fixtures/generate.mjs   # rebuild the fixtures
pnpm run build               # release binary
node scripts/bench.mjs       # the tables below
```

One process per figure, `VmHWM` for the peak, which includes Node's own ~40 MB
baseline.

**Reading:**

| Workbook                  | Rows    | Time    | Peak RSS |
| ------------------------- | ------- | ------- | -------- |
| `large-200000`            | 200 001 | 1.3 s   | 66 MB    |
| `large-600000`            | 600 001 | 3.7 s   | 68 MB    |
| `large-600000`, list only | —       | 0.003 s | 49 MB    |

Three times the rows, two megabytes apart. `batchSize` 1000 is the default
because it is the fastest measured and 39 MB lighter than 10000; below 100 the
FFI crossing costs more than the parsing.

**Writing**, four columns of which one holds dates:

| Rows      | Time   | Output  | Peak RSS |
| --------- | ------ | ------- | -------- |
| 100 000   | 1.0 s  | 1.9 MB  | 78 MB    |
| 600 000   | 7.3 s  | 11.3 MB | 102 MB   |
| 1 048 575 | 12.5 s | 19.8 MB | 135 MB   |

Most of that is not this package. At the peak sample of the 600 000 row export:
40 MB is an idle `node`, +50 MB is moving row arrays through *any* `Transform`
(a plain one with no addon peaks at 89.6 MB), +11 MB is output buffers awaiting
collection, and the Rust writer itself is ~0 — native memory outside V8 is
47.9 MB without the addon and 47.7 MB with it. Measured on its own, outside
Node, the Rust side holds **4.2 MB** for a sheet at Excel's maximum.

Output buffers are bounded independently of export size. They look linear while
small (18.9 MB of output → 20.6 MB held) but that is below the knee:

| Output produced | `external` at peak | Peak RSS |
| --------------- | ------------------ | -------- |
| 123 MB          | 63.5 MB            | 154 MB   |
| 247 MB          | 66.7 MB            | 163 MB   |
| **432 MB**      | **66.8 MB**        | 184 MB   |

Three and a half times the file for the same ceiling, in a sawtooth —
`3 → 15 → 31 → 46 → 16 → 33 → 50 → 4` — climbing to ~64 MB, collected, climbing
again. That is where V8 forces a collection over external memory; the ceiling
and the shape are measured, the attribution is inferred from the figure. Under a
hard container limit, note that a sawtooth sits near its peak, not its average.

The file comes out in pieces — 427 chunks for the 600 000 row export, largest
30 KB — so a consumer forwards fragments instead of holding a file.

**Flat memory when writing is bought with disk.** Rows spill to a temporary
file: ~178 MB of uncompressed XML for a sheet at Excel's maximum, deflating to
~20 MB out. One unlinked file, released by the kernel on close. Where the
temporary directory is memory-backed — `tmpfs`, `/dev/shm`, a Kubernetes
`emptyDir` with `medium: Memory` — that spill is RAM again *and does not appear
in RSS*. Point `tempDir` at real disk there.

Two things the bounds do not cover: **time** (no wall-clock budget; destroying a
stream releases the archive but does not interrupt the batch in flight), and
**memory safety in dependencies** — this trusts `calamine`, `zip` and
`rust_xlsxwriter`, and carries the only `unsafe` in the repository, two
expressions in `crates/xlsx-core/src/cursor.rs` whose invariants are written
above them.

## Contributing

Building, testing, fixtures and releases are in
[CONTRIBUTING.md](https://github.com/maleus-ai/xlsx/blob/main/CONTRIBUTING.md).

## Licence

MIT.
