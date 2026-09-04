#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const harness = join(here, 'continuity.mjs');
const report = join(here, 'continuity-report.mjs');
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
      const recall = rows.filter((row) => row.probe_kind === 'recall');
      assert.equal(new Set(recall.map((row) => row.task_battery_sha256)).size, 1, 'all modes must share one task battery');
      for (const label of new Set(recall.map((row) => row.label))) {
        const probeRows = recall.filter((row) => row.label === label);
        assert.equal(new Set(probeRows.map((row) => row.driver_script_sha256)).size, 3, `${label} must have one driver/oracle script per mode`);
        assert.equal(new Set(probeRows.map((row) => JSON.stringify(row.driver_profile))).size, 1, `${label} fake usage/delay differs by mode`);
      }
      const fresh = recall.filter((row) => row.mode === 'fresh');
      assert.ok(fresh.every((row) => row.persistent === false && row.memory_tool_calls === 0));
      assert.ok(fresh.every((row) => row.quality_pass === false && row.request_assertion === 'absent'));
      const resumed = recall.filter((row) => row.mode === 'session_resume');
      assert.ok(resumed.every((row) => row.loaded_session_id === row.fork_child_session_id));
      assert.ok(resumed.every((row) => row.memory_tool_calls === 0 && row.request_assertion === 'present' && row.quality_pass));
      const recalled = recall.filter((row) => row.mode === 'memory_recall');
      assert.ok(recalled.every((row) => row.activation_admitted === true && row.memory_seeded === true));
      assert.ok(recalled.every((row) => row.memory_tool_calls === 0 && row.request_assertion === 'present'));
      assert.ok(recalled.every((row) => row.quality_pass === row.request_assertion_matched));
      assert.ok(recalled.some((row) => row.quality_pass), 'automatic recall produced no relevant request context');
      assert.ok(recall.every((row) => row.answer_source === 'model_request_assertion'));
      assert.ok(recall.every((row) => row.tokens.source === 'acp_budget_notice'));
      assert.ok(recall.every((row) => row.tokens.probe_tokens === row.tokens.session_tokens_after - row.tokens.session_tokens_before));
      assert.ok(recall.every((row) => row.tokens.probe_tokens > 0));
      assert.equal(resumed.filter((row) => row.tokens.setup_tokens > 0).length, 4, 'one resume baseline per partition');
      assert.ok(resumed.every((row) => row.tokens.session_tokens_before > 0));
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

test('report refuses evidence bound to a different budget hash', async () => {
  await withTempDir(async (evidence) => {
    const runDir = join(evidence, 'run-mismatch');
    await mkdir(runDir, { recursive: true });
    await writeFile(join(runDir, 'continuity-manifest.json'), `${JSON.stringify({
      schema: 'wayland.nano.continuity-manifest/v1',
      measurement_mode: 'smoke',
      seed: 1010,
      budgets: { sha256: '0'.repeat(64) },
    })}\n`);
    const result = spawnSync(process.execPath, [report, '--evidence-dir', evidence, '--out', join(evidence, 'report.md')], {
      cwd: repo,
      encoding: 'utf8',
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /budget hash mismatch/);
  });
});
