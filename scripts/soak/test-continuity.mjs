#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const harness = join(here, 'continuity.mjs');
const repo = resolve(here, '..', '..');

function releaseBinary() {
  if (process.env.CONTINUITY_TEST_BINARY) return resolve(process.env.CONTINUITY_TEST_BINARY);
  const target = process.env.CARGO_TARGET_DIR
    ? resolve(process.env.CARGO_TARGET_DIR)
    : join(repo, 'target');
  return join(target, 'release', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano');
}

function run(args) {
  return spawnSync(process.execPath, [harness, ...args], {
    cwd: repo,
    encoding: 'utf8',
    env: { ...process.env, CONTINUITY_TESTING: '1' },
    timeout: 300_000,
  });
}

async function withTempDir(fn) {
  const base = join(here, 'evidence');
  await mkdir(base, { recursive: true });
  const dir = await mkdtemp(join(base, 'test-continuity-'));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}

test('preflight rejects a binary without the soak marker before creating evidence', async () => {
  await withTempDir(async (evidence) => {
    const result = run(['--mode', 'smoke', '--seed', '1010', '--binary', process.execPath, '--evidence-dir', evidence]);
    assert.equal(result.status, 2, result.stderr || result.stdout);
    assert.match(result.stderr, /lacks the soak-fake-model feature/);
    assert.equal((await import('node:fs')).existsSync(join(evidence, 'latest.json')), false);
  });
});

test('marked real-binary smoke evidence', async (t) => {
  await withTempDir(async (evidence) => {
    const result = run(['--mode', 'smoke', '--seed', '1010', '--binary', releaseBinary(), '--evidence-dir', evidence]);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const latest = JSON.parse(await readFile(join(evidence, 'latest.json'), 'utf8'));
    const rows = (await readFile(resolve(evidence, latest.ndjson), 'utf8'))
      .trim().split('\n').map((line) => JSON.parse(line));
    await t.test('emits one hash-bound NDJSON row for every continuity mode', () => {
      assert.deepEqual([...new Set(rows.map((row) => row.mode))].sort(), ['fresh', 'memory_recall', 'session_resume']);
      assert.ok(rows.every((row) => row.seed === 1010));
      assert.ok(rows.every((row) => /^[0-9a-f]{64}$/.test(row.binary_sha256)));
      assert.ok(rows.every((row) => /^[0-9a-f]{64}$/.test(row.journal_sha256)));
    });
    await t.test('records session-resume drift as a typed correct refusal', () => {
      const drift = rows.find((row) => row.mode === 'session_resume' && row.probe_kind === 'drift_refusal');
      assert.ok(drift, 'missing drift refusal row');
      assert.equal(drift.quality_pass, true);
      assert.equal(drift.refusal_kind, 'resume_drift');
      assert.equal(drift.silent_fallback, false);
    });
  });
});
