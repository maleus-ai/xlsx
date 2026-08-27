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
  "bomb-sharedstrings": buildBombSharedStrings,
  "bomb-styles": buildBombStyles,
  "bomb-rels": buildBombRels,
  "bomb-package-rels": buildBombPackageRels,
  "bomb-workbook": buildBombWorkbook,
  "bomb-inline": buildBombInline,
  "bomb-entries": buildBombEntries,
  "bomb-column-ref": buildBombColumnRef,
  "bomb-wide-rows": buildBombWideRows,
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
