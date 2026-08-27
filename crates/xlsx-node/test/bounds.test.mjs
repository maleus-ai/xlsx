import assert from "node:assert/strict";
import test from "node:test";

import { XlsxError, listSheets, xlsxRows } from "../lib/index.js";
import { PERMISSIVE, collect, fixture } from "./helpers.mjs";

const EIGHT_MIB = 8 * 1024 * 1024;

async function refusal(promise) {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  assert.fail("expected a refusal");
}

test("every hostile archive is refused at the part that blows up", async () => {
  // Each of these carries a single row of data. What blows up is the phase a
  // reader runs before it hands back its first row — which is the whole reason
  // the bounds live inside the reader rather than above it.
  const cases = [
    ["bomb-sharedstrings", "xl/sharedStrings.xml"],
    ["bomb-styles", "xl/styles.xml"],
    ["bomb-rels", "xl/_rels/workbook.xml.rels"],
    ["bomb-workbook", "xl/workbook.xml"],
    ["bomb-inline", "xl/worksheets/sheet1.xml"],
    ["lying-sizes", "xl/sharedStrings.xml"],
  ];

  for (const [name, part] of cases) {
    const error = await refusal(
      collect(
        xlsxRows(fixture(name), {
          maxDecompressedBytes: EIGHT_MIB,
          maxRows: 1_000_000,
        }),
      ),
    );

    assert.ok(error instanceof XlsxError, name);
    assert.equal(error.code, "DECOMPRESSED_BUDGET_EXCEEDED", name);
    assert.match(error.message, new RegExp(part.replaceAll(".", "\\.")), name);
  }
});

test("an archive of countless tiny entries is refused before it is opened", async () => {
  // A directory bomb rather than a byte bomb: entries that cost nothing to
  // inflate and a great deal to index, and the indexing happens before any byte
  // budget applies.
  const error = await refusal(
    collect(
      xlsxRows(fixture("bomb-entries"), {
        maxDecompressedBytes: EIGHT_MIB,
        maxRows: 1_000,
      }),
    ),
  );

  assert.equal(error.code, "TOO_MANY_ENTRIES");
});

test("listing sheets is bounded on what listing reads", async () => {
  // The listing path inflates two parts: the package relationships and the
  // workbook. Both are counted.
  for (const [name, code] of [
    ["bomb-workbook", "DECOMPRESSED_BUDGET_EXCEEDED"],
    ["bomb-package-rels", "DECOMPRESSED_BUDGET_EXCEEDED"],
    ["bomb-entries", "TOO_MANY_ENTRIES"],
    ["not-an-archive", "NOT_AN_ARCHIVE"],
  ]) {
    const error = await refusal(
      listSheets(fixture(name), { maxDecompressedBytes: EIGHT_MIB }),
    );
    assert.equal(error.code, code, name);
  }

  // A bomb in a part the listing never opens is not its problem: neither the
  // shared strings nor the workbook's own relationships are read to answer what
  // the tabs are called.
  for (const name of ["bomb-sharedstrings", "bomb-rels", "bomb-styles"]) {
    const sheets = await listSheets(fixture(name), {
      maxDecompressedBytes: EIGHT_MIB,
    });
    assert.deepEqual(sheets, [{ name: "Data", visible: true }], name);

    // Reading them, on the other hand, is refused exactly as before.
    const error = await refusal(
      collect(
        xlsxRows(fixture(name), {
          maxDecompressedBytes: EIGHT_MIB,
          maxRows: 1_000,
        }),
      ),
    );
    assert.equal(error.code, "DECOMPRESSED_BUDGET_EXCEEDED", name);
  }
});

test("listing sheets does not cost a walk of the archive", async () => {
  const file = fixture("large-600000");

  // Warm the page cache: what is being timed is inflating, not reading a disk.
  await listSheets(file, { maxDecompressedBytes: 1 << 30 });

  const started = performance.now();
  const sheets = await listSheets(file, { maxDecompressedBytes: 1 << 30 });
  const elapsed = performance.now() - started;

  assert.deepEqual(sheets, [{ name: "Data", visible: true }]);
  // Opening this workbook inflates 204 MB, which takes the better part of a
  // second. Listing reads two parts of a few kilobytes.
  assert.ok(elapsed < 50, `listing took ${elapsed.toFixed(1)} ms`);
});

test("the row budget cuts on the row that crosses it", async () => {
  // sheets-2 holds exactly 31 rows: one header plus thirty.
  const withinBudget = await collect(
    xlsxRows(fixture("sheets-2"), {
      maxDecompressedBytes: EIGHT_MIB,
      maxRows: 31,
    }),
  );
  assert.equal(withinBudget.length, 31);

  const error = await refusal(
    collect(
      xlsxRows(fixture("sheets-2"), {
        maxDecompressedBytes: EIGHT_MIB,
        maxRows: 30,
      }),
    ),
  );
  assert.equal(error.code, "ROW_BUDGET_EXCEEDED");
});

test("the row budget does not wait for the end of a large sheet", async () => {
  const stream = xlsxRows(fixture("large-200000"), {
    maxDecompressedBytes: 1 << 30,
    maxRows: 10,
    batchSize: 1_000,
  });

  const started = performance.now();
  const error = await refusal(collect(stream));
  const elapsed = performance.now() - started;

  assert.equal(error.code, "ROW_BUDGET_EXCEEDED");
  // Reading these 200 000 rows through takes over a second. The refusal lands
  // on the eleventh row; what is left in the timing is the budget walk.
  assert.ok(elapsed < 1_000, `refusal took ${elapsed.toFixed(0)} ms`);
});

test("a file that is not an archive is refused as such", async () => {
  const error = await refusal(
    collect(xlsxRows(fixture("not-an-archive"), PERMISSIVE)),
  );

  assert.equal(error.code, "NOT_AN_ARCHIVE");
});

test("an unknown sheet names the alternatives", async () => {
  const error = await refusal(
    collect(
      xlsxRows(fixture("sheets-2"), {
        ...PERMISSIVE,
        sheet: "Feuille absente",
      }),
    ),
  );

  assert.equal(error.code, "SHEET_NOT_FOUND");
  assert.match(error.message, /"Sheet1", "Sheet2"/);
});

test("a missing file is not reported as a bad upload", async () => {
  const error = await refusal(
    collect(xlsxRows("/tmp/il-n-y-a-rien-ici.xlsx", PERMISSIVE)),
  );

  assert.equal(error.code, "IO");
});

test("the budgets are mandatory", () => {
  const file = fixture("types");

  for (const options of [
    undefined,
    {},
    { maxRows: 10 },
    { maxDecompressedBytes: 10 },
    { maxDecompressedBytes: -1, maxRows: 10 },
    { maxDecompressedBytes: "beaucoup", maxRows: 10 },
    { maxDecompressedBytes: Number.NaN, maxRows: 10 },
  ]) {
    assert.throws(
      () => xlsxRows(file, options),
      (error) => error instanceof XlsxError && error.code === "INVALID_OPTION",
      JSON.stringify(options ?? null),
    );
  }

  // Explicit is fine: what is refused is the bound nobody set, not a large one.
  assert.doesNotThrow(() =>
    xlsxRows(file, {
      maxDecompressedBytes: Number.MAX_SAFE_INTEGER,
      maxRows: Number.MAX_SAFE_INTEGER,
    }).destroy(),
  );
});

test("a nonsensical batch size is refused", () => {
  for (const batchSize of [0, -1, 1.5, "mille"]) {
    assert.throws(
      () => xlsxRows(fixture("types"), { ...PERMISSIVE, batchSize }),
      (error) => error instanceof XlsxError && error.code === "INVALID_OPTION",
      String(batchSize),
    );
  }
});
