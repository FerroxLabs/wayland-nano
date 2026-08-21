#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');

const HEX = /^[0-9a-f]{64}$/;
const SHA = /^[0-9a-f]{40}$/;
const GIT_MAX_BUFFER = 256 * 1024 * 1024;
const DIFF_SIZE_CAP = 128 * 1024 * 1024;
const DEVIATION_PATH = '.planning/phases/07-wp-4-gate-cards-and-dogfood/07-UPSTREAM-DEVIATIONS.md';
const DEVIATION_COMMITS = ['e199fcbda37cd3d7ac9234dfb9f20b4fe2f9b97b','d5c7f10a9865218a7ec50a36e91ad8cda74aa3e5','076270079f4fc5fac3f1d664731ccfbd5cb1b25a'];
const DEVIATION_PATHS = ['crates/nano-cli/src/verify_cmd.rs','crates/nano-cli/tests/p4_rules.rs','crates/nano-cli/tests/verify_cmd.rs',
  'crates/nano-core/src/execrules.rs','crates/nano-core/tests/execrules.rs'];
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
function dogfood(file, deps = {}) {
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
  if(Object.values(value.cleanup).some((entry)=>entry!==true)) die('dogfood: cleanup claims');
  const observed=(deps.execute || executeDogfood)();
  if (JSON.stringify(value.good)!==JSON.stringify(observed.good)
      || JSON.stringify(value.mutants)!==JSON.stringify(observed.mutants)
      || Object.keys(value.cleanup).some((key)=>value.cleanup[key]!==observed.cleanup?.[key])) die('dogfood: independently observed mismatch');
}

function fixedRun(program,args,options={}) {
  const run=spawnSync(program,args,{cwd:options.cwd||process.cwd(),env:options.env||process.env,encoding:'utf8',windowsHide:true,
    timeout:options.timeout||900_000,maxBuffer:GIT_MAX_BUFFER});
  if(run.error||run.status!==(options.expected??0)) die(`dogfood execution failed: ${program}`);
  return run.stdout.trim();
}
function gateResult(binary,cwd,id,temp) {
  const bytes=fixedRun(binary,['verify','--gate',id,'--run-only','--json'],{cwd,env:{...process.env,TEMP:temp,TMP:temp},expected:id.startsWith('bad:')?3:0});
  return bytes;
}
function sealResult(bytes) { return {bytes,sha256:sha256(Buffer.from(bytes,'utf8'))}; }
function performDogfoodCleanup(resources,deps={}) {
  const spawn=deps.spawn||spawnSync, exists=deps.exists||fs.existsSync, remove=deps.remove||((path)=>fs.rmSync(path,{recursive:true,force:true}));
  const errors=[];
  for(const wt of resources.worktrees){
    if(!exists(wt)) continue;
    const run=spawn('git',['worktree','remove','--force',wt],{cwd:resources.repo,encoding:'utf8',windowsHide:true,maxBuffer:GIT_MAX_BUFFER});
    if(run.error||run.status!==0) errors.push(`remove:${require('node:path').basename(wt)}`);
  }
  const prune=spawn('git',['worktree','prune'],{cwd:resources.repo,encoding:'utf8',windowsHide:true,maxBuffer:GIT_MAX_BUFFER});
  if(prune.error||prune.status!==0) errors.push('prune');
  try { if(exists(resources.scratch)) remove(resources.scratch); } catch { errors.push('scratch-remove'); }
  for(const path of resources.fixedPaths) if(exists(path)) errors.push(`residue:${require('node:path').basename(path)}`);
  const listed=spawn('git',['worktree','list','--porcelain'],{cwd:resources.repo,encoding:'utf8',windowsHide:true,maxBuffer:GIT_MAX_BUFFER});
  if(listed.error||listed.status!==0) errors.push('enumerate');
  else if(resources.worktrees.some((wt)=>listed.stdout.includes(wt))) errors.push('registration-residue');
  if(errors.length) die(`dogfood cleanup failed: ${errors.slice(0,8).join(',')}`);
  return {cargo_targets_absent:true,packaging_tracked_clean:true,scratch_absent:true,worktrees_absent:true};
}
function executeDogfood(deps={}) {
  if(process.platform!=='win32') die('dogfood execution requires Windows');
  const path=require('node:path'); const root=process.cwd();
  const scratch=path.join('F:\\',`wayland-nano-wp4-dogfood-validator-${process.pid}`); const goodRoot=path.join(scratch,'good-wt');
  const target=path.join(scratch,'good-target'); const temp=path.join(scratch,'temp');
  const targetLink=path.join(goodRoot,'target'); const packageBins=path.join(goodRoot,'packaging','npm','binaries');
  const packageManifest=path.join(goodRoot,'packaging','npm','binaries-manifest.json');
  if(fs.existsSync(scratch)) die('dogfood cleanup root not pristine');
  const {directorySeal}=require('../lib/dirhash.cjs');
  const resources={repo:root,scratch,worktrees:['cf-wt','pv-wt','good-wt'].map((name)=>path.join(scratch,name)),
    fixedPaths:[scratch,target,temp,path.join(scratch,'cf-target'),path.join(scratch,'pv-target'),goodRoot,targetLink,packageBins,packageManifest]};
  let outcome; let primary; let cleanup; let cleanupError;
  try {
    fs.mkdirSync(temp,{recursive:true});
    fixedRun('git',['worktree','add','--detach',goodRoot,'HEAD']);
    fixedRun('cargo',['build','-p','nano-cli','-p','nano-sandbox','--bins','--target-dir',target],{cwd:goodRoot,timeout:1_200_000});
    fixedRun('cargo',['build','-p','nano-sandbox','--bin','wayland-nano-provision-dry-run','--bin','wayland-nano-sandbox-setup','--target-dir',target],{cwd:goodRoot,timeout:600_000});
    fixedRun('pwsh',['-NoProfile','-File','packaging/npm/scripts/pack.ps1','-Platform','all','-ArtifactRoot','gates/fixtures/install-payload/reference/binaries'],{cwd:goodRoot});
    fs.symlinkSync(target,targetLink,'junction');
    const binary=path.join(target,'debug','wayland-nano.exe');
    const ids=['config-schema','install-payload','provision-script'];
    const good=ids.map((gate_id)=>{const bytes=gateResult(binary,goodRoot,gate_id,temp);return {gate_id,
      fixture_seal:gate_id==='install-payload'?directorySeal(path.join(goodRoot,'packaging','npm')):
        directorySeal(path.join(goodRoot,'gates','fixtures',gate_id,gate_id==='config-schema'?'probes':'reference')),
      exit_code:0,result:sealResult(bytes)}});
    fs.copyFileSync(path.join(packageBins,'win32-x64','wayland-nano.exe'),path.join(packageBins,'linux-x64','wayland-nano'));
    const ipBytes=fixedRun(binary,['verify','--gate','install-payload','--run-only','--json'],{cwd:goodRoot,env:{...process.env,TEMP:temp,TMP:temp},expected:3});
    const ipSeal=directorySeal(path.join(goodRoot,'packaging','npm')).slice('sealed:dir-sha256:'.length);
    fixedRun('pwsh',['-NoProfile','-File','packaging/npm/scripts/pack.ps1','-Platform','all','-ArtifactRoot','gates/fixtures/install-payload/reference/binaries'],{cwd:goodRoot});
    const mutations=[['cf','gates/fixtures/config-schema/mutants/cf-m3/mutant.diff'],['pv',null]];
    const bad={ip:{bytes:ipBytes,seal:ipSeal}};
    for(const [kind,patch] of mutations){
      const wt=path.join(scratch,`${kind}-wt`), out=path.join(scratch,`${kind}-target`);
      fixedRun('git',['worktree','add','--detach',wt,'HEAD']);
      if(patch) fixedRun('git',['-C',wt,'apply',path.join(root,patch)]);
      else { const file=path.join(wt,'crates','nano-sandbox','src','setup_types.rs'); fs.writeFileSync(file,fs.readFileSync(file,'utf8').replace('NanoSandboxOffline','NanoOffline').replace('NanoSandboxOnline','NanoOnline')); }
      if(kind==='cf') fixedRun('cargo',['build','--manifest-path',path.join(wt,'Cargo.toml'),'-p','nano-cli','--target-dir',out],{timeout:1_200_000});
      else { fixedRun('cargo',['build','--manifest-path',path.join(wt,'Cargo.toml'),'-p','nano-cli','-p','nano-sandbox','--bins','--target-dir',out],{timeout:1_200_000}); fixedRun('cargo',['build','--manifest-path',path.join(wt,'Cargo.toml'),'-p','nano-sandbox','--bin','wayland-nano-provision-dry-run','--bin','wayland-nano-sandbox-setup','--target-dir',out],{timeout:600_000}); }
      fs.symlinkSync(out,path.join(wt,'target'),'junction');
      const id=kind==='cf'?'config-schema':'provision-script';
      const bytes=fixedRun(path.join(out,'debug','wayland-nano.exe'),['verify','--gate',id,'--run-only','--json'],{cwd:wt,env:{...process.env,TEMP:temp,TMP:temp},expected:3});
      const diff=spawnSync('git',['-C',wt,'diff','--binary','--full-index'],{encoding:null,maxBuffer:GIT_MAX_BUFFER});
      if(diff.error||diff.status!==0) die('dogfood mutation digest'); bad[kind]={bytes,seal:sha256(diff.stdout)};
    }
    const mutants=[{mutant_id:'cf-m3',gate_id:'config-schema',seal:bad.cf.seal,exit_code:3,result:sealResult(bad.cf.bytes)},
      {mutant_id:'ip-m1',gate_id:'install-payload',seal:bad.ip.seal,exit_code:3,result:sealResult(bad.ip.bytes)},
      {mutant_id:'pv-m2',gate_id:'provision-script',seal:bad.pv.seal,exit_code:3,result:sealResult(bad.pv.bytes)}];
    outcome={good,mutants};
  } catch(error) { primary=error; }
  finally { try { cleanup=performDogfoodCleanup(resources,deps.cleanup); } catch(error) { cleanupError=error; } }
  if(primary){ if(cleanupError) primary.message=`${primary.message}; ${cleanupError.message}`.slice(0,512); throw primary; }
  if(cleanupError) throw cleanupError;
  return {...outcome,cleanup};
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
function deviationAuthority(baseSha,productSha,value) {
  exact(value,['commits','path','paths','sha256'],'deviations');
  if(value.path!==DEVIATION_PATH||JSON.stringify(value.commits)!==JSON.stringify(DEVIATION_COMMITS)
      ||JSON.stringify(value.paths)!==JSON.stringify(DEVIATION_PATHS)) die('deviations: exact authority');
  shaFile(value.path,value.sha256,'deviations');
  for(const commit of DEVIATION_COMMITS){
    const ancestry=spawnSync('git',['merge-base','--is-ancestor',commit,productSha],{windowsHide:true,maxBuffer:GIT_MAX_BUFFER});
    if(ancestry.error||ancestry.status!==0) die('deviations: owner commit ancestry');
  }
  const phasePaths=git(['diff','--name-only',baseSha,productSha,'--','crates']).split(/\r?\n/).filter(Boolean).sort();
  if(JSON.stringify(phasePaths)!==JSON.stringify(DEVIATION_PATHS)) die('deviations: phase crate set');
  const byCommit=DEVIATION_COMMITS.flatMap((commit)=>git(['diff-tree','--no-commit-id','--name-only','-r',commit])
    .split(/\r?\n/).filter((path)=>path.startsWith('crates/'))).sort();
  if(JSON.stringify(byCommit)!==JSON.stringify(DEVIATION_PATHS)) die('deviations: commit attribution');
}
function audit(file, recheck) {
  const value = json(file);
  exact(value, ['audit_id', 'authority', 'base_sha', 'deviations', 'diff', 'findings', 'fix_round', 'identities', 'open_critical_high',
    'owned_paths', 'product_sha', 'product_tree', 'requirements', 'review', 'schema', 'support', 'threats'], 'audit');
  if (value.schema !== 'nano.wp4-audit/1' || typeof value.audit_id !== 'string' || value.audit_id.length < 8
      || !SHA.test(value.base_sha) || !SHA.test(value.product_sha) || !SHA.test(value.product_tree)) die('audit: identity');
  if (git(['rev-parse', `${value.product_sha}^{tree}`]) !== value.product_tree) die('audit: product tree');
  deviationAuthority(value.base_sha,value.product_sha,value.deviations);
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
const DOGFOOD_PATH = '.planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json';
const TOOLS_ROOT = 'F:\\w4e-tools';
const BUILD_COMMAND = 'cargo build -p nano-cli -p nano-sandbox --bins --target-dir F:/w4e-tools';
const COMMANDS = [BUILD_COMMAND,'node --test gates/tests/*.test.cjs',
  'node gates/tests/validate-seeded.cjs --seed 41041','node gates/tests/validate-seeded.cjs --seed 41042',
  'node gates/tests/validate-seeded.cjs --seed 41043',`node gates/tests/validate-evidence.cjs dogfood ${DOGFOOD_PATH}`,
  'node gates/tests/validate-evidence.cjs provenance UPSTREAM.md','just gate-all','cargo deny check'];
function commandSpec(command) {
  if (command === COMMANDS[0]) return { program:'cargo',args:['build','-p','nano-cli','-p','nano-sandbox','--bins','--target-dir','F:/w4e-tools'],timeout:1_200_000 };
  if (command === COMMANDS[1]) return { program:process.execPath,args:['--test','gates/tests/*.test.cjs'],timeout:900_000 };
  const seed = /^node gates\/tests\/validate-seeded\.cjs --seed (4104[123])$/.exec(command)?.[1];
  if (seed) return { program:process.execPath,args:['gates/tests/validate-seeded.cjs','--seed',seed],timeout:600_000 };
  if (command === COMMANDS[5]) return { program:process.execPath,args:['gates/tests/validate-evidence.cjs','dogfood',DOGFOOD_PATH],timeout:1_200_000 };
  if (command === COMMANDS[6]) return { program:process.execPath,args:['gates/tests/validate-evidence.cjs','provenance','UPSTREAM.md'],timeout:30_000 };
  if (command === COMMANDS[7]) return { program:'just',args:['gate-all'],timeout:1_800_000 };
  if (command === COMMANDS[8]) return { program:'cargo',args:['deny','check'],timeout:600_000 };
  die('builder: forbidden command');
}
function defaultRun(command) {
  const spec=commandSpec(command);
  const env={...process.env,CARGO_TARGET_DIR:TOOLS_ROOT,NANO_CLI_BIN:`${TOOLS_ROOT}\\debug\\wayland-nano.exe`,
    NANO_DRY_RUN_BIN:`${TOOLS_ROOT}\\debug\\wayland-nano-provision-dry-run.exe`,NANO_SETUP_BIN:`${TOOLS_ROOT}\\debug\\wayland-nano-sandbox-setup.exe`};
  return spawnSync(spec.program,spec.args,{cwd:process.cwd(),env,encoding:'utf8',windowsHide:true,timeout:spec.timeout,maxBuffer:GIT_MAX_BUFFER});
}
function controlledEnv(root=TOOLS_ROOT){return {CARGO_TARGET_DIR:root,NANO_CLI_BIN:`${root}\\debug\\wayland-nano.exe`,
  NANO_DRY_RUN_BIN:`${root}\\debug\\wayland-nano-provision-dry-run.exe`,NANO_SETUP_BIN:`${root}\\debug\\wayland-nano-sandbox-setup.exe`};}
function withNodeJunction(command,run,toolsRoot,deps={}) {
  if((deps.platform||process.platform)!=='win32') return run(command,{env:controlledEnv(toolsRoot)});
  const path=require('node:path'), target=path.join(process.cwd(),'target');
  const exists=deps.exists||fs.existsSync;
  if(exists(target)) die('builder: repository target preexists');
  const create=deps.create||((link,destination)=>spawnSync('powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command',
    '$ErrorActionPreference="Stop"; New-Item -ItemType Junction -LiteralPath $args[0] -Target $args[1] | Out-Null',link,destination],
    {encoding:'utf8',windowsHide:true,timeout:30_000,maxBuffer:GIT_MAX_BUFFER}));
  const remove=deps.remove||((link)=>spawnSync('powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command',
    '$ErrorActionPreference="Stop"; [IO.Directory]::Delete($args[0],$false)',link],
    {encoding:'utf8',windowsHide:true,timeout:30_000,maxBuffer:GIT_MAX_BUFFER}));
  const identity=deps.identity||((link,destination)=>fs.lstatSync(link).isSymbolicLink()
    && path.resolve(fs.realpathSync(link)).toLowerCase()===path.resolve(destination).toLowerCase());
  const made=create(target,toolsRoot);
  if(!made||made.error||made.status!==0) die('builder: target junction create failed');
  let primary; let result; let cleanup;
  try {
    if(!exists(target)||!identity(target,toolsRoot)) die('builder: target junction identity');
    result=run(command,{env:controlledEnv(toolsRoot)});
  } catch(error){primary=error;}
  finally {
    const removed=remove(target);
    if(!removed||removed.error||removed.status!==0||exists(target)) cleanup=new Error('builder: target junction cleanup failed');
  }
  if(primary){if(cleanup)primary.message=`${primary.message}; ${cleanup.message}`.slice(0,512);throw primary;}
  if(cleanup)throw cleanup;
  return result;
}
function artifact(value, where) {
  exact(value, ['path','sha256'], where);
  shaFile(value.path,value.sha256,where);
}
function defaultCanary(files) {
  const temp=process.env.TEMP || process.env.TMP;
  if (!temp || !/^F:[\\/]/i.test(temp)) die('builder: F canary temp');
  const dir=fs.mkdtempSync(require('node:path').join(temp,'wp4-builder-canary-'));
  const list=require('node:path').join(dir,'include.json'); const receipt=require('node:path').join(dir,'receipt.json');
  try {
    fs.writeFileSync(list,JSON.stringify(files));
    const run=spawnSync(process.execPath,['scripts/canary/scan.mjs','--include-list',list,'--receipt',receipt],
      {cwd:process.cwd(),encoding:'utf8',windowsHide:true,timeout:30_000,maxBuffer:GIT_MAX_BUFFER});
    if (run.error || run.status!==0) die('builder: canary execution');
    const value=json(receipt);
    if (value.hits!==0 || value.files_scanned!==files.length) die('builder: canary result');
    return {sha256:sha256(fs.readFileSync(receipt)),files:value.files_scanned,bytes:value.bytes_scanned};
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
}
function builder(file, requestFile, deps = {}) {
  const value = json(file);
  exact(value, ['audit','canary_files','cleanup_paths','commands','named_tests','product_sha','product_tree','schema'], 'builder');
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
  if (JSON.stringify(value.named_tests) !== JSON.stringify(NAMED) || JSON.stringify(value.commands) !== JSON.stringify(COMMANDS)) die('builder: exact battery');
  const dogfood=json(DOGFOOD_PATH);
  if (dogfood.product_sha !== value.product_sha) die('builder: dogfood must equal product head');
  const run=deps.run || defaultRun; const toolsRoot=deps.toolsRoot||TOOLS_ROOT;
  if(fs.existsSync(toolsRoot)) die('builder: owned tools root not pristine');
  try { for (const command of COMMANDS) {
    commandSpec(command);
    const result=command===COMMANDS[1]?withNodeJunction(command,run,toolsRoot,deps.junction):run(command,{env:controlledEnv(toolsRoot)});
    if (!result || result.error || result.status !== 0 || result.signal) die(`builder: controlled command failed: ${command}`);
    const output=`${result.stdout || ''}${result.stderr || ''}`;
    if (Buffer.byteLength(output)>DIFF_SIZE_CAP) die('builder: command output cap');
    if (command===COMMANDS[1]) for (const name of NAMED) {
      const hits=output.split(name).length-1;
      if (hits!==1) die(`builder: named test count ${name}`);
    }
  }} finally { if(fs.existsSync(toolsRoot)) fs.rmSync(toolsRoot,{recursive:true,force:true}); }
  const expectedCanary=[DOGFOOD_PATH,value.audit.path,...(value.audit.recheck_path?[value.audit.recheck_path]:[]),'UPSTREAM.md','docs/verify/gates.md'].sort();
  if (JSON.stringify([...value.canary_files].sort())!==JSON.stringify(expectedCanary)) die('builder: canary inventory');
  for (const path of value.canary_files) if (!fs.statSync(path).isFile()) die('builder: canary file');
  (deps.canary || defaultCanary)(value.canary_files);
  if (JSON.stringify(value.cleanup_paths)!==JSON.stringify([TOOLS_ROOT])) die('builder: exact cleanup root');
  deviationAuthority(audited.base_sha,value.product_sha,audited.deviations);
  if (git(['diff','--name-only',audited.base_sha,value.product_sha,'--','packaging','scripts/provision','.github','docs/STATUS.md'])) die('builder: producer ownership');
  if (!requestFile) return;
  const request = json(requestFile);
  exact(request, ['audit_sha256','base_sha','branch','builder_actions','builder_evidence_sha256','local_commands','pending',
    'product_sha','request_tip','requested','schema'], 'request');
  if (request.schema !== 'nano.wp4-promotion-request/1' || request.product_sha !== value.product_sha || !SHA.test(request.base_sha)
      || !SHA.test(request.request_tip) || typeof request.branch !== 'string' || !HEX.test(request.audit_sha256)
      || !HEX.test(request.builder_evidence_sha256) || request.builder_evidence_sha256 !== sha256(fs.readFileSync(file))) die('request: identity');
  if (JSON.stringify(request.local_commands)!==JSON.stringify(COMMANDS)) die('request: local commands');
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

function main(argv=process.argv.slice(2)) {
const [stage, file] = argv;
try {
  if (stage === 'dogfood') dogfood(file);
  else if (stage === 'provenance') provenance(file);
  else if (stage === 'workflow') workflow(file);
  else if (stage === 'audit') audit(file, argv[2]);
  else if (stage === 'builder') builder(file, argv[2]);
  else if (stage === 'ci') ci(file, argv[2] === '--expected-sha' ? argv[3] : '');
  else if (stage === 'final') finalEvidence(file, argv[2]);
  else die('usage');
  process.stdout.write(`${stage}: valid\n`);
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
}
module.exports={builder,dogfood,executeDogfood,performDogfoodCleanup,withNodeJunction,commandSpec,controlledEnv,COMMANDS,NAMED,TOOLS_ROOT};
if(require.main===module) main();
