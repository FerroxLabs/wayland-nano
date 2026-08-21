'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');
const { writeArtifact } = require('../lib/artifact-writer.cjs');
const validator = require('./validate-evidence.cjs');

const ROOT = path.resolve(__dirname, '..', '..');
const VALIDATOR = path.join(__dirname, 'validate-evidence.cjs');
const sha = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
const run = (args) => spawnSync(process.execPath, [VALIDATOR, ...args], { cwd: ROOT, encoding: 'utf8' });
const git = (args) => spawnSync('git', args, { cwd: ROOT, encoding: 'utf8' }).stdout.trim();
const gitBytes = (args) => spawnSync('git', args, { cwd: ROOT, encoding: null, maxBuffer: 256 * 1024 * 1024 }).stdout;
const PHASE_BASE='05637086c81e88550edb002a916a80aff4b278dc';
const DEV_COMMITS=['e199fcbda37cd3d7ac9234dfb9f20b4fe2f9b97b','d5c7f10a9865218a7ec50a36e91ad8cda74aa3e5','076270079f4fc5fac3f1d664731ccfbd5cb1b25a'];
const DEV_PATHS=['crates/nano-cli/src/verify_cmd.rs','crates/nano-cli/tests/p4_rules.rs','crates/nano-cli/tests/verify_cmd.rs','crates/nano-core/src/execrules.rs','crates/nano-core/tests/execrules.rs'];
const deviationRecord=()=>({path:'.planning/phases/07-wp-4-gate-cards-and-dogfood/07-UPSTREAM-DEVIATIONS.md',
  sha256:sha(fs.readFileSync(path.join(ROOT,'.planning/phases/07-wp-4-gate-cards-and-dogfood/07-UPSTREAM-DEVIATIONS.md'))),commits:DEV_COMMITS,paths:DEV_PATHS});
async function put(dir, name, value) {
  const file = path.join(dir, name);
  await writeArtifact(file, Buffer.from(typeof value === 'string' ? value : `${JSON.stringify(value)}\n`));
  return file;
}

test('evidence validators reject marker-only missing extra and mismatched payloads', async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TEMP || os.tmpdir(), 'wp4-evidence-'));
  try {
    for (const stage of ['audit','builder','ci']) {
      const marker = await put(dir, `${stage}-marker.json`, { schema:`nano.wp4-${stage}/1`,stage,complete:true,stop:false });
      assert.notEqual(run([stage, marker, '--expected-sha', '0'.repeat(40)]).status, 0, stage);
    }
    const extraDogfood = JSON.parse(fs.readFileSync(path.join(ROOT, '.planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json')));
    extraDogfood.spoof = true;
    assert.notEqual(run(['dogfood', await put(dir, 'dogfood-extra.json', extraDogfood)]).status, 0);
  } finally { fs.rmSync(dir, { recursive:true, force:true }); }
});

test('dogfood ignores claimed success and cleanup while requiring independent exact execution', async () => {
  const dir=fs.mkdtempSync(path.join(process.env.TEMP||os.tmpdir(),'wp4-dogfood-di-'));
  try {
    const source=JSON.parse(fs.readFileSync(path.join(ROOT,'.planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json')));
    const file=await put(dir,'valid.json',source); let executions=0;
    assert.doesNotThrow(()=>validator.dogfood(file,{execute:()=>{executions+=1;return {good:source.good,mutants:source.mutants}}}));
    assert.equal(executions,1);
    const forged=structuredClone(source); forged.good[0].result.bytes='{"outcome":"green","v":1,"verdicts":[]}';
    forged.good[0].result.sha256=sha(Buffer.from(forged.good[0].result.bytes));
    const forgedFile=await put(dir,'forged.json',forged);
    assert.throws(()=>validator.dogfood(forgedFile,{execute:()=>({good:source.good,mutants:source.mutants})}));
    const cleanup=structuredClone(source); cleanup.cleanup.paths=['F:/caller-chosen'];
    const cleanupFile=await put(dir,'cleanup.json',cleanup); let ran=false;
    assert.throws(()=>validator.dogfood(cleanupFile,{execute:()=>{ran=true;return {good:source.good,mutants:source.mutants}}}));
    assert.equal(ran,false);
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});

test('audit validator recomputes identity diff review and open high count', async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TEMP || os.tmpdir(), 'wp4-audit-'));
  try {
    const head = git(['rev-parse','HEAD']);
    const tree = git(['rev-parse','HEAD^{tree}']);
    const review = await put(dir, 'review.md', '# Independent review\n');
    const argv = ['diff','--binary','--full-index',PHASE_BASE,head,'--','gates','docs/verify/gates.md','UPSTREAM.md'];
    const owned=git(['diff','--name-only',PHASE_BASE,head,'--','gates','docs/verify/gates.md','UPSTREAM.md']).split(/\r?\n/).filter(Boolean);
    const value = { schema:'nano.wp4-audit/1', audit_id:'audit-test-01', base_sha:PHASE_BASE, product_sha:head, product_tree:tree,deviations:deviationRecord(),
      diff:{argv,sha256:sha(gitBytes(argv))}, review:{path:review,sha256:sha(fs.readFileSync(review))},
      owned_paths:owned, requirements:['CARD-01','CARD-02','CARD-03','CARD-04','CARD-05','CARD-06','CARD-07','CARD-08','PROV-03'],
      identities:{builder:'builder-wp4',auditor:'auditor-wp4',rechecker:'rechecker-wp4'},
      authority:{builder:'write:wp4-owned',auditor:'read-only',rechecker:'read-only'},
      support:[review,'gates/registry.json','docs/verify/gates.md'].map(file=>({path:file,sha256:sha(fs.readFileSync(file))})),
      threats:['T-07-A1','T-07-A2'], findings:[], open_critical_high:0, fix_round:0 };
    const file = await put(dir, 'audit.json', value);
    const validAudit=run(['audit',file]);assert.equal(validAudit.status,0,validAudit.stderr);
    value.product_tree='0'.repeat(40); assert.notEqual(run(['audit',await put(dir,'bad-tree.json',value)]).status,0);
    value.product_tree=tree; value.fix_round=2; assert.notEqual(run(['audit',await put(dir,'bad-round.json',value)]).status,0);
    value.fix_round=0; value.identities.rechecker=value.identities.auditor;
    assert.notEqual(run(['audit',await put(dir,'same-reviewer.json',value)]).status,0);
    value.identities.rechecker='rechecker-wp4'; value.extra=true; assert.notEqual(run(['audit',await put(dir,'extra.json',value)]).status,0);
    delete value.extra;
    for(const [name,paths] of [['zero',[]],['missing',DEV_PATHS.slice(0,-1)],['sixth',[...DEV_PATHS,'crates/nano-core/src/lib.rs']],['modified',[...DEV_PATHS.slice(0,-1),'crates/nano-core/src/lib.rs']]]){
      const bad=structuredClone(value);bad.deviations.paths=paths;
      assert.notEqual(run(['audit',await put(dir,`${name}-deviation.json`,bad)]).status,0,name);
    }
    const badDigest=structuredClone(value);badDigest.deviations.sha256='0'.repeat(64);
    assert.notEqual(run(['audit',await put(dir,'modified-deviation-doc.json',badDigest)]).status,0);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});

test('git evidence collection has explicit high-volume buffer and bounded diff cap', () => {
  const source=fs.readFileSync(VALIDATOR,'utf8');
  assert.match(source,/GIT_MAX_BUFFER = 256 \* 1024 \* 1024/);
  assert.match(source,/DIFF_SIZE_CAP = 128 \* 1024 \* 1024/);
  assert.match(source,/run\.stdout\.length > DIFF_SIZE_CAP/);
});

test('CI validator requires exact SHA literal seven jobs and completed success', async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TEMP || os.tmpdir(), 'wp4-ci-'));
  try {
    const head=git(['rev-parse','HEAD']);
    const names=['gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards'];
    const jobs=names.map((name,index)=>({databaseId:index+1,name,status:'completed',conclusion:'success',startedAt:'2026-01-01T00:00:00Z',completedAt:'2026-01-01T00:01:00Z',url:'https://example.invalid/job',steps:[]}));
    const value={headSha:head,status:'completed',conclusion:'success',jobs};
    assert.equal(run(['ci',await put(dir,'ci.json',value),'--expected-sha',head]).status,0);
    value.jobs=value.jobs.slice(0,-1); assert.notEqual(run(['ci',await put(dir,'missing.json',value),'--expected-sha',head]).status,0);
    value.jobs=jobs; value.jobs[0]={...value.jobs[0],conclusion:'skipped'}; assert.notEqual(run(['ci',await put(dir,'skipped.json',value),'--expected-sha',head]).status,0);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});

test('builder request and final contracts reject authority and stop spoofing', async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TEMP || os.tmpdir(), 'wp4-builder-'));
  try {
    const dogfoodPath='.planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json';
    const head=git(['rev-parse','HEAD']); const product=JSON.parse(fs.readFileSync(dogfoodPath)).product_sha;
    const tree=git(['rev-parse',`${product}^{tree}`]); const hex='a'.repeat(64);
    const reviewMd=await put(dir,'review.md','# Review\n'); const provenancePath='UPSTREAM.md';
    const diffArgv=['diff','--binary','--full-index',PHASE_BASE,product,'--','gates','docs/verify/gates.md','UPSTREAM.md'];
    const owned=git(['diff','--name-only',PHASE_BASE,product,'--','gates','docs/verify/gates.md','UPSTREAM.md']).split(/\r?\n/).filter(Boolean);
    const auditValue={schema:'nano.wp4-audit/1',audit_id:'audit-builder-01',base_sha:PHASE_BASE,product_sha:product,product_tree:tree,deviations:deviationRecord(),
      diff:{argv:diffArgv,sha256:sha(gitBytes(diffArgv))},review:{path:reviewMd,sha256:sha(fs.readFileSync(reviewMd))},owned_paths:owned,
      requirements:['CARD-01','CARD-02','CARD-03','CARD-04','CARD-05','CARD-06','CARD-07','CARD-08','PROV-03'],
      identities:{builder:'builder-wp4',auditor:'auditor-wp4',rechecker:'rechecker-wp4'},authority:{builder:'write:wp4-owned',auditor:'read-only',rechecker:'read-only'},
      support:[reviewMd,'gates/registry.json','docs/verify/gates.md'].map(file=>({path:file,sha256:sha(fs.readFileSync(file))})),threats:['T-07-A1','T-07-A2'],findings:[],open_critical_high:0,fix_round:0};
    const reviewPath=await put(dir,'review.json',auditValue);
    const builder={schema:'nano.wp4-builder/1',product_sha:product,product_tree:tree,
      audit:{open_critical_high:0,path:reviewPath,sha256:sha(fs.readFileSync(reviewPath)),recheck_path:null,recheck_sha256:null}, named_tests:['t-card-schema-valid','t-registry-closure-digests','t-ip-reference-scores-mm','t-pv-reference-scores-mm','t-cf-reference-scores-mm','t-ip-mutants-caught','t-pv-mutants-caught','t-cf-mutants-caught','t-fixture-digest-fails-closed','t-dirhash-canonical','t-meta-mutant-passing-is-gate-defect','t-summary-contract','t-gate-hash-drift-voids-validation'],
      commands:validator.COMMANDS,canary_files:[dogfoodPath,reviewPath,'UPSTREAM.md','docs/verify/gates.md'],cleanup_paths:['F:/definitely-absent-wp4-test']};
    const builderFile=await put(dir,'builder.json',builder);
    const executed=[]; let canaryRuns=0;
    const deps={run:(command)=>{executed.push(command);return {status:0,stdout:command===validator.COMMANDS[0]?validator.NAMED.join('\n'):'ok',stderr:''}},canary:()=>{canaryRuns+=1}};
    assert.doesNotThrow(()=>validator.builder(builderFile,undefined,deps));
    assert.deepEqual(executed,validator.COMMANDS); assert.equal(canaryRuns,1);
    const forged={...builder,passed:true}; const forgedFile=await put(dir,'forged.json',forged);
    assert.throws(()=>validator.builder(forgedFile,undefined,{run:()=>{throw new Error('must not execute')}}));
    const mismatch={...builder,commands:[...validator.COMMANDS].reverse()}; const mismatchFile=await put(dir,'mismatch.json',mismatch);
    let forbiddenExecuted=false; assert.throws(()=>validator.builder(mismatchFile,undefined,{run:()=>{forbiddenExecuted=true;return {status:0}}}));
    assert.equal(forbiddenExecuted,false);
    const jobs=['gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards'];
    const request={schema:'nano.wp4-promotion-request/1',branch:'feat/wp-4',base_sha:product,product_sha:product,request_tip:head,
      audit_sha256:hex,builder_evidence_sha256:sha(fs.readFileSync(builderFile)),
      local_commands:validator.COMMANDS,
      builder_actions:{github_modified:false,merged:false,pushed:false},pending:{ci:'pending',merge_sha:null,run_id:null,workflow_sha:null},
      requested:{ci_jobs:jobs,merge:'integrator-only',no_ff:true,push_fetch:true,workflow_job:'gate-cards'}};
    const requestFile=await put(dir,'request.json',request);
    assert.doesNotThrow(()=>validator.builder(builderFile,requestFile,deps));
    request.builder_actions.pushed=true; const badAuthority=await put(dir,'bad-authority.json',request);
    assert.throws(()=>validator.builder(builderFile,badAuthority,deps));

    const ciFile=await put(dir,'ci.json',{headSha:head,status:'completed',conclusion:'success',jobs:jobs.map((name,index)=>({databaseId:index+1,name,status:'completed',conclusion:'success',startedAt:'s',completedAt:'c',url:'u',steps:[]}))});
    const final={schema:'nano.wp4-final/1',builder_sha:head,integration_sha:head,ci_sha256:sha(fs.readFileSync(ciFile)),
      evid:{'EVID-01':true,'EVID-02':true,'EVID-03':true},canary:{hits:0,scratch_deleted:true},
      stop:{phase8_absent:true,wp5_absent:true,wp6_absent:true},owner_handoff:true};
    const finalFile=await put(dir,'final.md',`# Final\n\n\`\`\`json\n${JSON.stringify(final)}\n\`\`\`\n`);
    assert.equal(run(['final',finalFile,ciFile]).status,0);
    final.stop.wp5_absent=false; assert.notEqual(run(['final',await put(dir,'bad-final.md',`\`\`\`json\n${JSON.stringify(final)}\n\`\`\`\n`),ciFile]).status,0);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});
