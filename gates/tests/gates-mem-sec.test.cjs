'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
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

test('every sealed implementation mutant makes its named check fail at runtime', {
  timeout: 20 * 60_000,
}, () => {
  requireSubjects();
  const card = loadCard(CARD);
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'wayland-nano-mem-sec-mutants-'));
  const repo = path.join(scratch, 'repo');
  const target = path.join(scratch, 'target');
  try {
    let result = spawnSync('git', ['clone', '--quiet', '--no-local', '--no-hardlinks', ROOT, repo], {
      encoding: 'utf8', windowsHide: true,
    });
    assert.equal(result.status, 0, result.stderr);
    result = spawnSync('git', ['-C', repo, 'checkout', '--detach', 'HEAD'], {
      encoding: 'utf8', windowsHide: true,
    });
    assert.equal(result.status, 0, result.stderr);

    for (const mutant of card.validation.mutants) {
      assert.equal(mutant.must_fail.length, 1, `${mutant.id} must bind one check`);
      const number = mutant.must_fail[0].slice(3);
      const patchFile = path.join(
        repo,
        'gates',
        'fixtures',
        'mem-sec',
        `mem-sec-${Number(number)}`,
        'mutants',
        `${mutant.id}.diff`,
      );
      const patchBytes = fs.readFileSync(patchFile, 'utf8');
      assert.match(patchBytes, /^diff --git a\/crates\/nano-memory\/src\//u, mutant.id);
      assert.doesNotMatch(patchBytes, /gates\/fixtures/u, mutant.id);

      result = spawnSync('git', ['-C', repo, 'apply', '--unidiff-zero', patchFile], {
        encoding: 'utf8', windowsHide: true,
      });
      assert.equal(result.status, 0, `${mutant.id}: ${result.stderr}`);
      result = spawnSync('cargo', [
        'test', '--locked', '-p', 'nano-memory', '--test', 'mem_sec_cards',
        `mem_sec_${Number(number)}`, '--', '--exact', '--nocapture', '--test-threads=1',
      ], {
        cwd: repo,
        env: { ...process.env, CARGO_TARGET_DIR: target },
        encoding: 'utf8',
        timeout: 2 * 60_000,
        windowsHide: true,
      });
      const output = `${result.stdout || ''}\n${result.stderr || ''}`;
      assert.notEqual(result.status, 0, `${mutant.id} survived`);
      assert.doesNotMatch(output, /could not compile/u, `${mutant.id} only broke compilation`);
      assert.match(output, new RegExp(`test mem_sec_${Number(number)} \\.\\.\\. FAILED`, 'u'),
        `${mutant.id} did not fail its named runtime check`);

      result = spawnSync('git', ['-C', repo, 'apply', '--reverse', '--unidiff-zero', patchFile], {
        encoding: 'utf8', windowsHide: true,
      });
      assert.equal(result.status, 0, `${mutant.id} reverse: ${result.stderr}`);
      result = spawnSync('git', ['-C', repo, 'diff', '--quiet'], {
        encoding: 'utf8', windowsHide: true,
      });
      assert.equal(result.status, 0, `${mutant.id} left source drift`);
    }
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
});
