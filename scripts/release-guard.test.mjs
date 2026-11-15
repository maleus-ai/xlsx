import assert from "node:assert/strict";
import test from "node:test";

import {
  checkRelease,
  compareVersions,
  nextVersion,
  requiredRecipe,
} from "./lib/release-guard.mjs";

const changelogFor = (...versions) =>
  ["# Changelog", ...versions.map((v) => `## [${v}] — 2026-11-05`)].join(
    "\n\n",
  );

test("accepts a patch bump when the changelog has a section for the target version", () => {
  assert.deepEqual(
    checkRelease({
      current: "1.0.0",
      next: "1.0.1",
      changelog: changelogFor("1.0.1", "1.0.0"),
      recipes: [],
    }),
    [],
  );
});

test("refuses a bump without a changelog section for the target version", () => {
  const problems = checkRelease({
    current: "1.0.0",
    next: "1.1.0",
    changelog: changelogFor("1.0.0"),
    recipes: [],
  });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /## \[1\.1\.0\]/);
});

test("refuses a major bump without a migration recipe", () => {
  const problems = checkRelease({
    current: "1.4.2",
    next: "2.0.0",
    changelog: changelogFor("2.0.0"),
    recipes: [],
  });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /migrations\/2\.0\.0\.md/);
});

test("accepts 0.5.0, 1.0.0-rc.1, 1.0.0-rc.2, 1.0.0 when the 1.0.0 recipe exists", () => {
  const chain = ["0.5.0", "1.0.0-rc.1", "1.0.0-rc.2", "1.0.0"];
  const changelog = changelogFor(...chain.slice().reverse());
  for (let i = 1; i < chain.length; i++) {
    assert.deepEqual(
      checkRelease({
        current: chain[i - 1],
        next: chain[i],
        changelog,
        recipes: ["1.0.0.md"],
      }),
      [],
      `${chain[i - 1]} → ${chain[i]}`,
    );
  }
});

test("refuses to go from a release candidate back to an earlier one", () => {
  const problems = checkRelease({
    current: "1.0.0-rc.2",
    next: "1.0.0-rc.1",
    changelog: changelogFor("1.0.0-rc.1"),
    recipes: ["1.0.0.md"],
  });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /not ahead/);
});

test("orders prerelease identifiers numerically, and a release after its prereleases", () => {
  assert.ok(compareVersions("1.0.0-rc.2", "1.0.0-rc.10") < 0);
  assert.ok(compareVersions("1.0.0-rc.10", "1.0.0") < 0);
  assert.ok(compareVersions("0.9.9", "1.0.0-rc.1") < 0);
  assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
});

test("a keyword from a release candidate releases the version it prepares", () => {
  assert.equal(nextVersion("1.0.0-rc.3", "patch"), "1.0.0");
  assert.equal(nextVersion("1.0.0-rc.3", "minor"), "1.0.0");
  assert.equal(nextVersion("1.0.0-rc.3", "major"), "1.0.0");
  assert.equal(nextVersion("1.2.0-rc.1", "minor"), "1.2.0");
  assert.equal(nextVersion("1.2.0-rc.1", "major"), "2.0.0");
  assert.equal(nextVersion("1.2.3-rc.1", "minor"), "1.3.0");
});

test("a keyword from a stable version increments it", () => {
  assert.equal(nextVersion("1.2.3", "patch"), "1.2.4");
  assert.equal(nextVersion("1.2.3", "minor"), "1.3.0");
  assert.equal(nextVersion("1.2.3", "major"), "2.0.0");
  assert.equal(nextVersion("1.2.3", "1.0.0-rc.1"), "1.0.0-rc.1");
  assert.throws(() => nextVersion("1.2.3", "next"), /major\|minor\|patch/);
});

test("only the first version of a major from 1.0.0 on needs a recipe", () => {
  assert.equal(requiredRecipe("1.0.0-rc.3"), "migrations/1.0.0.md");
  assert.equal(requiredRecipe("3.0.0"), "migrations/3.0.0.md");
  assert.equal(requiredRecipe("1.0.1"), null);
  assert.equal(requiredRecipe("1.1.0"), null);
  assert.equal(requiredRecipe("0.6.0"), null);
});
