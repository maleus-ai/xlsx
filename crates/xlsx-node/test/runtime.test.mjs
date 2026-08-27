import assert from "node:assert/strict";
import test from "node:test";

import { xlsxRows } from "../lib/index.js";
import {
  PERMISSIVE,
  fixture as fixtureFile,
  openDescriptors,
  runInChild,
  settledDescriptors,
  timerCadence,
  waitForDescriptors,
} from "./helpers.mjs";

/** Peak RSS allowed for a full read of 600 000 rows, Node's own baseline included. */
const RSS_CEILING = 150 * 1024 * 1024;

test("a long read does not hold the event loop", async () => {
  // The claim being tested is that parsing happens on the libuv threadpool.
  // What shows that is whether a 10 ms timer keeps getting its turns for the
  // whole read — not how much it drifts, which on a shared runner says more
  // about the neighbours than about this library.
  let rows = 0;

  const { ticks, worstGapMs, durationMs } = await timerCadence(async () => {
    const stream = xlsxRows(fixtureFile("large-600000"), {
      maxDecompressedBytes: 1 << 30,
      maxRows: 1_000_000,
    });

    for await (const _row of stream) {
      rows += 1;
    }
  });

  assert.equal(rows, 600_001);

  // Measured at load 50 on a 16-core machine: a tick every 15 ms, worst gap
  // 42 ms. The same read with the parsing on the main thread would give no
  // ticks at all, so these leave an order of magnitude of room.
  assert.ok(
    ticks > durationMs / 100,
    `the timer fired ${ticks} times over ${durationMs.toFixed(0)} ms — ` +
      "once every " +
      (durationMs / Math.max(ticks, 1)).toFixed(0) +
      " ms",
  );
  assert.ok(
    worstGapMs < 500,
    `the loop stopped for ${worstGapMs.toFixed(0)} ms during the read`,
  );
});

test("six hundred thousand rows read under the memory ceiling", () => {
  const output = runInChild(`
    let rows = 0;
    let dates = 0;

    for await (const row of xlsxRows(${JSON.stringify(fixtureFile("large-600000"))}, {
      maxDecompressedBytes: ${1 << 30},
      maxRows: 1_000_000,
    })) {
      rows += 1;
      // Column B is a date column: it must not come out as a serial number.
      if (typeof row[1] === "string" && row[1].endsWith("Z")) dates += 1;
    }

    process.stdout.write(JSON.stringify({ rows, dates, peakRss: peakRss() }));
  `);

  const { rows, dates, peakRss } = JSON.parse(output);

  assert.equal(rows, 600_001);
  assert.equal(dates, 600_000);

  if (peakRss === null) {
    // Only Linux hands out a high-water mark without a dependency, and an
    // assertion with nothing behind it is worse than none.
    return;
  }

  assert.ok(
    peakRss < RSS_CEILING,
    `peaked at ${(peakRss / 1024 / 1024).toFixed(0)} MB, ceiling is ${RSS_CEILING / 1024 / 1024} MB`,
  );
});

test("destroying a stream mid-read does not hold the event loop", async () => {
  // `_destroy` calls `close()` from the main thread. A read in flight holds the
  // cursor's lock for as long as it takes to walk the archive, so a `close()`
  // that waited for that lock would let any client stall the server by starting
  // an upload and cutting it.
  const before = openDescriptors();

  const stream = xlsxRows(fixtureFile("large-600000"), {
    maxDecompressedBytes: 1 << 30,
    maxRows: 1_000_000,
  });

  // Start the first pull, which opens the archive and walks it.
  stream.resume();
  await new Promise((resolve) => setImmediate(resolve));

  let worstLatenessMs = 0;
  let previous = performance.now();
  const timer = setInterval(() => {
    const now = performance.now();
    worstLatenessMs = Math.max(worstLatenessMs, now - previous - 10);
    previous = now;
  }, 10);

  const startedDestroy = performance.now();
  stream.destroy();
  const destroyMs = performance.now() - startedDestroy;

  // Long enough for the walk that was in flight to finish underneath.
  await new Promise((resolve) => setTimeout(resolve, 600));
  clearInterval(timer);

  assert.ok(destroyMs < 50, `destroy() itself took ${destroyMs.toFixed(1)} ms`);
  assert.ok(
    worstLatenessMs < 50,
    `the timer was held up by ${worstLatenessMs.toFixed(1)} ms while a read was abandoned`,
  );

  // The archive comes back whenever the walk that was in flight finishes, which
  // on a loaded machine can outlast this test. Waiting for it here keeps the
  // release out of the next test's measurements.
  await waitForDescriptors(before);
});

test("breaking out of the loop closes the archive", async () => {
  const file = fixtureFile("large-200000");

  // One read first: the process opens things lazily, and that is not a leak.
  for await (const _row of xlsxRows(file, PERMISSIVE)) {
    break;
  }

  // Snapshots wait for the releases in flight to drain. `close()` does not
  // release the archive synchronously — a read in flight holds the cursor's
  // lock, so the release falls to the task on its way out — and a snapshot taken
  // before that lands counts an archive already on its way out.
  const baseline = await settledDescriptors();
  if (baseline === null) {
    return;
  }

  // Every abandoned stream is kept referenced, and that is the whole point of
  // the test. Dropped on the floor they become garbage, and the collector
  // eventually releases their archives whether or not `close()` ever ran — which
  // would make this pass against a reader that leaks. Holding them means the
  // count can only come back down because something released it on purpose.
  const abandoned = [];

  for (let run = 0; run < 20; run += 1) {
    const rows = xlsxRows(file, PERMISSIVE);
    abandoned.push(rows);

    let seen = 0;
    for await (const _row of rows) {
      seen += 1;
      if (seen === 3) {
        // What a validation failure on row three actually looks like.
        break;
      }
    }
  }

  assert.equal(
    await settledDescriptors(),
    baseline,
    `${abandoned.length} abandoned reads left their archives open`,
  );
});

test("dates ignore the process timezone", () => {
  // Run in a child, because a timezone has to be set before the process starts
  // to be worth testing at all.
  const body = `
    const rows = [];
    for await (const row of xlsxRows(${JSON.stringify(fixtureFile("types"))}, {
      maxDecompressedBytes: 1048576,
      maxRows: 10,
    })) {
      rows.push(row);
    }
    process.stdout.write(JSON.stringify(rows[0].slice(7, 9)));
  `;

  const kiritimati = runInChild(body, { TZ: "Pacific/Kiritimati" }); // UTC+14
  const niue = runInChild(body, { TZ: "Pacific/Niue" }); // UTC-11

  assert.equal(kiritimati, niue);
  assert.deepEqual(JSON.parse(kiritimati), [
    "2024-03-25T00:00:00.000Z",
    "2024-03-25T12:00:00.000Z",
  ]);
});
