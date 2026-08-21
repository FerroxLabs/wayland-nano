#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');

const HEX = /^[0-9a-f]{64}$/;
const SHA = /^[0-9a-f]{40}$/;
const GIT_MAX_BUFFER = 256 * 1024 * 1024;
const DIFF_SIZE_CAP = 128 * 1024 * 1024;
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
      || !HEX.test(value.registry_sha256)) die('dogfood: identity');
  const ancestry = spawnSync('git', ['merge-base','--is-ancestor',value.base_sha,value.product_sha],
    { windowsHide:true, maxBuffer:GIT_MAX_BUFFER });
  if (ancestry.error || ancestry.status !== 0) die('dogfood: ancestry');
  const registryAtProduct = spawnSync('git', ['show',`${value.product_sha}:gates/registry.json`],
    { encoding:null, windowsHide:true, maxBuffer:GIT_MAX_BUFFER });
  if (registryAtProduct.error || registryAtProduct.status !== 0 || sha256(registryAtProduct.stdout) !== value.registry_sha256) die('dogfood: product registry');
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
function git(args) {
  const run = spawnSync('git', args, { encoding: 'utf8', windowsHide: true, maxBuffer: GIT_MAX_BUFFER });
  if (run.error || run.status !== 0) die(`git ${args[0]} failed`);
  return run.stdout.trim();
}
function gitBytes(args) {
  const run = spawnSync('git', args, { encoding: null, windowsHide: true, maxBuffer: GIT_MAX_BUFFER });
  if (run.error || run.status !== 0) die(`git ${args[0]} failed`);
  if (run.stdout.length > DIFF_SIZE_CAP) die('git diff exceeds evidence size cap');
  return run.stdout;
}
function bool(value, where) { if (value !== true) die(`${where}: required true`); }
function shaFile(file, expected, where) {
  if (!HEX.test(expected) || sha256(fs.readFileSync(file)) !== expected) die(`${where}: digest`);
}
function audit(file, recheck) {
  const value = json(file);
  exact(value, ['audit_id', 'authority', 'base_sha', 'diff', 'findings', 'fix_round', 'identities', 'open_critical_high',
    'owned_paths', 'product_sha', 'product_tree', 'requirements', 'review', 'schema', 'support', 'threats'], 'audit');
  if (value.schema !== 'nano.wp4-audit/1' || typeof value.audit_id !== 'string' || value.audit_id.length < 8
      || !SHA.test(value.base_sha) || !SHA.test(value.product_sha) || !SHA.test(value.product_tree)) die('audit: identity');
  if (git(['rev-parse', `${value.product_sha}^{tree}`]) !== value.product_tree) die('audit: product tree');
  exact(value.diff, ['argv', 'sha256'], 'audit.diff');
  const argv = ['diff', '--binary', '--full-index', value.base_sha, value.product_sha, '--', 'gates', 'docs/verify/gates.md', 'UPSTREAM.md'];
  if (JSON.stringify(value.diff.argv) !== JSON.stringify(argv) || sha256(gitBytes(argv)) !== value.diff.sha256) die('audit: diff');
  exact(value.review, ['path', 'sha256'], 'audit.review');
  shaFile(value.review.path, value.review.sha256, 'audit.review');
  exact(value.identities, ['auditor','builder','rechecker'], 'audit.identities');
  const identities = Object.values(value.identities);
  if (identities.some((id) => typeof id !== 'string' || !/^[a-z][a-z0-9_-]{2,63}$/.test(id))
      || new Set(identities).size !== 3) die('audit: independent identities');
  exact(value.authority, ['auditor','builder','rechecker'], 'audit.authority');
  if (value.authority.builder !== 'write:wp4-owned' || value.authority.auditor !== 'read-only'
      || value.authority.rechecker !== 'read-only') die('audit: authority');
  if (!Array.isArray(value.owned_paths) || new Set(value.owned_paths).size !== value.owned_paths.length
      || value.owned_paths.some((path) => typeof path !== 'string' || !/^(gates\/|docs\/verify\/gates\.md$|UPSTREAM\.md$)/.test(path))) die('audit: owned paths');
  const diffPaths = git(['diff','--name-only',value.base_sha,value.product_sha,'--','gates','docs/verify/gates.md','UPSTREAM.md'])
    .split(/\r?\n/).filter(Boolean).sort();
  if (JSON.stringify([...value.owned_paths].sort()) !== JSON.stringify(diffPaths)) die('audit: incomplete path scope');
  const required = ['CARD-01','CARD-02','CARD-03','CARD-04','CARD-05','CARD-06','CARD-07','CARD-08','PROV-03'];
  if (JSON.stringify(value.requirements) !== JSON.stringify(required)
      || JSON.stringify(value.threats) !== JSON.stringify(['T-07-A1','T-07-A2'])) die('audit: coverage');
  if (!Array.isArray(value.support) || value.support.length < 3) die('audit: support');
  for (const [index, item] of value.support.entries()) {
    exact(item, ['path','sha256'], `audit.support[${index}]`);
    shaFile(item.path, item.sha256, `audit.support[${index}]`);
  }
  if (!Array.isArray(value.findings) || !Number.isInteger(value.open_critical_high) || value.open_critical_high < 0
      || !Number.isInteger(value.fix_round) || value.fix_round < 0 || value.fix_round > 1) die('audit: counts');
  let open = 0;
  for (const [index, finding] of value.findings.entries()) {
    exact(finding, ['evidence', 'file', 'id', 'severity', 'status', 'title'], `finding[${index}]`);
    if (!/^H-EVID-[0-9]{2}$/.test(finding.id) || !['critical','high','medium','low'].includes(finding.severity)
        || !['open','closed'].includes(finding.status) || typeof finding.file !== 'string'
        || typeof finding.title !== 'string' || typeof finding.evidence !== 'string') die('audit: finding');
    if (['critical','high'].includes(finding.severity) && finding.status === 'open') open += 1;
  }
  if (open !== value.open_critical_high || (value.fix_round === 1 && !recheck)) die('audit: open count/recheck');
  if (recheck) {
    const text = fs.readFileSync(recheck, 'utf8');
    const block = /```json\s*([\s\S]*?)\s*```/.exec(text)?.[1];
    if (!block) die('audit: recheck block');
    const fixed = JSON.parse(block);
    exact(fixed, ['audit_id','final_sha','final_tree','fix_round','open_critical_high','rechecker','schema'], 'recheck');
    if (fixed.schema !== 'nano.wp4-recheck/1' || fixed.audit_id !== value.audit_id || fixed.fix_round !== value.fix_round
        || fixed.rechecker !== value.identities.rechecker || fixed.open_critical_high !== 0
        || git(['rev-parse', `${fixed.final_sha}^{tree}`]) !== fixed.final_tree) die('audit: recheck');
  }
}
const NAMED = ['t-card-schema-valid','t-registry-closure-digests','t-ip-reference-scores-mm','t-pv-reference-scores-mm',
  't-cf-reference-scores-mm','t-ip-mutants-caught','t-pv-mutants-caught','t-cf-mutants-caught','t-fixture-digest-fails-closed',
  't-dirhash-canonical','t-meta-mutant-passing-is-gate-defect','t-summary-contract','t-gate-hash-drift-voids-validation'];
function commandEvidence(value, command, productSha, where) {
  exact(value, ['command','exit_code','output'], where);
  if (value.command !== command || value.exit_code !== 0) die(`${where}: command/exit`);
  exact(value.output, ['bytes','sha256'], `${where}.output`);
  if (typeof value.output.bytes !== 'string' || value.output.bytes.length === 0
      || sha256(Buffer.from(value.output.bytes, 'utf8')) !== value.output.sha256) die(`${where}: output`);
  const receipt = JSON.parse(value.output.bytes);
  exact(receipt, ['command','product_sha','status'], `${where}.receipt`);
  if (receipt.command !== command || receipt.product_sha !== productSha || receipt.status !== 'passed') die(`${where}: receipt`);
}
function artifact(value, where) {
  exact(value, ['path','sha256'], where);
  shaFile(value.path, value.sha256, where);
}
function builder(file, requestFile) {
  const value = json(file);
  exact(value, ['artifacts','audit','canary','cleanup','gates','named_tests','product_sha','product_tree','schema','seeds'], 'builder');
  if (value.schema !== 'nano.wp4-builder/1' || !SHA.test(value.product_sha)
      || git(['rev-parse', `${value.product_sha}^{tree}`]) !== value.product_tree) die('builder: identity');
  exact(value.audit, ['open_critical_high','path','recheck_path','recheck_sha256','sha256'], 'builder.audit');
  if (value.audit.open_critical_high !== 0) die('builder: audit');
  artifact({path:value.audit.path,sha256:value.audit.sha256}, 'builder.audit.file');
  if ((value.audit.recheck_path === null) !== (value.audit.recheck_sha256 === null)) die('builder: recheck pair');
  if (value.audit.recheck_path !== null) artifact({path:value.audit.recheck_path,sha256:value.audit.recheck_sha256}, 'builder.audit.recheck');
  audit(value.audit.path, value.audit.recheck_path || undefined);
  const audited = json(value.audit.path);
  const auditedProduct = value.audit.recheck_path
    ? JSON.parse(/```json\s*([\s\S]*?)\s*```/.exec(fs.readFileSync(value.audit.recheck_path,'utf8'))[1]).final_sha
    : audited.product_sha;
  if (auditedProduct !== value.product_sha) die('builder: audit product mismatch');
  if (JSON.stringify(value.named_tests) !== JSON.stringify(NAMED)) die('builder: named battery');
  if (!Array.isArray(value.seeds) || JSON.stringify(value.seeds.map((entry) => entry.seed)) !== JSON.stringify([41041,41042,41043])) die('builder: seeds');
  for (const entry of value.seeds) {
    exact(entry, ['command','exit_code','output','seed'], 'builder.seed');
    commandEvidence({command:entry.command,exit_code:entry.exit_code,output:entry.output}, `node gates/tests/validate-seeded.cjs --seed ${entry.seed}`, value.product_sha, 'builder.seed');
  }
  exact(value.gates, ['cargo_deny','dogfood','just_gate_all','node'], 'builder.gates');
  const commands = { cargo_deny:'cargo deny check', dogfood:'node gates/tests/validate-evidence.cjs dogfood .planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json',
    just_gate_all:'just gate-all', node:'node --test gates/tests/*.test.cjs' };
  for (const [name, gate] of Object.entries(value.gates)) commandEvidence(gate, commands[name], value.product_sha, `builder.gates.${name}`);
  exact(value.artifacts, ['dogfood','provenance','review'], 'builder.artifacts');
  for (const [name, item] of Object.entries(value.artifacts)) artifact(item, `builder.artifacts.${name}`);
  exact(value.canary, ['bytes_scanned','files_scanned','hits','include_sha256','receipt_sha256','scratch_deleted'], 'builder.canary');
  if (!Number.isInteger(value.canary.files_scanned) || value.canary.files_scanned < 1 || !Number.isInteger(value.canary.bytes_scanned)
      || value.canary.bytes_scanned < 1 || value.canary.hits !== 0 || !HEX.test(value.canary.include_sha256)
      || !HEX.test(value.canary.receipt_sha256)) die('builder: canary');
  bool(value.canary.scratch_deleted, 'builder.canary');
  exact(value.cleanup, ['cargo_targets_absent','github_unchanged','producer_diff_empty','scratch_absent','worktrees_absent'], 'builder.cleanup');
  for (const [name, state] of Object.entries(value.cleanup)) bool(state, `builder.cleanup.${name}`);
  if (!requestFile) return;
  const request = json(requestFile);
  exact(request, ['audit_sha256','base_sha','branch','builder_actions','builder_evidence_sha256','local_gates','pending',
    'product_sha','request_tip','requested','schema'], 'request');
  if (request.schema !== 'nano.wp4-promotion-request/1' || request.product_sha !== value.product_sha || !SHA.test(request.base_sha)
      || !SHA.test(request.request_tip) || typeof request.branch !== 'string' || !HEX.test(request.audit_sha256)
      || !HEX.test(request.builder_evidence_sha256) || request.builder_evidence_sha256 !== sha256(fs.readFileSync(file))) die('request: identity');
  exact(request.local_gates, ['cargo_deny','dogfood','just_gate_all','node','provenance'], 'request.local_gates');
  for (const state of Object.values(request.local_gates)) bool(state, 'request.local_gates');
  exact(request.builder_actions, ['github_modified','merged','pushed'], 'request.builder_actions');
  if (Object.values(request.builder_actions).some((state) => state !== false)) die('request: builder exceeded authority');
  exact(request.pending, ['ci','merge_sha','run_id','workflow_sha'], 'request.pending');
  if (Object.values(request.pending).some((state) => state !== null && state !== 'pending')) die('request: premature result');
  exact(request.requested, ['ci_jobs','merge','no_ff','push_fetch','workflow_job'], 'request.requested');
  const jobs = ['gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)',
    'gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards'];
  if (request.requested.merge !== 'integrator-only' || request.requested.no_ff !== true || request.requested.push_fetch !== true
      || request.requested.workflow_job !== 'gate-cards' || JSON.stringify(request.requested.ci_jobs) !== JSON.stringify(jobs)) die('request: topology');
}
const CI_JOBS = ['gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)',
  'gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards'];
function ci(file, expectedSha) {
  const value = json(file);
  exact(value, ['conclusion','headSha','jobs','status'], 'ci');
  if (!SHA.test(expectedSha) || value.headSha !== expectedSha || value.status !== 'completed' || value.conclusion !== 'success'
      || !Array.isArray(value.jobs) || value.jobs.length !== CI_JOBS.length) die('ci: run');
  const names = value.jobs.map((job) => job.name).sort();
  if (JSON.stringify(names) !== JSON.stringify([...CI_JOBS].sort())) die('ci: jobs');
  for (const job of value.jobs) {
    exact(job, ['completedAt','conclusion','databaseId','name','startedAt','status','steps','url'], 'ci.job');
    if (job.status !== 'completed' || job.conclusion !== 'success' || !Number.isInteger(job.databaseId)
        || typeof job.startedAt !== 'string' || typeof job.completedAt !== 'string' || typeof job.url !== 'string' || !Array.isArray(job.steps)) die('ci: job state');
    for (const step of job.steps) exact(step, ['completedAt','conclusion','name','number','startedAt','status'], 'ci.step');
  }
}
function finalEvidence(file, ciFile) {
  const text = fs.readFileSync(file, 'utf8');
  const blocks = [...text.matchAll(/```json\s*([\s\S]*?)\s*```/g)];
  if (blocks.length !== 1) die('final: closed block');
  const value = JSON.parse(blocks[0][1]);
  exact(value, ['builder_sha','canary','ci_sha256','evid','integration_sha','owner_handoff','schema','stop'], 'final');
  if (value.schema !== 'nano.wp4-final/1' || !SHA.test(value.builder_sha) || !SHA.test(value.integration_sha)) die('final: identity');
  shaFile(ciFile, value.ci_sha256, 'final.ci');
  exact(value.evid, ['EVID-01','EVID-02','EVID-03'], 'final.evid');
  for (const state of Object.values(value.evid)) bool(state, 'final.evid');
  exact(value.canary, ['hits','scratch_deleted'], 'final.canary');
  if (value.canary.hits !== 0) die('final: canary'); bool(value.canary.scratch_deleted, 'final.canary');
  exact(value.stop, ['phase8_absent','wp5_absent','wp6_absent'], 'final.stop');
  for (const state of Object.values(value.stop)) bool(state, 'final.stop');
  if (value.owner_handoff !== true || /pending|todo|tbd/i.test(text)) die('final: incomplete');
}

const [stage, file] = process.argv.slice(2);
try {
  if (stage === 'dogfood') dogfood(file);
  else if (stage === 'provenance') provenance(file);
  else if (stage === 'workflow') workflow(file);
  else if (stage === 'audit') audit(file, process.argv[4]);
  else if (stage === 'builder') builder(file, process.argv[4]);
  else if (stage === 'ci') ci(file, process.argv[4] === '--expected-sha' ? process.argv[5] : '');
  else if (stage === 'final') finalEvidence(file, process.argv[4]);
  else die('usage');
  process.stdout.write(`${stage}: valid\n`);
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
