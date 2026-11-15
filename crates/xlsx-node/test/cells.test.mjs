import assert from "node:assert/strict";
import test from "node:test";

import { readCells, toA1 } from "../lib/index.js";
import { fixture } from "./helpers.mjs";

const PERMISSIVE_CELLS = { maxDecompressedBytes: 1 << 30, maxCells: 1 << 20 };

async function cellsOf(name, options = {}) {
  const cells = [];
  for await (const cell of readCells(fixture(name), {
    ...PERMISSIVE_CELLS,
    ...options,
  })) {
    cells.push(cell);
  }
  return cells;
}

function at(cells, ref) {
  const cell = cells.find((c) => toA1(c) === ref);
  assert.ok(cell, `no cell at ${ref}`);
  return cell;
}

test("emits every cell the file declares, with its position", async () => {
  const cells = await cellsOf("formulas");

  assert.deepEqual(
    cells.map((c) => toA1(c)),
    [
      "A1",
      "B1",
      "C1",
      "A2",
      "B2",
      "C2",
      "A3",
      "B3",
      "C3",
      "D4",
      "D5",
      "E6",
      "F6",
      "G6",
      "A7",
      "B7",
      "C7",
    ],
  );
});

test("emits a cell without a value that only carries a style as empty", async () => {
  const cells = await cellsOf("formulas");

  assert.deepEqual(at(cells, "E6"), {
    row: 5,
    col: 4,
    type: "empty",
    value: null,
    styleId: 1,
  });
});

test("tells apart a date, an error and a string that the value alone would confuse", async () => {
  const cells = await cellsOf("types");

  assert.deepEqual(
    cells.map((c) => c.type),
    [
      "string",
      "string",
      "number",
      "number",
      "boolean",
      "boolean",
      "error",
      "date",
      "date",
      "number",
      "duration",
      "empty",
    ],
  );
  assert.equal(cells[6].value, "#DIV/0!");
  assert.equal(cells[7].value, "2024-03-25T00:00:00.000Z");
});

test("restores the text of a formula without the equals sign", async () => {
  const cells = await cellsOf("types");

  assert.deepEqual(cells[9].formula, { text: "C1*2", type: "normal" });
  assert.equal(cells[9].value, 84);
});

test("expands a shared formula on every derived cell and keeps its type shared", async () => {
  const cells = await cellsOf("formulas");

  assert.deepEqual(at(cells, "C1").formula, {
    text: "A1*B1",
    type: "shared",
    ref: "C1:C3",
  });
  assert.deepEqual(at(cells, "C2").formula, { text: "A2*B2", type: "shared" });
  assert.deepEqual(at(cells, "C3").formula, { text: "A3*B3", type: "shared" });
});

test("restores the range of an array formula on its master cell", async () => {
  const cells = await cellsOf("formulas");

  assert.deepEqual(at(cells, "D4").formula, {
    text: "A1:A2*2",
    type: "array",
    ref: "D4:D5",
  });
  assert.equal(at(cells, "D5").formula, undefined);
});

test("restores the cached value of a formula without recalculating it", async () => {
  const cells = await cellsOf("formulas");
  assert.equal(at(cells, "C3").value, 42);

  const uncached = (await cellsOf("types"))[11];
  assert.equal(uncached.type, "empty");
  assert.equal(uncached.value, null);
  assert.ok(uncached.formula);
});

test("every cell carries the index of its style, zero when it declares none", async () => {
  const cells = await cellsOf("formulas");

  assert.equal(at(cells, "A1").styleId, 0);
  assert.equal(at(cells, "G6").styleId, 2);
  assert.equal(at(cells, "B7").styleId, 1);
});

test("batch size does not change what comes out", async () => {
  const reference = await cellsOf("formulas");

  for (const batchSize of [1, 4, 10_000]) {
    assert.deepEqual(
      await cellsOf("formulas", { batchSize }),
      reference,
      `batchSize ${batchSize}`,
    );
  }
});

test("reads a sheet by name and lists the sheets off the same open", async () => {
  const stream = readCells(fixture("sheets-4"), {
    ...PERMISSIVE_CELLS,
    sheet: "Sheet3",
  });

  assert.deepEqual(
    (await stream.sheets()).map((s) => s.name),
    ["Sheet1", "Sheet2", "Sheet3", "Sheet4"],
  );

  let first;
  for await (const cell of stream) {
    first ??= cell;
  }
  assert.deepEqual([first.row, first.col], [0, 0]);
});
