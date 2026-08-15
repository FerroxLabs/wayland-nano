#!/usr/bin/env node
// Self-test for the B1 budget oracle (scripts/soak/budget-eval.mjs).
// Synthesizes sample series with known behaviour and asserts the verdict.
// Dependency-free: node scripts/soak/test-budgets.mjs
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { baselineMedians, evaluateB1, slopePerHour } from './budget-eval.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const budgets = JSON.parse(await readFile(join(here, 'budgets.json'), 'utf8'));

const start = Date.parse('2026-08-15T00:00:00Z');
const series = (startBytes, slopeBytesPerHour, hours, stepMinutes = 10) => {
  const out = [];
  for (let minutes = 0; minutes <= hours * 60; minutes += stepMinutes) {
    out.push({
      at: new Date(start + minutes * 60000).toISOString(),
      pid: 1,
      privateWorkingSetBytes: Math.round(startBytes + (slopeBytesPerHour * minutes) / 60),
      workingSetBytes: 0, handles: 100, threads: 20, openFds: null, nanoHomeBytes: 0, descendants: [],
    });
  }
  return out;
};

// 1. Genuine leak: +30 MB/h for 8h from 200 MB. Slope (30 MB/h > 16 MiB/h)
//    AND endRatio (~440 MB / ~222 MB baseline ≈ 2.0 > 1.25) breach; the
//    absolute ceiling (1.5 GiB) does not. Must FAIL — this is the case the
//    old harness passed.
const leak = evaluateB1(series(200_000_000, 30_000_000, 8), budgets.B1);
assert.equal(leak.status, 'FAIL', `leak series must FAIL: ${JSON.stringify(leak.detail)}`);
assert.equal(leak.detail.checks.slope, false, 'leak slope must breach');
assert.equal(leak.detail.checks.endRatio, false, 'leak endRatio must breach');
assert.equal(leak.detail.checks.absolute, true, 'leak stays under the absolute ceiling');

// 2. Flat series: no growth, well under every cap. Must PASS.
const flat = evaluateB1(series(200_000_000, 0, 8), budgets.B1);
assert.equal(flat.status, 'PASS', `flat series must PASS: ${JSON.stringify(flat.detail)}`);

// 3. Absolute-ceiling breach only: flat but above 1.5 GiB. Must FAIL.
const huge = evaluateB1(series(1_700_000_000, 0, 8), budgets.B1);
assert.equal(huge.status, 'FAIL', 'over-ceiling series must FAIL');
assert.equal(huge.detail.checks.absolute, false);
assert.equal(huge.detail.checks.endRatio, true, 'flat series keeps endRatio 1');
assert.equal(huge.detail.checks.slope, true, 'flat series keeps slope 0');

// 4. Slope is measured back exactly, and a slope exactly at the cap passes.
assert.equal(Math.round(slopePerHour(series(100_000_000, 30_000_000, 8), 'privateWorkingSetBytes')), 30_000_000);
const atCap = evaluateB1(series(100_000_000, budgets.B1.slopeBytesPerHour, 1), budgets.B1);
assert.equal(atCap.detail.checks.slope, true, 'slope exactly at the cap passes');

// 5. Baseline medians come from the first 10 samples, not sample[0].
const drifting = series(100, 1000, 2);
const medians = baselineMedians(drifting);
assert.ok(medians.privateWorkingSetBytes > drifting[0].privateWorkingSetBytes, 'median is not just sample[0]');
assert.ok(medians.privateWorkingSetBytes < drifting[9].privateWorkingSetBytes, 'median stays inside the baseline window');
assert.equal(baselineMedians(series(200_000_000, 0, 1)).privateWorkingSetBytes, 200_000_000);

console.log('test-budgets: all 5 B1 oracle checks green');
