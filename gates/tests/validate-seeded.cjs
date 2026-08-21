#!/usr/bin/env node
'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const ROOT = path.resolve(__dirname, '..', '..');
const LOCKED_CONFIG_BASE = '30dbe9d8311f1d2192774f04788f1107b6cbd631';
const { loadCard } = require('../lib/card.cjs');
const { canonicalJson, parseGateOutput } = require('../lib/contract.cjs');
const { writeArtifact } = require('../lib/artifact-writer.cjs');

const REQUIRED_TESTS = [
  't-card-schema-valid', 't-registry-closure-digests',
  't-ip-reference-scores-mm', 't-pv-reference-scores-mm', 't-cf-reference-scores-mm',
  't-ip-mutants-caught', 't-pv-mutants-caught', 't-cf-mutants-caught',
  't-fixture-digest-fails-closed', 't-dirhash-canonical',
  't-meta-mutant-passing-is-gate-defect', 't-summary-contract',
  't-gate-hash-drift-voids-validation',
];
const PACKS = Object.freeze({
  'install-payload': { inventory: [['IP-01', 'execution'], ['IP-02', 'structure'], ['IP-03', 'value'], ['IP-04', 'security'], ['IP-05', 'execution'], ['IP-06', 'structure']] },
  'provision-script': { inventory: [['PV-01', 'structure'], ['PV-02', 'security'], ['PV-03', 'relation'], ['PV-04', 'security'], ['PV-05', 'value'], ['PV-06', 'relation']] },
  'config-schema': { inventory: [['CF-01', 'execution'], ['CF-02', 'security'], ['CF-03', 'security'], ['CF-04', 'relation'], ['CF-05', 'value'], ['CF-06', 'structure']] },
});

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
function run(command, args, options = {}) {
  return spawnSync(command, args, { cwd: ROOT, encoding: 'utf8', timeout: 3_600_000,
    maxBuffer: 32 * 1024 * 1024, windowsHide: true, ...options });
}
function commandFailure(label, result) {
  const detail = `${result.error ? result.error.message : ''}\n${result.stderr || ''}\n${result.stdout || ''}`;
  throw new Error(`${label} failed (${result.status}): ${detail.slice(-8000)}`);
}
function lcg(seed) {
  let state = seed >>> 0;
  return () => { state = (Math.imul(1664525, state) + 1013904223) >>> 0; return state; };
}
function selectMutants(cards, seed) {
  const next = lcg(seed); const selected = {};
  for (const gateId of Object.keys(PACKS)) {
    const pool = cards[gateId].validation.mutants.map(({ id }) => id); const choice = [];
    while (choice.length < cards[gateId].validation.rotation_k) choice.push(pool.splice(next() % pool.length, 1)[0]);
    selected[gateId] = choice;
  }
  return selected;
}
function requireExhaustiveBattery() {
  const localTarget = path.join(ROOT, 'target');
  const build = run('cargo', ['build', '-p', 'nano-cli', '-p', 'nano-sandbox', '--bins'],
    { env: { ...process.env, CARGO_TARGET_DIR: localTarget } });
  if (build.status !== 0) commandFailure('gate-card prerequisite build', build);
  const testFiles = fs.readdirSync(path.join(ROOT, 'gates', 'tests'))
    .filter((name) => name.endsWith('.test.cjs')).sort()
    .map((name) => path.join('gates', 'tests', name));
  const result = run(process.execPath, ['--test-reporter=tap', '--test', ...testFiles]);
  if (result.status !== 0) commandFailure('exhaustive gate-card battery', result);
  const discovered = [...result.stdout.matchAll(/^\s*ok \d+ - (.+)$/gm)].map((match) => match[1]);
  for (const name of REQUIRED_TESTS) assert.equal(discovered.filter((candidate) => candidate === name).length, 1,
    `required test must be discovered exactly once: ${name}`);
  const test_inputs = Object.fromEntries(testFiles.map((file) =>
    [file.replaceAll('\\', '/'), sha256(fs.readFileSync(path.join(ROOT, file)))]));
  return { command: 'node --test gates/tests/', required_tests: REQUIRED_TESTS,
    test_inputs, status: 'green' };
}
function assertCaught(gateId, mutant, output) {
  const parsed = parseGateOutput(output, PACKS[gateId].inventory);
  if (parsed.failClosed) throw new Error(`GATE_DEFECT ${gateId} ${mutant.id}: ${parsed.failClosed}`);
  if (parsed.ok) throw new Error(`GATE_DEFECT ${gateId} ${mutant.id}`);
  const failures = parsed.failures.map(({ id }) => id);
  for (const expected of mutant.must_fail) if (!failures.includes(expected)) throw new Error(`${gateId} ${mutant.id} missed ${expected}`);
  return { failures, output_sha256: sha256(Buffer.from(output)) };
}
function runInstall(mutant) {
  const fixture = path.join(ROOT, 'gates', 'fixtures', 'install-payload', 'mutants', mutant.id);
  const result = run(process.execPath, [path.join(ROOT, 'gates', 'install-payload', 'gate.cjs'), fixture, mutant.fixture],
    { env: { ...process.env, NANO_WP4_TEMP_ROOT: path.join(ROOT, 'target', 'wp4-seeded-install') } });
  return { exit_code: result.status, ...assertCaught('install-payload', mutant, result.stdout) };
}
function runProvision(mutant) {
  const fixture = path.join(ROOT, 'gates', 'fixtures', 'provision-script', 'mutants', mutant.id, 'payload.json');
  const result = run(process.execPath, [path.join(ROOT, 'gates', 'provision-script', 'gate.cjs'), '--packet', fixture]);
  return { exit_code: result.status, ...assertCaught('provision-script', mutant, result.stdout) };
}
function runConfig(mutant, controlRoot) {
  const suffix = mutant.id.slice(-1); const worktree = path.join(controlRoot, `w${suffix}`);
  const target = path.join(controlRoot, `t${suffix}`);
  const patch = path.join(ROOT, 'gates', 'fixtures', 'config-schema', 'mutants', mutant.id, 'mutant.diff');
  let added = false;
  try {
    const add = run('git', ['worktree', 'add', '--detach', worktree, LOCKED_CONFIG_BASE]);
    if (add.status !== 0) commandFailure(`add ${mutant.id} worktree`, add); added = true;
    const apply = run('git', ['-C', worktree, 'apply', patch]);
    if (apply.status !== 0) commandFailure(`apply ${mutant.id}`, apply);
    const build = run('cargo', ['build', '--manifest-path', path.join(worktree, 'Cargo.toml'), '-p', 'nano-cli'],
      { env: { ...process.env, CARGO_TARGET_DIR: target } });
    if (build.status !== 0) commandFailure(`build ${mutant.id}`, build);
    const binary = path.join(target, 'debug', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano');
    const gated = run('bash', [path.join(ROOT, 'gates', 'config-schema', 'gate.sh'), path.join(ROOT, 'gates', 'fixtures', 'config-schema', 'probes')],
      { cwd: worktree, env: { ...process.env, NANO_CLI_BIN: binary, NANO_REPO_ROOT: worktree,
        NANO_GATE_ROOT: ROOT, NANO_CF_RUN_ROOT: controlRoot } });
    return { exit_code: gated.status, ...assertCaught('config-schema', mutant, gated.stdout) };
  } finally {
    if (added) run('git', ['worktree', 'remove', '--force', worktree]);
    fs.rmSync(target, { recursive: true, force: true });
    if (fs.existsSync(worktree) || fs.existsSync(target)) throw new Error(`cleanup residue ${mutant.id}`);
  }
}
function parseArgs(argv) {
  let seed; let output;
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === '--seed' && /^\d+$/.test(argv[i + 1] || '')) seed = Number(argv[i + 1]);
    else if (argv[i] === '--output' && argv[i + 1]) output = argv[i + 1];
    else throw new Error('USAGE: validate-seeded.cjs --seed <uint32> [--output <F-path>]');
  }
  if (!Number.isSafeInteger(seed) || seed < 0 || seed > 0xffffffff) throw new Error('SEED_INVALID');
  return { seed, output };
}
function confinedOutput(seed, supplied) {
  const evidenceRoot = path.resolve(process.env.NANO_WP4_EVIDENCE_ROOT || path.join(ROOT, 'target', 'wp4-seeded-evidence'));
  const output = path.resolve(supplied || path.join(evidenceRoot, `seed-${seed}.json`));
  if (path.parse(evidenceRoot).root.toUpperCase() !== 'F:\\' || (output !== evidenceRoot && !output.startsWith(`${evidenceRoot}${path.sep}`)))
    throw new Error('EVIDENCE_PATH_INVALID');
  fs.mkdirSync(evidenceRoot, { recursive: true });
  return output;
}
async function main(argv) {
  const { seed, output: supplied } = parseArgs(argv); const output = confinedOutput(seed, supplied);
  const cards = Object.fromEntries(Object.keys(PACKS).map((gateId) => [gateId, loadCard(path.join(ROOT, 'gates', gateId, 'card.md'))]));
  for (const card of Object.values(cards)) assert.equal(card.validation.rotation_k, 2);
  const selected = selectMutants(cards, seed); const exhaustive = requireExhaustiveBattery();
  const worktreesBefore = run('git', ['worktree', 'list', '--porcelain']);
  if (worktreesBefore.status !== 0) commandFailure('worktree inventory', worktreesBefore);
  const controlRoot = path.join(ROOT, 'target', `wp4-seeded-${process.pid}-${seed}`); fs.mkdirSync(controlRoot, { recursive: true });
  const observations = {};
  try {
    for (const gateId of Object.keys(PACKS)) {
      observations[gateId] = [];
      for (const id of selected[gateId]) {
        const mutant = cards[gateId].validation.mutants.find((candidate) => candidate.id === id);
        const observed = gateId === 'install-payload' ? runInstall(mutant)
          : gateId === 'provision-script' ? runProvision(mutant) : runConfig(mutant, controlRoot);
        observations[gateId].push({ fixture: mutant.fixture, id, must_fail: mutant.must_fail, observed });
      }
    }
  } finally { fs.rmSync(controlRoot, { recursive: true, force: true }); }
  const worktreesAfter = run('git', ['worktree', 'list', '--porcelain']);
  if (worktreesAfter.status !== 0) commandFailure('worktree inventory', worktreesAfter);
  const ownedRegistration = controlRoot.replaceAll('\\', '/').toLowerCase();
  assert.equal(worktreesAfter.stdout.replaceAll('\\', '/').toLowerCase().includes(ownedRegistration), false,
    'seeded worktree registration residue');
  assert.equal(fs.existsSync(controlRoot), false, 'seeded control root residue');
  const base = run('git', ['rev-parse', 'HEAD']); if (base.status !== 0) commandFailure('base SHA', base);
  const registryBytes = fs.readFileSync(path.join(ROOT, 'gates', 'registry.json')); const registry = JSON.parse(registryBytes);
  const inputs = Object.fromEntries(Object.keys(PACKS).map((gateId) => {
    const script = path.join(ROOT, 'gates', gateId, gateId === 'config-schema' ? 'gate.sh' : 'gate.cjs');
    return [gateId, { card_sha256: sha256(fs.readFileSync(path.join(ROOT, 'gates', gateId, 'card.md'))),
      closure_digest: registry.gates[gateId].closure_digest, reference: cards[gateId].validation.reference,
      script_sha256: sha256(fs.readFileSync(script)) }];
  }));
  const manifest = { base_sha: base.stdout.trim(), cleanup: { control_root_absent: true,
    owned_registrations_absent: true,
    worktree_inventory_after_sha256: sha256(Buffer.from(worktreesAfter.stdout)),
    worktree_inventory_before_sha256: sha256(Buffer.from(worktreesBefore.stdout)) }, exhaustive, inputs, observations,
    registry_sha256: sha256(registryBytes), rotation_k: 2, schema: 1, seed,
    validator_sha256: sha256(fs.readFileSync(__filename)) };
  const bytes = Buffer.from(canonicalJson(manifest)); if (bytes.length > 64 * 1024) throw new Error('EVIDENCE_TOO_LARGE');
  await writeArtifact(output, bytes);
  process.stdout.write(`${canonicalJson({ evidence: output, evidence_sha256: sha256(bytes), seed })}\n`);
}

module.exports = { lcg, selectMutants };
if (require.main === module) main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`seeded validation: ${error.message}\n`); process.exitCode = 1;
});
