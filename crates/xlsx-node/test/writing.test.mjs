import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

import {
  collect,
  PERMISSIVE,
  runInChild,
  settledDescriptors,
  timerCadence,
} from "./helpers.mjs";
import { xlsxRows, xlsxWriteStream, XlsxError } from "../lib/index.js";

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "xlsx-writing-"));
test.after(() => fs.rmSync(scratch, { recursive: true, force: true }));

function output(name) {
  return path.join(scratch, `${name}.xlsx`);
}

/** Write rows through the stream and read every row straight back. */
async function roundTrip(name, rows, options = {}) {
  const file = output(name);
  await pipeline(
    Readable.from(rows, { objectMode: true }),
    xlsxWriteStream(options),
    fs.createWriteStream(file),
  );
  return { file, rows: await collect(xlsxRows(file, PERMISSIVE)) };
}

test("a workbook written here is one this package can read back", async () => {
  const { rows } = await roundTrip("round-trip", [
    ["Ada", 42.5, true],
    ["Grace", -0.125, false],
  ]);

  assert.deepEqual(rows, [
    ["Ada", 42.5, true],
    ["Grace", -0.125, false],
  ]);
});

test("a declared date column comes back as a date, not as a number", async () => {
  // The whole reason `columns` exists. A serial written without a number
  // format reads back as 45376 — a number where the business expects a day.
  const { rows } = await roundTrip(
    "dates",
    [["Signed", new Date("2024-03-25T00:00:00Z")]],
    { columns: [{ header: "Label" }, { header: "Signed", type: "date" }] },
  );

  assert.deepEqual(rows[0], ["Label", "Signed"], "the header row");
  assert.equal(rows[1][1], "2024-03-25T00:00:00.000Z");
});

test("a Date outside a declared date column is refused, and says how to fix it", async () => {
  // Writing it anyway would put a serial with no number format in the cell,
  // which reads back as 45376. Refusing costs the caller one line of options;
  // accepting costs them a wrong value in a database.
  await assert.rejects(
    pipeline(
      Readable.from([[new Date("2024-03-25T00:00:00Z")]], { objectMode: true }),
      xlsxWriteStream({}),
      fs.createWriteStream(output("undeclared-date")),
    ),
    (thrown) => {
      assert.ok(thrown instanceof XlsxError, `got ${thrown}`);
      assert.equal(thrown.code, "INVALID_VALUE");
      assert.match(thrown.message, /type: "date"/);
      return true;
    },
  );
});

test("a header label is text even in a column declared to hold dates", async () => {
  // Regression: the header used to be pushed in with the rows, so "Signed" was
  // handed to the timestamp parser and the whole export failed on its first row.
  const { rows } = await roundTrip("header-in-date-column", [[new Date("2024-03-25T00:00:00Z")]], {
    columns: [{ header: "Signed", type: "date" }],
  });

  assert.deepEqual(rows[0], ["Signed"]);
  assert.equal(rows[1][0], "2024-03-25T00:00:00.000Z");
});

test("a string that looks like a formula is written as a string", async () => {
  const { rows } = await roundTrip("injection", [
    ["=1+1", "=cmd|'/c calc'!A0", "+1", "-1", "@SUM(A1)"],
  ]);

  assert.deepEqual(rows[0], ["=1+1", "=cmd|'/c calc'!A0", "+1", "-1", "@SUM(A1)"]);
});

test("null leaves a blank without shifting its neighbours", async () => {
  const { rows } = await roundTrip("blanks", [["left", null, "right"]]);
  assert.deepEqual(rows[0], ["left", null, "right"]);
});

test("the sheet carries the name it was given", async () => {
  const { file } = await roundTrip("sheet-name", [["x"]], { sheet: "Trimestre 1" });
  const sheets = await xlsxRows(file, PERMISSIVE).sheets();
  assert.deepEqual(
    sheets.map((sheet) => sheet.name),
    ["Trimestre 1"],
  );
});

test("a sheet name Excel refuses is refused here", async () => {
  assert.throws(() => xlsxWriteStream({ sheet: "a/b" }), (error) => {
    assert.equal(error.code, "INVALID_SHEET_NAME");
    return true;
  });
});

test("nothing is readable until the rows are in", async () => {
  // The documented shape of this stream, asserted rather than asserted about.
  // Rows spill to a temporary file as they arrive and the archive is built from
  // them at the end, so a caller waiting for a first chunk mid-import would wait
  // for something that cannot come.
  const stream = xlsxWriteStream({});
  let bytesBeforeEnd = 0;
  stream.on("data", (chunk) => {
    bytesBeforeEnd += chunk.length;
  });

  for (let row = 0; row < 5_000; row += 1) {
    stream.write([`row-${row}`, row]);
  }
  // Let anything that was going to be emitted, be emitted.
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(bytesBeforeEnd, 0, "bytes appeared before end()");

  stream.end();
  await new Promise((resolve) => stream.on("end", resolve));
  assert.ok(bytesBeforeEnd > 0, "and none appeared after it either");
});

test("the output arrives in pieces rather than as one buffer", async () => {
  // What makes the readable side worth having: a consumer sees the file in
  // fragments it can forward, not a single allocation of the whole thing.
  const stream = xlsxWriteStream({});
  const sizes = [];
  stream.on("data", (chunk) => sizes.push(chunk.length));

  const rows = Readable.from(
    (function* () {
      for (let row = 0; row < 50_000; row += 1) {
        yield [`row-${row}`, row, row % 2 === 0];
      }
    })(),
    { objectMode: true },
  );

  await pipeline(rows, stream, fs.createWriteStream(output("chunked")));

  assert.ok(sizes.length > 10, `only ${sizes.length} chunks`);
  assert.ok(
    Math.max(...sizes) < 1 << 20,
    `largest chunk was ${Math.max(...sizes)} bytes`,
  );
});

test("abandoning the stream gives the spill file back", async () => {
  const before = await settledDescriptors();

  for (let attempt = 0; attempt < 20; attempt += 1) {
    const stream = xlsxWriteStream({});
    for (let row = 0; row < 200; row += 1) {
      stream.write([`row-${row}`, row]);
    }
    // A consumer that walks away: no end(), no pipe, just gone.
    stream.destroy();
  }

  const after = await settledDescriptors();
  assert.ok(
    after <= before + 2,
    `descriptors went from ${before} to ${after}; the spill files were kept`,
  );
});

test("a long export does not hold the event loop", async () => {
  const rows = Readable.from(
    (function* () {
      for (let row = 0; row < 200_000; row += 1) {
        yield [`row-${row}`, row, row % 2 === 0];
      }
    })(),
    { objectMode: true },
  );

  const { ticks, durationMs } = await timerCadence(() =>
    pipeline(rows, xlsxWriteStream({}), fs.createWriteStream(output("cadence"))),
  );

  // Cadence, not absolute jitter: a loop that was held gives no ticks at all,
  // whatever else the machine is doing. One tick per 50 ms is a tenth of the
  // rate a free loop manages and still an order of magnitude above blocked.
  assert.ok(
    ticks > durationMs / 50,
    `${ticks} ticks over ${Math.round(durationMs)} ms`,
  );
});

test("a full export holds its memory flat", () => {
  // In a child, because peak RSS is a property of a process and `node --test`
  // runs these in one.
  const report = runInChild(`
    import { Readable } from "node:stream";
    import { pipeline } from "node:stream/promises";

    const rows = Readable.from((function* () {
      for (let row = 0; row < 200000; row += 1) {
        yield ["row-" + row, row, row % 2 === 0, "text for row " + row];
      }
    })(), { objectMode: true });

    let bytes = 0;
    await pipeline(rows, xlsxWriteStream({ sheet: "Export" }), async function (source) {
      for await (const chunk of source) { bytes += chunk.length; }
    });

    console.log(JSON.stringify({ bytes, peakRssKb: peakRss() / 1024 }));
  `);

  const { bytes, peakRssKb } = JSON.parse(report.trim().split("\n").pop());
  assert.ok(bytes > 0, "nothing was written");

  if (peakRssKb === null) {
    return; // Not Linux; the assertion would have nothing to stand on.
  }
  // The reader's own ceiling is 150 MB for 600 000 rows. Writing spills to disk
  // instead of holding rows, so it is held to a much tighter bar.
  assert.ok(
    peakRssKb < 120 * 1024,
    `peak RSS was ${Math.round(peakRssKb / 1024)} MB`,
  );
});

test("a row can name the sheet it goes to", async () => {
  const file = output("multi-sheet");
  await pipeline(
    Readable.from(
      [
        ["Ada", 1],
        ["Grace", 2],
        { sheet: "Q2", data: ["Alan", new Date("2024-03-25T00:00:00Z")] },
      ],
      { objectMode: true },
    ),
    xlsxWriteStream({
      sheet: "Q1",
      columns: [{ header: "Name" }, { header: "N" }],
      sheets: { Q2: { columns: [{ header: "Client" }, { header: "Signed", type: "date" }] } },
    }),
    fs.createWriteStream(file),
  );

  const sheets = await xlsxRows(file, PERMISSIVE).sheets();
  assert.deepEqual(sheets.map((s) => s.name), ["Q1", "Q2"]);

  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Q1" })), [
    ["Name", "N"],
    ["Ada", 1],
    ["Grace", 2],
  ]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Q2" })), [
    ["Client", "Signed"],
    ["Alan", "2024-03-25T00:00:00.000Z"],
  ]);
});

test("each sheet carries its own date columns", async () => {
  // The declaration is per sheet: column 0 holds dates on the second sheet and
  // plain text on the first. Getting this wrong would hand "2024-03-25" to the
  // timestamp parser on the wrong sheet, or write a serial with no format.
  const file = output("per-sheet-dates");
  await pipeline(
    Readable.from(
      [["2024-03-25"], { sheet: "Dates", data: ["2024-03-25"] }],
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "Text", sheets: { Dates: { columns: [{ type: "date" }] } } }),
    fs.createWriteStream(file),
  );

  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Text" })), [
    ["2024-03-25"],
  ]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Dates" })), [
    ["2024-03-25T00:00:00.000Z"],
  ]);
});

test("an unsorted source needs no sorting", async () => {
  // The property the whole shape exists for: rows arrive for whichever sheet,
  // in any order, and each lands under what that sheet already holds.
  const file = output("unsorted");
  await pipeline(
    Readable.from(
      [
        { sheet: "A", data: ["a0"] },
        { sheet: "B", data: ["b0"] },
        { sheet: "A", data: ["a1"] },
        { sheet: "C", data: ["c0"] },
        { sheet: "B", data: ["b1"] },
        { sheet: "A", data: ["a2"] },
      ],
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "A" }),
    fs.createWriteStream(file),
  );

  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "A" })), [
    ["a0"], ["a1"], ["a2"],
  ]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "B" })), [["b0"], ["b1"]]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "C" })), [["c0"]]);
});

test("naming the sheet a bare row would go to is the same sheet", async () => {
  const file = output("same-sheet-two-ways");
  await pipeline(
    Readable.from([["bare"], { sheet: "Data", data: ["named"] }], { objectMode: true }),
    xlsxWriteStream({ sheet: "Data" }),
    fs.createWriteStream(file),
  );

  const sheets = await xlsxRows(file, PERMISSIVE).sheets();
  assert.deepEqual(sheets.map((s) => s.name), ["Data"], "no second sheet was made");
  assert.deepEqual(await collect(xlsxRows(file, PERMISSIVE)), [["bare"], ["named"]]);
});

test("a row object without a sheet name is a bad value, not a bad option", async () => {
  // It is an element of the stream, so it is the data that is wrong, not the
  // configuration. A caller branching on `code` to tell "my config is wrong,
  // give up" from "this record is bad, report it" must not be misled — and the
  // neighbouring branch, a value that is neither array nor object, already
  // says INVALID_VALUE for the same class of problem.
  await assert.rejects(
    pipeline(
      Readable.from([{ data: ["x"] }], { objectMode: true }),
      xlsxWriteStream({}),
      fs.createWriteStream(output("nameless-sheet")),
    ),
    (error) => {
      assert.equal(error.code, "INVALID_VALUE");
      return true;
    },
  );
});

test("an export with no rows still carries its header", async () => {
  // The common case of a report that found nothing. A consumer reading its
  // columns from the first line needs that line to exist.
  const file = output("no-rows");
  await pipeline(
    Readable.from([], { objectMode: true }),
    xlsxWriteStream({ sheet: "Export", columns: [{ header: "Client" }, { header: "N" }] }),
    fs.createWriteStream(file),
  );

  assert.deepEqual(await collect(xlsxRows(file, PERMISSIVE)), [["Client", "N"]]);
});

test("a declared sheet that receives nothing still exists", async () => {
  const file = output("declared-unused");
  await pipeline(
    Readable.from([["a"]], { objectMode: true }),
    xlsxWriteStream({
      sheet: "Export",
      columns: [{ header: "C" }],
      sheets: { Other: { columns: [{ header: "X" }] } },
    }),
    fs.createWriteStream(file),
  );

  const sheets = await xlsxRows(file, PERMISSIVE).sheets();
  assert.deepEqual(sheets.map((s) => s.name), ["Export", "Other"]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Other" })), [["X"]]);
});

test("a bad name in sheets is refused where it is written", async () => {
  // Not at the first row that happens to go there: one bad name is one error,
  // whether or not any data reaches it.
  for (const name of ["a/b", "", "x".repeat(32), "ctrl\u0001"]) {
    assert.throws(
      () => xlsxWriteStream({ sheet: "Good", sheets: { [name]: { columns: [] } } }),
      (error) => {
        assert.equal(error.code, "INVALID_SHEET_NAME", `for ${JSON.stringify(name)}`);
        return true;
      },
    );
  }
});

test("a control character in a sheet name is refused", async () => {
  // It reaches the workbook XML raw and leaves a file no strict parser opens.
  assert.throws(
    () => xlsxWriteStream({ sheet: "Client\u0001A" }),
    (error) => {
      assert.equal(error.code, "INVALID_SHEET_NAME");
      return true;
    },
  );
});

test("a workbook stops at its sheet ceiling", async () => {
  // Each sheet holds a temporary file open until the workbook is finished.
  await assert.rejects(
    pipeline(
      Readable.from(
        (function* () {
          for (let i = 0; i < 10; i += 1) yield { sheet: `S${i}`, data: [i] };
        })(),
        { objectMode: true },
      ),
      xlsxWriteStream({ sheet: "S0", maxSheets: 4 }),
      fs.createWriteStream(output("sheet-ceiling")),
    ),
    (error) => {
      assert.equal(error.code, "TOO_MANY_SHEETS");
      return true;
    },
  );
});

test("columns given twice for one sheet are refused", async () => {
  assert.throws(
    () => xlsxWriteStream({ sheet: "Q1", columns: [], sheets: { Q1: { columns: [] } } }),
    (error) => {
      assert.equal(error.code, "INVALID_OPTION");
      return true;
    },
  );
});

test("rows keep going to the right sheet across a large export", async () => {
  // The row counter resets per sheet; if it did not, the second sheet would
  // start at the first one's height with a block of blanks above it.
  const file = output("multi-sheet-large");
  await pipeline(
    Readable.from(
      (function* () {
        for (let r = 0; r < 5_000; r += 1) yield [`a-${r}`];
        for (let r = 0; r < 3_000; r += 1) yield { sheet: "B", data: [`b-${r}`] };
      })(),
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "A" }),
    fs.createWriteStream(file),
  );

  const a = await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "A" }));
  const b = await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "B" }));
  assert.equal(a.length, 5_000);
  assert.equal(b.length, 3_000, "the second sheet must not inherit the first's height");
  assert.deepEqual(b[0], ["b-0"]);
});

test("an interleaved source batches as well as a sorted one", async () => {
  // Regression, and the reason rows are held per sheet rather than in one
  // queue. A single queue has to be flushed on every change of sheet, which on
  // an alternating source is a native call per row. Measured at this size:
  // 13.1 s against 0.5 s, and at 600 000 rows, 150 s against 5.8 s.
  //
  // Asserted as a ratio against the same rows in sheet order, not as a
  // wall-clock threshold — a first attempt at an absolute number passed
  // against the very bug it was written to catch. The scale matters too: the
  // same test over eight sheets and no date column did not separate the two at
  // all, so it is pinned to the shape that does.
  const SHEETS = 12;
  const ROWS = 50_000;
  const columns = [{ header: "a" }, { header: "b" }, { header: "d", type: "date" }];

  const run = async (interleaved, name) => {
    let seed = 12345;
    const started = performance.now();
    await pipeline(
      Readable.from(
        (function* () {
          for (let r = 0; r < ROWS; r += 1) {
            seed = (seed * 1103515245 + 12345) & 0x7fffffff;
            const sheet = interleaved
              ? `S${seed % SHEETS}`
              : `S${Math.floor((r * SHEETS) / ROWS)}`;
            yield { sheet, data: [`row-${r}`, r, "2024-03-25T00:00:00.000Z"] };
          }
        })(),
        { objectMode: true },
      ),
      xlsxWriteStream({
        sheet: "S0",
        sheets: Object.fromEntries(
          Array.from({ length: SHEETS }, (_, i) => [`S${i}`, { columns }]),
        ),
      }),
      fs.createWriteStream(output(name)),
    );
    return performance.now() - started;
  };

  const sorted = await run(false, "batching-sorted");
  const shuffled = await run(true, "batching-shuffled");

  assert.ok(
    shuffled < sorted * 4,
    `interleaved took ${Math.round(shuffled)} ms against ${Math.round(sorted)} ms in ` +
      "sheet order; rows are no longer being batched per sheet",
  );
});

test("every row of an interleaved source lands on its own sheet", async () => {
  const file = output("interleaved-correctness");
  const SHEETS = 5;
  const ROWS = 5_000;
  let seed = 999;
  const expected = new Map();

  await pipeline(
    Readable.from(
      (function* () {
        for (let r = 0; r < ROWS; r += 1) {
          seed = (seed * 1103515245 + 12345) & 0x7fffffff;
          const sheet = `S${seed % SHEETS}`;
          const value = `${sheet}-${r}`;
          if (!expected.has(sheet)) expected.set(sheet, []);
          expected.get(sheet).push(value);
          yield { sheet, data: [value] };
        }
      })(),
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "S0" }),
    fs.createWriteStream(file),
  );

  for (const [sheet, values] of expected) {
    const rows = await collect(xlsxRows(file, { ...PERMISSIVE, sheet }));
    assert.deepEqual(
      rows.map((row) => row[0]),
      values,
      `sheet ${sheet} lost rows, gained blanks, or reordered them`,
    );
  }
});
