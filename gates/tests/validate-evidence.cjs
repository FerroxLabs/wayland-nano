#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');

const HEX = /^[0-9a-f]{64}$/;
const SHA = /^[0-9a-f]{40}$/;
const sha256 = (value) => crypto.createHash('sha256').update(value).digest('hex');
const die = (message) => { throw new Error(message); };
function exact(value, keys, where) {
  if (!value || Object.getPrototypeOf(value) !== Object.prototype
      || Object.keys(value).sort().join('\0') !== [...keys].sort().join('\0')) die(`${where}: closed schema`);
}
function json(file) { return JSON.parse(fs.readFileSync(file, 'utf8')); }
function result(value, expected, where) {
  exact(value, ['bytes', 'sha256'], `${where}.result`);
  if (typeof value.bytes !== 'string' || sha256(Buffer.from(value.bytes, 'utf8')) !== value.sha256 || !HEX.test(value.sha256)) die(`${where}: digest`);
  const parsed = JSON.parse(value.bytes);
  exact(parsed, ['outcome', 'v', 'verdicts'], `${where}.result.bytes`);
  if (parsed.v !== 1 || parsed.outcome !== expected || !Array.isArray(parsed.verdicts)) die(`${where}: outcome`);
  for (const verdict of parsed.verdicts) {
    exact(verdict, ['category', 'id', 'passed'], `${where}.verdict`);
    if (!/^[A-Z]{2}-[0-9]{2}$/.test(verdict.id) || typeof verdict.passed !== 'boolean'
        || !['structure', 'value', 'relation', 'grounding', 'execution', 'security'].includes(verdict.category)) die(`${where}: identifiers-only`);
  }
}
function dogfood(file) {
  const value = json(file);
  exact(value, ['base_sha', 'cleanup', 'good', 'invocation', 'mutants', 'product_sha', 'registry_sha256', 'schema'], 'dogfood');
  if (value.schema !== 'nano.wp4-dogfood/1' || !SHA.test(value.base_sha) || !SHA.test(value.product_sha)
      || !HEX.test(value.registry_sha256) || value.registry_sha256 !== sha256(fs.readFileSync('gates/registry.json'))) die('dogfood: identity');
  exact(value.invocation, ['argv', 'binary', 'mode'], 'invocation');
  if (value.invocation.mode !== 'wp3-run-only' || value.invocation.binary !== 'target/debug/wayland-nano.exe'
      || JSON.stringify(value.invocation.argv) !== JSON.stringify(['verify', '--gate', '<registry-id>', '--run-only', '--json'])) die('dogfood: invocation');
  if (!Array.isArray(value.good) || value.good.length !== 3 || !Array.isArray(value.mutants) || value.mutants.length !== 3) die('dogfood: cardinality');
  const ids = ['config-schema', 'install-payload', 'provision-script'];
  for (const [index, item] of value.good.entries()) {
    exact(item, ['exit_code', 'fixture_seal', 'gate_id', 'result'], `good[${index}]`);
    if (item.gate_id !== ids[index] || item.exit_code !== 0 || !/^sealed:(dir-)?sha256:[0-9a-f]{64}$/.test(item.fixture_seal)) die('dogfood: good');
    result(item.result, 'green', `good[${index}]`);
  }
  const bad = [['cf-m3', 'config-schema'], ['ip-m1', 'install-payload'], ['pv-m2', 'provision-script']];
  for (const [index, item] of value.mutants.entries()) {
    exact(item, ['exit_code', 'gate_id', 'mutant_id', 'seal', 'result'], `mutants[${index}]`);
    if (item.mutant_id !== bad[index][0] || item.gate_id !== bad[index][1] || item.exit_code !== 3 || !HEX.test(item.seal)) die('dogfood: mutant');
    const parsed = JSON.parse(item.result.bytes); result(item.result, parsed.outcome, `mutants[${index}]`);
    if (!['red', 'fail_closed'].includes(parsed.outcome)) die('dogfood: mutant escaped');
  }
  exact(value.cleanup, ['cargo_targets_absent', 'packaging_tracked_clean', 'scratch_absent', 'worktrees_absent'], 'cleanup');
  if (Object.values(value.cleanup).some((entry) => entry !== true)) die('dogfood: cleanup');
}

const OWNED = [
  'gates/README.md', 'gates/lib/artifact-writer.cjs', 'gates/lib/atomic-replace-win32.ps1',
  'gates/lib/card.cjs', 'gates/lib/contract.cjs', 'gates/lib/dirhash.cjs', 'gates/registry.json',
  'gates/install-payload/card.md', 'gates/install-payload/gate.cjs', 'gates/install-payload/fixtures/generators/generators.cjs',
  'gates/provision-script/card.md', 'gates/provision-script/gate.cjs', 'gates/provision-script/fixtures/generators/generators.cjs',
  'gates/config-schema/card.md', 'gates/config-schema/gate.sh', 'gates/config-schema/launcher.cjs',
  'gates/config-schema/fixtures/generators/generators.cjs', 'docs/verify/gates.md', 'gates/tests/validate-evidence.cjs',
];
function provenance(file) {
  const text = fs.readFileSync(file, 'utf8');
  for (const owned of OWNED) {
    const needle = `| \`${owned}\` |`;
    if (text.split(needle).length !== 2) die(`provenance: ${owned}`);
  }
  if (!text.includes('## WP-4 gate-card provenance')) die('provenance: section');
}
function workflow(file) {
  const text = fs.readFileSync(file, 'utf8');
  for (const required of ['\n  gate-cards:', 'runs-on: windows-latest', "require('./gates/registry.json')", 'target/debug/wayland-nano', 'verify --gate "$gate_id" --run-only']) if (!text.includes(required)) die(`workflow: ${required}`);
  if (/gates\/(install-payload|provision-script|config-schema)\/gate|npx|verify-receipt/.test(text)) die('workflow: forbidden lane');
}
function generic(stage, file) {
  const value = json(file);
  exact(value, ['complete', 'schema', 'stage', 'stop'], stage);
  if (value.schema !== `nano.wp4-${stage}/1` || value.stage !== stage || value.complete !== true || value.stop !== false) die(`${stage}: incomplete`);
}

const [stage, file] = process.argv.slice(2);
try {
  if (stage === 'dogfood') dogfood(file);
  else if (stage === 'provenance') provenance(file);
  else if (stage === 'workflow') workflow(file);
  else if (['audit', 'builder', 'ci', 'final'].includes(stage)) generic(stage, file);
  else die('usage');
  process.stdout.write(`${stage}: valid\n`);
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
