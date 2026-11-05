#!/bin/sh
# Every check the library needs beyond rustfmt, clippy and cargo test, in the
# order CI runs them.
#
#   pnpm --filter @maleus/xlsx run ci:native
#
# Requires Rust (pinned by rust-toolchain.toml) and Docker: the last two steps
# build the musl addon inside Alpine and load it in the deployment image.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
NODE_PKG="$ROOT/crates/xlsx-node"

cd "$ROOT"
node fixtures/generate.mjs

# Released, not debug: the suite asserts a peak RSS and an event loop that keeps
# its cadence, and neither figure means anything unoptimised.
cd "$NODE_PKG"
pnpm run build:native
pnpm run test:native

# `binding.js` and `binding.d.ts` are generated from the Rust source and
# committed, because the publish job assembles the package without building it.
# A stale copy would ship an API that does not exist.
git diff --exit-code -- binding.js binding.d.ts

# The package ships its own copy of the README, and that copy is what npm
# displays.
diff -u "$ROOT/README.md" README.md

# `napi pre-publish` publishes the platform packages on its own. Reached through
# a lifecycle hook it runs once per `npm publish`, and the release then tries to
# publish what it has already put out.
node -e '
  const s = require("./package.json").scripts ?? {};
  const hooks = ["prepublish", "prepublishOnly", "prepare", "prepack", "postpublish"];
  const found = hooks.filter((h) => s[h]);
  if (found.length) {
    console.error(`publishing must run no lifecycle script; found: ${found.join(", ")}`);
    process.exit(1);
  }
'

cd "$ROOT"

# The core must stay buildable, and testable, without a Node runtime in its
# dependency tree.
if cargo tree -p xlsx-core | grep -q napi; then
  echo "xlsx-core depends on napi" >&2
  exit 1
fi

# `x86_64-unknown-linux-musl` is the target that matters: the generated
# applications deploy on `node:24-alpine`, pinned `linux/amd64`.
./scripts/build-musl.sh x64
docker run --rm --platform linux/amd64 -v "$ROOT:/work" -w /work node:24-alpine \
  node scripts/smoke-alpine.mjs
