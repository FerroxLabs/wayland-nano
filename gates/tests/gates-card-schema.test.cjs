'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');
const { parseCard } = require('../lib/card.cjs');
const { canonicalJson, parseGateOutput, renderGateOutput, scoreMutants } = require('../lib/contract.cjs');
const { digestDirectory } = require('../lib/dirhash.cjs');
const { readArtifact, writeArtifact } = require('../lib/artifact-writer.cjs');

let serial = 0;
function scratch(label) {
  const root = process.env.NANO_WP4_TEST_ROOT || path.join(ROOT, 'target', 'wp4-plan01-tests');
  fs.mkdirSync(root, { recursive: true });
  const dir = path.join(root, `${label}-${process.pid}-${serial++}`);
  fs.mkdirSync(dir);
  return dir;
}

function remove(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
}

function card(overrides = '') {
  return `before\n---\ncard: 1\ngate_id: install-payload\ndomain: repo-deliverable\ntier: 1\ngate_script_hash: ${'a'.repeat(64)}\nrelational_target:\n  artifact: staged tree\n  relation: bytes match\ndisclosure_default: opaque\nchecks:\n  - { id: IP-01, category: value, desc: byte pin, measures: exact hash }\nvalidation:\n  reference: sealed:dir-sha256:${'b'.repeat(64)}\n  pool_min: 1\n  pool_status: full\n  mutants:\n    - { id: ip-m1, class: fluent-but-wrong, why_fluent: plausible, expected_drop: 1, must_fail: [IP-01], fixture: sealed:dir-sha256:${'c'.repeat(64)} }\n  rotation_k: 1\n  last_validated: null\ngamed_modes:\n  - { mode: stale hash, status: sealed, note: mutant covers it }\nescape_hatch_bans:\n  - { ban: do not skip, check: IP-01 }\n${overrides}---\nafter\n`;
}

test('t-card-schema-valid', () => {
  const parsed = parseCard(card());
  assert.equal(parsed.gate_id, 'install-payload');
  assert.deepEqual(parsed.checks.map(({ id, category }) => [id, category]), [['IP-01', 'value']]);
  assert.throws(() => parseCard(card('mystery: true\n')), /CARD_INVALID unknown field mystery/);
  assert.throws(() => parseCard(card('gate_id: duplicate\n')), /CARD_INVALID duplicate field gate_id/);
  assert.throws(() => parseCard(card().replace('  relation: bytes match', '  relation: bytes match\n  mystery: hidden')), /CARD_INVALID unknown field relational_target.mystery/);
  assert.throws(() => parseCard(card().replace('IP-01', 'I-1')), /CARD_INVALID check id/);
  assert.throws(() => parseCard(card().replace('category: value', 'category: mystery')), /CARD_INVALID category/);
  assert.throws(() => parseCard(card().replace('status: sealed', 'status: handwaved')), /CARD_INVALID gamed mode/);
  assert.throws(() => parseCard(card().replace('must_fail: [IP-01]', 'must_fail: [IP-99]')), /CARD_INVALID mutant/);
  assert.throws(() => parseCard(card().replace('sealed:dir-sha256:', 'sealed:sha256:')), /CARD_INVALID seal/);
  assert.throws(() => parseCard(card().replace(/---\nafter/, 'after')), /CARD_INVALID machine block/);
});

test('sealed production cards satisfy the closed schema', () => {
  for (const gateId of ['install-payload', 'provision-script', 'config-schema']) {
    const cardPath = path.join(ROOT, 'gates', gateId, 'card.md');
    const parsed = parseCard(fs.readFileSync(cardPath, 'utf8'));
    assert.equal(parsed.gate_id, gateId);
    assert.equal(parsed.domain, 'repo-deliverable');
    assert.equal(parsed.tier, 1);
    assert.equal(parsed.checks.length, 6);
    assert.equal(new Set(parsed.checks.map(({ id }) => id)).size, 6);
    assert.ok(parsed.wrapped_tools.length >= 1);
    assert.ok(parsed.validation.mutants.length >= 5);
    assert.equal(parsed.validation.rotation_k, 2);
    assert.equal(parsed.validation.pool_status, 'full');
    assert.equal(parsed.validation.last_validated, parsed.gate_script_hash);
    assert.ok(parsed.gamed_modes.length >= 1);
    assert.ok(parsed.validation.mutants.every((mutant) => mutant.why_fluent
      && mutant.expected_drop >= 1 && mutant.must_fail.length >= 1));
  }
});

test('t-registry-closure-digests', () => {
  const registry = JSON.parse(fs.readFileSync(path.join(ROOT, 'gates', 'registry.json'), 'utf8'));
  assert.equal(registry.schema, 1);
  assert.deepEqual(Object.keys(registry.gates).sort(),
    ['config-schema', 'install-payload', 'provision-script']);
  assert.deepEqual(registry.requirements, {
    'CARD-05': 'install-payload',
    'CARD-06': 'provision-script',
    'CARD-07': 'config-schema',
  });
  for (const [gateId, entry] of Object.entries(registry.gates)) {
    assert.deepEqual(Object.keys(entry).sort(),
      ['card', 'closure', 'closure_digest', 'run_artifact', 'script']);
    assert.deepEqual(Object.keys(entry.closure).sort(),
      ['argv', 'cwd_policy', 'env', 'wrapped_tools']);
    assert.equal(entry.closure_digest,
      crypto.createHash('sha256').update(canonicalJson(entry.closure)).digest('hex'), gateId);
    for (const field of ['card', 'script', 'run_artifact']) {
      assert.equal(path.isAbsolute(entry[field]), false, `${gateId} ${field} absolute`);
      assert.equal(entry[field].split(/[\\/]/).includes('..'), false, `${gateId} ${field} traversal`);
      const resolved = fs.realpathSync(path.join(ROOT, entry[field]));
      assert.ok(resolved === ROOT || resolved.startsWith(`${ROOT}${path.sep}`), `${gateId} ${field} escape`);
    }
    const direct = entry.closure.argv[0] === entry.script;
    const interpreter = entry.closure.wrapped_tools.find(({ name, version }) =>
      name === entry.closure.argv[0] && version.length > 0);
    assert.ok(direct || (interpreter && entry.closure.argv[1] === entry.script), `${gateId} invocation`);
    assert.equal(entry.closure.argv.includes(entry.run_artifact), false, `${gateId} artifact in argv`);
    const cardTools = parseCard(fs.readFileSync(path.join(ROOT, entry.card), 'utf8')).wrapped_tools
      .map(({ name, version }) => ({ name, version: String(version) }));
    assert.deepEqual(entry.closure.wrapped_tools, cardTools, `${gateId} tool pins`);
  }
  for (const gateId of Object.values(registry.requirements)) assert.ok(registry.gates[gateId]);
});

test('t-fixture-digest-fails-closed', () => {
  const dir = scratch('digest-drift');
  try {
    fs.writeFileSync(path.join(dir, 'bytes.bin'), Buffer.from([0, 1, 2, 255]));
    const seal = digestDirectory(dir).digest;
    fs.writeFileSync(path.join(dir, 'bytes.bin'), Buffer.from([0, 1, 3, 255]));
    assert.notEqual(digestDirectory(dir).digest, seal);
    fs.symlinkSync(path.join(dir, 'bytes.bin'), path.join(dir, 'link'));
    assert.throws(() => digestDirectory(dir), /DIRHASH_INVALID symbolic link/);
  } finally { remove(dir); }
});

test('t-dirhash-canonical', () => {
  const dir = scratch('dirhash');
  try {
    fs.mkdirSync(path.join(dir, 'z'));
    fs.writeFileSync(path.join(dir, 'é.txt'), Buffer.from('exact\r\nbytes', 'utf8'));
    fs.writeFileSync(path.join(dir, 'z', 'a.txt'), Buffer.from('A'));
    const entries = [
      ['z/a.txt', crypto.createHash('sha256').update('A').digest('hex')],
      ['é.txt', crypto.createHash('sha256').update(Buffer.from('exact\r\nbytes')).digest('hex')],
    ].sort((a, b) => Buffer.compare(Buffer.from(a[0]), Buffer.from(b[0])));
    const manifest = entries.map(([name, hash]) => `${name}  ${hash}\n`).join('');
    const expected = crypto.createHash('sha256').update(manifest).digest('hex');
    assert.deepEqual(digestDirectory(dir), { digest: expected, manifest, entries });
    fs.writeFileSync(path.join(dir, 'e\u0301.txt'), 'collision');
    assert.throws(() => digestDirectory(dir), /DIRHASH_INVALID NFC collision/);
  } finally { remove(dir); }
});

test('t-meta-mutant-passing-is-gate-defect', () => {
  assert.deepEqual(scoreMutants('install-payload', [
    { id: 'ip-m1', output: 'FAIL IP-01 value\ngate: 0/1\n' },
  ], [['IP-01', 'value']]), { ok: true, defects: [] });
  assert.deepEqual(scoreMutants('install-payload', [
    { id: 'ip-m1', output: 'gate: 1/1\n' },
  ], [['IP-01', 'value']]), {
    ok: false,
    defects: ['GATE_DEFECT install-payload ip-m1'],
  });
});

test('t-summary-contract', () => {
  const inventory = [['IP-01', 'value'], ['IP-02', 'structure']];
  assert.equal(renderGateOutput(inventory, ['IP-02']), 'FAIL IP-02 structure\ngate: 1/2\n');
  assert.deepEqual(parseGateOutput('noise\ngate: 2/2\nFAIL IP-02 structure\ngate: 1/2\n', inventory), {
    ok: false, passed: 1, total: 2, failures: [{ id: 'IP-02', category: 'structure' }], failClosed: null,
  });
  for (const bad of ['', 'FAIL IP-99 value\ngate: 1/2', 'FAIL IP-01 value\ngate: 2/2', 'gate: 3/2']) {
    assert.notEqual(parseGateOutput(bad, inventory).failClosed, null);
  }
  assert.equal(canonicalJson({ z: 1, a: { y: 2, x: 1 } }), '{"a":{"x":1,"y":2},"z":1}');
});

test('t-gate-hash-drift-voids-validation', () => {
  const parsed = parseCard(card());
  assert.equal(parsed.validationCurrent('a'.repeat(64)), false, 'null validation is never current');
  const validated = card().replace('last_validated: null', `last_validated: ${'a'.repeat(64)}`);
  assert.equal(parseCard(validated).validationCurrent('a'.repeat(64)), true);
  assert.equal(parseCard(validated).validationCurrent('d'.repeat(64)), false);
});

test('artifact writer replaces atomically and leaves no residue', async () => {
  const dir = scratch('writer');
  const target = path.join(dir, 'evidence.json');
  try {
    fs.writeFileSync(target, 'old-complete');
    const seen = new Set();
    let running = true;
    const observer = (async () => {
      while (running) {
        try { seen.add(fs.readFileSync(target, 'utf8')); } catch (error) { seen.add(error.code); }
        await new Promise((resolve) => setImmediate(resolve));
      }
    })();
    await writeArtifact(target, Buffer.alloc(1024 * 128, 'n'));
    running = false;
    await observer;
    assert.deepEqual([...seen].filter((value) => value !== 'old-complete' && value !== 'n'.repeat(1024 * 128)), []);
    assert.equal((await readArtifact(target)).length, 1024 * 128);
    assert.deepEqual(fs.readdirSync(dir), ['evidence.json']);
  } finally { remove(dir); }
});

test('artifact writer contention, stale lock, failures, and recovery fail closed', async () => {
  const dir = scratch('writer-failures');
  const target = path.join(dir, 'report');
  const lock = `${target}.lock`;
  try {
    fs.writeFileSync(target, 'prior');
    fs.writeFileSync(lock, 'held');
    await assert.rejects(writeArtifact(target, 'new', { lockTimeoutMs: 80, retryMs: 10 }), /ARTIFACT_LOCK_TIMEOUT/);
    assert.equal(fs.readFileSync(target, 'utf8'), 'prior');
    fs.utimesSync(lock, new Date(0), new Date(0));
    await writeArtifact(target, 'new', { staleLockMs: 60 });
    assert.equal(fs.readFileSync(target, 'utf8'), 'new');
    assert.equal(fs.existsSync(`${target}.lock`), false);
    await assert.rejects(writeArtifact(target, 'lost', { injectFailure: 'sync' }), /ARTIFACT_WRITE_FAILED/);
    await assert.rejects(writeArtifact(target, 'lost', { injectFailure: 'replace' }), /ARTIFACT_WRITE_FAILED/);
    assert.equal(fs.readFileSync(target, 'utf8'), 'new');
    assert.deepEqual(fs.readdirSync(dir), ['report']);

    let reads = 0;
    assert.equal((await readArtifact(target, { read: async () => {
      reads++;
      if (reads === 1) throw new Error('transient');
      return Buffer.from('recovered');
    }, retryMs: 1 })).toString(), 'recovered');
    assert.equal(reads, 2);
    await assert.rejects(readArtifact(target, { read: async () => { throw new Error('bad'); }, retryMs: 1 }), /ARTIFACT_UNVERIFIABLE/);
  } finally { remove(dir); }
});

test('artifact writer CLI round trips exact bytes', () => {
  const dir = scratch('writer-cli');
  const target = path.join(dir, 'bytes');
  const cli = path.join(ROOT, 'gates', 'lib', 'artifact-writer.cjs');
  try {
    const bytes = Buffer.from([0, 13, 10, 255]);
    const write = spawnSync(process.execPath, [cli, 'write', target], { input: bytes });
    assert.equal(write.status, 0, write.stderr.toString());
    const read = spawnSync(process.execPath, [cli, 'read', target]);
    assert.equal(read.status, 0, read.stderr.toString());
    assert.deepEqual(read.stdout, bytes);
  } finally { remove(dir); }
});

test('Windows helper is the governed MoveFileExW replacement', () => {
  const helper = fs.readFileSync(path.join(ROOT, 'gates', 'lib', 'atomic-replace-win32.ps1'), 'utf8');
  assert.match(helper, /MoveFileExW/);
  assert.match(helper, /MOVEFILE_REPLACE_EXISTING/);
  assert.doesNotMatch(helper, /Copy-Item|Remove-Item|\[System\.IO\.File\]::Move/);
});
