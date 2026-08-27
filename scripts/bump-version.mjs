#!/usr/bin/env node
//
// Set the release version everywhere it is written.
//
//   node scripts/bump-version.mjs 0.2.0
//   node scripts/bump-version.mjs minor
//
// The version lives in four files and nothing keeps them in step: the Cargo
// workspace, the npm manifest, and the two lockfiles that record what those two
// say. Forgetting one of them either turns CI red or publishes a package whose
// manifest disagrees with the crate it was built from.
//
// Run it on a clean tree, read the diff, commit it, and tag.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const CARGO_TOML = path.join(ROOT, "Cargo.toml");
const PACKAGE_JSON = path.join(ROOT, "crates/xlsx-node/package.json");

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;

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

function nextVersion(current, requested) {
  if (SEMVER.test(requested)) {
    return requested;
  }

  const [major, minor, patch] = current.split(".").map(Number);
  switch (requested) {
    case "major":
      return `${major + 1}.0.0`;
    case "minor":
      return `${major}.${minor + 1}.0`;
    case "patch":
      return `${major}.${minor}.${patch + 1}`;
    default:
      return fail(
        `expected a version like 1.2.3, or major|minor|patch — got ${requested}`,
      );
  }
}

/** Refuse to go backwards: a published version cannot be republished. */
function assertForward(current, next) {
  const [a, b, c] = current.split(".").map(Number);
  const [x, y, z] = next.split(".").map(Number);
  const ordered = x > a || (x === a && (y > b || (y === b && z > c)));
  if (!ordered) {
    fail(
      `${next} is not ahead of ${current}; npm does not take a version back`,
    );
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
const next = nextVersion(current, requested);
assertForward(current, next);

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
run("pnpm", ["install", "--lockfile-only", "--silent"]);

// `binding.js` is generated, committed, and carries the version in fifty-four
// version checks of its own. Leaving it behind ships a package that refuses its
// own binary under NAPI_RS_ENFORCE_VERSION_CHECK — and turns CI red on the
// release commit, which is the worst moment to be red. A debug build regenerates
// exactly the same file as a release one, and takes seconds.
run("pnpm", ["--filter", "@maleus/xlsx-reader", "run", "build:debug"]);

console.log(
  "\nRead the diff, then open it as a pull request — `main` takes one:\n",
);
console.log(`  git checkout -b chore/release-${next}`);
console.log(`  git commit -am "chore: release ${next}"`);
console.log(
  `  git push -u origin chore/release-${next} && gh pr create --fill`,
);
console.log("\nOnce merged, tag what landed on main rather than the branch:");
console.log("a squash or a merge commit is not the commit you pushed.\n");
console.log("  git checkout main && git pull");
console.log(`  git tag v${next} && git push origin v${next}`);
