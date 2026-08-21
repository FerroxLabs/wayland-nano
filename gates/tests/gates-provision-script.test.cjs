'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');
const GATE = path.join(ROOT, 'gates', 'provision-script', 'gate.cjs');
const FIXTURES = path.join(ROOT, 'gates', 'fixtures', 'provision-script');
const GENERATOR = path.join(ROOT, 'gates', 'provision-script', 'fixtures', 'generators', 'generators.cjs');
const PRODUCERS = [
  'crates/nano-sandbox/src/bin/provision_dry_run/main.rs',
  'crates/nano-sandbox/src/setup_exec.rs',
  'crates/nano-sandbox/src/setup_types.rs',
];
const INVENTORY = [
  ['PV-01', 'structure'], ['PV-02', 'security'], ['PV-03', 'relation'],
  ['PV-04', 'security'], ['PV-05', 'value'], ['PV-06', 'relation'],
];
const { parseGateOutput } = require('../lib/contract.cjs');
const { parseCard, scriptHash } = require('../lib/card.cjs');
const { digestDirectory } = require('../lib/dirhash.cjs');

function sha(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(path.join(ROOT, file))).digest('hex');
}

function runFixture(relative) {
  return spawnSync(process.execPath, [GATE, '--packet', path.join(FIXTURES, relative, 'payload.json')], {
    cwd: ROOT, encoding: 'utf8', env: { ...process.env, NANO_PROVISION_FIXTURE_ROOT: FIXTURES },
  });
}

test('t-pv-reference-scores-mm', () => {
  const before = Object.fromEntries(PRODUCERS.map((file) => [file, sha(file)]));
  const first = runFixture('reference');
  assert.equal(first.status, 0, first.stderr);
  assert.deepEqual(parseGateOutput(first.stdout, INVENTORY), {
    ok: true, passed: 6, total: 6, failures: [], failClosed: null,
  });
  const check = spawnSync(process.execPath, [GENERATOR, '--check'], { cwd: ROOT, encoding: 'utf8' });
  assert.equal(check.status, 0, check.stderr);
  assert.deepEqual(Object.fromEntries(PRODUCERS.map((file) => [file, sha(file)])), before);
  assert.equal(digestDirectory(path.join(FIXTURES, 'reference')).entries.length, 1);
  const card = parseCard(fs.readFileSync(path.join(ROOT, 'gates', 'provision-script', 'card.md'), 'utf8'));
  assert.equal(card.checks.length, 6);
  assert.equal(card.validation.rotation_k, 2);
  assert.equal(card.validation.reference, `sealed:dir-sha256:${digestDirectory(path.join(FIXTURES, 'reference')).digest}`);
  assert.equal(card.gate_script_hash, scriptHash(GATE));
  assert.equal(card.validationCurrent(scriptHash(GATE)), true);
});

test('t-pv-mutants-caught', () => {
  const metadata = JSON.parse(fs.readFileSync(path.join(FIXTURES, 'manifest.json'), 'utf8'));
  assert.equal(metadata.schema, 1);
  assert.equal(metadata.mutants.length, 6);
  for (const mutant of metadata.mutants) {
    const result = runFixture(path.join('mutants', mutant.id));
    assert.notEqual(result.status, 0, `${mutant.id} unexpectedly passed`);
    const parsed = parseGateOutput(result.stdout, INVENTORY);
    assert.equal(parsed.failClosed, null, `${mutant.id}: ${result.stdout}`);
    assert.ok(mutant.must_fail.every((id) => parsed.failures.some((failure) => failure.id === id)), `${mutant.id}: ${result.stdout}`);
    const cardMutant = parseCard(fs.readFileSync(path.join(ROOT, 'gates', 'provision-script', 'card.md'), 'utf8')).validation.mutants.find(({ id }) => id === mutant.id);
    assert.equal(cardMutant.fixture, `sealed:dir-sha256:${digestDirectory(path.join(FIXTURES, 'mutants', mutant.id)).digest}`);
  }
});

test('packet/live arms are explicit and exclusive', () => {
  const none = spawnSync(process.execPath, [GATE], { cwd: ROOT, encoding: 'utf8' });
  assert.notEqual(none.status, 0);
  assert.equal(none.stdout, 'gate: 0/6\n');
  const both = spawnSync(process.execPath, [GATE, '--packet', 'x', '--live'], { cwd: ROOT, encoding: 'utf8' });
  assert.notEqual(both.status, 0);
  assert.equal(both.stdout, 'gate: 0/6\n');
});

test('provision gate seal bytes are LF-pinned for fresh checkouts', () => {
  const bytes = fs.readFileSync(GATE);
  assert.equal(bytes.includes(Buffer.from('\r\n')), false, 'gate bytes must remain LF-only');
  const attr = spawnSync('git', ['check-attr', 'eol', '--', 'gates/provision-script/gate.cjs'], {
    cwd: ROOT, encoding: 'utf8',
  });
  assert.equal(attr.status, 0, attr.stderr);
  assert.match(attr.stdout, /gate\.cjs: eol: lf\s*$/);
  const card = parseCard(fs.readFileSync(path.join(ROOT, 'gates', 'provision-script', 'card.md'), 'utf8'));
  assert.equal(scriptHash(GATE), card.gate_script_hash);
});

test('Windows live arm refuses elevation and preserves external state', { skip: process.platform !== 'win32' }, () => {
  const dryRun = process.env.NANO_DRY_RUN_BIN || path.join(ROOT, 'target', 'debug', 'wayland-nano-provision-dry-run.exe');
  const setup = process.env.NANO_SETUP_BIN || path.join(ROOT, 'target', 'debug', 'wayland-nano-sandbox-setup.exe');
  assert.equal(fs.existsSync(dryRun), true, `missing required Windows dry-run capability: ${dryRun}`);
  assert.equal(fs.existsSync(setup), true, `missing required Windows setup capability: ${setup}`);
  const result = spawnSync(process.execPath, [GATE, '--live'], {
    cwd: ROOT, encoding: 'utf8', env: { ...process.env, NANO_DRY_RUN_BIN: dryRun, NANO_SETUP_BIN: setup },
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.equal(parseGateOutput(result.stdout, INVENTORY).ok, true);
});
