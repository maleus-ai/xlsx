#!/usr/bin/env node
//
// The measurements the README quotes. One child process per figure, because
// peak RSS is a property of a process.
//
//   node scripts/bench.mjs            # the whole table
//   node scripts/bench.mjs batch      # just the batch-size sweep

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const FACADE = path.join(REPO_ROOT, "crates/xlsx-node/lib/index.js");

function fixture(name) {
  const file = path.join(REPO_ROOT, "fixtures/out", `${name}.xlsx`);
  if (!fs.existsSync(file)) {
    execFileSync("node", ["fixtures/generate.mjs", name], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });
  }
  return file;
}

/** Run one read in a child and bring back its own figures. */
function measure({ file, sheet, batchSize, listOnly = false }) {
  const script = `
    import fs from "node:fs";
    import { listSheets, xlsxRows } from ${JSON.stringify(FACADE)};

    const peak = () => {
      const m = /^VmHWM:\\s+(\\d+) kB$/m.exec(fs.readFileSync("/proc/self/status", "utf8"));
      return m ? Number(m[1]) * 1024 : null;
    };

    const options = {
      maxDecompressedBytes: ${1 << 30},
      maxRows: 2_000_000,
      ${sheet ? `sheet: ${JSON.stringify(sheet)},` : ""}
      ${batchSize ? `batchSize: ${batchSize},` : ""}
    };

    const started = performance.now();
    let rows = 0;

    if (${listOnly}) {
      await listSheets(${JSON.stringify(file)}, options);
    } else {
      for await (const row of xlsxRows(${JSON.stringify(file)}, options)) {
        rows += 1;
      }
    }

    process.stdout.write(JSON.stringify({
      rows,
      elapsedMs: performance.now() - started,
      peakRss: peak(),
    }));
  `;

  const out = execFileSync(
    process.execPath,
    ["--input-type=module", "--eval", script],
    { encoding: "utf8", cwd: REPO_ROOT },
  );
  return JSON.parse(out);
}

const mb = (bytes) => (bytes / 1024 / 1024).toFixed(0);
const ms = (value) => value.toFixed(0);

function row(label, result) {
  console.log(
    `${label.padEnd(28)} ${String(result.rows).padStart(8)} rows  ` +
      `${ms(result.elapsedMs).padStart(7)} ms  ${mb(result.peakRss).padStart(5)} MB`,
  );
}

function throughput() {
  console.log("\n## Reading\n");
  for (const rows of [200_000, 600_000]) {
    row(`large-${rows}`, measure({ file: fixture(`large-${rows}`) }));
  }
  console.log("\n## Listing sheets\n");
  row(
    "large-600000 (list only)",
    measure({ file: fixture("large-600000"), listOnly: true }),
  );
}

function batchSweep() {
  // Two runs each, best kept: the spread between runs is a few per cent, which
  // is the same order as the difference the batch size makes at the top of the
  // range. Reporting a single run would read a trend into noise.
  console.log("\n## Batch size, on 600 000 rows (best of two)\n");
  const file = fixture("large-600000");
  for (const batchSize of [1, 10, 100, 1_000, 10_000]) {
    const runs = [measure({ file, batchSize }), measure({ file, batchSize })];
    const best = runs.reduce((a, b) => (a.elapsedMs <= b.elapsedMs ? a : b));
    row(`batchSize ${batchSize}`, best);
  }
}

const what = process.argv[2] ?? "all";
if (what === "all" || what === "read") throughput();
if (what === "all" || what === "batch") batchSweep();
