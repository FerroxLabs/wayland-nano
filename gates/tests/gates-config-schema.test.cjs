'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');
const { loadCard, scriptHash } = require('../lib/card.cjs');
const { directorySeal } = require('../lib/dirhash.cjs');

const ROOT = path.resolve(__dirname, '..', '..');
const LOCKED_BASE = '30dbe9d8311f1d2192774f04788f1107b6cbd631';
const CARD = path.join(ROOT, 'gates', 'config-schema', 'card.md');
const GATE = path.join(ROOT, 'gates', 'config-schema', 'gate.sh');
const FIXTURES = path.join(ROOT, 'gates', 'fixtures', 'config-schema');
const GENERATOR = path.join(ROOT, 'gates', 'config-schema', 'fixtures', 'generators', 'generators.cjs');
const INVENTORY = new Map([
  ['CF-01', 'execution'], ['CF-02', 'security'], ['CF-03', 'security'],
  ['CF-04', 'relation'], ['CF-05', 'value'], ['CF-06', 'structure'],
]);

function run(command, args, options = {}) {
  return spawnSync(command, args, { cwd: ROOT, encoding: 'utf8', timeout: 400_000, ...options });
}

function parseOutput(output) {
  const failures = [...output.matchAll(/^FAIL (CF-[0-9]{2}) (\w+)$/gm)].map((m) => [m[1], m[2]]);
  const summaries = [...output.matchAll(/^gate: (\d+)\/(\d+)$/gm)];
  assert.equal(summaries.length, 1, output);
  assert.equal(Number(summaries[0][2]), INVENTORY.size, output);
  assert.equal(Number(summaries[0][1]), INVENTORY.size - failures.length, output);
  for (const [id, category] of failures) assert.equal(INVENTORY.get(id), category, output);
  return { failures: new Set(failures.map(([id]) => id)), passed: Number(summaries[0][1]) };
}

function generate() {
  const result = run(process.execPath, [GENERATOR, '--check']);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const card = loadCard(CARD);
  assert.equal(card.gate_script_hash, scriptHash(GATE));
  assert.equal(card.validation.reference, directorySeal(path.join(FIXTURES, 'probes')));
  for (const mutant of card.validation.mutants) {
    const dir = path.join(FIXTURES, 'mutants', mutant.id);
    assert.equal(mutant.fixture, directorySeal(dir), mutant.id);
    const patch = fs.readFileSync(path.join(dir, 'mutant.diff'), 'utf8');
    const paths = [...patch.matchAll(/^(?:--- a|\+\+\+ b)\/(.+)$/gm)].map((match) => match[1]);
    assert(paths.length === 2 && paths.every((entry) => [
      'crates/nano-core/src/execrules.rs',
      'crates/nano-cli/src/rules_cmds.rs',
      'crates/nano-model/data/providerCatalog.vendored.json',
    ].includes(entry)), `patch escape: ${mutant.id}`);
  }
}

test('t-cf-reference-scores-mm', () => {
  generate();
  const target = process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target');
  const binary = path.join(target, 'debug', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano');
  const build = run('cargo', ['build', '-p', 'nano-cli']);
  assert.equal(build.status, 0, build.stderr || build.stdout);
  const result = run('bash', [GATE, path.join(FIXTURES, 'probes')], {
    env: { ...process.env, NANO_CLI_BIN: binary, NANO_REPO_ROOT: ROOT },
  });
  const scored = parseOutput(result.stdout);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(scored.passed, 6);
  assert.equal(scored.failures.size, 0);
});

test('t-cf-mutants-caught', { timeout: 2_800_000 }, () => {
  generate();
  const card = fs.readFileSync(CARD, 'utf8');
  const declared = [...card.matchAll(/^    - id: (cf-m[1-6])\r?\n(?:.*\r?\n)*?      must_fail: \[([^\]]+)\]/gm)]
    .map((m) => ({ id: m[1], mustFail: m[2].split(',').map((v) => v.trim()) }));
  assert.equal(declared.length, 6);
  assert.equal(run('git', ['worktree', 'prune']).status, 0);
  const nonce = `${process.pid}-${Date.now().toString(36)}`;
  const control = process.env.NANO_CF_TEMP_ROOT || path.join(ROOT, 'target', `cf-${nonce}`);
  fs.mkdirSync(control, { recursive: true });
  try {
    for (const mutant of declared) {
      const wt = path.join(control, `w${mutant.id.slice(-1)}`);
      const target = path.join(control, `t${mutant.id.slice(-1)}`);
      const add = run('git', ['worktree', 'add', '--detach', wt, LOCKED_BASE]);
      assert.equal(add.status, 0, add.stderr || add.stdout);
      try {
        const patch = path.join(FIXTURES, 'mutants', mutant.id, 'mutant.diff');
        const applied = run('git', ['-C', wt, 'apply', '--check', patch]);
        assert.equal(applied.status, 0, applied.stderr || applied.stdout);
        assert.equal(run('git', ['-C', wt, 'apply', patch]).status, 0);
        const build = run('cargo', ['build', '--manifest-path', path.join(wt, 'Cargo.toml'), '-p', 'nano-cli'], {
          env: { ...process.env, CARGO_TARGET_DIR: target },
        });
        assert.equal(build.status, 0, build.stderr || build.stdout);
        const binary = path.join(target, 'debug', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano');
        const gated = run('bash', [GATE, path.join(FIXTURES, 'probes')], {
          cwd: wt,
          env: { ...process.env, NANO_CLI_BIN: binary, NANO_REPO_ROOT: wt, NANO_GATE_ROOT: ROOT },
        });
        const score = parseOutput(gated.stdout);
        assert.notEqual(score.passed, 6, `GATE_DEFECT config-schema ${mutant.id}`);
        for (const id of mutant.mustFail) assert(score.failures.has(id), `${mutant.id} did not fail ${id}\n${gated.stdout}`);
      } finally {
        run('git', ['worktree', 'remove', '--force', wt]);
        fs.rmSync(target, { recursive: true, force: true });
        assert.equal(fs.existsSync(wt), false, `worktree residue: ${wt}`);
        assert.equal(fs.existsSync(target), false, `target residue: ${target}`);
      }
    }
  } finally {
    fs.rmSync(control, { recursive: true, force: true });
    run('git', ['worktree', 'prune']);
  }
  const registrations = run('git', ['worktree', 'list', '--porcelain']).stdout;
  assert.doesNotMatch(registrations, new RegExp(control.replace(/[\\^$.*+?()[\]{}|]/g, '\\$&')));
});

test('t-cf-cleanup-survives-injected-failure', () => {
  const failureRoot = process.env.NANO_CF_TEMP_ROOT || path.join(ROOT, 'target', 'cf-failure');
  fs.mkdirSync(failureRoot, { recursive: true });
  const scratch = fs.mkdtempSync(path.join(failureRoot, 'cf-clean-'));
  const wt = path.join(scratch, 'w');
  const target = path.join(scratch, 't');
  try {
    assert.equal(run('git', ['worktree', 'add', '--detach', wt, LOCKED_BASE]).status, 0);
    fs.mkdirSync(target);
    fs.writeFileSync(path.join(target, 'partial'), 'injected build residue');
    assert.throws(() => { throw new Error('injected'); }, /injected/);
  } finally {
    run('git', ['worktree', 'remove', '--force', wt]);
    fs.rmSync(target, { recursive: true, force: true });
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  assert.equal(fs.existsSync(wt), false);
  assert.equal(fs.existsSync(target), false);
  assert.doesNotMatch(run('git', ['worktree', 'list', '--porcelain']).stdout, new RegExp(wt.replace(/[\\^$.*+?()[\]{}|]/g, '\\$&')));
});
