import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);

/** Path to a fixture, generating it first if it is not there yet. */
export function fixture(name) {
  const file = path.join(REPO_ROOT, "fixtures/out", `${name}.xlsx`);
  if (!fs.existsSync(file)) {
    execFileSync("node", ["fixtures/generate.mjs", name], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });
  }
  return file;
}

/** Generous bounds, for the tests that are about reading rather than refusing. */
export const PERMISSIVE = {
  maxDecompressedBytes: 1 << 30,
  maxRows: 1 << 20,
};

/** Peak RSS of this process, in bytes. Linux only. */
export function peakRss() {
  try {
    const status = fs.readFileSync("/proc/self/status", "utf8");
    const match = /^VmHWM:\s+(\d+) kB$/m.exec(status);
    return match ? Number(match[1]) * 1024 : null;
  } catch {
    return null;
  }
}

/** Descriptors this process holds, right now. Linux only. */
export function openDescriptors() {
  try {
    return fs.readdirSync("/proc/self/fd").length;
  } catch {
    return null;
  }
}

/**
 * Descriptors this process holds once the releases in flight have drained.
 *
 * `close()` does not release the archive synchronously, on purpose: a read in
 * flight holds the cursor's lock for as long as it takes to walk the archive, so
 * `close()` raises a flag and leaves the release to the task on its way out.
 * That makes any single snapshot a reading of whatever happened to have
 * finished, and comparing two of them a race — one that lands on a loaded runner
 * long before it lands on an idle one.
 *
 * So the count is read until it stops moving. A leak stabilises too, at a higher
 * number, which is what the assertion is looking for.
 */
/**
 * Wait until this process is back down to `target` descriptors.
 *
 * Stability is not the same as quiescence: while a release is still pending the
 * count sits perfectly still at the wrong value, so a test that abandons a read
 * has to wait for the archive to actually come back rather than for the number
 * to stop moving. Otherwise the release lands in the middle of the next test and
 * takes its baseline with it.
 */
export async function waitForDescriptors(
  target,
  { intervalMs = 10, timeoutMs = 10_000 } = {},
) {
  if (target === null) {
    return null;
  }

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const now = openDescriptors();
    if (now === target) {
      return now;
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  return openDescriptors();
}

export async function settledDescriptors({
  stableForMs = 60,
  intervalMs = 10,
  timeoutMs = 5_000,
} = {}) {
  let last = openDescriptors();
  if (last === null) {
    return null;
  }

  const deadline = Date.now() + timeoutMs;
  let stableSince = Date.now();

  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, intervalMs));

    const now = openDescriptors();
    if (now !== last) {
      last = now;
      stableSince = Date.now();
      continue;
    }

    if (Date.now() - stableSince >= stableForMs) {
      return now;
    }
  }

  return last;
}

/**
 * Run a snippet in a child process and bring back what it wrote to stdout.
 *
 * Peak RSS is a property of a process, and `node --test` runs every test in one
 * of them: a figure taken after another test has just read 600 000 rows is that
 * test's figure, not this one's. Anything measuring memory, or needing an
 * environment set before startup, goes through here.
 */
export function runInChild(body, env = {}) {
  const script = `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import { listSheets, xlsxRows, xlsxWriteStream, XlsxError } from ${JSON.stringify(path.join(REPO_ROOT, "crates/xlsx-node/lib/index.js"))};

    const peakRss = () => {
      const m = /^VmHWM:\\s+(\\d+) kB$/m.exec(fs.readFileSync("/proc/self/status", "utf8"));
      return m ? Number(m[1]) * 1024 : null;
    };

    ${body}
  `;

  return execFileSync(
    process.execPath,
    ["--input-type=module", "--eval", script],
    { cwd: REPO_ROOT, encoding: "utf8", env: { ...process.env, ...env } },
  );
}

export async function collect(stream) {
  const rows = [];
  for await (const row of stream) {
    rows.push(row);
  }
  return rows;
}

/**
 * How a 10 ms timer fared while `work` ran.
 *
 * Returns the number of times it fired, the worst single gap, and how long the
 * work took. What separates a loop that kept its turns from one that was held is
 * the *cadence*, not the jitter: measured on a machine at load 50, a full read of
 * 600 000 rows gives a tick every 15 ms with a worst gap of 42 ms, while four
 * seconds of synchronous work on the main thread gives no ticks at all. Absolute
 * jitter, by contrast, is mostly a reading of what else the machine is doing.
 */
export async function timerCadence(work) {
  let ticks = 0;
  let worstGapMs = 0;
  let previous = performance.now();

  const started = performance.now();
  const timer = setInterval(() => {
    const now = performance.now();
    worstGapMs = Math.max(worstGapMs, now - previous - 10);
    previous = now;
    ticks += 1;
  }, 10);

  try {
    await work();
  } finally {
    clearInterval(timer);
  }

  return { ticks, worstGapMs, durationMs: performance.now() - started };
}
