'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');
const CARD = path.join(ROOT, 'gates', 'mem-sec', 'card.md');
const SCRIPT = path.join(ROOT, 'gates', 'mem-sec', 'gate.cjs');
const FIXTURES = path.join(ROOT, 'gates', 'fixtures', 'mem-sec');
const RECALL_FIXTURE = path.join(ROOT, 'gates', 'fixtures', 'memory-retrieval-recall-v1');
const { loadCard, scriptHash } = require('../lib/card.cjs');
const { canonicalJson, parseGateOutput } = require('../lib/contract.cjs');
const { directorySeal } = require('../lib/dirhash.cjs');

function requireSubjects() {
  for (const subject of [CARD, SCRIPT, FIXTURES, path.join(ROOT, 'crates', 'nano-memory', 'tests', 'mem_sec_cards.rs')]) {
    assert.equal(fs.existsSync(subject), true, `missing mem-sec subject: ${subject}`);
  }
}

test('mem-sec card has six checks and five sealed mutants per check', () => {
  requireSubjects();
  const card = loadCard(CARD);
  assert.deepEqual(card.checks.map(({ id }) => id), [
    'MS-01', 'MS-02', 'MS-03', 'MS-04', 'MS-05', 'MS-06',
  ]);
  assert.equal(card.validation.mutants.length, 30);
  assert.equal(card.validation.reference, directorySeal(FIXTURES));
  const fixtureSeals = new Set(fs.readdirSync(FIXTURES, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => directorySeal(path.join(FIXTURES, entry.name))));
  for (const check of card.checks) {
    const mutants = card.validation.mutants.filter(({ must_fail: mustFail }) => mustFail.includes(check.id));
    assert.ok(mutants.length >= 5, `${check.id} mutant coverage`);
    for (const mutant of mutants) assert.ok(fixtureSeals.has(mutant.fixture), `${mutant.id} seal`);
  }
  assert.equal(card.gate_script_hash, scriptHash(SCRIPT));
  assert.equal(card.validation.last_validated, scriptHash(SCRIPT));
});

test('mem-sec registry closure and supplementary recall seal remain exact', () => {
  requireSubjects();
  const registry = JSON.parse(fs.readFileSync(path.join(ROOT, 'gates', 'registry.json'), 'utf8'));
  const entry = registry.gates['mem-sec'];
  assert.ok(entry);
  assert.equal(entry.closure_digest,
    crypto.createHash('sha256').update(canonicalJson(entry.closure)).digest('hex'));
  assert.equal(entry.run_artifact, 'gates/fixtures/mem-sec');
  assert.equal(registry.requirements['MEM-SEC'], 'mem-sec');
  assert.equal(directorySeal(RECALL_FIXTURE),
    'sealed:dir-sha256:5555558a1def7f320ab73949863335fb6dd9d13c2fe99c9117a255c7c1cef6a3');
});

test('mem-sec green harness emits the exact gate output contract', () => {
  requireSubjects();
  const artifact = process.platform === 'win32' ? `\\\\?\\${FIXTURES}` : FIXTURES;
  const result = spawnSync(process.execPath, [SCRIPT, artifact], {
    cwd: ROOT,
    encoding: 'utf8',
    timeout: 5 * 60_000,
    windowsHide: true,
  });
  assert.equal(result.status, 0, `${result.stderr}\n${result.stdout}`);
  const summaries = result.stdout.match(/^gate: 6\/6$/gmu) || [];
  assert.equal(summaries.length, 1, result.stdout);
  const failLines = result.stdout.split(/\r?\n/).filter((line) => line.startsWith('FAIL '));
  assert.ok(failLines.every((line) => /^FAIL MS-0[1-6] /.test(line)));
  const inventory = loadCard(CARD).checks.map(({ id, category }) => [id, category]);
  assert.deepEqual(parseGateOutput(result.stdout, inventory), {
    ok: true,
    passed: 6,
    total: 6,
    failures: [],
    failClosed: null,
  });
});
