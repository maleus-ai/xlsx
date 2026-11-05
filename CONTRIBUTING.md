# Working on this repository

This repository is a mirror. The library is developed in an internal
repository and exported here, one commit per sync; anything committed here
directly is overwritten by the next one. Issues and pull requests are welcome
all the same: a maintainer reviews a pull request here, then carries the change
over to the internal repository, from where it comes back with the next sync.

## What you need

- rustup: `rust-toolchain.toml` pins the toolchain and its components, and
  rustup installs it on the first `cargo` command
- Node 24 and pnpm
- Docker, for the two things that have to run on the real target: the musl build
  and the Alpine smoke test

## Layout

```
crates/xlsx-core     the reader (calamine) and the writer (rust_xlsxwriter).
                     No napi anywhere in its tree.
crates/xlsx-node     the binding (napi-rs)  → @maleus/xlsx
fixtures/            the fixture generator. Nothing is committed.
scripts/             benchmarks, the musl build, the Alpine smoke test, and
                     ci-native.sh, every check beyond cargo.
```

The core carries no binding, and `pnpm run ci:native` checks it:
`cargo tree -p xlsx-core | grep napi` must come back empty. That is what keeps the core testable in plain Rust,
and what stops product policy from leaking into a reader that cannot import it.

`crates/xlsx-node/binding.js` and `binding.d.ts` are generated from the Rust
source and committed, because the publish job assembles the package without
building it. `pnpm run ci:native` fails if they are out of date, so rebuild and
commit them whenever the binding's surface changes.

`CellValue` is shared by both halves on purpose. It is the same set of variants
in either direction, which is what lets `tests/writing.rs` assert the strongest
property in the repository: read a workbook, write the rows straight back out,
read the result, and compare. The writer cannot drift from the reader without
that going red.

Three things do not survive that trip, each pinned by its own test: `Text("")`
comes back `Empty`, `Error("#N/A")` comes back `Text("#N/A")`, and a date before
1900 is refused outright because Excel has no serial for it.

## The loop

```sh
pnpm install
node fixtures/generate.mjs        # fixtures are rebuilt, never committed
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
pnpm run build && pnpm test       # the Node suite needs a release binary
pnpm run ci:native                # everything the maintainers' CI runs beyond cargo
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
and publishes six npm packages: `@maleus/xlsx`, which carries no binary,
and one package per platform carrying exactly one. They share a version, which
`bump-version.mjs` sets before the release and `prepare-npm-packages.mjs` copies
into the platform manifests.

`napi pre-publish` is not used, and there is deliberately no `prepublishOnly`
script: it publishes the platform packages itself, so anything that then
publishes them again lands on a version the registry has already staged. That
is how a release once got half-published.

`scripts/prepare-npm-packages.mjs` runs between `napi create-npm-dirs` and
`napi artifacts`. It strips the search metadata from the five — published as is,
one search for "excel" would return six near-identical results, only one of which
anybody should install — and it writes the root package's
`optionalDependencies`.

Those cannot live in the committed manifest: they name packages that do not
exist until the release job creates them, so there is nothing for
`pnpm install --frozen-lockfile` to resolve and no lockfile to generate. Deriving
them from the directories about to be published is also what keeps them from
drifting out of step with the release.

Releases are made by the maintainers: the version is bumped in the internal
repository with `scripts/bump-version.mjs`, which writes it in the four places
it lives (the Cargo workspace and `Cargo.lock`, the npm manifest and the
internal repository's lockfile) and
regenerates `binding.js`, which carries it too. The next sync brings the bump
here, and the release is a tag on that commit.

The platform packages are not bumped: they do not exist until the release
job creates them, and `prepare-npm-packages.mjs` gives them the manifest's
version at that point. Committing their names to the manifest looks tidy and is
not workable: a bump would point them at a version npm does not have yet,
`pnpm install` drops what it cannot resolve, and `--frozen-lockfile` then
refuses the tree — so the release commit would be red until the release it is
supposed to produce had already happened.

### Authentication

Publishing goes through npm's trusted publishing: the job exchanges its OIDC
token for a short-lived credential, so the repository holds no npm secret to
leak or rotate, and provenance is attested without asking for it.

That attestation is why the publish job runs on a GitHub-hosted runner while
everything else runs on blacksmith: npm rejects a provenance bundle produced on
a self-hosted runner, and blacksmith counts as one. Nothing in that job
compiles, so it costs nothing to move.
