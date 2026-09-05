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
  writeFile,
} from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { dirname, join, relative, resolve } from 'node:path';
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
const injection = args.get('--inject') ?? null;
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, 'target');
const binary = resolve(args.get('--binary') ?? join(targetDir, 'release', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano'));
const evidenceRoot = resolve(args.get('--evidence-dir') ?? join(here, 'evidence'));
const fixturePath = join(repo, 'gates', 'fixtures', 'memory-retrieval-recall-v1', 'fixture.json');
const budgetsPath = join(here, 'continuity-budgets.json');
const modes = ['fresh', 'session_resume', 'memory_recall'];
const activeHosts = new Set();

if (!['smoke', 'ci', 'receipt'].includes(runMode)
  || !Number.isSafeInteger(seed)
  || seed < 0
  || (injection !== null && (process.env.CONTINUITY_TESTING !== '1' || injection !== 'fresh-leak'))) {
  console.error('usage: continuity.mjs --mode smoke|ci|receipt --seed <u32> [--binary <path>] [--evidence-dir <dir>]');
  process.exit(2);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

async function sourceHash(path) {
  return sha256((await readFile(path, 'utf8')).replaceAll('\r\n', '\n'));
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

async function bootstrapHome(home, artifact, memoryEnabled) {
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
    `enabled = ${memoryEnabled}`,
    `write = "${memoryEnabled ? 'SessionAndProject' : 'Off'}"`,
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
    let timer;
    const response = new Promise((resolveResponse, reject) => this.pending.set(String(id), {
      resolve: (value) => {
        clearTimeout(timer);
        resolveResponse(value);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    }));
    timer = setTimeout(() => {
      const pending = this.pending.get(String(id));
      if (pending) {
        this.pending.delete(String(id));
        pending.reject(new Error(`ACP timeout id=${id}: ${this.stderr}`));
      }
    }, 30_000);
    this.child.stdin.write(`${frame}\n`);
    return response;
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
  const text = `${directives.map((directive) => JSON.stringify(directive)).join('\n')}\n`;
  await writeFile(path, text);
  return sha256(text);
}

function seededRandom(scope) {
  let state = Number.parseInt(sha256(`${seed}:${scope}`).slice(0, 8), 16) >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

function toolDirective(name, input, random = () => 0) {
  return { kind: 'tool_call', name, arguments: input, usage: scriptedUsage(random), latency_ms: 1 + Math.floor(random() * 3) };
}

function textDirective(text = 'continuity probe complete', random = () => 0) {
  return { kind: 'text', text, usage: scriptedUsage(random), latency_ms: 1 + Math.floor(random() * 3) };
}

function scriptedUsage(random) {
  return {
    input_tokens: 180 + Math.floor(random() * 121),
    output_tokens: 12 + Math.floor(random() * 13),
  };
}

function assertRequestDirective(query, mode, present) {
  const random = seededRandom(`probe:${query.label}`);
  const usage = scriptedUsage(random);
  const latencyMs = 1 + Math.floor(random() * 3);
  return {
    directive: {
      kind: 'assert_request',
      needle: query.expected_answer,
      present,
      text: `request assertion ${mode} ${query.label} ${present ? 'present' : 'absent'}`,
      usage,
      latency_ms: latencyMs,
    },
    profile: { usage, latency_ms: latencyMs },
  };
}

async function initializeHost(acp) {
  const initialized = await acp.request('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } } });
  if (!initialized.result) throw new Error(`initialize refused: ${JSON.stringify(initialized)} ${acp.stderr}`);
}

async function openActivatedSession(home, workspace, keys, script, project, agent, strategy = 'fresh') {
  const acp = spawnHost(home, workspace, script);
  await initializeHost(acp);
  const created = await acp.signed(keys, 'session/new', { strategy, cwd: workspace, project, agent });
  if (!created.result?.sessionId) throw new Error(`session/new refused: ${JSON.stringify(created)} ${acp.stderr}`);
  return {
    acp,
    sessionId: created.result.sessionId,
    fingerprint: created.result?._meta?.waylandNanoResumeFingerprint,
  };
}

async function seedCorpus(home, workspace, keys, fixture, runDir) {
  const setup = new Map();
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
    const random = seededRandom(`seed:${project}:${agent}`);
    await writeScript(script, [
      ...values.map(({ kind, value }) => toolDirective('memory_propose', { kind, value }, random)),
      textDirective(`seeded ${project}/${agent}`, random),
    ]);
    const { acp, sessionId } = await openActivatedSession(home, workspace, keys, script, project, agent);
    const seeded = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: `Seed the frozen fixture partition ${project}/${agent}.` }] });
    if (!seeded.result) throw new Error(`fixture seed refused ${project}/${agent}: ${JSON.stringify(seeded)} ${acp.stderr}`);
    const completed = acp.frames.filter((frame) => frame?.params?.update?.sessionUpdate === 'tool_call_update' && frame.params.update.status === 'completed');
    if (completed.length < values.length) throw new Error(`fixture seed incomplete ${project}/${agent}: ${completed.length}/${values.length}`);
    const setupTokens = sessionTokens(acp.frames, sessionId);
    if (setupTokens <= 0) throw new Error(`memory seed emitted no setup tokens: ${project}/${agent}`);
    setup.set(partition, { setup_tokens: setupTokens, setup_session_ids: [sessionId] });
    await acp.close();
  }
  return setup;
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

function sessionTokens(frames, sessionId) {
  return [...frames].reverse().find((frame) =>
    frame?.method === '_wayland/session/budget'
      && frame?.params?.sessionId === sessionId
      && Number.isSafeInteger(frame?.params?.session_tokens))?.params.session_tokens ?? 0;
}

async function journalDigest(path) {
  if (!path) return sha256(Buffer.alloc(0));
  return sha256(await readFile(path));
}

async function runCausalPrompt(
  acp,
  sessionId,
  query,
  mode,
  journalPath,
  taskBatterySha256,
  driverScriptSha256,
  driverProfile,
  extra,
) {
  const beforeIndex = acp.frames.length;
  const observedBefore = sessionTokens(acp.frames, sessionId);
  const sessionTokensBefore = observedBefore || extra.session_token_baseline || 0;
  const started = performance.now();
  const response = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: query.text }] });
  const latencyMs = performance.now() - started;
  const frames = acp.frames.slice(beforeIndex);
  const sessionTokensAfter = sessionTokens(acp.frames, sessionId);
  const answerObserved = frames
    .filter((frame) => frame?.params?.update?.sessionUpdate === 'agent_message_chunk')
    .map((frame) => String(frame.params.update.content?.text ?? ''))
    .join('');
  const assertionMarker = `request assertion ${mode} ${query.label} ${extra.request_assertion}`;
  const assertionMatched = Boolean(response.result) && answerObserved.includes(assertionMarker);
  const refusalKind = response?.error?.data?.nanoError?.kind ?? null;
  if (mode === 'fresh' && (!assertionMatched || refusalKind !== null)) {
    const failure = refusalKind === 'model_protocol' ? 'fresh_leakage' : 'fresh_protocol_refusal';
    throw new Error(`${failure}: ${query.label}`);
  }
  const memoryToolCalls = frames.filter((frame) =>
    frame?.params?.update?.sessionUpdate === 'tool_call'
      && String(frame?.params?.update?.title ?? '').startsWith('memory_')).length;
  const setupTokens = extra.setup_tokens ?? 0;
  const setupSessionIds = extra.setup_session_ids ?? [];
  const {
    session_token_baseline: _baseline,
    setup_tokens: _setup,
    setup_session_ids: _setupIds,
    ...evidenceExtra
  } = extra;
  const probeTokens = sessionTokensAfter - sessionTokensBefore;
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
    answer_source: 'model_request_assertion',
    request_assertion: extra.request_assertion,
    request_assertion_matched: assertionMatched,
    isolation_pass: mode === 'fresh' ? true : null,
    memory_tool_calls: memoryToolCalls,
    quality_basis: 'actual_model_request_contains_fixture_answer',
    quality_pass: extra.expect_continuity && assertionMatched,
    task_battery_sha256: taskBatterySha256,
    driver_script_sha256: driverScriptSha256,
    driver_profile: driverProfile,
    latency_ms: Number(latencyMs.toFixed(3)),
    tokens: {
      source: 'acp_budget_notice',
      setup_tokens: setupTokens,
      setup_session_ids: setupSessionIds,
      session_tokens_before: sessionTokensBefore,
      session_tokens_after: sessionTokensAfter,
      probe_tokens: probeTokens,
      total_tokens: setupTokens + probeTokens,
    },
    journal_sha256: await journalDigest(journalPath),
    refusal_kind: refusalKind,
    ...evidenceExtra,
  };
}

async function measureFresh(home, workspace, keys, queries, runDir, taskBatterySha256) {
  const rows = [];
  for (const [partition, partitionQueries] of groupQueries(queries)) {
    const [project, agent] = partition.split('\0');
    const script = join(runDir, `fresh-${project}-${agent}.jsonl`);
    const probes = partitionQueries.map((query) => assertRequestDirective(query, 'fresh', false));
    const driverScriptSha256 = await writeScript(script, probes.map((probe) => probe.directive));
    const { acp, sessionId } = await openActivatedSession(
      home, workspace, keys, script, project, agent, 'fresh');
    const journalPath = join(home, 'sessions', `${sessionId}.jsonl`);
    for (const [index, query] of partitionQueries.entries()) {
      const promptQuery = injection === 'fresh-leak'
        ? { ...query, text: `${query.text}\n${query.expected_answer}` }
        : query;
      rows.push(await runCausalPrompt(acp, sessionId, promptQuery, 'fresh', journalPath, taskBatterySha256, driverScriptSha256, probes[index].profile, {
        request_assertion: 'absent',
        expect_continuity: false,
        persistent: false,
        activation_admitted: true,
        memory_seeded: false,
      }));
    }
    await acp.close();
  }
  return rows;
}

async function measureSessionResume(home, workspace, keys, queries, runDir, taskBatterySha256) {
  const rows = [];
  for (const [partition, partitionQueries] of groupQueries(queries)) {
    const [project, agent] = partition.split('\0');
    const parentScript = join(runDir, `resume-parent-${project}-${agent}.jsonl`);
    const parentRandom = seededRandom(`resume-parent:${project}:${agent}`);
    await writeScript(parentScript, partitionQueries.map((query) =>
      textDirective(query.expected_answer, parentRandom)));
    const parent = await openActivatedSession(home, workspace, keys, parentScript, project, agent, 'fresh');
    for (const query of partitionQueries) {
      const response = await parent.acp.request('session/prompt', {
        sessionId: parent.sessionId,
        prompt: [{ type: 'text', text: query.text }],
      });
      if (!response.result) throw new Error(`resume parent prompt failed: ${JSON.stringify(response)}`);
    }
    const forkResponse = await parent.acp.request('_wayland/session/fork', { sessionId: parent.sessionId });
    const fork = forkResponse.result;
    if (!fork?.child_session_id || !fork?.resume_fingerprint || fork.parent_digest_before !== fork.parent_digest_after) {
      throw new Error(`activated fork failed: ${JSON.stringify(forkResponse)} ${parent.acp.stderr}`);
    }
    const parentTokenBaseline = sessionTokens(parent.acp.frames, parent.sessionId);
    if (parentTokenBaseline <= 0) throw new Error('resume parent emitted no token baseline');
    await parent.acp.close();

    const script = join(runDir, `session-resume-${project}-${agent}.jsonl`);
    const probes = partitionQueries.map((query) =>
      assertRequestDirective(query, 'session_resume', true));
    const driverScriptSha256 = await writeScript(script, probes.map((probe) => probe.directive));
    const acp = spawnHost(home, workspace, script);
    await initializeHost(acp);
    const drift = await acp.signed(keys, 'session/load', {
      strategy: 'session_resume', sessionId: fork.child_session_id,
      fingerprint: '0'.repeat(64), cwd: workspace, project, agent,
    });
    const refusalKind = drift?.error?.data?.nanoError?.kind ?? null;
    rows.push({
      schema: 'wayland.nano.continuity-probe/v1', mode: 'session_resume', seed,
      probe_kind: 'drift_refusal', label: `drift-${project}-${agent}`, project, agent_id: agent,
      quality_pass: refusalKind === 'resume_drift', refusal_kind: refusalKind,
      silent_fallback: Boolean(drift.result), latency_ms: 0,
      tokens: {
        source: 'acp_budget_notice', setup_tokens: 0, setup_session_ids: [],
        session_tokens_before: 0, session_tokens_after: 0, probe_tokens: 0, total_tokens: 0,
      },
      journal_sha256: await journalDigest(join(home, 'sessions', `${fork.child_session_id}.jsonl`)),
      fork_child_session_id: fork.child_session_id,
    });
    const loaded = await acp.signed(keys, 'session/load', {
      strategy: 'session_resume', sessionId: fork.child_session_id,
      fingerprint: fork.resume_fingerprint, cwd: workspace, project, agent,
    });
    if (!loaded.result) throw new Error(`fork child load refused: ${JSON.stringify(loaded)} ${acp.stderr}`);
    const journalPath = join(home, 'sessions', `${fork.child_session_id}.jsonl`);
    for (const [index, query] of partitionQueries.entries()) {
      rows.push(await runCausalPrompt(acp, fork.child_session_id, query, 'session_resume', journalPath, taskBatterySha256, driverScriptSha256, probes[index].profile, {
        request_assertion: 'present',
        expect_continuity: true,
        persistent: true,
        activation_admitted: true,
        memory_seeded: false,
        loaded_session_id: fork.child_session_id,
        fork_child_session_id: fork.child_session_id,
        session_token_baseline: index === 0 ? parentTokenBaseline : 0,
        setup_tokens: index === 0 ? parentTokenBaseline : 0,
        setup_session_ids: index === 0 ? [parent.sessionId] : [],
      }));
    }
    await acp.close();
  }
  return rows;
}

async function measureMemoryRecall(
  home,
  workspace,
  keys,
  queries,
  runDir,
  taskBatterySha256,
  memorySetup,
) {
  const rows = [];
  for (const [partition, partitionQueries] of groupQueries(queries)) {
    const [project, agent] = partition.split('\0');
    const script = join(runDir, `memory-recall-${project}-${agent}.jsonl`);
    const probes = partitionQueries.map((query) =>
      assertRequestDirective(query, 'memory_recall', true));
    const driverScriptSha256 = await writeScript(script, probes.map((probe) => probe.directive));
    const { acp, sessionId } = await openActivatedSession(
      home, workspace, keys, script, project, agent, 'memory_recall');
    const journalPath = join(home, 'sessions', `${sessionId}.jsonl`);
    for (const [index, query] of partitionQueries.entries()) {
      const setup = memorySetup.get(partition);
      if (!setup) throw new Error(`memory setup accounting missing: ${project}/${agent}`);
      rows.push(await runCausalPrompt(acp, sessionId, query, 'memory_recall', journalPath, taskBatterySha256, driverScriptSha256, probes[index].profile, {
        request_assertion: 'present',
        expect_continuity: true,
        persistent: true,
        activation_admitted: true,
        memory_seeded: true,
        setup_tokens: index === 0 ? setup.setup_tokens : 0,
        setup_session_ids: index === 0 ? setup.setup_session_ids : [],
      }));
    }
    await acp.close();
  }
  return rows;
}

const { bytes: binaryBytes, artifact } = await preflight();
const startedAt = iso();
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
  ...fixture.decisions.map((row) => [row.id, `${row.summary} ${row.why} ${row.how_to_apply}`]),
]);
for (const query of fixture.queries) {
  query.expected_answer = query.relevant_ids.map((id) => fixtureRows.get(id)).join(' | ');
}
const queries = runMode === 'smoke'
  ? ['q01-deployment-a-a', 'q02-database-b-a', 'q07-port-a-b', 'q08-logs-b-b'].map((label) => fixture.queries.find((query) => query.label === label))
  : fixture.queries;
const taskBattery = queries.map((query) => ({
  label: query.label,
  text: query.text,
  project: query.project,
  agent_id: query.agent_id,
  relevant_ids: query.relevant_ids,
  expected_answer_sha256: sha256(query.expected_answer),
}));
const taskBatterySha256 = sha256(canonical(taskBattery));
let budgetSha = null;
try { budgetSha = sha256(canonical(JSON.parse(await readFile(budgetsPath, 'utf8')))); } catch {}
const harnessSha = await sourceHash(fileURLToPath(import.meta.url));
const ndjsonPath = join(runDir, 'continuity.ndjson');
const allRows = [];
for (const mode of modes) {
  const home = join(runDir, `home-${mode}`);
  await mkdir(home, { recursive: true });
  const keys = await bootstrapHome(home, artifact, mode === 'memory_recall');
  let rows;
  if (mode === 'fresh') {
    rows = await measureFresh(home, workspace, keys, queries, runDir, taskBatterySha256);
  } else {
    if (mode === 'memory_recall') {
      const memorySetup = await seedCorpus(home, workspace, keys, fixture, runDir);
      rows = await measureMemoryRecall(
        home, workspace, keys, queries, runDir, taskBatterySha256, memorySetup);
    } else {
      rows = await measureSessionResume(home, workspace, keys, queries, runDir, taskBatterySha256);
    }
  }
  for (const row of rows) {
    row.binary_sha256 = artifact.executable_sha256;
    row.budget_sha256 = budgetSha;
    row.harness_sha256 = harnessSha;
    row.task_battery_sha256 = taskBatterySha256;
    await appendFile(ndjsonPath, `${canonical(row)}\n`);
    allRows.push(row);
  }
  await rm(join(home, 'fixture-keys'), { recursive: true, force: true });
  await rm(join(home, 'activation', 'receipt-signer.keyref'), { force: true });
}
const ndjsonBytes = await readFile(ndjsonPath);
const accounting = Object.fromEntries(modes.map((mode) => {
  const rows = allRows.filter((row) => row.mode === mode && row.probe_kind === 'recall');
  const setupTokens = rows.reduce((sum, row) => sum + row.tokens.setup_tokens, 0);
  const probeTokens = rows.reduce((sum, row) => sum + row.tokens.probe_tokens, 0);
  const totalTokens = rows.reduce((sum, row) => sum + row.tokens.total_tokens, 0);
  if (totalTokens !== setupTokens + probeTokens) {
    throw new Error(`token accounting is not conserved: ${mode}`);
  }
  return [mode, {
    setup_tokens: setupTokens,
    probe_tokens: probeTokens,
    total_tokens: totalTokens,
  }];
}));
const manifest = {
  schema: 'wayland.nano.continuity-manifest/v1',
  measurement_mode: runMode,
  seed,
  source_commit_sha: artifact.source_commit_sha,
  cargo_lock_sha256: artifact.cargo_lock_sha256,
  binary: { path: native(binary), sha256: artifact.executable_sha256, size: binaryBytes.length, feature: 'nano-agent/soak-fake-model' },
  fixture: { path: relative(repo, fixturePath).replaceAll('\\', '/'), sha256: sha256(fixtureBytes), labels_modified: false },
  budgets: { path: 'scripts/soak/continuity-budgets.json', sha256: budgetSha },
  harness: { path: 'scripts/soak/continuity.mjs', sha256: harnessSha, hash_normalization: 'crlf-to-lf' },
  task_battery: { sha256: taskBatterySha256, rows: taskBattery.length },
  causal_oracles: {
    fresh: 'nonpersistent request asserts fixture answer absent',
    session_resume: 'activated fork child request asserts replayed answer present',
    memory_recall: 'admitted automatic recall request asserts retrieved answer present',
    token_source: 'ACP _wayland/session/budget notifications',
    setup_attribution: 'first_probe_row_per_mode_partition',
    token_conservation: 'every emitted setup token is attributed once; total_tokens=setup_tokens+probe_tokens',
    drift_tokens: 'excluded: refusal performs no model turn',
  },
  ndjson: { path: relative(evidenceRoot, ndjsonPath).replaceAll('\\', '/'), sha256: sha256(ndjsonBytes), rows: allRows.length },
  modes,
  started_at: startedAt,
  completed_at: iso(),
  accounting,
};
const manifestPath = join(runDir, 'continuity-manifest.json');
await writeFile(manifestPath, `${canonical(manifest)}\n`);
const latest = {
  manifest: relative(evidenceRoot, manifestPath).replaceAll('\\', '/'),
  ndjson: relative(evidenceRoot, ndjsonPath).replaceAll('\\', '/'),
};
await writeFile(join(evidenceRoot, 'latest.json'), `${canonical(latest)}\n`);
console.log(`continuity: ${allRows.length} evidence rows -> ${manifestPath}`);
