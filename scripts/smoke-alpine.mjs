#!/usr/bin/env node
//
// The production check: does the musl binary load and read inside the image the
// generated applications actually deploy on?
//
//   docker run --rm -v "$PWD:/work" -w /work node:24-alpine node scripts/smoke-alpine.mjs

import assert from "node:assert/strict";
import fs from "node:fs";

import { listSheets, xlsxRows } from "../crates/xlsx-node/lib/index.js";

const PERMISSIVE = { maxDecompressedBytes: 1 << 30, maxRows: 1_000_000 };

function peakRssMb() {
  const match = /^VmHWM:\s+(\d+) kB$/m.exec(
    fs.readFileSync("/proc/self/status", "utf8"),
  );
  return match ? Math.round(Number(match[1]) / 1024) : null;
}

const sheets = await listSheets("fixtures/out/sheets-4.xlsx", PERMISSIVE);
assert.deepEqual(
  sheets.map((sheet) => sheet.name),
  ["Sheet1", "Sheet2", "Sheet3", "Sheet4"],
);

let rows = 0;
for await (const _row of xlsxRows(
  "fixtures/out/large-600000.xlsx",
  PERMISSIVE,
)) {
  rows += 1;
}
assert.equal(rows, 600_001);

let refused = null;
try {
  for await (const _row of xlsxRows("fixtures/out/bomb-sharedstrings.xlsx", {
    maxDecompressedBytes: 8 * 1024 * 1024,
    maxRows: 1_000,
  })) {
    // unreachable
  }
} catch (error) {
  refused = error.code;
}
assert.equal(refused, "DECOMPRESSED_BUDGET_EXCEEDED");

console.log(
  `node:24-alpine — ${sheets.length} sheets, ${rows} rows, ` +
    `peak RSS ${peakRssMb()} MB, hostile archive refused`,
);
