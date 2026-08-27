import assert from "node:assert/strict";
import test from "node:test";

import { listSheets, xlsxRows } from "../lib/index.js";
import { PERMISSIVE, collect, fixture } from "./helpers.mjs";

test("lists sheets with their visibility", async () => {
  const sheets = await listSheets(fixture("hidden-sheets"), PERMISSIVE);

  assert.deepEqual(sheets, [
    { name: "Data", visible: true },
    { name: "Paramètres", visible: false },
    { name: "Notes", visible: false },
  ]);
});

test("reads the first sheet when none is named", async () => {
  const rows = await collect(xlsxRows(fixture("sheets-4"), PERMISSIVE));

  assert.equal(rows.length, 31);
  assert.equal(rows[1][0], "Sheet1");
});

test("reads a sheet by name", async () => {
  const rows = await collect(
    xlsxRows(fixture("sheets-8"), { ...PERMISSIVE, sheet: "Sheet6" }),
  );

  assert.equal(rows.length, 31);
  assert.equal(rows[1][0], "Sheet6");
});

test("types every kind of cell", async () => {
  const rows = await collect(xlsxRows(fixture("types"), PERMISSIVE));

  assert.deepEqual(rows, [
    [
      "shared text",
      "inline & escaped",
      42,
      3.5,
      true,
      false,
      "#DIV/0!",
      // The same serial a number cell would hold, told apart by its numFmt
      // alone. This is the distinction a reader without the style table loses,
      // and it loses it silently: a wrong value in the database, not a throw.
      "2024-03-25T00:00:00.000Z",
      "2024-03-25T12:00:00.000Z",
      // A formula cell yields its cached result, not its formula.
      84,
      // `[h]:mm:ss` is an elapsed time, not a point in one: an ISO 8601
      // duration, with hours that do not wrap at 24.
      "PT30H0M0S",
      // A formula with no cached result: formulas are never recalculated.
      null,
    ],
  ]);
});

test("the 1904 date system lands on the same day", async () => {
  const rows = await collect(xlsxRows(fixture("dates-1904"), PERMISSIVE));

  assert.deepEqual(rows, [
    ["2024-03-25T00:00:00.000Z", "2024-03-25T12:00:00.000Z"],
  ]);
});

test("sparse rows keep their column index", async () => {
  for (const name of ["sparse", "sparse-nodim"]) {
    const rows = await collect(xlsxRows(fixture(name), PERMISSIVE));

    // Row 4 is absent from the sheet and is not materialised.
    assert.equal(rows.length, 4, name);
    // A hole in the middle stays a hole; neighbours do not shift left.
    assert.deepEqual(rows[1], [5, null, 7, 8], name);
    // Holes at the end are padded out to the widest row seen, not truncated.
    assert.deepEqual(rows[2], [9, 10, null, null], name);
    assert.deepEqual(rows[3], [null, null, 11, null], name);
  }
});

test("the sheet list can be read from the stream, on one open", async () => {
  const rows = xlsxRows(fixture("sheets-4"), PERMISSIVE);

  assert.deepEqual(
    (await rows.sheets()).map((sheet) => sheet.name),
    ["Sheet1", "Sheet2", "Sheet3", "Sheet4"],
  );

  // And the stream still reads through afterwards.
  assert.equal((await collect(rows)).length, 31);
});

test("batch size does not change what comes out", async () => {
  const reference = await collect(xlsxRows(fixture("sheets-2"), PERMISSIVE));

  for (const batchSize of [1, 7, 10_000]) {
    const rows = await collect(
      xlsxRows(fixture("sheets-2"), { ...PERMISSIVE, batchSize }),
    );
    assert.deepEqual(rows, reference, `batchSize ${batchSize}`);
  }
});

test("every tab of a sixteen-tab workbook reads through, twenty times over", async () => {
  // The failure that started all of this: unmodified exceljs read 20/20 at two
  // tabs, 19/20 at four, and 0/20 at five.
  const file = fixture("sheets-16");

  for (let run = 0; run < 20; run += 1) {
    const sheets = await listSheets(file, PERMISSIVE);
    assert.equal(sheets.length, 16, `run ${run}`);

    for (const [index, sheet] of sheets.entries()) {
      const rows = await collect(
        xlsxRows(file, { ...PERMISSIVE, sheet: sheet.name }),
      );
      assert.equal(rows.length, 31, `run ${run}, ${sheet.name}`);
      assert.equal(
        rows[1][0],
        `Sheet${index + 1}`,
        `run ${run}, ${sheet.name}`,
      );
    }
  }
});
