# Changelog

All notable changes to `@maleus/xlsx` are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the package follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.1] - 2026-11-11

### Security

- `zip` is updated to 9.5.0, which fixes [GHSA-m665-v7pm-7grj](https://github.com/advisories/GHSA-m665-v7pm-7grj), [GHSA-9hj8-73fh-9963](https://github.com/advisories/GHSA-9hj8-73fh-9963), [GHSA-hjqx-j4rw-g7xv](https://github.com/advisories/GHSA-hjqx-j4rw-g7xv), [GHSA-8hxf-qr8p-62x4](https://github.com/advisories/GHSA-8hxf-qr8p-62x4) and [GHSA-q7v9-x93c-xqv3](https://github.com/advisories/GHSA-q7v9-x93c-xqv3). None of them is reachable through this package, which reads archives through their central directory, never extracts them, and builds `zip` with `deflate` as its only codec. Every 8.x release of `zip` has been yanked upstream.
- `calamine` is updated to 0.38.0 and `rust_xlsxwriter` to 0.102.0, the first releases built on `zip` 9.

### Changed

- An archive with an entry that inflates past its declared size is refused with `CORRUPT` as soon as the output passes the declared size, instead of with `DECOMPRESSED_BUDGET_EXCEEDED` once `maxDecompressedBytes` is reached.

### Fixed

- Writing reports `IO` when the temporary file for a new worksheet cannot be created, instead of panicking.
