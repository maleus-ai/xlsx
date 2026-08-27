#!/usr/bin/env node
//
// Strip the search metadata from the per-platform packages.
//
// `napi create-npm-dirs` copies the root package's description and keywords into
// each of the five platform packages. Published as is, a search for "excel"
// returns six near-identical results, only one of which anybody should install.
// The platform packages are an implementation detail of how a native binary
// reaches a machine: they are resolved by name through `optionalDependencies`
// and never searched for.
//
//   node scripts/trim-platform-packages.mjs
//
// Run between `napi create-npm-dirs` and `napi pre-publish`.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const NPM_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "../crates/xlsx-node/npm",
);

if (!fs.existsSync(NPM_DIR)) {
  console.error(
    `${NPM_DIR} does not exist; run \`napi create-npm-dirs\` first`,
  );
  process.exit(1);
}

for (const entry of fs.readdirSync(NPM_DIR)) {
  const manifest = path.join(NPM_DIR, entry, "package.json");
  if (!fs.existsSync(manifest)) {
    continue;
  }

  const pkg = JSON.parse(fs.readFileSync(manifest, "utf8"));

  delete pkg.keywords;
  // Says what it is, so that anyone who lands on the page knows why it exists
  // and what to install instead.
  pkg.description = `The ${entry} binary for @maleus/xlsx-reader. Installed automatically; install @maleus/xlsx-reader instead.`;

  fs.writeFileSync(manifest, `${JSON.stringify(pkg, null, 2)}\n`);
  console.log(`trimmed ${entry}`);
}
