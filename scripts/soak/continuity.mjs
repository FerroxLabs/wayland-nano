#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign as signEd25519,
} from 'node:crypto';
import {
  appendFile,
  mkdir,
  readFile,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  args.set(process.argv[index], process.argv[index + 1]);
}
const runMode = args.get('--mode') ?? 'smoke';
const seed = Number(args.get('--seed') ?? 1010);
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, 'target');
const binary = resolve(args.get('--binary') ?? join(targetDir, 'release', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano'));
const evidenceRoot = resolve(args.get('--evidence-dir') ?? join(here, 'evidence'));
const fixturePath = join(repo, 'gates', 'fixtures', 'memory-retrieval-recall-v1', 'fixture.json');
const budgetsPath = join(here, 'continuity-budgets.json');
const modes = ['fresh', 'session_resume', 'memory_recall'];
const activeHosts = new Set();

if (!['smoke', 'ci', 'receipt'].includes(runMode) || !Number.isSafeInteger(seed) || seed < 0) {
  console.error('usage: continuity.mjs --mode smoke|ci|receipt --seed <u32> [--binary <path>] [--evidence-dir <dir>]');
  process.exit(2);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function base64url(bytes) {
  return Buffer.from(bytes).toString('base64url');
}

function keyFromSeed(seedBytes) {
  const prefix = Buffer.from('302e020100300506032b657004220420', 'hex');
  const privateKey = createPrivateKey({ key: Buffer.concat([prefix, seedBytes]), format: 'der', type: 'pkcs8' });
  const publicDer = createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  return { privateKey, publicKey: [...publicDer.subarray(publicDer.length - 32)] };
}

function deterministicKey(label) {
  return keyFromSeed(createHash('sha256').update(`wayland-nano-continuity:${seed}:${label}`).digest());
}

function signDomain(key, domain, value) {
  const message = Buffer.concat([Buffer.from(domain), Buffer.from(canonical(value))]);
  return base64url(signEd25519(null, message, key.privateKey));
}

function iso(offsetSeconds = 0) {
  return new Date(Date.now() + offsetSeconds * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function native(path) {
  return process.platform === 'win32' ? path.replaceAll('/', '\\') : path;
}

function killHostTree(child) {
  if (!child?.pid) return;
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
  } else {
    try { child.kill('SIGKILL'); } catch {}
  }
  activeHosts.delete(child);
}

function killActiveHosts() {
  for (const child of [...activeHosts]) killHostTree(child);
}

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    killActiveHosts();
    process.exit(signal === 'SIGINT' ? 130 : 143);
  });
}
process.once('uncaughtException', (error) => {
  killActiveHosts();
  console.error(error);
  process.exit(1);
});
process.once('unhandledRejection', (error) => {
  killActiveHosts();
  console.error(error);
  process.exit(1);
});

async function tighten(path) {
  if (process.platform === 'win32') {
    const whoami = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'whoami.exe');
    const who = spawnSync(whoami, ['/user', '/fo', 'csv', '/nh'], { encoding: 'utf8' });
    const match = who.stdout.match(/"([^"]+)","(S-[^"]+)"/i);
    if (who.status !== 0 || !match) throw new Error(`cannot resolve current user SID: ${who.stderr || who.stdout}`);
    const result = spawnSync('icacls', [native(path), '/inheritance:r', '/grant:r', `*${match[2]}:F`], { encoding: 'utf8', windowsHide: true });
    if (result.status !== 0) throw new Error(`cannot restrict ${path}: ${result.stderr || result.stdout}`);
  } else {
    const result = spawnSync('chmod', ['600', path], { encoding: 'utf8' });
    if (result.status !== 0) throw new Error(`cannot restrict ${path}: ${result.stderr || result.stdout}`);
  }
}

async function gitIdentity(binaryBytes) {
  const commits = spawnSync('git', ['rev-list', '--all'], { cwd: repo, encoding: 'utf8' }).stdout.trim().split(/\s+/).filter(Boolean);
  const source = commits.find((commit) => /^[0-9a-f]{40}$/.test(commit) && binaryBytes.includes(commit));
  const lock = sha256(await readFile(join(repo, 'Cargo.lock')));
  if (!source || !binaryBytes.includes(lock)) {
    throw new Error('binary source identity is not a reachable commit with the current Cargo.lock; build release from a clean checkout');
  }
  return { source_commit_sha: source, cargo_lock_sha256: lock, executable_sha256: sha256(binaryBytes) };
}

async function preflight() {
  const bytes = await readFile(binary);
  if (!bytes.includes('wayland-nano-soak-fake')) {
    console.error(`continuity preflight FAIL: ${binary} lacks the soak-fake-model feature (marker 'wayland-nano-soak-fake' not found). Build with: cargo build --release -p nano-cli -F nano-agent/soak-fake-model`);
    process.exit(2);
  }
  return { bytes, artifact: await gitIdentity(bytes) };
}

function snapshot(keys) {
  const issuer = (agent) => ({
    subject_id: `desktop-${agent}`,
    principal_id: agent,
    epoch: 1,
    revoked: false,
    keys: {
      [`desktop-${agent}-key-1`]: { public_key: keys[agent].publicKey, epoch: 1, revoked: false },
    },
    projects: ['project-a', 'project-b'],
  });
  return {
    admin_id: 'continuity-root',
    admin_epoch: 1,
    admin_public_key: keys.admin.publicKey,
    recovery_public_key: keys.recovery.publicKey,
    receipt_signer_public_key: keys.receipt.publicKey,
    local_cli_public_key: keys.local.publicKey,
    issuers: { 'desktop-bot-a': issuer('bot-a'), 'desktop-bot-b': issuer('bot-b') },
    retired_subjects: [],
    retired_principals: [],
    operations: {},
    nonces: {},
    unknown_records: [],
  };
}

async function bootstrapHome(home, artifact) {
  const activation = join(home, 'activation');
  const keysDir = join(home, 'fixture-keys');
  await mkdir(activation, { recursive: true });
  await mkdir(keysDir, { recursive: true });
  const keys = {
    admin: deterministicKey('admin'),
    recovery: deterministicKey('recovery'),
    receipt: deterministicKey('receipt'),
    local: deterministicKey('local'),
    'bot-a': deterministicKey('bot-a'),
    'bot-b': deterministicKey('bot-b'),
  };
  const receiptSeed = createHash('sha256').update(`wayland-nano-continuity:${seed}:receipt`).digest();
  const receiptSeedPath = join(keysDir, 'receipt-signer.seed');
  const receiptReferencePath = join(activation, 'receipt-signer.keyref');
  await writeFile(receiptSeedPath, receiptSeed);
  await writeFile(receiptReferencePath, `${canonical({ provider: 'file', reference: native(resolve(receiptSeedPath)), role: 'receipt_signer' })}\n`);
  await tighten(receiptSeedPath);
  await tighten(receiptReferencePath);

  const authority = snapshot(keys);
  const authorityDigest = sha256(canonical(authority));
  const receipt = {
    admin_epoch: 1,
    admin_id: authority.admin_id,
    authority_journal_position: 1,
    authority_snapshot_sha256: authorityDigest,
    receipt_signer_key_id: `receipt-ed25519-${sha256(Buffer.from(keys.receipt.publicKey)).slice(0, 32)}`,
    root_public_key_fingerprint: sha256(Buffer.from(keys.admin.publicKey)),
    schema: 'wayland.nano.admin-bootstrap-receipt/v1',
  };
  receipt.signature = signDomain(keys.receipt, 'WAYLAND-NANO-ADMIN-BOOTSTRAP\0v1\0', receipt);
  const authorityRows = [
    { record_type: 'bootstrap', sequence: 1, snapshot: authority },
    { record_type: 'bootstrap_receipt', sequence: 2, receipt: canonical(receipt) },
  ];
  await writeFile(join(activation, 'authority.jsonl'), `${authorityRows.map(canonical).join('\n')}\n`);
  const enablement = {
    operation_id: `continuity-enable-${seed}`,
    enabled: true,
    artifact,
    admin_epoch: 1,
    issuer_epoch: 1,
    grant_epoch: 1,
    revocation_epoch: 1,
    not_after: iso(86_400),
  };
  await writeFile(join(activation, 'enablement.jsonl'), `${canonical(enablement)}\n`);
  await writeFile(join(activation, 'enablement.anchor'), sha256(canonical(enablement)));
  await writeFile(join(home, 'memory-policy.toml'), [
    'enabled = true',
    'write = "SessionAndProject"',
    'read_scope = "SessionAndProject"',
    'embedding_backend = "HashedLocal"',
    'deletion = "Never"',
    'min_tier = "ModelInference"',
    '',
    '[retention]',
    'episodes = 1000',
    'facts = 1000',
    'bytes = 16777216',
    '',
  ].join('\n'));
  await mkdir(join(home, 'agents'), { recursive: true });
  await writeFile(join(home, 'agents', 'bot-a.agent.toml'), 'id = "bot-a"\n');
  await writeFile(join(home, 'agents', 'bot-b.agent.toml'), 'id = "bot-b"\n');
  return keys;
}

let activationOrdinal = 0;
function signedFrame(keys, id, method, { strategy, fallback = 'none', sessionId = null, fingerprint = null, cwd, project, agent }) {
  activationOrdinal += 1;
  const activationId = `continuity-${seed}-${process.pid}-${activationOrdinal}`;
  const carrier = {
    activation_id: activationId,
    alg: 'Ed25519',
    budgets: {
      max_cost_microcents: 1_000_000_000,
      max_input_tokens: 1_000_000,
      max_output_tokens: 1_000_000,
      max_tool_calls: 1000,
      max_turns: 1000,
      wall_clock_ms: 3_600_000,
    },
    capabilities: ['filesystem.read'],
    continuity: { fallback, resume_fingerprint: fingerprint, strategy },
    controls: [],
    deadline: iso(3600),
    idempotency_key: `idem-${activationId}`,
    issued_at: iso(-1),
    issuer_id: `desktop-${agent}`,
    key_id: `desktop-${agent}-key-1`,
    nonce: `nonce-${activationId}`,
    not_after: iso(3600),
    not_before: iso(-5),
    principal_id: agent,
    product_subject_id: `desktop-${agent}`,
    project_id: project,
    schema: 'wayland.nano.activation/v1',
    session_id: sessionId,
  };
  carrier.signature = signDomain(keys[agent], 'WAYLAND-NANO-ACTIVATION\0v1\0', carrier);
  return canonical({
    id,
    jsonrpc: '2.0',
    method,
    params: { cwd: native(cwd), mcpServers: [], sessionId, _meta: { waylandNanoActivation: carrier } },
  });
}

class Acp {
  constructor(child) {
    this.child = child;
    this.frames = [];
    this.stderr = '';
    this.nextId = 1;
    this.pending = new Map();
    createInterface({ input: child.stdout }).on('line', (line) => this.onLine(line));
    child.stderr.on('data', (chunk) => { this.stderr += String(chunk); });
    this.exited = new Promise((resolveExit) => child.once('exit', (code, signal) => {
      activeHosts.delete(child);
      for (const pending of this.pending.values()) pending.reject(new Error(`host exited code=${code} signal=${signal}: ${this.stderr}`));
      this.pending.clear();
      resolveExit({ code, signal });
    }));
  }

  onLine(line) {
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    this.frames.push(frame);
    if (frame.method === 'session/request_permission') {
      this.child.stdin.write(`${canonical({ jsonrpc: '2.0', id: frame.id, result: { outcome: { outcome: 'selected', optionId: 'allow' } } })}\n`);
      return;
    }
    const pending = this.pending.get(String(frame.id));
    if (pending && !frame.method) {
      this.pending.delete(String(frame.id));
      pending.resolve(frame);
    }
  }

  request(method, params) {
    const id = this.nextId++;
    return this.requestFrame(id, canonical({ jsonrpc: '2.0', id, method, params }));
  }

  signed(keys, method, options) {
    const id = this.nextId++;
    return this.requestFrame(id, signedFrame(keys, id, method, options));
  }

  requestFrame(id, frame) {
    const response = new Promise((resolveResponse, reject) => this.pending.set(String(id), { resolve: resolveResponse, reject }));
    this.child.stdin.write(`${frame}\n`);
    return Promise.race([
      response,
      new Promise((_, reject) => setTimeout(() => reject(new Error(`ACP timeout id=${id}: ${this.stderr}`)), 30_000)),
    ]);
  }

  async close() {
    this.child.stdin.end();
    const result = await Promise.race([
      this.exited,
      new Promise((resolveExit) => setTimeout(() => resolveExit(null), 5000)),
    ]);
    if (result === null) {
      killHostTree(this.child);
      await this.exited;
    }
  }
}

function spawnHost(home, workspace, script) {
  const child = spawn(binary, ['acp-host'], {
    cwd: workspace,
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe'],
    env: {
      ...process.env,
      FLUX_API_KEY: 'wayland-nano-continuity-placeholder',
      NANO_HOME: home,
      NANO_SOAK_MODEL_SCRIPT: script,
    },
  });
  activeHosts.add(child);
  return new Acp(child);
}

async function writeScript(path, directives) {
  await writeFile(path, `${directives.map((directive) => JSON.stringify(directive)).join('\n')}\n`);
}

function toolDirective(name, input) {
  return { kind: 'tool_call', name, arguments: input, usage: { input_tokens: 320, output_tokens: 24 }, latency_ms: 1 };
}

function textDirective(text = 'continuity probe complete') {
  return { kind: 'text', text, usage: { input_tokens: 48, output_tokens: 8 }, latency_ms: 1 };
}

async function openSession(home, workspace, keys, script, project, agent, strategy = 'fresh') {
  const acp = spawnHost(home, workspace, script);
  const initialized = await acp.request('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } } });
  if (!initialized.result) throw new Error(`initialize refused: ${JSON.stringify(initialized)} ${acp.stderr}`);
  const created = await acp.signed(keys, 'session/new', { strategy, cwd: workspace, project, agent });
  if (!created.result?.sessionId) throw new Error(`session/new refused: ${JSON.stringify(created)} ${acp.stderr}`);
  return {
    acp,
    sessionId: created.result.sessionId,
    fingerprint: created.result?._meta?.waylandNanoResumeFingerprint,
  };
}

async function seedCorpus(home, workspace, keys, fixture, runDir) {
  const rows = [
    ...fixture.facts.map((value) => ({ kind: 'fact', value })),
    ...fixture.decisions.map((value) => ({ kind: 'decision', value })),
  ];
  const partitions = new Map();
  for (const row of rows) {
    const key = `${row.value.project}\0${row.value.agent_id}`;
    if (!partitions.has(key)) partitions.set(key, []);
    partitions.get(key).push(row);
  }
  for (const [partition, values] of [...partitions].sort(([left], [right]) => left.localeCompare(right))) {
    const [project, agent] = partition.split('\0');
    const script = join(runDir, `seed-${project}-${agent}.jsonl`);
    await writeScript(script, values.flatMap(({ kind, value }) => [
      toolDirective('memory_propose', { kind, value }),
      textDirective(`seeded ${value.id}`),
    ]));
    const { acp, sessionId } = await openSession(home, workspace, keys, script, project, agent);
    for (const { value } of values) {
      const seeded = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: `Seed frozen fixture row ${value.id} for ${project}/${agent}.` }] });
      if (!seeded.result) throw new Error(`fixture seed refused ${project}/${agent}/${value.id}: ${JSON.stringify(seeded)} ${acp.stderr}`);
    }
    const completed = acp.frames.filter((frame) => frame?.params?.update?.sessionUpdate === 'tool_call_update' && frame.params.update.status === 'completed');
    if (completed.length < values.length) throw new Error(`fixture seed incomplete ${project}/${agent}: ${completed.length}/${values.length}`);
    await acp.close();
  }
}

function groupQueries(queries) {
  const grouped = new Map();
  for (const query of queries) {
    const key = `${query.project}\0${query.agent_id}`;
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key).push(query);
  }
  return [...grouped].sort(([left], [right]) => left.localeCompare(right));
}

async function journalUsage(path) {
  const text = await readFile(path, 'utf8');
  const turns = text.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
    .filter((row) => row?.op?.type === 'turn_end');
  const usage = turns.at(-1)?.op?.usage ?? {};
  return {
    input_tokens: Number(usage.input_tokens ?? 0),
    output_tokens: Number(usage.output_tokens ?? 0),
    total_tokens: Number(usage.input_tokens ?? 0) + Number(usage.output_tokens ?? 0),
  };
}

async function runPrompt(acp, sessionId, query, mode, journalPath, seededIds) {
  const before = acp.frames.length;
  const started = performance.now();
  const response = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: query.text }] });
  const latencyMs = performance.now() - started;
  if (!response.result) throw new Error(`${mode}/${query.label} prompt refused: ${JSON.stringify(response)} ${acp.stderr}`);
  const frames = acp.frames.slice(before);
  const outputs = frames
    .filter((frame) => frame?.params?.update?.sessionUpdate === 'tool_call_update' && frame.params.update.status === 'completed')
    .map((frame) => String(frame.params.update.rawOutput ?? ''));
  const outputEvidence = outputs.join('\n');
  const toolCompleted = outputs.some((output) => /^len:[1-9][0-9]*$/.test(output));
  const answerObserved = frames
    .filter((frame) => frame?.params?.update?.sessionUpdate === 'agent_message_chunk')
    .map((frame) => String(frame.params.update.content?.text ?? ''))
    .join('');
  const seededRelevant = query.relevant_ids.filter((id) => seededIds.has(id));
  const answerMatch = answerObserved.includes(query.expected_answer);
  const journalBytes = await readFile(journalPath);
  return {
    schema: 'wayland.nano.continuity-probe/v1',
    mode,
    seed,
    probe_kind: 'recall',
    label: query.label,
    project: query.project,
    agent_id: query.agent_id,
    relevant_ids: query.relevant_ids,
    expected_answer_sha256: sha256(query.expected_answer),
    answer_observed_sha256: sha256(answerObserved),
    answer_match: answerMatch,
    seeded_relevant_ids: seededRelevant,
    tool_completed_nonempty: toolCompleted,
    retrieval_output_bytes: Buffer.byteLength(outputEvidence),
    retrieval_output_sha256: sha256(outputEvidence),
    quality_basis: 'partitioned_seed+nonempty_digest+fixture_answer',
    quality_pass: seededRelevant.length === query.relevant_ids.length && toolCompleted && answerMatch,
    latency_ms: Number(latencyMs.toFixed(3)),
    tokens: await journalUsage(journalPath),
    journal_sha256: sha256(journalBytes),
  };
}

async function forkSession(home, sessionId) {
  const output = spawnSync(binary, ['session', 'fork', sessionId], {
    encoding: 'utf8',
    windowsHide: true,
    env: { ...process.env, NANO_HOME: home },
  });
  if (output.status !== 0) throw new Error(`session fork failed: ${output.stderr || output.stdout}`);
  const result = JSON.parse(output.stdout.trim());
  if (result.parent_digest_before !== result.parent_digest_after) throw new Error('session fork mutated its parent');
  return result;
}

async function createResumeParent(home, workspace, keys, runDir, project, agent) {
  const script = join(runDir, `resume-parent-${project}-${agent}.jsonl`);
  await writeScript(script, [textDirective('resume parent established')]);
  const { acp, sessionId, fingerprint } = await openSession(home, workspace, keys, script, project, agent, 'fresh');
  const response = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Establish the deterministic parent transcript.' }] });
  if (!response.result || !fingerprint) throw new Error(`resume parent setup failed: ${JSON.stringify(response)}`);
  await acp.close();
  return { sessionId, fingerprint, fork: await forkSession(home, sessionId) };
}

async function measureMode(mode, home, workspace, keys, queries, runDir, binarySha) {
  const rows = [];
  const memoryRows = (await readFile(join(home, 'memory.jsonl'), 'utf8')).trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  const seededIds = new Set(memoryRows.flatMap((row) => {
    if (row?.op?.type === 'memory_write_fact') return [row.op.fact_id];
    if (row?.op?.type === 'memory_write_decision') return [row.op.decision_id];
    return [];
  }));
  for (const [partition, partitionQueries] of groupQueries(queries)) {
    const [project, agent] = partition.split('\0');
    const script = join(runDir, `${mode}-${project}-${agent}.jsonl`);
    await writeScript(script, partitionQueries.flatMap((query) => [
      toolDirective('memory_recall', { query: query.text }),
      textDirective(`fixture answer ${query.label}: ${query.expected_answer}`),
    ]));
    let acp;
    let sessionId;
    let parent = null;
    if (mode === 'session_resume') {
      parent = await createResumeParent(home, workspace, keys, runDir, project, agent);
      acp = spawnHost(home, workspace, script);
      const initialized = await acp.request('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } } });
      if (!initialized.result) throw new Error(`resume initialize refused: ${JSON.stringify(initialized)}`);
      const loaded = await acp.signed(keys, 'session/load', {
        strategy: 'session_resume',
        sessionId: parent.sessionId,
        fingerprint: parent.fingerprint,
        cwd: workspace,
        project,
        agent,
      });
      if (!loaded.result) throw new Error(`session/load refused: ${JSON.stringify(loaded)} ${acp.stderr}`);
      sessionId = parent.sessionId;
      const driftHost = spawnHost(home, workspace, join(runDir, `resume-parent-${project}-${agent}.jsonl`));
      const driftInit = await driftHost.request('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } } });
      if (!driftInit.result) throw new Error(`drift initialize refused: ${JSON.stringify(driftInit)}`);
      const drift = await driftHost.signed(keys, 'session/load', {
        strategy: 'session_resume', sessionId, fingerprint: '0'.repeat(64), cwd: workspace, project, agent,
      });
      const refusalKind = drift?.error?.data?.nanoError?.kind ?? null;
      rows.push({
        schema: 'wayland.nano.continuity-probe/v1', mode, seed, probe_kind: 'drift_refusal',
        label: `drift-${project}-${agent}`, project, agent_id: agent,
        quality_pass: refusalKind === 'resume_drift', refusal_kind: refusalKind,
        silent_fallback: Boolean(drift.result), latency_ms: 0, tokens: { input_tokens: 0, output_tokens: 0, total_tokens: 0 },
        journal_sha256: sha256(await readFile(join(home, 'sessions', `${sessionId}.jsonl`))),
        fork: parent.fork,
      });
      await driftHost.close();
    } else {
      ({ acp, sessionId } = await openSession(home, workspace, keys, script, project, agent, mode));
    }
    const journalPath = join(home, 'sessions', `${sessionId}.jsonl`);
    for (const query of partitionQueries) {
      const row = await runPrompt(acp, sessionId, query, mode, journalPath, seededIds);
      row.binary_sha256 = binarySha;
      rows.push(row);
    }
    await acp.close();
  }
  return rows;
}

const { bytes: binaryBytes, artifact } = await preflight();
await mkdir(evidenceRoot, { recursive: true });
const stamp = new Date().toISOString().replace(/[-:.]/g, '');
const runDir = join(evidenceRoot, `run-${stamp}-${runMode}-${seed}-${process.pid}`);
const workspace = join(runDir, 'workspace');
await mkdir(workspace, { recursive: true });
await writeFile(join(workspace, 'continuity-state.txt'), 'wayland nano continuity measurement\n');
const fixtureBytes = await readFile(fixturePath);
const fixture = JSON.parse(fixtureBytes);
const fixtureRows = new Map([
  ...fixture.facts.map((row) => [row.id, `${row.subject} ${row.predicate} ${row.object}`]),
  ...fixture.decisions.map((row) => [row.id, `${row.summary}: ${row.how_to_apply}`]),
]);
for (const query of fixture.queries) {
  query.expected_answer = query.relevant_ids.map((id) => fixtureRows.get(id)).join(' | ');
}
const queries = runMode === 'smoke'
  ? ['q01-deployment-a-a', 'q02-database-b-a', 'q07-port-a-b', 'q08-logs-b-b'].map((label) => fixture.queries.find((query) => query.label === label))
  : fixture.queries;
let budgetSha = null;
try { budgetSha = sha256(await readFile(budgetsPath)); } catch {}
const ndjsonPath = join(runDir, 'continuity.ndjson');
const allRows = [];
for (const mode of modes) {
  const home = join(runDir, `home-${mode}`);
  const keys = await bootstrapHome(home, artifact);
  await seedCorpus(home, workspace, keys, fixture, runDir);
  const rows = await measureMode(mode, home, workspace, keys, queries, runDir, artifact.executable_sha256);
  for (const row of rows) {
    row.binary_sha256 = artifact.executable_sha256;
    row.budget_sha256 = budgetSha;
    await appendFile(ndjsonPath, `${canonical(row)}\n`);
    allRows.push(row);
  }
  await rm(join(home, 'fixture-keys'), { recursive: true, force: true });
  await rm(join(home, 'activation', 'receipt-signer.keyref'), { force: true });
}
const ndjsonBytes = await readFile(ndjsonPath);
const manifest = {
  schema: 'wayland.nano.continuity-manifest/v1',
  measurement_mode: runMode,
  seed,
  source_commit_sha: artifact.source_commit_sha,
  cargo_lock_sha256: artifact.cargo_lock_sha256,
  binary: { path: native(binary), sha256: artifact.executable_sha256, size: binaryBytes.length, feature: 'nano-agent/soak-fake-model' },
  fixture: { path: relative(repo, fixturePath).replaceAll('\\', '/'), sha256: sha256(fixtureBytes), labels_modified: false },
  budgets: { path: 'scripts/soak/continuity-budgets.json', sha256: budgetSha },
  ndjson: { path: relative(evidenceRoot, ndjsonPath).replaceAll('\\', '/'), sha256: sha256(ndjsonBytes), rows: allRows.length },
  modes,
  completed_at: iso(),
};
const manifestPath = join(runDir, 'continuity-manifest.json');
await writeFile(manifestPath, `${canonical(manifest)}\n`);
const latest = {
  manifest: relative(evidenceRoot, manifestPath).replaceAll('\\', '/'),
  ndjson: relative(evidenceRoot, ndjsonPath).replaceAll('\\', '/'),
};
await writeFile(join(evidenceRoot, 'latest.json'), `${canonical(latest)}\n`);
console.log(`continuity: ${allRows.length} evidence rows -> ${manifestPath}`);
