// The checks a version must pass before it is written anywhere.
//
// Pure: every function takes text and versions, never a path, so the rules are
// testable without a checkout. `bump-version.mjs` reads the files and calls
// `checkRelease` before touching any of them.

const VERSION =
  /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

/**
 * Split a semver version into its core and prerelease identifiers.
 *
 * @throws {Error} when `version` is not `x.y.z` or `x.y.z-<prerelease>`.
 */
export function parseVersion(version) {
  const match = VERSION.exec(version);
  if (!match) {
    throw new Error(
      `expected a version like 1.2.3 or 1.2.3-rc.1 — got ${version}`,
    );
  }
  const [, major, minor, patch, prerelease] = match;
  return {
    major: Number(major),
    minor: Number(minor),
    patch: Number(patch),
    prerelease: prerelease ? prerelease.split(".") : [],
  };
}

/** Semver precedence: negative when `a` comes first, zero when equal. */
export function compareVersions(a, b) {
  const left = parseVersion(a);
  const right = parseVersion(b);
  for (const field of ["major", "minor", "patch"]) {
    if (left[field] !== right[field]) {
      return left[field] - right[field];
    }
  }
  // A release outranks every prerelease of the same core.
  if (left.prerelease.length === 0 || right.prerelease.length === 0) {
    return right.prerelease.length - left.prerelease.length;
  }
  const length = Math.max(left.prerelease.length, right.prerelease.length);
  for (let i = 0; i < length; i++) {
    const x = left.prerelease[i];
    const y = right.prerelease[i];
    if (x === undefined || y === undefined) {
      return x === undefined ? -1 : 1;
    }
    if (x === y) {
      continue;
    }
    const numeric = /^\d+$/;
    if (numeric.test(x) && numeric.test(y)) {
      return Number(x) - Number(y);
    }
    if (numeric.test(x) !== numeric.test(y)) {
      return numeric.test(x) ? -1 : 1;
    }
    return x < y ? -1 : 1;
  }
  return 0;
}

/**
 * The version after `current` for a request: an explicit version as given, or
 * `major`, `minor` or `patch` applied the way semver and `npm version` do.
 *
 * From a prerelease, a keyword first releases the version the prerelease
 * leads to: `1.0.0-rc.3` with `patch`, `minor` or `major` is `1.0.0`, and
 * `1.2.3-rc.1` with `minor` is `1.3.0` since `1.2.3` is not a minor.
 *
 * @throws {Error} when `requested` is neither a version nor a keyword.
 */
export function nextVersion(current, requested) {
  if (VERSION.test(requested)) {
    return requested;
  }

  const { major, minor, patch, prerelease } = parseVersion(current);
  const core = `${major}.${minor}.${patch}`;
  const released = prerelease.length > 0;
  switch (requested) {
    case "major":
      return released && minor === 0 && patch === 0 ? core : `${major + 1}.0.0`;
    case "minor":
      return released && patch === 0 ? core : `${major}.${minor + 1}.0`;
    case "patch":
      return released ? core : `${major}.${minor}.${patch + 1}`;
    default:
      throw new Error(
        `expected a version like 1.2.3 or 1.2.3-rc.1, or major|minor|patch — got ${requested}`,
      );
  }
}

/**
 * The recipe a version must ship, or `null` when it breaks nothing.
 *
 * The first version of a major from 1.0.0 on, and every prerelease leading to
 * it, is the one that breaks compatibility: `1.0.0-rc.2` and `1.0.0` both need
 * `migrations/1.0.0.md`, `1.0.1` and `1.1.0` need none.
 */
export function requiredRecipe(version) {
  const { major, minor, patch } = parseVersion(version);
  if (major === 0 || minor !== 0 || patch !== 0) {
    return null;
  }
  return `migrations/${major}.0.0.md`;
}

/** Whether `changelog` has a `## [<version>]` section heading. */
export function hasChangelogSection(changelog, version) {
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^## \\[${escaped}\\]`, "m").test(changelog);
}

/**
 * Every reason `next` cannot be released after `current`; empty when it can.
 *
 * `recipes` lists the files under `migrations/`, by name.
 */
export function checkRelease({ current, next, changelog, recipes }) {
  const problems = [];
  if (compareVersions(next, current) <= 0) {
    problems.push(
      `${next} is not ahead of ${current}; npm does not take a version back`,
    );
  }
  if (!hasChangelogSection(changelog, next)) {
    problems.push(`CHANGELOG.md has no "## [${next}]" section`);
  }
  const recipe = requiredRecipe(next);
  if (recipe && !recipes.includes(recipe.slice("migrations/".length))) {
    problems.push(`${next} breaks compatibility and ${recipe} is missing`);
  }
  return problems;
}
