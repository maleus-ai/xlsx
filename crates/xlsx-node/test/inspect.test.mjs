import assert from "node:assert/strict";
import test from "node:test";

import { inspectSheet, inspectWorkbook } from "../lib/index.js";
import { PERMISSIVE, fixture } from "./helpers.mjs";

const EIGHT_MIB = 8 * 1024 * 1024;

test("restores core and app properties, dates as written", async () => {
  const { properties } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.equal(properties.creator, "Ada & Grace");
  assert.equal(properties.created, "2024-03-25T09:30:00Z");
  assert.equal(properties.application, "Microsoft Excel");
  assert.equal(properties.company, "Contoso");
  assert.equal(properties.subject ?? null, null);
});

test("restores a workbook name and a sheet-scoped name with its scope", async () => {
  const { definedNames } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.equal(definedNames[0].name, "Statuses");
  assert.equal(definedNames[0].formula, "Lists!$A$1:$A$3");
  assert.equal(definedNames[0].localSheetId ?? null, null);
  assert.equal(definedNames[1].localSheetId, 0);
  assert.equal(definedNames[1].hidden, true);
  assert.equal(definedNames[1].comment, "scoped");
});

test("restores a defined name that refers to #REF! without interpreting it", async () => {
  const { definedNames } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.equal(definedNames[2].formula, "#REF!$A$1");
});

test("restores the extensions the workbook declares, and only its own", async () => {
  const { extensions } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.deepEqual(extensions, [
    "GoogleSheetsCustomDataVersion2",
    "{140A7094-0E35-4892-8432-C4D2E57EDEB5}",
  ]);
});

test("reports a VBA project, pivot tables, external links and drawings", async () => {
  const inspection = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.equal(inspection.date1904, true);
  assert.deepEqual(inspection.parts, {
    vbaProject: true,
    pivotTables: true,
    externalLinks: true,
    drawings: true,
  });
  assert.deepEqual(inspection.sheets, [
    { name: "Data", sheetId: 1, state: "visible" },
    { name: "Lists", sheetId: 2, state: "hidden" },
  ]);
});

test("resolves built-in number formats and restores custom ones as declared", async () => {
  const { numFmts, styles } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.deepEqual(
    numFmts.map(({ numFmtId, formatCode }) => [numFmtId, formatCode ?? null]),
    [
      [0, "General"],
      [14, "mm-dd-yy"],
      [27, null],
      [164, '#,##0.00 "€"'],
    ],
  );
  // A reference is an index into the table, not the numFmtId it holds.
  assert.deepEqual(
    styles.map((style) => style.numFmtRef),
    [0, 3, 1, 2, 0],
  );
  assert.deepEqual(
    styles.map((style) => numFmts[style.numFmtRef].numFmtId),
    [0, 164, 14, 27, 0],
  );
});

test("refers to each font and fill by its index in its table", async () => {
  const { fonts, fills, styles } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );

  assert.equal(fonts.length, 2);
  assert.equal(fills.length, 3);
  assert.deepEqual(
    styles.map((style) => [style.fontRef ?? null, style.fillRef ?? null]),
    [
      [0, 0],
      [1, 1],
      [0, 2],
      [0, 0],
      // The last style refers to a font the table does not declare.
      [null, 0],
    ],
  );
});

test("restores a theme colour with its tint, without converting it to RGB", async () => {
  const { fonts } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );
  const font = fonts[1];

  assert.equal(font.bold, true);
  assert.equal(font.italic, false);
  assert.equal(font.underline, "double");
  assert.equal(font.size, 14);
  assert.equal(font.color.theme, 1);
  assert.equal(font.color.tint, -0.25);
  assert.equal(font.color.rgb ?? null, null);
});

test("restores a gradient fill with its stops", async () => {
  const { fills } = await inspectWorkbook(
    fixture("workbook-anatomy"),
    PERMISSIVE,
  );
  const gradient = fills[2].gradientFill;

  assert.equal(gradient.type, "linear");
  assert.equal(gradient.degree, 90);
  assert.deepEqual(
    gradient.stops.map((stop) => [
      stop.position,
      stop.color.rgb ?? stop.color.theme,
    ]),
    [
      [0, "FF0000FF"],
      [1, 4],
    ],
  );
  assert.equal(fills[1].patternFill.patternType, "solid");
  assert.equal(fills[1].patternFill.fgColor.rgb, "FFFFFF00");
});

test("restores a workbook without styles.xml with a default style", async () => {
  const inspection = await inspectWorkbook(
    fixture("bare-workbook"),
    PERMISSIVE,
  );

  assert.equal(inspection.styles.length, 1);
  assert.equal(inspection.styles[0].numFmtRef, 0);
  assert.equal(inspection.styles[0].fontRef ?? null, null);
  assert.equal(inspection.numFmts[0].formatCode, "General");
  assert.deepEqual(inspection.fonts, []);
  assert.deepEqual(inspection.definedNames, []);
});

test("holds a font shared by many styles once", async () => {
  const { fonts, styles } = await inspectWorkbook(
    fixture("bomb-styles-shared"),
    { maxDecompressedBytes: EIGHT_MIB },
  );

  assert.equal(fonts.length, 1);
  assert.equal(styles.length, 100_000);
  assert.ok(styles.every((style) => style.fontRef === 0));
});

test("refuses an inflated styles table under the byte budget", async () => {
  await assert.rejects(
    inspectWorkbook(fixture("bomb-styles"), {
      maxDecompressedBytes: EIGHT_MIB,
    }),
    { name: "XlsxError", code: "DECOMPRESSED_BUDGET_EXCEEDED" },
  );
});

const sheetAnatomy = () =>
  inspectSheet(fixture("sheet-anatomy"), { ...PERMISSIVE, sheet: "Data" });

test("restores merged ranges in A1 notation", async () => {
  assert.deepEqual((await sheetAnatomy()).mergeCells, ["A1:B1", "C2:C4"]);
});

test("merges standard and x14 validations in one list, marking the extension", async () => {
  const { dataValidations } = await sheetAnatomy();

  assert.equal(dataValidations.length, 2);
  assert.equal(dataValidations[0].type, "list");
  assert.equal(dataValidations[0].extension, false);
  assert.equal(dataValidations[0].formula1, '"Ouvert,Fermé"');
  assert.equal(dataValidations[1].extension, true);
  assert.deepEqual(dataValidations[1].sqref, ["E2:E6"]);
  assert.equal(dataValidations[1].formula1, "Lists!$A$1:$A$3");
});

test("splits a multiple sqref into a list of ranges", async () => {
  const { conditionalFormatting, dataValidations } = await sheetAnatomy();

  assert.deepEqual(conditionalFormatting[0].sqref, ["A1:A6", "C1:C6"]);
  assert.deepEqual(dataValidations[0].sqref, ["B2:B6", "D2"]);
});

test("restores conditional formatting rules with their priority and formulas", async () => {
  const { rules } = (await sheetAnatomy()).conditionalFormatting[0];

  assert.equal(rules.length, 2);
  assert.equal(rules[0].type, "cellIs");
  assert.equal(rules[0].priority, 2);
  assert.equal(rules[0].operator, "greaterThan");
  assert.deepEqual(rules[0].formulas, ["10"]);
  assert.equal(rules[1].timePeriod, "last7Days");
  assert.equal(rules[1].stopIfTrue, true);
});

test("merges standard and x14 conditional formats, restoring an extended rule once", async () => {
  const { conditionalFormatting } = await sheetAnatomy();

  assert.deepEqual(
    conditionalFormatting.map(({ sqref, extension, rules }) => ({
      sqref,
      extension,
      rules: rules.map((rule) => rule.type),
    })),
    [
      {
        sqref: ["A1:A6", "C1:C6"],
        extension: false,
        rules: ["cellIs", "timePeriod"],
      },
      { sqref: ["B2:B6"], extension: false, rules: ["dataBar"] },
      {
        sqref: ["F2:F6", "F8"],
        extension: true,
        rules: ["expression", "iconSet"],
      },
    ],
  );
  const [expression, iconSet] = conditionalFormatting[2].rules;
  assert.deepEqual(expression.formulas, ['Lists!$A$1="Ouvert"']);
  assert.equal(expression.dxfId ?? null, null);
  assert.deepEqual(iconSet.formulas, []);
});

test("restores a table with its columns and its totals row", async () => {
  const [table] = (await sheetAnatomy()).tables;

  assert.equal(table.name, "Suivi");
  assert.equal(table.displayName, "Suivi_2024");
  assert.equal(table.ref, "A1:C6");
  assert.equal(table.totalsRowCount, 1);
  assert.deepEqual(
    table.tableColumns.map((c) => c.name),
    ["Client", "Montant", "TTC"],
  );
  assert.equal(table.tableColumns[1].totalsRowFunction, "sum");
  assert.equal(
    table.tableColumns[2].calculatedColumnFormula,
    "Suivi_2024[[#This Row],[Montant]]*1.2",
  );
});

test("restores a comment with its author and its cell", async () => {
  const { commentAuthors, comments } = await sheetAnatomy();

  assert.deepEqual(commentAuthors, ["Ada", "Grace"]);
  assert.deepEqual(
    comments.map(({ ref, authorRef, text }) => ({
      ref,
      author: commentAuthors[authorRef] ?? null,
      text,
    })),
    [
      { ref: "B2", author: "Grace", text: "Grace: à vérifier" },
      { ref: "A6", author: "Ada", text: "Simple" },
      // Names an author the part does not list.
      { ref: "C3", author: null, text: "Orphan" },
    ],
  );
  assert.equal(comments[2].authorRef ?? null, null);
});

test("holds an author shared by many comments once", async () => {
  const { commentAuthors, comments } = await inspectSheet(
    fixture("bomb-comment-authors"),
    { sheet: "Data", maxDecompressedBytes: EIGHT_MIB },
  );

  assert.equal(commentAuthors.length, 1);
  assert.equal(comments.length, 20_000);
  assert.ok(comments.every((comment) => comment.authorRef === 0));
});

test("resolves the target of an external link through the sheet relationships", async () => {
  const { hyperlinks } = await sheetAnatomy();

  assert.equal(hyperlinks[0].ref, "A1");
  assert.equal(hyperlinks[0].target, "https://github.com/maleus-ai/xlsx");
  assert.equal(hyperlinks[0].tooltip, "Le site");
  assert.equal(hyperlinks[1].location, "'Data'!A1");
  assert.equal(hyperlinks[1].target ?? null, null);
});

test("restores frozen panes, hidden columns 0-based and hidden rows", async () => {
  const inspection = await sheetAnatomy();

  assert.equal(inspection.pane.state, "frozen");
  assert.equal(inspection.pane.ySplit, 2);
  assert.equal(inspection.cols[0].firstCol, 1);
  assert.equal(inspection.cols[0].hidden, true);
  assert.deepEqual(
    [inspection.cols[1].firstCol, inspection.cols[1].lastCol],
    [3, 5],
  );
  assert.deepEqual(inspection.hiddenRows, [2, 3]);
  assert.deepEqual(inspection.autoFilter, { ref: "A1:C6" });
  assert.equal(inspection.pictureCount, 2);
});

test("reports that a protection has a password without ever restoring its hash", async () => {
  const { sheetProtection } = await sheetAnatomy();

  assert.equal(sheetProtection.passwordProtected, true);
  assert.equal(sheetProtection.formatCells, false);
  assert.equal(sheetProtection.formatColumns, true);
  assert.ok(!JSON.stringify(sheetProtection).includes("CC1A"));
});

test("refuses a sheet relationship that leaves the archive or names a missing part", async () => {
  for (const name of ["sheet-missing-part", "sheet-escaping-relationship"]) {
    await assert.rejects(
      inspectSheet(fixture(name), { ...PERMISSIVE, sheet: "Data" }),
      { name: "XlsxError", code: "CORRUPT" },
      name,
    );
  }
});

test("refuses sheet metadata and linked parts that inflate past the byte budget", async () => {
  for (const name of [
    "bomb-comments",
    "bomb-merges",
    "bomb-validations",
    "bomb-hyperlink-targets",
    "bomb-sheet-records",
    "bomb-sheet-cells",
  ]) {
    await assert.rejects(
      inspectSheet(fixture(name), {
        sheet: "Data",
        maxDecompressedBytes: EIGHT_MIB,
      }),
      { name: "XlsxError", code: "DECOMPRESSED_BUDGET_EXCEEDED" },
      name,
    );
  }
});

test("the sheet to inspect is required", async () => {
  await assert.rejects(inspectSheet(fixture("types"), PERMISSIVE), {
    name: "XlsxError",
    code: "INVALID_OPTION",
  });
});

test("the byte budget is required", async () => {
  await assert.rejects(inspectWorkbook(fixture("types"), {}), {
    name: "XlsxError",
    code: "INVALID_OPTION",
  });
});
