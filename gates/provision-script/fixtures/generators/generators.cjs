#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { writeArtifact } = require('../../../lib/artifact-writer.cjs');

const ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const OUTPUT = path.join(ROOT, 'gates', 'fixtures', 'provision-script');
const DRY_RUN = process.env.NANO_DRY_RUN_BIN || path.join(ROOT, 'target', 'debug', 'wayland-nano-provision-dry-run.exe');
const MUTANTS = [
  { id: 'pv-m1', why_fluent: 'blank service identity treated as deferred configuration', expected_drop: 1, must_fail: ['PV-02'], mutate: (p) => { p.offline_username = ''; } },
  { id: 'pv-m2', why_fluent: 'legacy donor identities retained during a namespace refactor', expected_drop: 1, must_fail: ['PV-02'], mutate: (p) => { p.offline_username = 'CodexSandboxOffline'; p.online_username = 'CodexSandboxOnline'; } },
  { id: 'pv-m3', why_fluent: 'root normalization accidentally preserves a duplicate operation', expected_drop: 1, must_fail: ['PV-03'], mutate: (p) => { p.write_roots = ['F:\\nano-work', 'F:\\nano-work']; } },
  { id: 'pv-m4', why_fluent: 'an older setup protocol is accepted for compatibility', expected_drop: 1, must_fail: ['PV-05'], mutate: (p) => { p.version = 4; } },
  { id: 'pv-m5', why_fluent: 'uninstall cleanup broadens an owned path into a wildcard', expected_drop: 1, must_fail: ['PV-06'], mutate: (p) => { p.uninstall = true; p.deny_write_paths = ['F:\\NanoSandbox*']; } },
  { id: 'pv-m6', why_fluent: 'a convenient elevation hint is added to the wire payload', expected_drop: 1, must_fail: ['PV-01'], mutate: (p) => { p.requires_elevation = true; } },
];

function extractFramedJson(stdout) {
  const marker = '--- payload (what the elevated helper would receive) ---';
  const end = '--- to execute (ELEVATED PowerShell, after review) ---';
  const startAt = stdout.indexOf(marker);
  const endAt = stdout.indexOf(end, startAt + marker.length);
  if (startAt < 0 || endAt < 0) throw new Error('DRY_RUN_MARKERS_MISSING');
  return JSON.parse(stdout.slice(startAt + marker.length, endAt).trim());
}

function authenticPayload() {
  const result = spawnSync(DRY_RUN, [], {
    cwd: ROOT, encoding: 'utf8', env: { ...process.env, NANO_HOME: 'F:\\wayland-nano-fixture', USERNAME: 'fixture-user' },
  });
  if (result.status !== 0) throw new Error(`DRY_RUN_FAILED ${result.stderr}`);
  const payload = extractFramedJson(result.stdout);
  payload.nano_home = 'F:\\wayland-nano-fixture';
  payload.command_cwd = 'F:\\wayland-nano-fixture';
  payload.real_user = 'fixture-user';
  payload.cancellation_path = 'F:\\wayland-nano-fixture\\.sandbox\\cancel-00000000000000000000000000000000';
  return payload;
}

function bytes(value) { return Buffer.from(`${JSON.stringify(value, null, 2)}\n`, 'utf8'); }

async function construct(destination) {
  const reference = authenticPayload();
  fs.mkdirSync(path.join(destination, 'reference'), { recursive: true });
  await writeArtifact(path.join(destination, 'reference', 'payload.json'), bytes(reference));
  for (const mutant of MUTANTS) {
    const payload = structuredClone(reference);
    mutant.mutate(payload);
    fs.mkdirSync(path.join(destination, 'mutants', mutant.id), { recursive: true });
    await writeArtifact(path.join(destination, 'mutants', mutant.id, 'payload.json'), bytes(payload));
  }
  const manifest = { schema: 1, source: 'real marker-framed wayland-nano-provision-dry-run output', mutants: MUTANTS.map(({ id, why_fluent, expected_drop, must_fail }) => ({ id, why_fluent, expected_drop, must_fail })) };
  await writeArtifact(path.join(destination, 'manifest.json'), bytes(manifest));
}

function inventory(root) {
  const out = new Map();
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const absolute = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(absolute);
      else out.set(path.relative(root, absolute).replaceAll('\\', '/'), fs.readFileSync(absolute));
    }
  }
  walk(root); return out;
}

async function main() {
  if (process.argv[2] === '--check') {
    const scratchRoot = process.env.NANO_WP4_TEST_ROOT || path.join(ROOT, 'target', 'wp4-plan03-tests');
    fs.mkdirSync(scratchRoot, { recursive: true });
    const scratch = fs.mkdtempSync(path.join(scratchRoot, 'generator-check-'));
    try {
      await construct(scratch);
      const expected = inventory(OUTPUT); const actual = inventory(scratch);
      if (expected.size !== actual.size || [...expected].some(([name, value]) => !actual.has(name) || !actual.get(name).equals(value))) throw new Error('GENERATED_FIXTURES_DRIFT');
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
    return;
  }
  fs.mkdirSync(OUTPUT, { recursive: true });
  await construct(OUTPUT);
}

module.exports = { extractFramedJson, authenticPayload, construct };
if (require.main === module) main().catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
