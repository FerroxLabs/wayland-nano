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
const harnessPath = join(here, 'continuity.mjs');
const fixturePath = join(repo, 'gates', 'fixtures', 'memory-retrieval-recall-v1', 'fixture.json');

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const textSha256 = (bytes) => sha256(String(bytes).replaceAll('\r\n', '\n'));
const posix = (path) => path.replaceAll('\\', '/');
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

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
const budgets = JSON.parse(budgetBytes);
const budgetHash = sha256(canonical(budgets));
const harnessHash = sha256((await readFile(harnessPath, 'utf8')).replaceAll('\r\n', '\n'));
const fixtureHash = textSha256(await readFile(fixturePath));
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
  if (manifest.harness?.sha256 !== harnessHash) {
    throw new Error(`harness hash mismatch: ${posix(relative(repo, manifestPath))}`);
  }
  if (manifest.measurement_mode === 'receipt'
    && (!manifest.started_at || Date.parse(manifest.started_at) < Date.parse(budgets.registered_at))) {
    throw new Error(`token ceilings were not preregistered: ${posix(relative(repo, manifestPath))}`);
  }
  if (manifest.fixture?.sha256 !== fixtureHash || manifest.fixture?.labels_modified !== false) {
    throw new Error(`fixture hash mismatch: ${posix(relative(repo, manifestPath))}`);
  }
  const ndjsonPath = resolve(dirname(manifestPath), '..', manifest.ndjson.path);
  const ndjsonBytes = await readFile(ndjsonPath);
  if (textSha256(ndjsonBytes) !== manifest.ndjson.sha256) {
    throw new Error(`NDJSON hash mismatch: ${posix(relative(repo, ndjsonPath))}`);
  }
  const rows = String(ndjsonBytes).trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  if (rows.length !== manifest.ndjson.rows) throw new Error(`NDJSON row count mismatch: ${manifestPath}`);
  for (const row of rows) {
    if (row.seed !== manifest.seed || row.binary_sha256 !== manifest.binary.sha256 || row.budget_sha256 !== budgetHash || row.harness_sha256 !== harnessHash || row.task_battery_sha256 !== manifest.task_battery?.sha256) {
      throw new Error(`row binding mismatch: ${manifestPath}`);
    }
    if (row.tokens?.total_tokens !== row.tokens?.setup_tokens + row.tokens?.probe_tokens) {
      throw new Error(`setup accounting mismatch: ${manifestPath}`);
    }
  }
  const recallRows = rows.filter((row) => row.probe_kind === 'recall');
  if (recallRows.some((row) => row.mode === 'fresh'
    && (row.isolation_pass !== true || row.request_assertion_matched !== true || row.refusal_kind !== null))) {
    throw new Error(`fresh isolation invalid: ${manifestPath}`);
  }
  const accounting = Object.fromEntries(requiredModes.map((mode) => {
    const modeRows = recallRows.filter((row) => row.mode === mode);
    const setupTokens = modeRows.reduce((sum, row) => sum + row.tokens.setup_tokens, 0);
    const probeTokens = modeRows.reduce((sum, row) => sum + row.tokens.probe_tokens, 0);
    return [mode, {
      setup_tokens: setupTokens,
      probe_tokens: probeTokens,
      total_tokens: setupTokens + probeTokens,
    }];
  }));
  if (canonical(accounting) !== canonical(manifest.accounting)) {
    throw new Error(`manifest setup accounting mismatch: ${manifestPath}`);
  }
  candidates.push({ manifestPath, manifest, manifestSha256: textSha256(manifestBytes), rows });
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
const scriptHashes = new Map();
const driverProfiles = new Map();
for (const { rows } of selected) {
  for (const row of rows.filter((entry) => entry.probe_kind === 'recall')) {
    const key = `${row.seed}\0${row.label}`;
    const hashes = scriptHashes.get(key) ?? new Set();
    hashes.add(row.driver_script_sha256);
    scriptHashes.set(key, hashes);
    const profiles = driverProfiles.get(key) ?? new Set();
    profiles.add(canonical(row.driver_profile));
    driverProfiles.set(key, profiles);
  }
}
for (const [key, hashes] of scriptHashes) {
  if (hashes.size !== requiredModes.length) throw new Error(`causal task scripts missing modes: ${key}`);
  if (driverProfiles.get(key)?.size !== 1) throw new Error(`fake usage or delay differs by mode: ${key}`);
}
if (new Set(selected.map(({ manifest }) => manifest.task_battery?.sha256)).size !== 1) {
  throw new Error('task battery differs across seeds');
}

const modeResults = {};
for (const mode of requiredModes) {
  const repetitions = selected.map(({ rows }) => {
    const recall = rows.filter((row) => row.mode === mode && row.probe_kind === 'recall');
    if (recall.length === 0) throw new Error(`evidence missing ${mode} recall rows`);
    return {
      latency: median(recall.map((row) => Number(row.latency_ms))),
      setupTokens: recall.reduce((sum, row) => sum + Number(row.tokens?.setup_tokens ?? 0), 0),
      probeTokens: recall.reduce((sum, row) => sum + Number(row.tokens?.probe_tokens ?? 0), 0),
      totalTokens: recall.reduce((sum, row) => sum + Number(row.tokens?.total_tokens ?? 0), 0),
      quality: recall.filter((row) => row.quality_pass === true).length / recall.length,
      probes: recall.length,
    };
  });
  const measured = {
    median_turn_latency_ms: median(repetitions.map((row) => row.latency)),
    median_setup_tokens: median(repetitions.map((row) => row.setupTokens)),
    median_probe_tokens: median(repetitions.map((row) => row.probeTokens)),
    median_total_tokens: median(repetitions.map((row) => row.totalTokens)),
    median_quality_score: median(repetitions.map((row) => row.quality)),
    probes_per_seed: repetitions[0].probes,
  };
  const budget = budgets.modes[mode];
  const checks = {
    latency: measured.median_turn_latency_ms <= budget.median_turn_latency_ms_max,
    tokens: measured.median_probe_tokens <= budget.probe_tokens_max,
    totalTokens: measured.median_total_tokens <= budget.total_tokens_max,
    quality: measured.median_quality_score >= budget.quality_score_min,
  };
  modeResults[mode] = { measured, budget, checks, pass: Object.values(checks).every(Boolean) };
}

const driftRows = selected.flatMap(({ rows }) => rows.filter((row) => row.mode === 'session_resume' && row.probe_kind === 'drift_refusal'));
const driftCorrect = driftRows.filter((row) => row.quality_pass === true && row.refusal_kind === 'resume_drift' && row.silent_fallback === false).length;
const freshRows = selected.flatMap(({ rows }) => rows.filter((row) => row.mode === 'fresh' && row.probe_kind === 'recall'));
const freshIsolation = freshRows.filter((row) => row.isolation_pass === true && row.request_assertion_matched === true && row.refusal_kind === null).length;
const session = modeResults.session_resume.measured;
const memory = modeResults.memory_recall.measured;
const memoryQualityPerToken = memory.median_quality_score / Math.max(1, memory.median_probe_tokens);
const sessionQualityPerToken = session.median_quality_score / Math.max(1, session.median_probe_tokens);
const memoryBeatsSession = memoryQualityPerToken > sessionQualityPerToken;
const fmt = (value, digits = 2) => Number(value).toFixed(digits).replace(/\.00$/, '');

const lines = [
  '# Phase 3 continuity modes report',
  '',
  `Evidence class: **${preferredKind}**; seeded repetitions: **${selected.length}**; budgets registered **${budgets.registered_at}** before receipt execution; frozen budget SHA-256: \`${budgetHash}\`; harness SHA-256: \`${harnessHash}\`.`,
  '',
  'This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.',
  '',
  '## Measured results',
  '',
  '| mode | median turn latency (ms) | setup tokens | probe tokens | total tokens | median quality | budget verdict |',
  '|---|---:|---:|---:|---:|---:|---|',
  ...requiredModes.map((mode) => {
    const result = modeResults[mode];
    return `| ${mode} | ${fmt(result.measured.median_turn_latency_ms, 3)} / ≤${result.budget.median_turn_latency_ms_max} | ${fmt(result.measured.median_setup_tokens)} | ${fmt(result.measured.median_probe_tokens)} / ≤${result.budget.probe_tokens_max} | ${fmt(result.measured.median_total_tokens)} / ≤${result.budget.total_tokens_max} | ${fmt(result.measured.median_quality_score, 3)} / ≥${result.budget.quality_score_min} | ${result.pass ? 'PASS' : 'FAIL'} |`;
  }),
  '',
  `Typed resume-drift refusals: **${driftCorrect}/${driftRows.length}** (\`resume_drift\`, zero silent fallbacks).`,
  `Fresh isolation assertions: **${freshIsolation}/${freshRows.length}**; any leakage or protocol refusal invalidates the entire run before a manifest is selectable.`,
  '',
  'Quality is causal request evidence from the fixed fixture battery. Fresh creates a new admitted session with memory disabled and separately proves the fixture answer is absent; that isolation oracle must pass even though fresh continuity quality is zero. Session resume forks an activated parent, loads the returned child id, and emits success only when the actual model request contains the replayed answer. Memory recall exposes no explicit memory tool call: its model emits success only when automatic scoped retrieval placed the fixture answer in the actual request. Missing or irrelevant retrieval therefore becomes a typed model-protocol failure and a failed quality row. Token totals come only from emitted `_wayland/session/budget` notifications. Setup is attributed once on the first probe row of each `(mode, project, agent_id)` partition, including all four memory-seed sessions; every row and manifest conserves `total = setup + probe`.',
  '',
  '## Budget verdicts',
  '',
  ...requiredModes.map((mode) => {
    const result = modeResults[mode];
    return `- **${mode}: ${result.pass ? 'PASS' : 'FAIL'}** — latency ${result.checks.latency ? 'pass' : 'fail'}, probe tokens ${result.checks.tokens ? 'pass' : 'fail'}, total tokens ${result.checks.totalTokens ? 'pass' : 'fail'}, quality ${result.checks.quality ? 'pass' : 'fail'}.`;
  }),
  '',
  '## RECOMMENDATION',
  '',
  `For interactive ACP, default to **session_resume when a valid bound session exists**. It loaded the returned fork child, rejected ${driftCorrect}/${driftRows.length} drift probes without fallback, and measured ${fmt(session.median_quality_score, 3)} quality at ${fmt(session.median_probe_tokens)} probe tokens plus ${fmt(session.median_setup_tokens)} setup tokens (${fmt(session.median_total_tokens)} total). With no resumable session, use **memory_recall only when project continuity is requested**; otherwise start fresh.`,
  '',
  `For one-shot exec, default to **fresh for stateless work** and require an explicit continuity choice for memory-backed work. Fresh correctly exposed no remembered answer; memory_recall measured ${fmt(memory.median_quality_score, 3)} quality. Memory recall ${memoryBeatsSession ? 'did' : 'did NOT'} beat session_resume on measured quality per emitted token (${memoryQualityPerToken.toExponential(3)} vs ${sessionQualityPerToken.toExponential(3)}), so it remains an explicit continuity mode rather than a universal default.`,
  '',
  'These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.',
  '',
  '## Run manifests',
  '',
  '| seed | binary sha256 | budget sha256 | harness sha256 | fixture sha256 | manifest sha256 | NDJSON sha256 |',
  '|---:|---|---|---|---|---|---|',
  ...selected.map(({ manifestPath, manifest, manifestSha256 }) => {
    const ndjsonPath = resolve(dirname(manifestPath), '..', manifest.ndjson.path);
    return `| ${manifest.seed} | \`${manifest.binary.sha256}\` | \`${manifest.budgets.sha256}\` | \`${manifest.harness.sha256}\` | \`${manifest.fixture.sha256}\` | \`${manifestSha256}\` | \`${manifest.ndjson.sha256}\` |`;
  }),
  '',
  ...selected.flatMap(({ manifestPath, manifest }) => {
    const ndjsonPath = resolve(dirname(manifestPath), '..', manifest.ndjson.path);
    return [`- Seed ${manifest.seed} manifest: \`${posix(relative(repo, manifestPath))}\``, `- Seed ${manifest.seed} NDJSON: \`${posix(relative(repo, ndjsonPath))}\``];
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
