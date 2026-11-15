#!/usr/bin/env node
//
// Set the release version everywhere it is written.
//
//   node scripts/bump-version.mjs 0.2.0
//   node scripts/bump-version.mjs 1.0.0-rc.1
//   node scripts/bump-version.mjs minor
//
// The changelog section for the target version is written first: the script
// refuses a version `crates/xlsx-node/CHANGELOG.md` does not describe, and the
// first version of a new major without its `migrations/<major>.0.0.md`.
//
// The version lives in four files and nothing keeps them in step: the Cargo
// workspace, the npm manifest, and the two lockfiles that record what those two
// say. Forgetting one of them either turns CI red or publishes a package whose
// manifest disagrees with the crate it was built from.
//
// Run it on a clean tree, read the diff, and ship it through a pull request;
// the release itself is a tag on the public repository's commit that carries it.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { checkRelease, nextVersion } from "./lib/release-guard.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const CARGO_TOML = path.join(ROOT, "Cargo.toml");
const PACKAGE_JSON = path.join(ROOT, "crates/xlsx-node/package.json");

const CHANGELOG = path.join(ROOT, "crates/xlsx-node/CHANGELOG.md");
const MIGRATIONS = path.join(ROOT, "crates/xlsx-node/migrations");

function fail(message) {
  console.error(message);
  process.exit(1);
}

function currentVersion() {
  const cargo = fs.readFileSync(CARGO_TOML, "utf8");
  const match = /\[workspace\.package\][^[]*?\nversion = "([^"]+)"/s.exec(
    cargo,
  );
  if (!match) {
    fail("no [workspace.package] version in Cargo.toml");
  }
  return match[1];
}

function resolveNext(current, requested) {
  try {
    return nextVersion(current, requested);
  } catch (error) {
    return fail(error.message);
  }
}

/**
 * Refuse a version that cannot ship: one that does not move forward, one the
 * changelog does not describe, a breaking one without its migration recipe.
 */
function assertReleasable(current, next) {
  const recipes = fs.existsSync(MIGRATIONS) ? fs.readdirSync(MIGRATIONS) : [];
  const problems = checkRelease({
    current,
    next,
    changelog: fs.readFileSync(CHANGELOG, "utf8"),
    recipes,
  });
  if (problems.length > 0) {
    fail(problems.map((problem) => `  - ${problem}`).join("\n"));
  }
}

function run(command, args) {
  console.log(`  ${command} ${args.join(" ")}`);
  execFileSync(command, args, { cwd: ROOT, stdio: "inherit" });
}

const requested = process.argv[2];
if (!requested) {
  fail("usage: node scripts/bump-version.mjs <version|major|minor|patch>");
}

const current = currentVersion();
const next = resolveNext(current, requested);
assertReleasable(current, next);

console.log(`${current} → ${next}\n`);

// The Cargo workspace. Anchored on the `[workspace.package]` table so that a
// dependency pinned to the same string is left alone.
const cargo = fs.readFileSync(CARGO_TOML, "utf8");
const bumped = cargo.replace(
  /(\[workspace\.package\][^[]*?\nversion = ")[^"]+(")/s,
  `$1${next}$2`,
);
if (bumped === cargo) {
  fail("could not rewrite the version in Cargo.toml");
}
fs.writeFileSync(CARGO_TOML, bumped);
console.log("  Cargo.toml");

// The npm manifest.
const manifest = JSON.parse(fs.readFileSync(PACKAGE_JSON, "utf8"));
manifest.version = next;
fs.writeFileSync(PACKAGE_JSON, `${JSON.stringify(manifest, null, 2)}\n`);
console.log("  crates/xlsx-node/package.json");

// The lockfiles, which record what the two above say. `--workspace` keeps cargo
// to the members' own versions rather than refreshing every dependency.
run("cargo", ["update", "--workspace", "--quiet"]);
// The npm lockfile is the monorepo's, at the workspace root.
run("pnpm", ["install", "--workspace-root", "--lockfile-only", "--silent"]);

// `binding.js` is generated, committed, and carries the version in fifty-four
// version checks of its own. Leaving it behind ships a package that refuses its
// own binary under NAPI_RS_ENFORCE_VERSION_CHECK — and turns CI red on the
// release commit, which is the worst moment to be red. A debug build regenerates
// exactly the same file as a release one, and takes seconds.
run("pnpm", ["--filter", "@maleus/xlsx", "run", "build:debug"]);

console.log("\nRead the diff, then ship it through a pull request:\n");
console.log(`  git commit -am "chore(xlsx): release ${next}"`);
console.log("  gh stack submit --auto");
console.log(
  `\nOnce merged, export it and tag v${next} on the exported commit (PUBLISHING.md).`,
);
