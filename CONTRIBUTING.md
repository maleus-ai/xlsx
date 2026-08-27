# Working on this repository

## What you need

- Rust 1.95 (the toolchain CI pins)
- Node 24 and pnpm
- Docker, for the two things that have to run on the real target: the musl build
  and the Alpine smoke test

## Layout

```
crates/xlsx-core     the reader (calamine). No napi anywhere in its tree.
crates/xlsx-node     the binding (napi-rs)  → @maleus/xlsx-reader
fixtures/            the fixture generator. Nothing is committed.
scripts/             benchmarks, the musl build, the Alpine smoke test.
```

The core carries no binding, and CI checks it: `cargo tree -p xlsx-core | grep
napi` must come back empty. That is what keeps the core testable in plain Rust,
and what stops product policy from leaking into a reader that cannot import it.

`crates/xlsx-node/binding.js` and `binding.d.ts` are generated from the Rust
source and committed, because the publish job assembles the package without
building it. CI fails if they are out of date, so rebuild and commit them
whenever the binding's surface changes.

## The loop

```sh
pnpm install
node fixtures/generate.mjs        # fixtures are rebuilt, never committed
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
pnpm run build && pnpm test       # the Node suite needs a release binary
```

The Node suite is built in release on purpose: it asserts a peak RSS and an
event loop that keeps its cadence, and neither figure means anything
unoptimised.

The production target, and the deployment image it has to load in:

```sh
./scripts/build-musl.sh           # built in Alpine, from an image pinned by digest
docker run --rm --platform linux/amd64 -v "$PWD:/work" -w /work \
  node:24-alpine node scripts/smoke-alpine.mjs
```

`linux-x64-musl` is the target that matters: the applications this was built for
deploy on `node:24-alpine`. Building it inside Alpine rather than cross-compiling
means the binary is linked against the libc it will run on, and pinning the image
by digest keeps a compiler out of the supply chain of a native module that runs
unsandboxed wherever it is installed.

## Fixtures

Nothing under `fixtures/out/` is committed. `node fixtures/generate.mjs` rebuilds
everything; passing names rebuilds only those. The Rust and Node suites both
generate a fixture on demand if it is missing, so a fresh clone needs no
preparation.

The hostile ones are assembled byte by byte — local header, raw deflate, central
directory, EOCD — by `fixtures/lib/zip.mjs`, because the point is to produce what
a real library would refuse to write: an archive that lies about its sizes, one
that declares sixty-five thousand entries, one whose cell reference names column
36 119 382.

Adding a bound means adding the archive that defeats it without one. Several of
the bounds in this reader exist because a fixture was written first and the
measurement was worse than anybody expected.

## What the tests are holding

Some of the properties here are inherited rather than written — XML entities that
are never expanded, an archive read through its central directory rather than
walked forward — and an inherited property is one a version bump can take away
without saying so. Those have tests of their own. If a dependency upgrade makes
one of them fail, the failure is the point; do not adjust the test to match the
new behaviour without deciding, first, whether the new behaviour is acceptable.

The same goes for `crates/xlsx-core/src/cursor.rs`, the one `unsafe` block in the
repository. Its invariants are written above it and one of them —
`XlsxReader: Send` — is asserted at compile time, because the binding hands a
reader to the libuv threadpool and the property comes from a chain of fields that
a dependency bump could quietly break.

## Releasing

Tagging `v*` runs `.github/workflows/release.yml`, which builds the five targets
and publishes six npm packages: `@maleus/xlsx-reader`, which carries no binary,
and one package per platform carrying exactly one. They share a version;
`napi pre-publish` keeps them in step.

`scripts/trim-platform-packages.mjs` strips the search metadata from the five
before they go out — published as is, one search for "excel" would return six
near-identical results, only one of which anybody should install.

Publishing needs an `NPM_TOKEN` secret with write access to the `@maleus` scope,
not to a list of packages: the platform packages for a new version do not exist
until the job creates them.
