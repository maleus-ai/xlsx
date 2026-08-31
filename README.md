# @maleus/xlsx-reader

Spreadsheets submitted by users are not safe, and well-crafted malicious XLSX
files can be used to take down backend servers.

This package is FAST, and it reads one worksheet of an `.xlsx` file as a stream
of typed rows, **at bounded memory**. It writes one the same way: rows in, an
`.xlsx` out in pieces, at a memory ceiling the size of the export does not move.

Written in Rust on [`calamine`](https://github.com/tafia/calamine) for reading
and [`rust_xlsxwriter`](https://github.com/jmcnamara/rust_xlsxwriter) for
writing, published to npm as `@maleus/xlsx-reader` through a
[napi-rs](https://napi.rs) binding.

```js
import { xlsxRows } from "@maleus/xlsx-reader";

for await (const row of xlsxRows(uploadedFile.path, {
  maxDecompressedBytes: 512 * 1024 * 1024,
  maxRows: 1_000_000,
})) {
  // row: Array<string | number | boolean | null>
}
```

```js
import { xlsxWriteStream } from "@maleus/xlsx-reader";

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

### Writing

```ts
function xlsxWriteStream(options?: XlsxWriteStreamOptions): XlsxWriteStream;

interface XlsxWriteStreamOptions {
  sheet?: string; // defaults to "Sheet1"
  columns?: Array<{ header?: string; type?: "date" }>;
  tempDir?: string; // defaults to the platform temporary directory
  batchSize?: number; // defaults to 1000
}
```

`xlsxWriteStream` returns a `Transform`: rows go in on the writable side in
object mode, and the `.xlsx` comes out on the readable side as `Buffer` chunks,
so it drops into a `pipeline` between whatever produces the rows and wherever
the file is going — a file, a socket, an HTTP response.

**Bytes arrive only once the rows are in.** Each row is flushed to a spill file
as the next one is written, and the archive is assembled from those files when
the writable side ends. So nothing is readable before `end()`: it is a stream,
but not a transform of rows into bytes as they arrive. A caller waiting on a
first chunk mid-export waits for something that cannot come.

### What a written row looks like

A row is `Array<string | number | boolean | Date | null>`, each value at **its
column index**. `null` leaves a cell blank without shifting its neighbours,
which is the placement rule the reader applies coming the other way.

**Dates are declared, never guessed.** A column that holds timestamps says so:

```js
xlsxWriteStream({
  columns: [{ header: "Client" }, { header: "Signed", type: "date" }],
});
```

In an XLSX a date is a serial *plus* a `numFmt`. Write the serial alone and the
cell reads back as `45376` — a number where the business expects a day, which is
the same defect this package's reader exists to stop, arriving from the other
side. So a `Date` in a column that was not declared is **refused** rather than
written wrongly, and the error says which column to declare.

Inference is not on offer either: `"2024-03-25"` is a perfectly good product
reference as well as a perfectly good day, and nothing in the value says which
one you meant.

Strings in a declared date column are parsed as ISO 8601, which means the
reader's own output feeds straight back in. A UTC offset is refused rather than
dropped — XLSX stores no timezone, so honouring `+02:00` would shift the value
and ignoring it would keep the wrong one. Excel counts days from 1900 and has no
serial for anything earlier; a date before then is refused too, and a caller who
must keep one writes it as text.

### Every string is written as a string

There is no way to emit a formula. A value that begins with `=`, `+`, `-` or `@`
— a name someone typed into a form, `=cmd|'/c calc'!A0` included — reaches the
sheet as those characters, and Excel shows them.

This is a capability the package does not have rather than a default it applies,
which is the difference that matters: there is no option to turn it off, and no
path by which a value from an untrusted source becomes something Excel evaluates.

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
- **Writes one worksheet**, streaming, with the same memory property: rows spill
  to a temporary file as they arrive, so the Rust side holds about four
  megabytes whether the sheet has a thousand rows or Excel's full 1 048 576.
- **Writes dates as dates**, with the number format that makes them one, and
  refuses to write a timestamp it cannot place rather than putting some other
  day in the cell.

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

Writing has the mirror of that goal and none of that threat model. The data
comes from the program that owns it, not from a stranger, so there are no
mandatory budgets on the writing side — mirroring them there would be a ceiling
nobody needs on data nobody attacked. What is enforced is the grid Excel
actually defines, 1 048 576 rows by 16 384 columns, reported with the cell that
crossed it.

What carries over is the memory property, with one honest qualification. On the
Rust side it holds outright: **the size of the export does not decide how much
memory it uses** — a report of ten rows and a report of a million both cost
about four megabytes. On the Node side the rows cost
nothing that grows, and the output chunks sit under a ceiling of their own that
does not move with the size of the export —
[Measurements](#where-the-output-buffers-go) has the figures.

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

It reads one worksheet and writes one worksheet. It does not read CSV or ODS,
recompute formulas, or offer a synchronous API. Anything else has to argue its
way past that sentence first.

On the writing side in particular, it does not **edit a workbook that already
exists** — there is no open-change-save. It creates a new file, and a workbook
it produces has one sheet, no formulas, no charts, no images, and no formatting
beyond the number formats that make a date a date. If you need any of those,
this is the wrong package and `rust_xlsxwriter` underneath it will do all of
them.

Four things it is worth being explicit about, because the bounds above might
suggest otherwise:

- **It does not inspect content.** Macros, embedded objects and external links
  are neither examined nor reported. Formula injection in a value that gets
  re-exported somewhere else is the consumer's problem, not this reader's.
- **Flat memory when writing is bought with disk.** Rows spill to a temporary
  file rather than being held: roughly 178 MB of uncompressed XML for a sheet at
  Excel's maximum, deflating to about 20 MB on the way out. It is one unlinked
  file, released by the kernel when the handle closes. Where the temporary
  directory is memory-backed — a `tmpfs`, a `/dev/shm`, a Kubernetes `emptyDir`
  with `medium: Memory` — that spill is RAM again *and does not appear in the
  process's RSS*, so the reading stays flat while the machine fills up. Point
  `tempDir` at real disk on those hosts.
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

Writing, through the same facade, one process per figure, four columns of which
one holds dates:

| Rows          | Time   | Output  | Peak RSS |
| ------------- | ------ | ------- | -------- |
| 100 000       | 1.0 s  | 1.9 MB  | 78 MB    |
| 300 000       | 3.3 s  | 5.7 MB  | 96 MB    |
| 600 000       | 7.3 s  | 11.3 MB | 102 MB   |
| 1 048 575     | 12.5 s | 19.8 MB | 135 MB   |

That figure is a whole Node process, and most of it is not this package. Taken
apart at the peak sample, for the 600 000 row export:

| Where the 101 MB is                                  |         |
| ---------------------------------------------------- | ------- |
| An idle `node` process                               | 40 MB   |
| Moving 600 000 row arrays through *any* `Transform`  | +50 MB  |
| Output buffers not yet collected                     | +11 MB  |
| The Rust writer itself                               | ~0 MB   |

The middle line is Node's, not ours: the same 600 000 rows through a plain
`Transform` with no addon involved peak at 89.6 MB. And the last line is
measured rather than assumed — native memory outside V8 comes to 47.9 MB
without the addon and 47.7 MB with it, so the writer's own footprint is inside
the noise. On its own, outside Node, the Rust side holds **4.2 MB** for a sheet
at Excel's maximum.

### Where the output buffers go

The third line of that breakdown is the chunks handed to JavaScript, and they
are not collected as they are dropped — so on a small export `external` reads as
roughly the size of the file produced:

| Rows      | Output  | `external` at peak |
| --------- | ------- | ------------------ |
| 100 000   | 1.8 MB  | 3.5 MB             |
| 600 000   | 10.8 MB | 12.5 MB            |
| 1 048 575 | 18.9 MB | 20.6 MB            |

Those three figures look like a straight line, and they are not one — every one
of them sits below the point where it bends. Pushed further, with text that does
not compress away:

| Output produced | `external` at peak | Peak RSS |
| --------------- | ------------------ | -------- |
| 123 MB          | 63.5 MB            | 154 MB   |
| 247 MB          | 66.7 MB            | 163 MB   |
| **432 MB**      | **66.8 MB**        | 184 MB   |

Three and a half times the file for the same ceiling. Sampled over time it is a
sawtooth — `3 → 15 → 31 → 46 → 16 → 33 → 50 → 4` — climbing to about 64 MB,
being collected, and climbing again. That number is where V8 forces a global
collection over externally allocated memory; the ceiling and the shape are
measured, the attribution to that threshold is an inference from the figure.

So the buffers are **bounded independently of how large the export is**, at
around 67 MB, and what they hold is cyclical garbage rather than accumulation.
Nothing here needs a ceiling of its own.

The one case worth knowing about: under a hard container limit close to the
working set, a sawtooth means RSS sits near its peak rather than near its
average. It is bounded and predictable, but it is not 20 MB.

The file comes out in pieces rather than in one allocation — 427 chunks for the
600 000 row export, the largest 30 KB — so a consumer forwards fragments instead
of holding a file.

## Contributing

Building, testing, how the fixtures are made and how a release goes out are in
[CONTRIBUTING.md](https://github.com/maleus-ai/xlsx/blob/main/CONTRIBUTING.md).

## Licence

MIT.
