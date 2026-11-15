import assert from "node:assert/strict";
import test from "node:test";

import { parseRef, toA1 } from "../lib/index.js";

test("toA1 and parseRef are inverses on a cell and on a range", () => {
  assert.equal(toA1({ row: 2, col: 1 }), "B3");
  assert.deepEqual(parseRef("B3"), {
    start: { row: 2, col: 1 },
    end: { row: 2, col: 1 },
  });

  const range = parseRef("C2:XFD1048576");
  assert.deepEqual(range, {
    start: { row: 1, col: 2 },
    end: { row: 1_048_575, col: 16_383 },
  });
  assert.equal(`${toA1(range.start)}:${toA1(range.end)}`, "C2:XFD1048576");
});

test("parseRef ignores the markers of an absolute reference", () => {
  assert.deepEqual(parseRef("$A$1:B$2"), parseRef("A1:B2"));
});

test("parseRef refuses a string that is not an A1 range", () => {
  for (const ref of [
    "A:A",
    "1:1",
    "Sheet1!A1",
    "A1 B2",
    "A1:B2:C3",
    "",
    "A0",
    "XFE1",
    "A1048577",
  ]) {
    assert.throws(
      () => parseRef(ref),
      { name: "XlsxError", code: "INVALID_VALUE" },
      JSON.stringify(ref),
    );
  }
});

test("toA1 goes from Z to AA and from ZZ to AAA", () => {
  assert.equal(toA1({ row: 0, col: 25 }), "Z1");
  assert.equal(toA1({ row: 0, col: 26 }), "AA1");
  assert.equal(toA1({ row: 0, col: 701 }), "ZZ1");
  assert.equal(toA1({ row: 0, col: 702 }), "AAA1");
});

test("toA1 refuses a position outside a worksheet", () => {
  for (const position of [
    { row: -1, col: 0 },
    { row: 0, col: 16_384 },
    { row: 1_048_576, col: 0 },
    { row: 1.5, col: 0 },
  ]) {
    assert.throws(() => toA1(position), {
      name: "XlsxError",
      code: "INVALID_VALUE",
    });
  }
});

test("toA1 refuses two bare numbers, which would leave the order of row and column to guess", () => {
  for (const args of [[2, 1], [undefined], [null], []]) {
    assert.throws(() => toA1(...args), {
      name: "XlsxError",
      code: "INVALID_VALUE",
    });
  }
});
