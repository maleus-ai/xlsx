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

test("an object in the stream starts a new sheet", async () => {
  const file = output("multi-sheet");
  await pipeline(
    Readable.from(
      [
        ["Ada", 1],
        ["Grace", 2],
        { sheet: "Q2", columns: [{ header: "Client" }, { header: "Signed", type: "date" }] },
        ["Alan", new Date("2024-03-25T00:00:00Z")],
      ],
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "Q1", columns: [{ header: "Name" }, { header: "N" }] }),
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
      [["2024-03-25"], { sheet: "Dates", columns: [{ type: "date" }] }, ["2024-03-25"]],
      { objectMode: true },
    ),
    xlsxWriteStream({ sheet: "Text" }),
    fs.createWriteStream(file),
  );

  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Text" })), [
    ["2024-03-25"],
  ]);
  assert.deepEqual(await collect(xlsxRows(file, { ...PERMISSIVE, sheet: "Dates" })), [
    ["2024-03-25T00:00:00.000Z"],
  ]);
});

test("a repeated sheet name is refused as it is asked for", async () => {
  await assert.rejects(
    pipeline(
      Readable.from([["a"], { sheet: "Q1" }], { objectMode: true }),
      xlsxWriteStream({ sheet: "Q1" }),
      fs.createWriteStream(output("repeated-sheet")),
    ),
    (error) => {
      assert.equal(error.code, "INVALID_SHEET_NAME");
      return true;
    },
  );
});

test("a sheet instruction without a name is refused", async () => {
  await assert.rejects(
    pipeline(
      Readable.from([{ columns: [{ header: "x" }] }], { objectMode: true }),
      xlsxWriteStream({}),
      fs.createWriteStream(output("nameless-sheet")),
    ),
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
        yield { sheet: "B" };
        for (let r = 0; r < 3_000; r += 1) yield [`b-${r}`];
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
