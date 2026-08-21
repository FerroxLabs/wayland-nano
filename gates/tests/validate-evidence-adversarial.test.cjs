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
    assert.doesNotThrow(()=>validator.dogfood(file,{execute:()=>{executions+=1;return {good:source.good,mutants:source.mutants,cleanup:source.cleanup}}}));
    assert.equal(executions,1);
    const forged=structuredClone(source); forged.good[0].result.bytes='{"outcome":"green","v":1,"verdicts":[]}';
    forged.good[0].result.sha256=sha(Buffer.from(forged.good[0].result.bytes));
    const forgedFile=await put(dir,'forged.json',forged);
    assert.throws(()=>validator.dogfood(forgedFile,{execute:()=>({good:source.good,mutants:source.mutants,cleanup:source.cleanup})}));
    const cleanup=structuredClone(source); cleanup.cleanup.paths=['F:/caller-chosen'];
    const cleanupFile=await put(dir,'cleanup.json',cleanup); let ran=false;
    assert.throws(()=>validator.dogfood(cleanupFile,{execute:()=>{ran=true;return {good:source.good,mutants:source.mutants,cleanup:source.cleanup}}}));
    assert.equal(ran,false);
    const allFalse=structuredClone(source);for(const key of Object.keys(allFalse.cleanup))allFalse.cleanup[key]=false;
    const allFalseFile=await put(dir,'false-cleanup.json',allFalse);let falseRan=false;
    assert.throws(()=>validator.dogfood(allFalseFile,{execute:()=>{falseRan=true}}));assert.equal(falseRan,false);
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});

test('dogfood cleanup checks every git action and post-enumerates residue',()=>{
  const resources={repo:'F:/repo',scratch:'F:/owned',worktrees:['F:/owned/a','F:/owned/b'],fixedPaths:['F:/owned','F:/owned/t']};
  const okSpawn=(program,args)=>({status:0,stdout:args[1]==='list'?'':'',stderr:''});
  assert.deepEqual(validator.performDogfoodCleanup(resources,{spawn:okSpawn,exists:()=>false,remove:()=>{}}),
    {cargo_targets_absent:true,packaging_tracked_clean:true,scratch_absent:true,worktrees_absent:true});
  const calls=[];const failRemove=(program,args)=>{calls.push(args.join(' '));return {status:args[1]==='remove'?1:0,stdout:'',stderr:''}};
  assert.throws(()=>validator.performDogfoodCleanup(resources,{spawn:failRemove,exists:(path)=>path.endsWith('/a'),remove:()=>{}}),/remove:a/);
  assert.ok(calls.some((call)=>call==='worktree prune'),'prune attempted after failed remove');
  const residue=(program,args)=>({status:0,stdout:args[1]==='list'?`worktree F:/owned/a\n`:'',stderr:''});
  assert.throws(()=>validator.performDogfoodCleanup(resources,{spawn:residue,exists:()=>false,remove:()=>{}}),/registration-residue/);
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
      commands:validator.COMMANDS,canary_files:[dogfoodPath,reviewPath,'UPSTREAM.md','docs/verify/gates.md'],cleanup_paths:[validator.TOOLS_ROOT]};
    const builderFile=await put(dir,'builder.json',builder);
    const executed=[]; const envs=[]; let canaryRuns=0; const toolsRoot=path.join(dir,'owned-tools');
    let junctionExists=false;
    let productExists=false;const productRoot=path.join(dir,'product-wt');
    const productSpawn=(program,args)=>{const joined=args.join(' ');if(joined.startsWith('worktree add')){productExists=true;return {status:0,stdout:''}}if(joined.includes('rev-parse HEAD^{tree}'))return {status:0,stdout:`${tree}\n`};if(joined.includes('rev-parse HEAD'))return {status:0,stdout:`${product}\n`};if(joined.startsWith('rev-parse'))return {status:0,stdout:`${tree}\n`};if(joined.startsWith('worktree remove')){productExists=false;return {status:0,stdout:''}}return {status:0,stdout:''}};
    const deps={toolsRoot,run:(command,control)=>{executed.push(command);envs.push(control.env);if(command===validator.COMMANDS[0]){fs.mkdirSync(toolsRoot,{recursive:true});fs.writeFileSync(path.join(toolsRoot,'owned'),'x');}return {status:0,stdout:command===validator.COMMANDS[1]?validator.NAMED.join('\n'):'ok',stderr:''}},canary:()=>{canaryRuns+=1},
      productWorktree:{root:productRoot,exists:()=>productExists,spawn:productSpawn},
      junction:{platform:'win32',exists:()=>junctionExists,create:()=>{junctionExists=true;return {status:0}},identity:()=>true,remove:()=>{junctionExists=false;return {status:0}}}};
    assert.doesNotThrow(()=>validator.builder(builderFile,undefined,deps));
    assert.deepEqual(executed,validator.COMMANDS); assert.equal(canaryRuns,1);
    assert.equal(fs.existsSync(toolsRoot),false);assert.equal(junctionExists,false);
    assert.equal(productExists,false);
    for(const env of envs)assert.deepEqual(env,validator.controlledEnv(toolsRoot));
    fs.mkdirSync(toolsRoot,{recursive:true});assert.throws(()=>validator.builder(builderFile,undefined,deps));fs.rmSync(toolsRoot,{recursive:true,force:true});
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

test('Windows Node junction is fail-closed and always absent afterward',()=>{
  const command=validator.COMMANDS[1],run=()=>({status:0,stdout:'ok',stderr:''});
  let removed=false;
  assert.throws(()=>validator.withNodeJunction(command,run,'F:/w4e-tools',{platform:'win32',exists:()=>true,remove:()=>{removed=true;return {status:0}}}),/preexists/);
  assert.equal(removed,false,'preexisting target is never deleted');
  assert.throws(()=>validator.withNodeJunction(command,run,'F:/w4e-tools',{platform:'win32',exists:()=>false,create:()=>({status:1})}),/create failed/);
  let state=false;
  assert.throws(()=>validator.withNodeJunction(command,run,'F:/w4e-tools',{platform:'win32',exists:()=>state,create:()=>{state=true;return {status:0}},identity:()=>true,remove:()=>({status:1})}),/cleanup failed/);
  state=false;
  assert.throws(()=>validator.withNodeJunction(command,run,'F:/w4e-tools',{platform:'win32',exists:()=>state,create:()=>{state=true;return {status:0}},identity:()=>true,remove:()=>({status:0})}),/cleanup failed/);
  state=false;
  const result=validator.withNodeJunction(command,run,'F:/w4e-tools',{platform:'win32',exists:()=>state,create:()=>{state=true;return {status:0}},identity:()=>true,remove:()=>{state=false;return {status:0}}});
  assert.equal(result.status,0);assert.equal(state,false);
});

test('PowerShell 5.1 junction argv uses fixed Path and never LiteralPath or Force',()=>{
  const source=fs.readFileSync(VALIDATOR,'utf8');
  assert.match(source,/param\(\$link,\$destination\)[^\n]*New-Item -ItemType Junction -Path \$link -Target \$destination/);
  assert.match(source,/param\(\$link\)[^\n]*Directory\]::Delete\(\$link,\$false\)/);
  assert.doesNotMatch(source,/\$args\[[01]\]/);
  assert.doesNotMatch(source,/New-Item -ItemType Junction -LiteralPath/);
  assert.doesNotMatch(source,/New-Item -ItemType Junction[^\n]*-Force/);
  assert.match(source,/if\(exists\(target\)\) die\('builder: repository target preexists'\)/);
});

test('real Windows PowerShell creates verifies and removes the owned junction', {skip:process.platform!=='win32'},()=>{
  const scratch=`F:\\w4ps5-junction-smoke-${process.pid}-${Date.now().toString(36)}`;
  const destination=path.join(scratch,'destination'),link=path.join(scratch,'junction');
  assert.equal(fs.existsSync(scratch),false,'unique smoke root must begin absent');
  fs.mkdirSync(destination,{recursive:true});
  try {
    const create=spawnSync('powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command',
      '& { param($link,$destination) $ErrorActionPreference="Stop"; New-Item -ItemType Junction -Path $link -Target $destination | Out-Null }',link,destination],{encoding:'utf8',windowsHide:true});
    assert.equal(create.status,0,create.stderr);assert.equal(fs.lstatSync(link).isSymbolicLink(),true);
    assert.equal(path.resolve(fs.realpathSync(link)).toLowerCase(),path.resolve(destination).toLowerCase());
    const remove=spawnSync('powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command',
      '& { param($link) $ErrorActionPreference="Stop"; [IO.Directory]::Delete($link,$false) }',link],{encoding:'utf8',windowsHide:true});
    assert.equal(remove.status,0,remove.stderr);assert.equal(fs.existsSync(link),false);
  } finally {if(fs.existsSync(link))fs.rmSync(link,{recursive:true,force:true});if(fs.existsSync(scratch))fs.rmSync(scratch,{recursive:true,force:true});}
  assert.equal(fs.existsSync(link),false);assert.equal(fs.existsSync(destination),false);assert.equal(fs.existsSync(scratch),false);
});

test('product worktree pins bytes and canonicalizes cleanup registrations',()=>{
  const product='a'.repeat(40),tree='b'.repeat(40),root='F:/W4E-Product';let exists=false;
  const spawnFor=(mode='ok')=>(program,args)=>{const call=args.join(' ');
    if(call.startsWith('worktree add')){if(mode==='add-fail')return {status:1,stdout:''};exists=true;return {status:0,stdout:''};}
    if(call.includes('rev-parse HEAD^{tree}'))return {status:0,stdout:`${mode==='tree-fail'?'c'.repeat(40):tree}\n`};
    if(call.includes('rev-parse HEAD'))return {status:0,stdout:`${mode==='head-fail'?'d'.repeat(40):product}\n`};
    if(call.startsWith('rev-parse'))return {status:0,stdout:`${tree}\n`};
    if(call.startsWith('worktree remove')){if(mode==='remove-fail')return {status:1,stdout:''};exists=false;return {status:0,stdout:''};}
    if(call==='worktree list --porcelain')return {status:0,stdout:mode==='registration'?`worktree \\\\?\\f:\\w4e-product\n`:''};
    return {status:0,stdout:''};};
  assert.throws(()=>validator.withProductWorktree(product,()=>{}, {root,exists:()=>exists,spawn:spawnFor('add-fail')}),/add failed/);
  for(const mode of ['head-fail','tree-fail','remove-fail','registration']){exists=false;assert.throws(()=>validator.withProductWorktree(product,()=>{}, {root,exists:()=>exists,spawn:spawnFor(mode)}),/identity|cleanup/,mode);}
  exists=false;let seen='';assert.equal(validator.withProductWorktree(product,(cwd)=>{seen=cwd;return 'ok'}, {root,exists:()=>exists,spawn:spawnFor()}),'ok');
  assert.equal(seen,root);assert.equal(exists,false);
  assert.equal(validator.canonicalPath('\\\\?\\F:\\W4E-Product'),validator.canonicalPath('f:/w4e-product'));
});
