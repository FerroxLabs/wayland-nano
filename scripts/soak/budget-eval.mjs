// B1 budget oracle for the S10 soak harness.
// Extracted from soak.mjs so the evaluation is unit-testable without running
// a soak (soak.mjs executes the run at import time). Dependency-free.

const BASELINE_WINDOW = 10;

export function median(values) {
  const sorted = values.filter(Number.isFinite).slice().sort((a, b) => a - b);
  if (sorted.length === 0) return 0;
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

// Median of every numeric sample field over the first BASELINE_WINDOW samples.
// S10-SOAK-DESIGN §4.1 anchors budgets to measured baselines; the harness
// approximates the hour-1 baseline with the first 10 one-minute samples.
export function baselineMedians(samples, windowSize = BASELINE_WINDOW) {
  const window = samples.slice(0, windowSize);
  const medians = {};
  for (const field of new Set(window.flatMap((sample) => Object.keys(sample ?? {})))) {
    const values = window.map((sample) => Number(sample?.[field])).filter(Number.isFinite);
    if (values.length > 0) medians[field] = median(values);
  }
  return medians;
}

// Least-squares slope of `field` over the sample series, in units per hour,
// using the samples' own `at` timestamps.
export function slopePerHour(samples, field) {
  const points = samples
    .map((sample) => ({ t: Date.parse(sample?.at), value: Number(sample?.[field]) }))
    .filter((point) => Number.isFinite(point.t) && Number.isFinite(point.value));
  if (points.length < 2) return 0;
  const t0 = points[0].t;
  const n = points.length;
  let sx = 0; let sy = 0; let sxy = 0; let sxx = 0;
  for (const { t, value } of points) {
    const x = (t - t0) / 3600000;
    sx += x; sy += value; sxy += x * value; sxx += x * x;
  }
  const denominator = n * sxx - sx * sx;
  if (denominator === 0) return 0;
  return (n * sxy - sx * sy) / denominator;
}

// B1 binds three ways (S10-SOAK-DESIGN §4.2): absolute ceiling, end ratio vs
// the baseline window, and leak slope. All three are enforced; any breach
// fails the budget. The detail carries every measured value and threshold so
// the manifest tells the full B1 story.
export function evaluateB1(samples, budget) {
  const values = samples.map((sample) => Number(sample?.privateWorkingSetBytes ?? 0));
  const peakBytes = Math.max(0, ...values);
  const finalBytes = values.length > 0 ? values[values.length - 1] : 0;
  const baselineBytes = median(values.slice(0, BASELINE_WINDOW));
  const endRatio = baselineBytes > 0 ? finalBytes / baselineBytes : 1;
  const slopeBytesPerHour = slopePerHour(samples, 'privateWorkingSetBytes');
  const checks = {
    absolute: peakBytes <= budget.absoluteBytes,
    endRatio: endRatio <= budget.endRatio,
    slope: slopeBytesPerHour <= budget.slopeBytesPerHour,
  };
  return {
    status: Object.values(checks).every(Boolean) ? 'PASS' : 'FAIL',
    detail: {
      peakBytes, absoluteBytes: budget.absoluteBytes,
      baselineBytes, finalBytes,
      endRatio: Number(endRatio.toFixed(4)), endRatioMax: budget.endRatio,
      slopeBytesPerHour: Math.round(slopeBytesPerHour), slopeMaxBytesPerHour: budget.slopeBytesPerHour,
      checks,
    },
  };
}
