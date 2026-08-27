#!/usr/bin/env node
//
// Turn what `napi create-npm-dirs` produced into what should actually be
// published, and wire the root package to it.
//
//   node scripts/prepare-npm-packages.mjs
//
// Run *after* `napi create-npm-dirs` and `napi artifacts`, and after any other
// pnpm command: writing the optionalDependencies below puts `package.json` out
// of step with the lockfile, and `pnpm exec` refuses to run anything once that
// is true. Two jobs:
//
// **Strip the search metadata from the platform packages.** `create-npm-dirs`
// copies the root package's description and keywords into each of the five.
// Published as is, a search for "excel" returns six near-identical results, only
// one of which anybody should install. They are an implementation detail of how
// a native binary reaches a machine: resolved by name, never searched for.
//
// **Write the root package's `optionalDependencies`.** They cannot live in the
// committed manifest: they name packages that do not exist until this job
// creates them, so `pnpm install --frozen-lockfile` has nothing to resolve and
// the lockfile cannot be generated. `napi pre-publish` does not add them either
// (checked against @napi-rs/cli 3.8.6). So they are written here, from the
// directories that are about to be published — which is also what keeps them
// from drifting out of step with the release.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PACKAGE_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "../crates/xlsx-node",
);
const NPM_DIR = path.join(PACKAGE_DIR, "npm");

if (!fs.existsSync(NPM_DIR)) {
  console.error(
    `${NPM_DIR} does not exist; run \`napi create-npm-dirs\` first`,
  );
  process.exit(1);
}

function read(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function write(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

const rootManifest = path.join(PACKAGE_DIR, "package.json");
const root = read(rootManifest);
const optional = {};

for (const entry of fs.readdirSync(NPM_DIR).sort()) {
  const manifest = path.join(NPM_DIR, entry, "package.json");
  if (!fs.existsSync(manifest)) {
    continue;
  }

  const platform = read(manifest);

  delete platform.keywords;
  // Says what it is, so that anyone who lands on the page knows why it exists
  // and what to install instead.
  platform.description =
    `The ${entry} binary for ${root.name}. ` +
    `Installed automatically; install ${root.name} instead.`;

  write(manifest, platform);
  optional[platform.name] = platform.version;

  console.log(`prepared ${platform.name}@${platform.version}`);
}

const names = Object.keys(optional);
if (names.length === 0) {
  console.error("no platform packages found; nothing to wire up");
  process.exit(1);
}

root.optionalDependencies = optional;
write(rootManifest, root);

console.log(`wired ${names.length} optional dependencies into ${root.name}`);
