'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');
const { writeArtifact } = require('../lib/artifact-writer.cjs');

const ROOT = path.resolve(__dirname, '..', '..');
const VALIDATOR = path.join(__dirname, 'validate-evidence.cjs');
const sha = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
const run = (args) => spawnSync(process.execPath, [VALIDATOR, ...args], { cwd: ROOT, encoding: 'utf8' });
const git = (args) => spawnSync('git', args, { cwd: ROOT, encoding: 'utf8' }).stdout.trim();
const gitBytes = (args) => spawnSync('git', args, { cwd: ROOT, encoding: null }).stdout;
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

test('audit validator recomputes identity diff review and open high count', async () => {
  const dir = fs.mkdtempSync(path.join(process.env.TEMP || os.tmpdir(), 'wp4-audit-'));
  try {
    const head = git(['rev-parse','HEAD']);
    const tree = git(['rev-parse','HEAD^{tree}']);
    const review = await put(dir, 'review.md', '# Independent review\n');
    const argv = ['diff','--binary','--full-index',head,head,'--','gates','docs/verify/gates.md','UPSTREAM.md'];
    const value = { schema:'nano.wp4-audit/1', audit_id:'audit-test-01', base_sha:head, product_sha:head, product_tree:tree,
      diff:{argv,sha256:sha(gitBytes(argv))}, review:{path:review,sha256:sha(fs.readFileSync(review))},
      owned_paths:['gates/tests/validate-evidence.cjs'], requirements:['CARD-01','CARD-02','CARD-03','CARD-04','CARD-05','CARD-06','CARD-07','CARD-08','PROV-03'],
      threats:['T-07-A1'], findings:[], open_critical_high:0, fix_round:0 };
    const file = await put(dir, 'audit.json', value);
    assert.equal(run(['audit',file]).status,0);
    value.product_tree='0'.repeat(40); assert.notEqual(run(['audit',await put(dir,'bad-tree.json',value)]).status,0);
    value.product_tree=tree; value.fix_round=2; assert.notEqual(run(['audit',await put(dir,'bad-round.json',value)]).status,0);
    value.fix_round=0; value.extra=true; assert.notEqual(run(['audit',await put(dir,'extra.json',value)]).status,0);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
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
    const head=git(['rev-parse','HEAD']); const tree=git(['rev-parse','HEAD^{tree}']); const hex='a'.repeat(64);
    const gate=(command)=>({command,passed:true});
    const builder={schema:'nano.wp4-builder/1',product_sha:head,product_tree:tree,
      audit:{open_critical_high:0,review_sha256:hex}, named_tests:['t-card-schema-valid','t-registry-closure-digests','t-ip-reference-scores-mm','t-pv-reference-scores-mm','t-cf-reference-scores-mm','t-ip-mutants-caught','t-pv-mutants-caught','t-cf-mutants-caught','t-fixture-digest-fails-closed','t-dirhash-canonical','t-meta-mutant-passing-is-gate-defect','t-summary-contract','t-gate-hash-drift-voids-validation'],
      seeds:[41041,41042,41043].map(seed=>({seed,passed:true,sha256:hex})),
      gates:{cargo_deny:gate('cargo deny check'),dogfood:gate('verify --run-only'),just_gate_all:gate('just gate-all'),node:gate('node --test')},
      artifacts:{dogfood_sha256:hex,provenance_sha256:hex,review_sha256:hex},
      canary:{files_scanned:3,bytes_scanned:99,hits:0,include_sha256:hex,receipt_sha256:hex,scratch_deleted:true},
      cleanup:{cargo_targets_absent:true,github_unchanged:true,producer_diff_empty:true,scratch_absent:true,worktrees_absent:true}};
    const builderFile=await put(dir,'builder.json',builder);
    assert.equal(run(['builder',builderFile]).status,0);
    const jobs=['gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards'];
    const request={schema:'nano.wp4-promotion-request/1',branch:'feat/wp-4',base_sha:head,product_sha:head,request_tip:head,
      audit_sha256:hex,builder_evidence_sha256:sha(fs.readFileSync(builderFile)),
      local_gates:{cargo_deny:true,dogfood:true,just_gate_all:true,node:true,provenance:true},
      builder_actions:{github_modified:false,merged:false,pushed:false},pending:{ci:'pending',merge_sha:null,run_id:null,workflow_sha:null},
      requested:{ci_jobs:jobs,merge:'integrator-only',no_ff:true,push_fetch:true,workflow_job:'gate-cards'}};
    const requestFile=await put(dir,'request.json',request);
    assert.equal(run(['builder',builderFile,requestFile]).status,0);
    request.builder_actions.pushed=true; assert.notEqual(run(['builder',builderFile,await put(dir,'bad-authority.json',request)]).status,0);

    const ciFile=await put(dir,'ci.json',{headSha:head,status:'completed',conclusion:'success',jobs:jobs.map((name,index)=>({databaseId:index+1,name,status:'completed',conclusion:'success',startedAt:'s',completedAt:'c',url:'u',steps:[]}))});
    const final={schema:'nano.wp4-final/1',builder_sha:head,integration_sha:head,ci_sha256:sha(fs.readFileSync(ciFile)),
      evid:{'EVID-01':true,'EVID-02':true,'EVID-03':true},canary:{hits:0,scratch_deleted:true},
      stop:{phase8_absent:true,wp5_absent:true,wp6_absent:true},owner_handoff:true};
    const finalFile=await put(dir,'final.md',`# Final\n\n\`\`\`json\n${JSON.stringify(final)}\n\`\`\`\n`);
    assert.equal(run(['final',finalFile,ciFile]).status,0);
    final.stop.wp5_absent=false; assert.notEqual(run(['final',await put(dir,'bad-final.md',`\`\`\`json\n${JSON.stringify(final)}\n\`\`\`\n`),ciFile]).status,0);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});
