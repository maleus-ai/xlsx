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

### Bumping the version

The version is written in four files and nothing keeps them in step: the Cargo
workspace, the npm manifest, and the two lockfiles that record what those say.
Forgetting one either turns CI red or publishes a package whose manifest
disagrees with the crate it was built from.

```sh
node scripts/bump-version.mjs minor    # or major, patch, or 1.2.3
```

It refuses to go backwards, because a published npm version cannot be taken
back — which also means it cannot undo itself. `git checkout` can.

The platform packages are not bumped here: they do not exist until the release
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

A trusted publisher is configured per package, on a page that only exists once
the package does — which makes the first release a chicken-and-egg. It is
resolved by publishing that one by hand.

### The first release, by hand

The five binaries still come from CI, because nobody has all five toolchains.
Running the release workflow without a tag stops after `build`: the `publish` job
only runs on a tag.

```sh
# `gh workflow run` says nothing about the run it created, so it has to be looked
# up: the newest run of that workflow, once the API has got round to listing it.
gh workflow run release.yml --repo maleus-ai/xlsx
sleep 5
RUN=$(gh run list --repo maleus-ai/xlsx --workflow release.yml \
  --limit 1 --json databaseId -q '.[0].databaseId')

# Prints the run it is following, which is also how you check it picked the one
# you just dispatched rather than something else that happened to start.
gh run watch "$RUN" --repo maleus-ai/xlsx

# A finished run does not mean downloadable artifacts: asking too early creates
# the directories and puts nothing in them, without failing. Wait for all five
# to be listed.
until [ "$(gh api "repos/maleus-ai/xlsx/actions/runs/$RUN/artifacts" \
  -q '.total_count')" = "5" ]; do sleep 5; done

# Passing the run explicitly matters: without it, `gh run download` takes the
# newest artifact in the repository, whichever run produced it.
gh run download "$RUN" --repo maleus-ai/xlsx \
  --dir crates/xlsx-node/artifacts --pattern 'bindings-*'

# It puts each artifact in a directory of its own; the assembly wants them side
# by side.
find crates/xlsx-node/artifacts -name '*.node' \
  -exec mv {} crates/xlsx-node/artifacts/ \;

cd crates/xlsx-node
pnpm exec napi create-npm-dirs
node ../../scripts/prepare-npm-packages.mjs
pnpm exec napi artifacts

# Not `napi pre-publish`: it publishes the platform packages itself unless told
# not to, and everything else it does has already been done above.
npm login
for dir in npm/*/; do npm publish "$dir" --access public; done
npm publish --access public

# `prepare-npm-packages.mjs` wrote optionalDependencies into the manifest for
# the publish. That state does not belong in the repository.
git checkout package.json
```

Then, on npmjs.com, add a trusted publisher to each of the six packages —
GitHub Actions, `maleus-ai/xlsx`, workflow `release.yml` — and every release
after that is a tag and nothing else.
