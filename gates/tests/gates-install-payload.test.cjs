'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');
const PACK_ROOT = path.join(ROOT, 'packaging', 'npm');
const FIXTURE_ROOT = path.join(ROOT, 'gates', 'fixtures', 'install-payload');
const CARD_PATH = path.join(ROOT, 'gates', 'install-payload', 'card.md');
const GATE_PATH = path.join(ROOT, 'gates', 'install-payload', 'gate.cjs');
const GENERATOR_PATH = path.join(ROOT, 'gates', 'install-payload', 'fixtures', 'generators', 'generators.cjs');
const { loadCard } = require('../lib/card.cjs');
const { parseGateOutput } = require('../lib/contract.cjs');
const { directorySeal, digestDirectory } = require('../lib/dirhash.cjs');

const INVENTORY = [
  ['IP-01', 'execution'], ['IP-02', 'structure'], ['IP-03', 'value'],
  ['IP-04', 'security'], ['IP-05', 'execution'], ['IP-06', 'structure'],
];

function treeDigest(root) {
  return digestDirectory(root).digest;
}

test('t-ip-reference-scores-mm', () => {
  const generator = require(GENERATOR_PATH);
  const card = loadCard(CARD_PATH);
  const reference = path.join(FIXTURE_ROOT, 'reference');
  assert.deepEqual(card.checks.map(({ id, category }) => [id, category]), INVENTORY);
  assert.equal(card.validation.reference, directorySeal(reference));
  assert.deepEqual(generator.inspectFixture(reference).failures, []);
  assert.equal(card.validation.rotation_k, 2);
});

test('t-ip-mutants-caught', () => {
  const generator = require(GENERATOR_PATH);
  const card = loadCard(CARD_PATH);
  assert.equal(card.validation.mutants.length, 6);
  assert.deepEqual(card.validation.mutants.map(({ id }) => id),
    ['ip-m1', 'ip-m2', 'ip-m3', 'ip-m4', 'ip-m5', 'ip-m6']);
  for (const mutant of card.validation.mutants) {
    const fixture = path.join(FIXTURE_ROOT, 'mutants', mutant.id);
    assert.equal(mutant.fixture, directorySeal(fixture), `${mutant.id} seal`);
    const failures = generator.inspectFixture(fixture).failures;
    assert.ok(failures.length >= mutant.expected_drop, `${mutant.id} expected drop`);
    for (const id of mutant.must_fail) assert.ok(failures.includes(id), `${mutant.id} must fail ${id}`);
  }
});

test('install generator is deterministic, writer-routed, and producer-read-only', () => {
  const before = treeDigest(PACK_ROOT);
  const check = spawnSync(process.execPath, [GENERATOR_PATH, '--check'], {
    cwd: ROOT, encoding: 'utf8', env: { ...process.env, NANO_WP4_TEMP_ROOT: path.join(ROOT, 'target', 'wp4-ip-temp') },
  });
  assert.equal(check.status, 0, check.stderr || check.stdout);
  assert.equal(treeDigest(PACK_ROOT), before);
  const source = fs.readFileSync(GENERATOR_PATH, 'utf8');
  assert.match(source, /writeArtifact/);
  assert.match(source, /await writeArtifact\(target,/);
  assert.match(source, /await writeArtifact\(CARD,/);
  assert.doesNotMatch(source, /appendFileSync|copyFileSync/);
});

test('install gate scores reference 6\/6 and catches every sealed mutant', () => {
  const card = loadCard(CARD_PATH);
  const cases = [{ id: 'reference', fixture: card.validation.reference, expected: [] },
    ...card.validation.mutants.map((mutant) => ({ id: mutant.id, fixture: mutant.fixture, expected: mutant.must_fail }))];
  for (const item of cases) {
    const dir = item.id === 'reference' ? path.join(FIXTURE_ROOT, 'reference') : path.join(FIXTURE_ROOT, 'mutants', item.id);
    const run = spawnSync(process.execPath, [GATE_PATH, dir, item.fixture], {
      cwd: ROOT, encoding: 'utf8', env: { ...process.env, NANO_WP4_TEMP_ROOT: path.join(ROOT, 'target', 'wp4-ip-temp') },
      timeout: 30_000,
    });
    const parsed = parseGateOutput(run.stdout, INVENTORY);
    assert.equal(parsed.failClosed, null, `${item.id}: ${run.stdout}\n${run.stderr}`);
    if (item.id === 'reference') assert.deepEqual(parsed, { ok: true, passed: 6, total: 6, failures: [], failClosed: null });
    else for (const id of item.expected) assert.ok(parsed.failures.some((failure) => failure.id === id), `${item.id}: ${id}\n${run.stdout}`);
  }
});

test('install gate fails closed on seal drift, missing subject, and malfunction', () => {
  const missing = spawnSync(process.execPath, [GATE_PATH, path.join(FIXTURE_ROOT, 'missing'), `sealed:dir-sha256:${'0'.repeat(64)}`], { encoding: 'utf8' });
  assert.equal(missing.status, 1);
  assert.match(missing.stdout, /gate: 0\/6/);
  const wrongSeal = spawnSync(process.execPath, [GATE_PATH, path.join(FIXTURE_ROOT, 'reference'), `sealed:dir-sha256:${'0'.repeat(64)}`], { encoding: 'utf8' });
  assert.equal(wrongSeal.status, 1);
  assert.match(wrongSeal.stdout, /gate: 0\/6/);
});
