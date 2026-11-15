# Changelog

Every notable change to `@maleus/xlsx`, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A breaking change names its entry in the migration recipe: the first version of every major ships one under `migrations/`, at the root of the package, written to be applied step by step — by a person or by a coding agent.

## [1.0.0-rc.1] — 2026-11-12

The first release candidate of 1.0.0, published under the `next` dist-tag.

### Changed

All breaking; recipe: [migrations/1.0.0.md](migrations/1.0.0.md).

- **Breaking** — `listSheets` is `peekSheets` (`peek-sheets`).
- **Breaking** — `xlsxRows` is `readRows`, and `XlsxRowsOptions` is `ReadRowsOptions` (`read-rows`).
- **Breaking** — a row is `{ row, values }`: `row` is its 0-based position in the sheet, so a row after a gap says where it sits (`row-object`).
- **Breaking** — `SheetInfo` is `{ name, sheetId, state }`: `state` tells `hidden` from `veryHidden`, and `sheetId` is the identifier the workbook declares (`sheet-info-state`).
- **Breaking** — a sheet whose state the format does not define, or that declares no numeric `sheetId`, is refused with `CORRUPT` (`unknown-sheet-state-corrupt`).
- **Breaking** — `xlsxWriteStream` is `writeWorkbook`, and `XlsxWriteStreamOptions` is `WriteWorkbookOptions` (`write-workbook`).
- **Breaking** — a row naming its sheet carries `values`, not `data` (`sheet-row-values`).
- **Breaking** — `XlsxErrorCode` is documented as open: a minor version may add a code (`error-code-open-union`).

### Added

- `inspectSheet`: everything a sheet declares besides its cells — merged ranges, data validations and conditional formats (`x14` included), tables, comments, hyperlinks, panes, column definitions, hidden rows, protection, autofilter, picture count. Every part read is charged against the byte budget, and so are the copy of its target each hyperlink receives and the memory every record built from those parts takes beyond its text.
- `inspectWorkbook`: sheets, defined names, document properties, date system, the kinds of parts the package carries, and the cell styles — each with its alignment, referring by index (`numFmtRef`, `fontRef`, `fillRef`) to the number formats, fonts and fills held once in their own tables — as ECMA-376 declares them. Reads no sheet.
- `readCells`, a stream of every cell a sheet declares with its type, its formula — shared formulas expanded — and the index of its style, bounded by a new `maxCells` budget and the `CELL_BUDGET_EXCEEDED` code.
- `parseRef` and `toA1`, pure helpers between an A1 reference and 0-based positions.
- The package ships this changelog, and from 1.0.0 on a migration recipe for every breaking version under `migrations/`.
- Release candidates are published under the `next` dist-tag; `latest` only ever names a stable version.

## [0.5.1]

### Security

- The reader and writer move to zip 9.5, calamine 0.38 and rust_xlsxwriter 0.102.

### Changed

- An archive entry that inflates past the size it declares is refused with `CORRUPT` rather than charged against the byte budget.

## [0.5.0] — 2026-11-05

The first version this changelog records.
