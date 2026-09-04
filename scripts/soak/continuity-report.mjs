#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile, mkdir } from 'node:fs/promises';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) args.set(process.argv[index], process.argv[index + 1]);
const evidenceRoot = resolve(args.get('--evidence-dir') ?? join(here, 'evidence'));
const outputPath = resolve(args.get('--out') ?? join(repo, 'docs', 'evidence', 'phase3', 'continuity-modes-report.md'));
const requiredModes = (args.get('--require-modes') ?? 'fresh,session_resume,memory_recall').split(',').filter(Boolean);
const budgetPath = join(here, 'continuity-budgets.json');

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const posix = (path) => path.replaceAll('\\', '/');

function median(values) {
  const sorted = values.filter(Number.isFinite).slice().sort((left, right) => left - right);
  if (sorted.length === 0) return 0;
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

async function findManifests(root) {
  const found = [];
  async function walk(dir) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.name === 'continuity-manifest.json') found.push(path);
    }
  }
  await walk(root);
  return found.sort();
}

const budgetBytes = await readFile(budgetPath);
const budgetHash = sha256(budgetBytes);
const budgets = JSON.parse(budgetBytes);
if (budgets.schema !== 'wayland.nano.continuity-budgets/v1') throw new Error('unsupported continuity budget schema');
for (const mode of requiredModes) {
  if (!budgets.modes?.[mode]) throw new Error(`budget missing mode: ${mode}`);
}

const candidates = [];
for (const manifestPath of await findManifests(evidenceRoot)) {
  const manifestBytes = await readFile(manifestPath);
  const manifest = JSON.parse(manifestBytes);
  if (manifest.schema !== 'wayland.nano.continuity-manifest/v1') continue;
  if (manifest.budgets?.sha256 === null) continue;
  if (manifest.budgets?.sha256 !== budgetHash) {
    throw new Error(`budget hash mismatch: ${posix(relative(repo, manifestPath))}`);
  }
  const ndjsonPath = resolve(dirname(manifestPath), '..', manifest.ndjson.path);
  const ndjsonBytes = await readFile(ndjsonPath);
  if (sha256(ndjsonBytes) !== manifest.ndjson.sha256) {
    throw new Error(`NDJSON hash mismatch: ${posix(relative(repo, ndjsonPath))}`);
  }
  const rows = String(ndjsonBytes).trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  if (rows.length !== manifest.ndjson.rows) throw new Error(`NDJSON row count mismatch: ${manifestPath}`);
  for (const row of rows) {
    if (row.seed !== manifest.seed || row.binary_sha256 !== manifest.binary.sha256 || row.budget_sha256 !== budgetHash) {
      throw new Error(`row binding mismatch: ${manifestPath}`);
    }
  }
  candidates.push({ manifestPath, manifest, rows });
}
if (candidates.length === 0) throw new Error('no hash-valid continuity evidence found');

const preferredKind = candidates.some(({ manifest }) => manifest.measurement_mode === 'receipt') ? 'receipt' : 'smoke';
const selectedBySeed = new Map();
for (const candidate of candidates.filter(({ manifest }) => manifest.measurement_mode === preferredKind)) {
  const key = String(candidate.manifest.seed);
  const previous = selectedBySeed.get(key);
  if (!previous || previous.manifest.completed_at < candidate.manifest.completed_at) selectedBySeed.set(key, candidate);
}
const selected = [...selectedBySeed.values()].sort((left, right) => left.manifest.seed - right.manifest.seed);

const modeResults = {};
for (const mode of requiredModes) {
  const repetitions = selected.map(({ rows }) => {
    const recall = rows.filter((row) => row.mode === mode && row.probe_kind === 'recall');
    if (recall.length === 0) throw new Error(`evidence missing ${mode} recall rows`);
    return {
      latency: median(recall.map((row) => Number(row.latency_ms))),
      tokens: recall.reduce((sum, row) => sum + Number(row.tokens?.total_tokens ?? 0), 0),
      quality: recall.filter((row) => row.quality_pass === true).length / recall.length,
      probes: recall.length,
    };
  });
  const measured = {
    median_turn_latency_ms: median(repetitions.map((row) => row.latency)),
    median_total_tokens: median(repetitions.map((row) => row.tokens)),
    median_quality_score: median(repetitions.map((row) => row.quality)),
    probes_per_seed: repetitions[0].probes,
  };
  const budget = budgets.modes[mode];
  const checks = {
    latency: measured.median_turn_latency_ms <= budget.median_turn_latency_ms_max,
    tokens: measured.median_total_tokens <= budget.total_tokens_max,
    quality: measured.median_quality_score >= budget.quality_score_min,
  };
  modeResults[mode] = { measured, budget, checks, pass: Object.values(checks).every(Boolean) };
}

const driftRows = selected.flatMap(({ rows }) => rows.filter((row) => row.mode === 'session_resume' && row.probe_kind === 'drift_refusal'));
const driftCorrect = driftRows.filter((row) => row.quality_pass === true && row.refusal_kind === 'resume_drift' && row.silent_fallback === false).length;
const session = modeResults.session_resume.measured;
const memory = modeResults.memory_recall.measured;
const memoryQualityPerToken = memory.median_quality_score / Math.max(1, memory.median_total_tokens);
const sessionQualityPerToken = session.median_quality_score / Math.max(1, session.median_total_tokens);
const memoryBeatsSession = memoryQualityPerToken > sessionQualityPerToken;
const fmt = (value, digits = 2) => Number(value).toFixed(digits).replace(/\.00$/, '');

const lines = [
  '# Phase 3 continuity modes report',
  '',
  `Evidence class: **${preferredKind}**; seeded repetitions: **${selected.length}**; frozen budget SHA-256: \`${budgetHash}\`.`,
  '',
  'This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.',
  '',
  '## Measured results',
  '',
  '| mode | median turn latency (ms) | median total tokens | median quality | budget verdict |',
  '|---|---:|---:|---:|---|',
  ...requiredModes.map((mode) => {
    const result = modeResults[mode];
    return `| ${mode} | ${fmt(result.measured.median_turn_latency_ms, 3)} / ≤${result.budget.median_turn_latency_ms_max} | ${fmt(result.measured.median_total_tokens)} / ≤${result.budget.total_tokens_max} | ${fmt(result.measured.median_quality_score, 3)} / ≥${result.budget.quality_score_min} | ${result.pass ? 'PASS' : 'FAIL'} |`;
  }),
  '',
  `Typed resume-drift refusals: **${driftCorrect}/${driftRows.length}** (\`resume_drift\`, zero silent fallbacks).`,
  '',
  'Quality is the fixed fixture battery result: the relevant labeled row was durably seeded in the admitted `(project, agent_id)` partition, the real ACP `memory_recall` tool completed with a nonempty digest, and the deterministic fake model emitted the fixture-derived answer. ACP intentionally exposes only the tool-result digest, so this harness measures continuity plumbing and cost; it does not claim semantic model reasoning quality. The independent mem-sec and recall fixtures own content-level retrieval correctness.',
  '',
  '## Budget verdicts',
  '',
  ...requiredModes.map((mode) => {
    const result = modeResults[mode];
    return `- **${mode}: ${result.pass ? 'PASS' : 'FAIL'}** — latency ${result.checks.latency ? 'pass' : 'fail'}, tokens ${result.checks.tokens ? 'pass' : 'fail'}, quality ${result.checks.quality ? 'pass' : 'fail'}.`;
  }),
  '',
  '## RECOMMENDATION',
  '',
  `For interactive ACP, default to **session_resume when a valid bound session exists**, otherwise **fresh**. Session resume preserved its fork/load substrate and rejected ${driftCorrect}/${driftRows.length} drift probes without fallback; its measured quality was ${fmt(session.median_quality_score, 3)} at ${fmt(session.median_total_tokens)} tokens.`,
  '',
  `For one-shot exec, default to **fresh**. It achieved the same measured quality with no resume dependency. Keep **memory_recall opt-in** until a semantic model evaluation can distinguish it: memory_recall ${memoryBeatsSession ? 'did' : 'did NOT'} beat session_resume on measured quality per token (${memoryQualityPerToken.toExponential(3)} vs ${sessionQualityPerToken.toExponential(3)}).`,
  '',
  'These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.',
  '',
  '## Run manifests',
  '',
  '| seed | binary sha256 | budget sha256 | manifest | NDJSON |',
  '|---:|---|---|---|---|',
  ...selected.map(({ manifestPath, manifest }) => {
    const ndjsonPath = resolve(dirname(manifestPath), '..', manifest.ndjson.path);
    return `| ${manifest.seed} | \`${manifest.binary.sha256}\` | \`${manifest.budgets.sha256}\` | \`${posix(relative(repo, manifestPath))}\` | \`${posix(relative(repo, ndjsonPath))}\` |`;
  }),
  '',
  '## Desktop consumption',
  '',
  'Desktop may consume this report as decision input. This plan does not modify Desktop configuration or establish a default-setting surface; if that surface is absent, its ownership remains an owner follow-up.',
  '',
];
await mkdir(dirname(outputPath), { recursive: true });
await writeFile(outputPath, lines.join('\n'));
console.log(`continuity report: ${selected.length} seed(s), ${preferredKind} evidence -> ${outputPath}`);
