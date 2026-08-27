// XLSX part templates shared by the fixture generators.

export const CONTENT_TYPES = (sheetCount, { sharedStrings = true } = {}) => {
  const sheets = Array.from(
    { length: sheetCount },
    (_, i) =>
      `<Override PartName="/xl/worksheets/sheet${i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`,
  ).join("");
  return (
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
    `<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">` +
    `<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>` +
    `<Default Extension="xml" ContentType="application/xml"/>` +
    `<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>` +
    sheets +
    `<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>` +
    (sharedStrings
      ? `<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>`
      : "") +
    `</Types>`
  );
};

export const ROOT_RELS =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
  `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
  `<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>` +
  `</Relationships>`;

export const workbook = (sheetNames, { date1904 = false } = {}) => {
  const sheets = sheetNames
    .map(
      (name, i) =>
        `<sheet name="${escapeXml(name)}" sheetId="${i + 1}" r:id="rId${i + 1}"/>`,
    )
    .join("");
  return (
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
    `<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
    `xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">` +
    `<workbookPr date1904="${date1904 ? "1" : "0"}"/>` +
    `<sheets>${sheets}</sheets>` +
    `</workbook>`
  );
};

export const workbookRels = (sheetCount, { sharedStrings = true } = {}) => {
  const base =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
  const sheets = Array.from(
    { length: sheetCount },
    (_, i) =>
      `<Relationship Id="rId${i + 1}" Type="${base}/worksheet" Target="worksheets/sheet${i + 1}.xml"/>`,
  ).join("");
  const styles = `<Relationship Id="rId${sheetCount + 1}" Type="${base}/styles" Target="styles.xml"/>`;
  const strings = sharedStrings
    ? `<Relationship Id="rId${sheetCount + 2}" Type="${base}/sharedStrings" Target="sharedStrings.xml"/>`
    : "";
  return (
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
    `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
    sheets +
    styles +
    strings +
    `</Relationships>`
  );
};

/// Style index 1 is `numFmtId="14"` — the built-in short date. A cell carrying
/// `s="1"` is a date; without this table its value is just a number.
export const STYLES =
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
  `<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
  `<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>` +
  `<fills count="1"><fill><patternFill patternType="none"/></fill></fills>` +
  `<borders count="1"><border/></borders>` +
  `<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>` +
  `<cellXfs count="4">` +
  `<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>` +
  `<xf numFmtId="14" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>` +
  `<xf numFmtId="22" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>` +
  // numFmt 46 is `[h]:mm:ss`, an elapsed time rather than a point in one.
  `<xf numFmtId="46" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>` +
  `</cellXfs>` +
  `</styleSheet>`;

export const sharedStrings = (words) =>
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
  `<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ` +
  `count="${words.length}" uniqueCount="${words.length}">` +
  words.map((w) => `<si><t>${escapeXml(w)}</t></si>`).join("") +
  `</sst>`;

export function escapeXml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

export function columnName(index) {
  let name = "";
  let n = index;
  do {
    name = String.fromCharCode(65 + (n % 26)) + name;
    n = Math.floor(n / 26) - 1;
  } while (n >= 0);
  return name;
}

/// Excel serial for a date, 1900 system: 1899-12-30 is day 0, and the phantom
/// 1900-02-29 is already accounted for by that origin.
export function excelSerial(date) {
  return (date.getTime() - Date.UTC(1899, 11, 30)) / 86_400_000;
}
