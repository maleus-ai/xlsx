#!/usr/bin/env node
//
// Fixture generator. Nothing here is committed: fixtures are rebuilt on demand,
// which is also how they stay honest about what they claim to contain.
//
//   node fixtures/generate.mjs              # everything
//   node fixtures/generate.mjs large-600000 # one fixture
//
// The hostile archives are assembled byte by byte (see lib/zip.mjs) because the
// whole point is to produce what a real writer would refuse to produce.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { ZipWriter } from "./lib/zip.mjs";
import {
  CONTENT_TYPES,
  ROOT_RELS,
  STYLES,
  columnName,
  escapeXml,
  excelSerial,
  sharedStrings,
  workbook,
  workbookRels,
} from "./lib/xlsx.mjs";

const OUT_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), "out");

const SHEET_OPEN =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
  `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
  `<sheetData>`;
const SHEET_CLOSE = `</sheetData></worksheet>`;

// ---------------------------------------------------------------------------
// large-{200000,600000}: 10 columns, one of them dates, shared strings pooled.
// ---------------------------------------------------------------------------

const HEADERS = [
  "label",
  "signed_at",
  "quantity",
  "unit_price",
  "is_active",
  "region_id",
  "customer_id",
  "product_id",
  "warehouse_id",
  "batch_id",
];

const WORD_POOL_SIZE = 5_000;

function wordPool() {
  const words = [...HEADERS];
  for (let i = 0; i < WORD_POOL_SIZE; i += 1) {
    words.push(`libellé-${i.toString(36)}-${(i * 7919) % 100_000}`);
  }
  return words;
}

function* largeSheet(rowCount, poolSize) {
  yield SHEET_OPEN;

  let chunk = "";
  // Header row: the first ten shared strings.
  chunk += `<row r="1">`;
  for (let c = 0; c < HEADERS.length; c += 1) {
    chunk += `<c r="${columnName(c)}1" t="s"><v>${c}</v></c>`;
  }
  chunk += `</row>`;

  const epoch = excelSerial(new Date(Date.UTC(2024, 0, 1)));

  for (let r = 0; r < rowCount; r += 1) {
    const row = r + 2;
    const stringIndex = HEADERS.length + (r % poolSize);
    chunk += `<row r="${row}">`;
    chunk += `<c r="A${row}" t="s"><v>${stringIndex}</v></c>`;
    chunk += `<c r="B${row}" s="1"><v>${epoch + (r % 730)}</v></c>`;
    chunk += `<c r="C${row}"><v>${r % 1000}</v></c>`;
    chunk += `<c r="D${row}"><v>${((r % 9973) / 100).toFixed(2)}</v></c>`;
    chunk += `<c r="E${row}" t="b"><v>${r % 2}</v></c>`;
    for (let c = 5; c < 10; c += 1) {
      chunk += `<c r="${columnName(c)}${row}"><v>${(r * (c + 1)) % 65_536}</v></c>`;
    }
    chunk += `</row>`;

    if (r % 1_000 === 999) {
      yield chunk;
      chunk = "";
    }
  }

  yield chunk;
  yield SHEET_CLOSE;
}

async function buildLarge(rowCount) {
  const words = wordPool();
  const zip = new ZipWriter(path.join(OUT_DIR, `large-${rowCount}.xlsx`));
  await zip.add("[Content_Types].xml", CONTENT_TYPES(1));
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add("xl/_rels/workbook.xml.rels", workbookRels(1));
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/sharedStrings.xml", sharedStrings(words));
  await zip.add(
    "xl/worksheets/sheet1.xml",
    largeSheet(rowCount, WORD_POOL_SIZE),
  );
  zip.close();
}

// ---------------------------------------------------------------------------
// sheets-{2,4,8,16}: the multi-tab regression that killed exceljs past four.
// ---------------------------------------------------------------------------

function smallSheet(sheetIndex, rowCount = 30) {
  let xml = SHEET_OPEN;
  xml += `<row r="1"><c r="A1" t="inlineStr"><is><t>sheet</t></is></c><c r="B1" t="inlineStr"><is><t>row</t></is></c></row>`;
  for (let r = 0; r < rowCount; r += 1) {
    const row = r + 2;
    xml += `<row r="${row}">`;
    xml += `<c r="A${row}" t="inlineStr"><is><t>Sheet${sheetIndex + 1}</t></is></c>`;
    xml += `<c r="B${row}"><v>${r}</v></c>`;
    xml += `</row>`;
  }
  return xml + SHEET_CLOSE;
}

async function buildSheets(count) {
  const names = Array.from({ length: count }, (_, i) => `Sheet${i + 1}`);
  const zip = new ZipWriter(path.join(OUT_DIR, `sheets-${count}.xlsx`));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(count, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(names));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(count, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  for (let i = 0; i < count; i += 1) {
    await zip.add(`xl/worksheets/sheet${i + 1}.xml`, smallSheet(i));
  }
  zip.close();
}

// ---------------------------------------------------------------------------
// Typing, sparseness, hidden sheets, 1904 date system.
// ---------------------------------------------------------------------------

async function buildTypes() {
  const zip = new ZipWriter(path.join(OUT_DIR, "types.xlsx"));
  const sheet =
    SHEET_OPEN +
    `<row r="1">` +
    `<c r="A1" t="s"><v>0</v></c>` +
    `<c r="B1" t="inlineStr"><is><t>inline &amp; escaped</t></is></c>` +
    `<c r="C1"><v>42</v></c>` +
    `<c r="D1"><v>3.5</v></c>` +
    `<c r="E1" t="b"><v>1</v></c>` +
    `<c r="F1" t="b"><v>0</v></c>` +
    `<c r="G1" t="e"><v>#DIV/0!</v></c>` +
    `<c r="H1" s="1"><v>45376</v></c>` +
    `<c r="I1" s="2"><v>45376.5</v></c>` +
    `<c r="J1"><f>C1*2</f><v>84</v></c>` +
    `<c r="K1" s="3"><v>1.25</v></c>` +
    // A formula whose writing application cached no result.
    `<c r="L1"><f>C1*2</f></c>` +
    `</row>` +
    SHEET_CLOSE;
  await zip.add("[Content_Types].xml", CONTENT_TYPES(1));
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add("xl/_rels/workbook.xml.rels", workbookRels(1));
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/sharedStrings.xml", sharedStrings(["shared text"]));
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

async function buildSparse({ declareDimension = true } = {}) {
  const name = declareDimension ? "sparse" : "sparse-nodim";
  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  const dimension = declareDimension ? `<dimension ref="A1:D5"/>` : "";
  const sheet =
    SHEET_OPEN.replace("<sheetData>", `${dimension}<sheetData>`) +
    // Full width, establishes the sheet's four columns.
    `<row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>2</v></c><c r="C1"><v>3</v></c><c r="D1"><v>4</v></c></row>` +
    // Hole in the middle: B is absent.
    `<row r="2"><c r="A2"><v>5</v></c><c r="C2"><v>7</v></c><c r="D2"><v>8</v></c></row>` +
    // Hole at the end: C and D are absent.
    `<row r="3"><c r="A3"><v>9</v></c><c r="B3"><v>10</v></c></row>` +
    // Whole row 4 is absent; row 5 starts at C.
    `<row r="5"><c r="C5"><v>11</v></c></row>` +
    SHEET_CLOSE;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

/// Row 1 carries only A1, row 2 carries A to D. Pins what happens to a row that
/// is closed before the sheet has shown how wide it gets.
async function buildNarrowFirstRow() {
  const zip = new ZipWriter(path.join(OUT_DIR, "narrow-first-row.xlsx"));
  const sheet =
    SHEET_OPEN +
    `<row r="1"><c r="A1"><v>1</v></c></row>` +
    `<row r="2"><c r="A2"><v>2</v></c><c r="B2"><v>3</v></c><c r="C2"><v>4</v></c><c r="D2"><v>5</v></c></row>` +
    `<row r="3"><c r="A3"><v>6</v></c></row>` +
    SHEET_CLOSE;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

async function buildDates1904() {
  const zip = new ZipWriter(path.join(OUT_DIR, "dates-1904.xlsx"));
  // 43914 in the 1904 system is the same wall-clock day as 45376 in the 1900 one.
  const sheet =
    SHEET_OPEN +
    `<row r="1"><c r="A1" s="1"><v>43914</v></c><c r="B1" s="2"><v>43914.5</v></c></row>` +
    SHEET_CLOSE;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"], { date1904: true }));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

async function buildHiddenSheets() {
  const zip = new ZipWriter(path.join(OUT_DIR, "hidden-sheets.xlsx"));
  const wb =
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
    `<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
    `xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">` +
    `<sheets>` +
    `<sheet name="Data" sheetId="1" r:id="rId1"/>` +
    `<sheet name="Paramètres" sheetId="2" state="hidden" r:id="rId2"/>` +
    `<sheet name="Notes" sheetId="3" state="veryHidden" r:id="rId3"/>` +
    `</sheets></workbook>`;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(3, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", wb);
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(3, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  for (let i = 0; i < 3; i += 1) {
    await zip.add(`xl/worksheets/sheet${i + 1}.xml`, smallSheet(i, 3));
  }
  zip.close();
}

/// A workbook whose sheets are declared with explicit attributes, and nothing
/// else in it: `sheets` is the list of `<sheet>` attribute strings, in order.
async function buildDeclaredSheets(name, sheets) {
  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  const wb =
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
    `<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
    `xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">` +
    `<sheets>` +
    sheets
      .map((attributes, i) => `<sheet ${attributes} r:id="rId${i + 1}"/>`)
      .join("") +
    `</sheets></workbook>`;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(sheets.length, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", wb);
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(sheets.length, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  for (let i = 0; i < sheets.length; i += 1) {
    await zip.add(`xl/worksheets/sheet${i + 1}.xml`, smallSheet(i, 3));
  }
  zip.close();
}

/// Sheet identifiers that do not follow the tab order, as after a reorder.
const buildSheetIds = () =>
  buildDeclaredSheets("sheet-ids", [
    `name="Synthèse" sheetId="7"`,
    `name="Données" sheetId="3"`,
  ]);

/// A sheet state the format does not define.
const buildUnknownSheetState = () =>
  buildDeclaredSheets("unknown-sheet-state", [
    `name="Data" sheetId="1"`,
    `name="Secret" sheetId="2" state="secret"`,
  ]);

/// Entry names as some generators write them: `\` as the separator, and a case
/// that differs from the targets the relationships declare.
async function buildBackslashEntries() {
  const zip = new ZipWriter(path.join(OUT_DIR, "backslash-entries.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(2, { sharedStrings: false }),
  );
  await zip.add("_rels\\.rels", ROOT_RELS);
  await zip.add("XL\\Workbook.xml", workbook(["Data", "Notes"]));
  await zip.add(
    "xl\\_rels\\workbook.xml.rels",
    workbookRels(2, { sharedStrings: false }),
  );
  await zip.add("xl\\styles.xml", STYLES);
  await zip.add("xl\\worksheets\\sheet1.xml", smallSheet(0, 3));
  await zip.add("xl\\worksheets\\sheet2.xml", smallSheet(1, 3));
  zip.close();
}

/// A sheet whose first cell is C3: two empty rows and two empty columns before
/// any data, then a gap of one row.
async function buildStartsLower() {
  const zip = new ZipWriter(path.join(OUT_DIR, "starts-lower.xlsx"));
  const sheet =
    SHEET_OPEN +
    `<row r="3"><c r="C3"><v>1</v></c><c r="D3"><v>2</v></c></row>` +
    `<row r="5"><c r="D5"><v>3</v></c></row>` +
    SHEET_CLOSE;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

/// Formulas of every kind, cells that only carry a style, cells without `r`.
async function buildFormulas() {
  const zip = new ZipWriter(path.join(OUT_DIR, "formulas.xlsx"));
  const sheet =
    SHEET_OPEN +
    // A shared formula: written once on C1, its range declared there, and
    // reused by C2 and C3 with nothing but the shared index.
    `<row r="1"><c r="A1"><v>2</v></c><c r="B1"><v>3</v></c>` +
    `<c r="C1"><f t="shared" ref="C1:C3" si="0">A1*B1</f><v>6</v></c></row>` +
    `<row r="2"><c r="A2"><v>4</v></c><c r="B2"><v>5</v></c>` +
    `<c r="C2"><f t="shared" si="0"/><v>20</v></c></row>` +
    `<row r="3"><c r="A3"><v>6</v></c><c r="B3"><v>7</v></c>` +
    `<c r="C3"><f t="shared" si="0"/><v>42</v></c></row>` +
    // An array formula: declared on its master cell only.
    `<row r="4"><c r="D4"><f t="array" ref="D4:D5">A1:A2*2</f><v>4</v></c></row>` +
    `<row r="5"><c r="D5"><v>8</v></c></row>` +
    // A styled cell with no value, self-closing; a bare declared cell; a date.
    `<row r="6"><c r="E6" s="1"/><c r="F6"/><c r="G6" s="2"><v>45376.5</v></c></row>` +
    // Cells that do not say where they sit.
    `<row r="7"><c><v>1</v></c><c s="1"/><c><v>3</v></c></row>` +
    SHEET_CLOSE;
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

/// Everything a workbook declares about itself: properties, defined names, the
/// 1904 date system, a VBA project, pivot and drawing parts, an external link,
/// and a styles table with custom formats, fonts, pattern and gradient fills.
async function buildWorkbookAnatomy() {
  const zip = new ZipWriter(path.join(OUT_DIR, "workbook-anatomy.xlsx"));
  const R =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(2, { sharedStrings: false }),
  );
  await zip.add(
    "_rels/.rels",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
      `<Relationship Id="rId1" Type="${R}/officeDocument" Target="xl/workbook.xml"/>` +
      `<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>` +
      `<Relationship Id="rId3" Type="${R}/extended-properties" Target="docProps/app.xml"/>` +
      `</Relationships>`,
  );
  await zip.add(
    "docProps/core.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" ` +
      `xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" ` +
      `xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">` +
      `<dc:creator>Ada &amp; Grace</dc:creator><cp:lastModifiedBy>Alan</cp:lastModifiedBy>` +
      `<dcterms:created xsi:type="dcterms:W3CDTF">2024-03-25T09:30:00Z</dcterms:created>` +
      `<dcterms:modified xsi:type="dcterms:W3CDTF">2024-04-01T17:00:00Z</dcterms:modified>` +
      `<dc:title>Planning</dc:title><cp:keywords>budget, 2024</cp:keywords>` +
      `</cp:coreProperties>`,
  );
  await zip.add(
    "docProps/app.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">` +
      `<Application>Microsoft Excel</Application><AppVersion>16.0300</AppVersion><Company>Contoso</Company>` +
      `</Properties>`,
  );
  await zip.add(
    "xl/workbook.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="${R}">` +
      `<workbookPr date1904="1"/>` +
      // An extension of a view, not of the workbook.
      `<bookViews><workbookView><extLst><ext uri="{nested-view-extension}"/></extLst></workbookView></bookViews>` +
      `<sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Lists" sheetId="2" state="hidden" r:id="rId2"/></sheets>` +
      `<definedNames>` +
      `<definedName name="Statuses">Lists!$A$1:$A$3</definedName>` +
      `<definedName name="Local" localSheetId="0" hidden="1" comment="scoped">Data!$B$2</definedName>` +
      `<definedName name="Broken">#REF!$A$1</definedName>` +
      `</definedNames>` +
      `<extLst>` +
      `<ext uri="GoogleSheetsCustomDataVersion2"><go:sheetsCustomData xmlns:go="http://customooxmlschemas.google.com/" r:id="rId9"/></ext>` +
      `<ext uri="{140A7094-0E35-4892-8432-C4D2E57EDEB5}"><x15:workbookPr xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main" chartTrackingRefBase="1"/></ext>` +
      `</extLst>` +
      `</workbook>`,
  );
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
      `<Relationship Id="rId1" Type="${R}/worksheet" Target="worksheets/sheet1.xml"/>` +
      `<Relationship Id="rId2" Type="${R}/worksheet" Target="/xl/worksheets/sheet2.xml"/>` +
      `<Relationship Id="rId3" Type="${R}/styles" Target="styles.xml"/>` +
      `<Relationship Id="rId4" Type="http://schemas.microsoft.com/office/2006/relationships/vbaProject" Target="vbaProject.bin"/>` +
      `<Relationship Id="rId5" Type="${R}/externalLink" Target="externalLinks/externalLink1.xml"/>` +
      `</Relationships>`,
  );
  await zip.add(
    "xl/styles.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
      `<numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0.00 &quot;€&quot;"/></numFmts>` +
      `<fonts count="2">` +
      `<font><sz val="11"/><name val="Calibri"/></font>` +
      `<font><b/><i val="0"/><u val="double"/><vertAlign val="superscript"/><sz val="14"/>` +
      `<color theme="1" tint="-0.25"/><name val="Arial"/></font>` +
      `</fonts>` +
      `<fills count="3">` +
      `<fill><patternFill patternType="none"/></fill>` +
      `<fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/><bgColor indexed="64"/></patternFill></fill>` +
      `<fill><gradientFill degree="90"><stop position="0"><color rgb="FF0000FF"/></stop>` +
      `<stop position="1"><color theme="4"/></stop></gradientFill></fill>` +
      `</fills>` +
      `<borders count="1"><border/></borders>` +
      `<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>` +
      `<cellXfs count="5">` +
      `<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>` +
      `<xf numFmtId="164" fontId="1" fillId="1" borderId="0" xfId="0" applyNumberFormat="1">` +
      `<alignment horizontal="center" vertical="top" wrapText="1" indent="2" textRotation="45"/></xf>` +
      `<xf numFmtId="14" fontId="0" fillId="2" borderId="0" xfId="0"/>` +
      `<xf numFmtId="27" fontId="0" fillId="0" borderId="0" xfId="0"><alignment horizontal="sideways"/></xf>` +
      // Refers to a third font, which the table does not have: it resolves to
      // the default font, unless the dxf's bold font leaked into the table.
      `<xf numFmtId="0" fontId="2" fillId="0" borderId="0" xfId="0"/>` +
      `</cellXfs>` +
      // A differential format's font must not land in the font table.
      `<dxfs count="1"><dxf><font><b/></font></dxf></dxfs>` +
      `</styleSheet>`,
  );
  await zip.add("xl/vbaProject.bin", "not really a VBA project");
  await zip.add(
    "xl/externalLinks/externalLink1.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>`,
  );
  await zip.add(
    "xl/pivotCache/pivotCacheDefinition1.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>`,
  );
  await zip.add(
    "xl/drawings/drawing1.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"/>`,
  );
  await zip.add("xl/worksheets/sheet1.xml", smallSheet(0, 3));
  await zip.add("xl/worksheets/sheet2.xml", smallSheet(1, 3));
  zip.close();
}

const R_NS =
  "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A one-sheet workbook whose sheet part and sheet relationships are given,
/// plus any other parts, for the sheet inspection.
async function buildInspectedSheet(name, { sheet, rels, parts = {} }) {
  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  if (rels !== undefined) {
    await zip.add(
      "xl/worksheets/_rels/sheet1.xml.rels",
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${rels}</Relationships>`,
    );
  }
  for (const [part, content] of Object.entries(parts)) {
    await zip.add(part, content);
  }
  zip.close();
}

const WORKSHEET_OPEN =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
  `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
  `xmlns:r="${R_NS}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" ` +
  `xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">`;

/// Everything a sheet declares besides its cells, in the order of the schema.
const buildSheetAnatomy = () =>
  buildInspectedSheet("sheet-anatomy", {
    sheet:
      WORKSHEET_OPEN +
      `<sheetPr><tabColor rgb="FFFF0000"/></sheetPr>` +
      `<dimension ref="A1:F6"/>` +
      `<sheetViews><sheetView workbookViewId="0">` +
      `<pane xSplit="1" ySplit="2" topLeftCell="B3" activePane="bottomRight" state="frozen"/>` +
      `</sheetView></sheetViews>` +
      `<cols><col min="2" max="2" width="0" hidden="1"/><col min="4" max="6" width="18.5" outlineLevel="1"/></cols>` +
      `<sheetData>` +
      `<row r="1"><c r="A1"><v>1</v></c></row>` +
      `<row r="3" hidden="1"><c r="A3"><v>3</v></c></row>` +
      `<row hidden="1"><c r="A4"><v>4</v></c></row>` +
      `<row r="6"><c r="A6"><v>6</v></c></row>` +
      `</sheetData>` +
      `<sheetProtection password="CC1A" sheet="1" formatCells="0" sort="0"/>` +
      `<autoFilter ref="A1:C6"/>` +
      `<mergeCells count="2"><mergeCell ref="A1:B1"/><mergeCell ref="C2:C4"/></mergeCells>` +
      `<conditionalFormatting sqref="A1:A6 C1:C6">` +
      `<cfRule type="cellIs" dxfId="0" priority="2" operator="greaterThan"><formula>10</formula></cfRule>` +
      `<cfRule type="timePeriod" dxfId="1" priority="1" timePeriod="last7Days" stopIfTrue="1"><formula>FLOOR(A1,1)=TODAY()-1</formula></cfRule>` +
      `<cfRule type="sparkles" priority="3"/>` +
      `</conditionalFormatting>` +
      `<conditionalFormatting sqref="B2:B6">` +
      `<cfRule type="dataBar" priority="4"><dataBar><cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar>` +
      `<extLst><ext uri="{B025F937-C7B1-47D3-B67F-A62EFF666E3E}"><x14:id>{00000000-0000-0000-0000-000000000001}</x14:id></ext></extLst>` +
      `</cfRule></conditionalFormatting>` +
      `<dataValidations count="1">` +
      `<dataValidation type="list" allowBlank="1" showInputMessage="1" showErrorMessage="1" ` +
      `errorStyle="warning" errorTitle="Statut" error="Choisir dans la liste" sqref="B2:B6 D2">` +
      `<formula1>"Ouvert,Fermé"</formula1></dataValidation>` +
      `</dataValidations>` +
      `<hyperlinks>` +
      `<hyperlink ref="A1" r:id="rId1" tooltip="Le site" display="maleus-ai/xlsx"/>` +
      `<hyperlink ref="A6" location="'Data'!A1"/>` +
      `</hyperlinks>` +
      `<drawing r:id="rId2"/>` +
      `<tableParts count="1"><tablePart r:id="rId3"/></tableParts>` +
      `<extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}"><x14:conditionalFormattings>` +
      // Extends the standard data bar above: not restored twice.
      `<x14:conditionalFormatting><x14:cfRule type="dataBar" id="{00000000-0000-0000-0000-000000000001}">` +
      `<x14:dataBar minLength="0" maxLength="100"><x14:cfvo type="autoMin"/><x14:cfvo type="autoMax"/></x14:dataBar>` +
      `</x14:cfRule><xm:sqref>B2:B6</xm:sqref></x14:conditionalFormatting>` +
      `<x14:conditionalFormatting>` +
      `<x14:cfRule type="expression" priority="5" id="{00000000-0000-0000-0000-000000000002}">` +
      `<xm:f>Lists!$A$1="Ouvert"</xm:f><x14:dxf><font><b/></font></x14:dxf></x14:cfRule>` +
      `<x14:cfRule type="iconSet" priority="6" id="{00000000-0000-0000-0000-000000000003}">` +
      `<x14:iconSet iconSet="3Triangles" custom="1">` +
      `<x14:cfvo type="percent"><xm:f>0</xm:f></x14:cfvo><x14:cfvo type="percent"><xm:f>33</xm:f></x14:cfvo>` +
      `<x14:cfvo type="percent"><xm:f>67</xm:f></x14:cfvo><x14:cfIcon iconSet="3Arrows" iconId="0"/>` +
      `</x14:iconSet></x14:cfRule>` +
      `<xm:sqref>F2:F6 F8</xm:sqref></x14:conditionalFormatting>` +
      `</x14:conditionalFormattings></ext>` +
      `<ext uri="{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}">` +
      `<x14:dataValidations count="1"><x14:dataValidation type="list" allowBlank="1">` +
      `<x14:formula1><xm:f>Lists!$A$1:$A$3</xm:f></x14:formula1><xm:sqref>E2:E6</xm:sqref>` +
      `</x14:dataValidation></x14:dataValidations>` +
      `</ext></extLst>` +
      `</worksheet>`,
    rels:
      `<Relationship Id="rId1" Type="${R_NS}/hyperlink" Target="https://github.com/maleus-ai/xlsx" TargetMode="External"/>` +
      `<Relationship Id="rId2" Type="${R_NS}/drawing" Target="../drawings/drawing1.xml"/>` +
      `<Relationship Id="rId3" Type="${R_NS}/table" Target="../tables/table1.xml"/>` +
      `<Relationship Id="rId4" Type="${R_NS}/comments" Target="../comments1.xml"/>`,
    parts: {
      "xl/tables/table1.xml":
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Suivi" ` +
        `displayName="Suivi_2024" ref="A1:C6" totalsRowCount="1">` +
        `<autoFilter ref="A1:C5"/>` +
        `<tableColumns count="3">` +
        `<tableColumn id="1" name="Client" totalsRowLabel="Total"/>` +
        `<tableColumn id="2" name="Montant" totalsRowFunction="sum"/>` +
        `<tableColumn id="3" name="TTC"><calculatedColumnFormula>Suivi_2024[[#This Row],[Montant]]*1.2</calculatedColumnFormula></tableColumn>` +
        `</tableColumns></table>`,
      "xl/drawings/drawing1.xml":
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing">` +
        `<xdr:twoCellAnchor><xdr:pic/></xdr:twoCellAnchor>` +
        `<xdr:twoCellAnchor><xdr:sp/></xdr:twoCellAnchor>` +
        `<xdr:oneCellAnchor><xdr:pic/></xdr:oneCellAnchor>` +
        `</xdr:wsDr>`,
      "xl/comments1.xml":
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
        `<authors><author>Ada</author><author>Grace</author></authors>` +
        `<commentList>` +
        `<comment ref="B2" authorId="1"><text><r><rPr><b/></rPr><t>Grace:</t></r><r><t xml:space="preserve"> à vérifier</t></r></text></comment>` +
        `<comment ref="A6" authorId="0"><text><t>Simple</t><rPh sb="0" eb="1"><t>ignored</t></rPh></text></comment>` +
        `<comment ref="C3" authorId="7"><text><t>Orphan</t></text></comment>` +
        `</commentList></comments>`,
    },
  });

/// A table part the sheet's relationships name and the archive does not hold.
const buildSheetMissingPart = () =>
  buildInspectedSheet("sheet-missing-part", {
    sheet:
      WORKSHEET_OPEN +
      `<sheetData/><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>`,
    rels: `<Relationship Id="rId1" Type="${R_NS}/table" Target="../tables/table9.xml"/>`,
  });

/// A relationship that climbs out of the package.
const buildSheetEscapingRelationship = () =>
  buildInspectedSheet("sheet-escaping-relationship", {
    sheet: WORKSHEET_OPEN + `<sheetData/><drawing r:id="rId1"/></worksheet>`,
    rels: `<Relationship Id="rId1" Type="${R_NS}/drawing" Target="../../../../etc/passwd"/>`,
  });

/// A comments part that inflates far past a small budget, behind one cell.
const buildBombComments = () =>
  buildInspectedSheet("bomb-comments", {
    sheet:
      WORKSHEET_OPEN +
      `<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>`,
    rels: `<Relationship Id="rId1" Type="${R_NS}/comments" Target="../comments1.xml"/>`,
    parts: {
      "xl/comments1.xml":
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
        `<authors><author>a</author></authors><commentList>` +
        `<comment ref="A1" authorId="0"><text><t>${"x".repeat(32 * 1024 * 1024)}</t></text></comment>` +
        `</commentList></comments>`,
    },
  });

/// Amplification through the comments, within the byte budget: one author
/// named with 4 MiB, and 20 000 empty comments of a few bytes, all of them
/// referring to it. A reader that copied the name into every comment would
/// hand out 80 GiB of author names.
const buildBombCommentAuthors = () =>
  buildInspectedSheet("bomb-comment-authors", {
    sheet:
      WORKSHEET_OPEN +
      `<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData></worksheet>`,
    rels: `<Relationship Id="rId1" Type="${R_NS}/comments" Target="../comments1.xml"/>`,
    parts: {
      "xl/comments1.xml": repeat(
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
          `<comments xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
          `<authors><author>${"A".repeat(4 * 1024 * 1024)}</author></authors><commentList>`,
        () => `<comment ref="A1" authorId="0"/>`,
        20_000,
        `</commentList></comments>`,
      ),
    },
  });

/// Amplification through the hyperlinks: one relationship whose target is
/// 4 MiB, and 1 000 hyperlinks of a few bytes naming it. Each link receives
/// its own copy of the target, 4 GiB in all.
const buildBombHyperlinkTargets = () =>
  buildInspectedSheet("bomb-hyperlink-targets", {
    sheet:
      WORKSHEET_OPEN +
      `<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>` +
      `<hyperlinks>${'<hyperlink ref="A1" r:id="rId1"/>'.repeat(1_000)}</hyperlinks>` +
      `</worksheet>`,
    rels: `<Relationship Id="rId1" Type="${R_NS}/hyperlink" Target="https://${"a".repeat(4 * 1024 * 1024)}" TargetMode="External"/>`,
  });

/// Amplification through the records a sheet part builds, within the byte
/// budget: 100 000 empty validations of 17 bytes, each a record of a few
/// hundred, then one validation over 2.5 million ranges of one letter, each
/// a string several times the two bytes it is read from. About 7 MB of XML
/// build records past 80 MB.
const buildBombSheetRecords = () =>
  buildInspectedSheet("bomb-sheet-records", {
    sheet: repeat(
      WORKSHEET_OPEN +
        `<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>` +
        `<dataValidations>`,
      () => `<dataValidation/>`,
      100_000,
      `<dataValidation sqref="${"A ".repeat(2_500_000)}"/></dataValidations></worksheet>`,
    ),
  });

/// A sheet part inflating past a small budget through ordinary cells alone,
/// which the inspection skips: no record is built, only the stream is charged.
const buildBombSheetCells = () =>
  buildInspectedSheet("bomb-sheet-cells", {
    sheet: repeat(
      WORKSHEET_OPEN + `<sheetData>`,
      (i) => `<row r="${i + 1}"><c r="A${i + 1}"><v>1</v></c></row>`,
      300_000,
      `</sheetData></worksheet>`,
    ),
  });

/// Sheet metadata alone — merged ranges, then validations — inflating past a
/// small budget, on a sheet of one cell.
const buildBombSheetMetadata = (name, element) => () =>
  buildInspectedSheet(name, {
    sheet:
      WORKSHEET_OPEN +
      `<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>` +
      element +
      `</worksheet>`,
  });

/// A workbook with no styles part, no properties, no defined names.
async function buildBareWorkbook() {
  const zip = new ZipWriter(path.join(OUT_DIR, "bare-workbook.xlsx"));
  const R =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
      `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
      `<Relationship Id="rId1" Type="${R}/worksheet" Target="worksheets/sheet1.xml"/>` +
      `</Relationships>`,
  );
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

// ---------------------------------------------------------------------------
// Hostile archives. Each carries a single row of data: what blows up is the
// phase the reader runs *before* handing back its first row.
// ---------------------------------------------------------------------------

const ONE_ROW_SHEET =
  SHEET_OPEN + `<row r="1"><c r="A1"><v>1</v></c></row>` + SHEET_CLOSE;

function* repeat(prefix, unit, count, suffix, batch = 20_000) {
  yield prefix;
  let chunk = "";
  for (let i = 0; i < count; i += 1) {
    chunk += unit(i);
    if (i % batch === batch - 1) {
      yield chunk;
      chunk = "";
    }
  }
  yield chunk;
  yield suffix;
}

async function buildBombSharedStrings() {
  const count = 3_000_000;
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-sharedstrings.xlsx"));
  await zip.add("[Content_Types].xml", CONTENT_TYPES(1));
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add("xl/_rels/workbook.xml.rels", workbookRels(1));
  await zip.add("xl/styles.xml", STYLES);
  await zip.add(
    "xl/sharedStrings.xml",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="${count}" uniqueCount="${count}">`,
      (i) => `<si><t>chaîne partagée numéro ${i}</t></si>`,
      count,
      `</sst>`,
    ),
  );
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

async function buildBombStyles() {
  const count = 3_000_000;
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-styles.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add(
    "xl/styles.xml",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
        `<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>` +
        `<fills count="1"><fill><patternFill patternType="none"/></fill></fills>` +
        `<borders count="1"><border/></borders>` +
        `<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>` +
        `<cellXfs count="${count}">`,
      () =>
        `<xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>`,
      count,
      `</cellXfs></styleSheet>`,
    ),
  );
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

/// Amplification through the styles, within the byte budget: one font named
/// with 4 MiB, and 100 000 cell formats of a few bytes, all of them referring
/// to it. A reader that copied the font into every style would hand out
/// 400 GiB of font names.
async function buildBombStylesShared() {
  const count = 100_000;
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-styles-shared.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add(
    "xl/styles.xml",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
        `<fonts count="1"><font><name val="${"F".repeat(4 * 1024 * 1024)}"/></font></fonts>` +
        `<cellXfs count="${count}">`,
      () => `<xf/>`,
      count,
      `</cellXfs></styleSheet>`,
    ),
  );
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

async function buildBombRels() {
  const count = 2_000_000;
  const base =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-rels.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
        `<Relationship Id="rId1" Type="${base}/worksheet" Target="worksheets/sheet1.xml"/>` +
        `<Relationship Id="rId2" Type="${base}/styles" Target="styles.xml"/>`,
      (i) =>
        `<Relationship Id="rIdPad${i}" Type="${base}/hyperlink" Target="https://example.invalid/${i}" TargetMode="External"/>`,
      count,
      `</Relationships>`,
    ),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

/// The other relationships file. `bomb-rels` bombs the workbook's, which only a
/// full read opens; this one bombs the package's, which is the first part any
/// reader touches — a listing included.
async function buildBombPackageRels() {
  const count = 2_000_000;
  const base =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-package-rels.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add(
    "_rels/.rels",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
        `<Relationship Id="rId1" Type="${base}/officeDocument" Target="xl/workbook.xml"/>`,
      (i) =>
        `<Relationship Id="rIdPad${i}" Type="${base}/thumbnail" Target="docProps/thumbnail-${i}.jpeg"/>`,
      count,
      `</Relationships>`,
    ),
  );
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

async function buildBombWorkbook() {
  const count = 2_000_000;
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-workbook.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add(
    "xl/workbook.xml",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
        `xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>` +
        `<sheet name="Data" sheetId="1" r:id="rId1"/>`,
      (i) =>
        `<sheet name="Onglet de remplissage ${i}" sheetId="${i + 2}" r:id="rId1"/>`,
      count,
      `</sheets></workbook>`,
    ),
  );
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

async function buildBombInline() {
  const target = 300 * 1024 * 1024;
  const filler = "A".repeat(64 * 1024);
  function* sheet() {
    yield SHEET_OPEN + `<row r="1"><c r="A1" t="inlineStr"><is><t>`;
    for (let written = 0; written < target; written += filler.length) {
      yield filler;
    }
    yield `</t></is></c></row>` + SHEET_CLOSE;
  }
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-inline.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet());
  zip.close();
}

/// The central directory (and the local header) claim 64 bytes for an entry
/// that deflates to 64 MB. Only a reader counting what actually leaves the
/// inflater notices.
async function buildLyingSizes() {
  const target = 64 * 1024 * 1024;
  const zip = new ZipWriter(path.join(OUT_DIR, "lying-sizes.xlsx"));
  await zip.add("[Content_Types].xml", CONTENT_TYPES(1));
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add("xl/_rels/workbook.xml.rels", workbookRels(1));
  await zip.add("xl/styles.xml", STYLES);
  const unit = `<si><t>x</t></si>`;
  const count = Math.floor(target / unit.length);
  await zip.addLying(
    "xl/sharedStrings.xml",
    repeat(
      `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="${count}" uniqueCount="${count}">`,
      () => unit,
      count,
      `</sst>`,
    ),
    64,
  );
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  zip.close();
}

/// Not a byte bomb but a directory bomb: 65 000 entries of one byte each. They
/// cost nothing to inflate and a great deal to index, and the indexing happens
/// before any byte budget applies. 65 535 is as far as a plain ZIP can go — the
/// count is a `u16` — which is why this one stops there.
async function buildBombEntries() {
  const count = 65_000;
  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-entries.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", ONE_ROW_SHEET);
  for (let i = 0; i < count; i += 1) {
    // Long names: what an entry costs to index is its record plus its name.
    await zip.add(
      `pad/${i}-nom-tres-long-pour-gonfler-le-repertoire-central-de-l-archive.xml`,
      "x",
      { store: true },
    );
  }
  zip.close();
}

/// Amplification through a cell reference rather than through bytes. `calamine`
/// builds a column index by accumulating `col * 26 + letter` with no ceiling, so
/// `r="BZZZZZ1"` names column 36 119 382 — and a row vector that wide is 1.1 GB
/// for two kilobytes of upload. Every extra letter multiplies it by 26.
async function buildBombColumnRef() {
  await buildWideSheet("bomb-column-ref", "BZZZZZ", 1);
}

/// The same amplification inside the format's own grid: `XFD` is a real column,
/// and a thousand rows padded to it is half a gigabyte for ten kilobytes.
async function buildBombWideRows() {
  await buildWideSheet("bomb-wide-rows", "XFD", 1_000);
}

/// Amplification through a shared formula index. A reader that keeps shared
/// formulas in a table indexed by `si` sizes it by the largest index it meets,
/// so a sixty-byte cell declaring `si="1000000000000"` asks for terabytes. The
/// derived cell after it refers to the same index.
async function buildBombSharedFormula() {
  const sheet =
    SHEET_OPEN +
    `<row r="1"><c r="A1"><v>2</v></c>` +
    `<c r="B1"><f t="shared" ref="B1:B2" si="1000000000000">A1*2</f><v>4</v></c></row>` +
    `<row r="2"><c r="A2"><v>3</v></c>` +
    `<c r="B2"><f t="shared" si="1000000000000"/><v>6</v></c></row>` +
    SHEET_CLOSE;

  const zip = new ZipWriter(path.join(OUT_DIR, "bomb-shared-formula.xlsx"));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

/// Amplification through copies. A 64 KiB shared formula written once, then
/// reused by 4 000 cells: each derived cell is thirty bytes of XML and gets the
/// whole template expanded for its position — 256 MiB of formula text out of an
/// archive that inflates to a few hundred kilobytes.
async function buildBombSharedFormulaText() {
  const template = "A1+".repeat((64 * 1024) / 3) + "A1";
  let sheet =
    SHEET_OPEN +
    `<row r="1"><c r="A1"><v>1</v></c>` +
    `<c r="B1"><f t="shared" ref="B1:B4000" si="0">${template}</f><v>0</v></c></row>`;
  for (let r = 2; r <= 4_000; r += 1) {
    sheet += `<row r="${r}"><c r="B${r}"><f t="shared" si="0"/><v>0</v></c></row>`;
  }
  sheet += SHEET_CLOSE;
  await buildCopiesSheet("bomb-shared-formula-text", sheet, []);
}

/// The same amplification through the shared string table: one 64 KiB string,
/// referred to by 4 000 cells.
async function buildBombSharedStringCopies() {
  let sheet = SHEET_OPEN;
  for (let r = 1; r <= 4_000; r += 1) {
    sheet += `<row r="${r}"><c r="A${r}" t="s"><v>0</v></c></row>`;
  }
  sheet += SHEET_CLOSE;
  await buildCopiesSheet("bomb-shared-string-copies", sheet, [
    "x".repeat(64 * 1024),
  ]);
}

async function buildCopiesSheet(name, sheet, strings) {
  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  await zip.add("[Content_Types].xml", CONTENT_TYPES(1));
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add("xl/_rels/workbook.xml.rels", workbookRels(1));
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/sharedStrings.xml", sharedStrings(strings));
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

async function buildWideSheet(name, lastColumn, rowCount) {
  let sheet = SHEET_OPEN;
  for (let r = 1; r <= rowCount; r += 1) {
    sheet += `<row r="${r}"><c r="A${r}"><v>1</v></c><c r="${lastColumn}${r}"><v>2</v></c></row>`;
  }
  sheet += SHEET_CLOSE;

  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

/// XML entity expansion, in the sheet. Nine levels of ten, so `&a9;` expands to
/// a billion characters if anything expands it at all.
async function buildXxeBillionLaughs() {
  let entities = `<!ENTITY a0 "${"a".repeat(48)}">`;
  for (let level = 1; level <= 9; level += 1) {
    entities += `<!ENTITY a${level} "${`&a${level - 1};`.repeat(10)}">`;
  }

  await buildEntitySheet(
    "xxe-billion-laughs",
    `<!DOCTYPE worksheet [${entities}]>`,
    `<row r="1"><c r="A1" t="inlineStr"><is><t>&a9;</t></is></c></row>`,
  );
}

/// External entities: a file the process can read, and a URL it could call.
async function buildXxeExternalEntity() {
  await buildEntitySheet(
    "xxe-external-entity",
    `<!DOCTYPE worksheet [` +
      `<!ENTITY file SYSTEM "file:///etc/passwd">` +
      `<!ENTITY out SYSTEM "http://127.0.0.1:9/callback">` +
      `]>`,
    `<row r="1"><c r="A1" t="inlineStr"><is><t>&file;</t></is></c>` +
      `<c r="B1" t="inlineStr"><is><t>&out;</t></is></c></row>`,
  );
}

async function buildEntitySheet(name, doctype, rows) {
  const sheet =
    `<?xml version="1.0"?>` +
    doctype +
    `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
    `<sheetData>${rows}</sheetData></worksheet>`;

  const zip = new ZipWriter(path.join(OUT_DIR, `${name}.xlsx`));
  await zip.add(
    "[Content_Types].xml",
    CONTENT_TYPES(1, { sharedStrings: false }),
  );
  await zip.add("_rels/.rels", ROOT_RELS);
  await zip.add("xl/workbook.xml", workbook(["Data"]));
  await zip.add(
    "xl/_rels/workbook.xml.rels",
    workbookRels(1, { sharedStrings: false }),
  );
  await zip.add("xl/styles.xml", STYLES);
  await zip.add("xl/worksheets/sheet1.xml", sheet);
  zip.close();
}

async function buildNotAnArchive() {
  fs.writeFileSync(
    path.join(OUT_DIR, "not-an-archive.xlsx"),
    "Ceci n'est pas une archive ZIP.\n",
  );
}

// ---------------------------------------------------------------------------

const FIXTURES = {
  "large-200000": () => buildLarge(200_000),
  "large-600000": () => buildLarge(600_000),
  "sheets-2": () => buildSheets(2),
  "sheets-4": () => buildSheets(4),
  "sheets-8": () => buildSheets(8),
  "sheets-16": () => buildSheets(16),
  types: buildTypes,
  sparse: () => buildSparse(),
  "narrow-first-row": buildNarrowFirstRow,
  "sparse-nodim": () => buildSparse({ declareDimension: false }),
  "dates-1904": buildDates1904,
  "hidden-sheets": buildHiddenSheets,
  "sheet-ids": buildSheetIds,
  "unknown-sheet-state": buildUnknownSheetState,
  "starts-lower": buildStartsLower,
  "backslash-entries": buildBackslashEntries,
  formulas: buildFormulas,
  "workbook-anatomy": buildWorkbookAnatomy,
  "bare-workbook": buildBareWorkbook,
  "sheet-anatomy": buildSheetAnatomy,
  "sheet-missing-part": buildSheetMissingPart,
  "sheet-escaping-relationship": buildSheetEscapingRelationship,
  "bomb-comments": buildBombComments,
  "bomb-comment-authors": buildBombCommentAuthors,
  "bomb-hyperlink-targets": buildBombHyperlinkTargets,
  "bomb-sheet-records": buildBombSheetRecords,
  "bomb-sheet-cells": buildBombSheetCells,
  "bomb-merges": buildBombSheetMetadata(
    "bomb-merges",
    `<mergeCells>${'<mergeCell ref="A1:B2"/>'.repeat(600_000)}</mergeCells>`,
  ),
  "bomb-validations": buildBombSheetMetadata(
    "bomb-validations",
    `<dataValidations>${'<dataValidation type="whole" sqref="A1"><formula1>1</formula1></dataValidation>'.repeat(200_000)}</dataValidations>`,
  ),
  "bomb-sharedstrings": buildBombSharedStrings,
  "bomb-styles": buildBombStyles,
  "bomb-styles-shared": buildBombStylesShared,
  "bomb-rels": buildBombRels,
  "bomb-package-rels": buildBombPackageRels,
  "bomb-workbook": buildBombWorkbook,
  "bomb-inline": buildBombInline,
  "bomb-entries": buildBombEntries,
  "bomb-column-ref": buildBombColumnRef,
  "bomb-wide-rows": buildBombWideRows,
  "bomb-shared-formula": buildBombSharedFormula,
  "bomb-shared-formula-text": buildBombSharedFormulaText,
  "bomb-shared-string-copies": buildBombSharedStringCopies,
  "xxe-billion-laughs": buildXxeBillionLaughs,
  "xxe-external-entity": buildXxeExternalEntity,
  "lying-sizes": buildLyingSizes,
  "not-an-archive": buildNotAnArchive,
};

async function main() {
  fs.mkdirSync(OUT_DIR, { recursive: true });

  const requested = process.argv.slice(2);
  const names = requested.length > 0 ? requested : Object.keys(FIXTURES);

  for (const name of names) {
    const build = FIXTURES[name];
    if (!build) {
      console.error(`fixture inconnue : ${name}`);
      console.error(`disponibles : ${Object.keys(FIXTURES).join(", ")}`);
      process.exitCode = 1;
      return;
    }
    const out = path.join(OUT_DIR, `${name}.xlsx`);
    if (fs.existsSync(out) && requested.length === 0) {
      continue;
    }
    const started = process.hrtime.bigint();
    await build();
    const elapsed = Number(process.hrtime.bigint() - started) / 1e6;
    const size = fs.statSync(out).size;
    console.log(
      `${name.padEnd(20)} ${(size / 1024 / 1024).toFixed(2).padStart(8)} Mo  ${elapsed.toFixed(0).padStart(6)} ms`,
    );
  }
}

await main();
