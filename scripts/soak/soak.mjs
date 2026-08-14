#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile, appendFile, readdir, stat } from 'node:fs/promises';
import { spawn, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1]);
const mode = args.get('--mode') ?? 'receipt';
const durationSeconds = Number(args.get('--duration-seconds') ?? (mode === 'ci' ? 600 : mode === 'smoke' ? 60 : 28800));
const seed = Number(args.get('--seed') ?? 1010);
const binary = resolve(args.get('--binary') ?? join(repo, 'target', 'release', process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano'));
const fakeScript = resolve(args.get('--fake-script') ?? join(here, 'fake-model-script.jsonl'));
const evidenceDir = resolve(args.get('--evidence-dir') ?? join(here, 'evidence'));
const stamp = new Date().toISOString().replace(/[-:.]/g, '').replace('Z', 'Z');
const runDir = join(evidenceDir, `run-${stamp}`);
const nanoHome = join(tmpdir(), `wayland-nano-s10-home-${process.pid}-${seed}`);
const workspace = join(tmpdir(), `wayland-nano-s10-work-${process.pid}-${seed}`);
const eventPath = join(runDir, 'soak-journal.ndjson');
const samplePath = join(runDir, 'soak-samples.ndjson');
const manifestPath = join(runDir, `soak-manifest-${stamp}.json`);
const budgets = JSON.parse(await readFile(join(here, 'budgets.json'), 'utf8'));
await mkdir(runDir, { recursive: true });
await mkdir(nanoHome, { recursive: true });
await mkdir(workspace, { recursive: true });
await writeFile(join(workspace, 'soak-state.txt'), 'S10 durable state\n');
for (let i = 0; i < 5000; i += 1) {
  const dir = join(workspace, 'search-tree', String(Math.floor(i / 100)));
  if (i % 100 === 0) await mkdir(dir, { recursive: true });
  await writeFile(join(dir, `entry-${i}.txt`), `S10 searchable ${i}\n`);
}

let rng = seed >>> 0;
function random() { rng = (rng * 1664525 + 1013904223) >>> 0; return rng / 2 ** 32; }
const appendEvent = async (type, detail = {}) => appendFile(eventPath, `${JSON.stringify({ at: new Date().toISOString(), type, ...detail })}\n`);
const sha256 = async (path) => createHash('sha256').update(await readFile(path)).digest('hex');
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

class Acp {
  constructor(child) {
    this.child = child; this.nextId = 1; this.pending = new Map(); this.frames = [];
    this.lastFrameAt = Date.now(); this.closed = false;
    createInterface({ input: child.stdout }).on('line', (line) => this.onLine(line));
    child.on('exit', (code, signal) => {
      this.closed = true;
      for (const { reject } of this.pending.values()) reject(new Error(`host exited code=${code} signal=${signal}`));
      this.pending.clear();
    });
  }
  onLine(line) {
    this.lastFrameAt = Date.now();
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    this.frames.push(frame);
    if (frame.method === 'session/request_permission') {
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id: frame.id, result: { outcome: { outcome: 'selected', optionId: 'allow' } } })}\n`);
      return;
    }
    const pending = this.pending.get(String(frame.id));
    if (pending && !frame.method) { this.pending.delete(String(frame.id)); pending.resolve(frame); }
  }
  request(method, params) {
    const id = this.nextId++;
    return new Promise((resolveRequest, reject) => {
      this.pending.set(String(id), { resolve: resolveRequest, reject });
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    });
  }
}

function spawnHost() {
  const child = spawn(binary, ['acp-host'], {
    cwd: workspace, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
    env: { ...process.env, FLUX_API_KEY: 'wayland-nano-soak-placeholder', NANO_HOME: nanoHome, NANO_SOAK_MODEL_SCRIPT: fakeScript },
  });
  child.stderr.on('data', (chunk) => appendEvent('host_stderr', { text: String(chunk).slice(0, 2048) }));
  return new Acp(child);
}

async function initialize(acp, sessionId) {
  const initialized = await acp.request('initialize', { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true } } });
  if (!initialized.result) throw new Error(`initialize refused: ${JSON.stringify(initialized)}`);
  if (sessionId) {
    const loaded = await acp.request('session/load', { sessionId, cwd: workspace, mcpServers: [] });
    if (!loaded.result) throw new Error(`session/load refused: ${JSON.stringify(loaded)}`);
    return sessionId;
  }
  const created = await acp.request('session/new', { cwd: workspace, mcpServers: [] });
  if (!created.result?.sessionId) throw new Error(`session/new refused: ${JSON.stringify(created)}`);
  return created.result.sessionId;
}

function sample(pid) {
  const command = process.platform === 'win32'
    ? spawnSync('powershell', ['-NoProfile', '-File', join(here, 'sample-oracles.ps1'), '-PidValue', String(pid), '-NanoHome', nanoHome], { encoding: 'utf8' })
    : spawnSync('sh', [join(here, 'sample-oracles.sh'), String(pid), nanoHome], { encoding: 'utf8' });
  if (command.status !== 0) throw new Error(`sampler: ${command.stderr || command.stdout}`);
  return JSON.parse(command.stdout.trim());
}

function alive(pid) {
  if (process.platform === 'win32') return spawnSync('tasklist', ['/fo', 'csv', '/nh', '/fi', `PID eq ${pid}`], { encoding: 'utf8' }).stdout.includes(String(pid));
  return spawnSync('kill', ['-0', String(pid)]).status === 0;
}

async function hardKill(acp, point) {
  let before = { descendants: [] };
  try { before = sample(acp.child.pid); } catch {}
  const pid = acp.child.pid;
  if (process.platform === 'win32') spawnSync('taskkill', ['/F', '/PID', String(pid)], { windowsHide: true });
  else spawnSync('kill', ['-9', String(pid)]);
  for (let i = 0; i < 50 && alive(pid); i += 1) await sleep(100);
  const orphans = (before.descendants ?? []).filter((childPid) => alive(childPid));
  await appendEvent('kill', { pid, point, descendantsBefore: before.descendants, orphans });
  return { pid, point, descendantsBefore: before.descendants ?? [], orphans, replay: false };
}

const startedAt = new Date();
const deadline = Date.now() + durationSeconds * 1000;
const killTargets = mode === 'receipt' ? Math.max(6, Math.min(10, Math.round(durationSeconds / 3600))) : mode === 'baseline' ? 0 : 2;
const killTimes = Array.from({ length: killTargets }, (_, index) => startedAt.getTime() + durationSeconds * 1000 * (index + 1) / (killTargets + 1));
let nextKill = 0; let sessionId; let turns = 0; let typedErrors = 0; let unknownErrors = 0; let watchdogBreaches = 0;
let acp = spawnHost();
sessionId = await initialize(acp);
const journalPath = join(nanoHome, 'sessions', `${sessionId}.jsonl`);
const kills = [];
const samples = [];
let nextSampleAt = 0;
await appendEvent('run_start', { mode, durationSeconds, seed, pid: acp.child.pid, sessionId });

while (Date.now() < deadline) {
  if (Date.now() >= nextSampleAt) {
    try { const current = sample(acp.child.pid); samples.push(current); await appendFile(samplePath, `${JSON.stringify(current)}\n`); }
    catch (error) { await appendEvent('sampler_flake', { message: String(error) }); }
    nextSampleAt = Date.now() + (mode === 'smoke' ? 5000 : 60000);
  }
  if (nextKill < killTimes.length && Date.now() >= killTimes[nextKill]) {
    const record = await hardKill(acp, nextKill === 1 ? 'chained-kill-resume' : 'mid-turn');
    kills.push(record); nextKill += 1;
    acp = spawnHost();
    sessionId = await initialize(acp, sessionId);
    record.replay = true;
    for (let follow = 0; follow < 3; follow += 1) {
      const response = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: `post-kill replay verification ${follow}` }] });
      if (response.result) turns += 1; else typedErrors += 1;
    }
    continue;
  }
  const workload = ['exec', 'fs', 'search', 'pty', 'mcp', 'subagent', 'compaction', 'model'][Math.floor(random() * 8)];
  await appendEvent('turn_start', { turn: turns + 1, workload });
  try {
    const response = await acp.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: `S10 ${workload} turn ${turns + 1}; execute the scripted program.` }] });
    if (response.result) { turns += 1; await appendEvent('turn_end', { turn: turns, status: 'PASS' }); }
    else { typedErrors += 1; await appendEvent('turn_end', { status: 'typed_error', response }); }
  } catch (error) {
    if (!acp.closed) unknownErrors += 1;
    await appendEvent('turn_end', { status: acp.closed ? 'injected_kill' : 'unknown_error', message: String(error) });
  }
  if (Date.now() - acp.lastFrameAt > budgets.B12.maximumFrameGapMs) watchdogBreaches += 1;
}

if (!acp.closed) await hardKill(acp, 'run-end-cleanup');
const verifier = spawnSync(process.execPath, [join(here, 'verify-journal.mjs'), journalPath], { encoding: 'utf8' });
const journalOracle = verifier.stdout.trim() ? JSON.parse(verifier.stdout) : { tornMiddle: 1, tornTail: 0, replayClean: false };
const journalBytes = (await stat(journalPath)).size;
const homeBytes = samples.at(-1)?.nanoHomeBytes ?? 0;
const peak = (field) => Math.max(0, ...samples.map((entry) => Number(entry[field] ?? 0)));
const durationMs = Date.now() - startedAt.getTime();
const rate = turns / (durationMs / 3600000);
const result = (id, status, detail) => ({ id, name: budgets[id].name, status, detail });
const results = [
  result('B1', peak('privateWorkingSetBytes') <= budgets.B1.absoluteBytes ? 'PASS' : 'FAIL', `peak=${peak('privateWorkingSetBytes')}`),
  result('B2', process.platform === 'win32' ? (peak('handles') <= budgets.B2.absolute ? 'PASS' : 'FAIL') : 'SKIP', `peak=${peak('handles')}`),
  result('B3', peak('threads') <= budgets.B3.absolute ? 'PASS' : 'FAIL', `peak=${peak('threads')}`),
  result('B4', process.platform === 'win32' ? 'SKIP' : (peak('openFds') <= budgets.B4.absolute ? 'PASS' : 'FAIL'), `peak=${peak('openFds')}`),
  result('B5', journalBytes <= (mode === 'receipt' ? budgets.B5.absoluteBytes : Math.max(2097152, budgets.B5.absoluteBytes * durationSeconds / 28800)) ? 'PASS' : 'FAIL', `bytes=${journalBytes}`),
  result('B6', homeBytes <= (mode === 'receipt' ? budgets.B6.growthBytes : Math.max(16777216, budgets.B6.growthBytes * durationSeconds / 28800)) ? 'PASS' : 'FAIL', `bytes=${homeBytes}`),
  result('B7', kills.every((kill) => kill.orphans.length === 0) ? 'PASS' : 'FAIL', `orphans=${kills.flatMap((kill) => kill.orphans).length}`),
  result('B8', journalOracle.tornMiddle === 0 && journalOracle.replayClean ? 'PASS' : 'FAIL', JSON.stringify(journalOracle)),
  result('B9', rate >= budgets.B9.turnsPerHour ? 'PASS' : 'FAIL', `turnsPerHour=${rate.toFixed(2)}`),
  result('B10', unknownErrors === 0 && typedErrors / Math.max(1, turns) <= budgets.B10.maximumRate ? 'PASS' : 'FAIL', `typed=${typedErrors} unknown=${unknownErrors}`),
  result('B11', mode === 'receipt' ? 'FAIL' : 'SKIP', mode === 'receipt' ? 'live segment must be run with shipped binary' : 'fake-only run'),
  result('B12', watchdogBreaches === 0 ? 'PASS' : 'FAIL', `breaches=${watchdogBreaches}`),
];
const rustc = spawnSync('rustc', ['--version'], { cwd: repo, encoding: 'utf8' }).stdout.trim();
const manifest = {
  gate: 'S10', track: 'B', source_sha: spawnSync('git', ['rev-parse', 'HEAD'], { cwd: repo, encoding: 'utf8' }).stdout.trim(),
  dirty: Boolean(spawnSync('git', ['status', '--porcelain'], { cwd: repo, encoding: 'utf8' }).stdout.trim()),
  started_at: startedAt.toISOString(), finished_at: new Date().toISOString(), durationMs, mode, seed, turnCount: turns, kills,
  binary: { path: binary, sha256: await sha256(binary), features: ['nano-agent/soak-fake-model'], rustc },
  baselineMedians: samples[0] ?? {}, sampleSeriesDigest: await sha256(samplePath), sessionJournalPath: journalPath,
  counts: { pass: results.filter((entry) => entry.status === 'PASS').length, fail: results.filter((entry) => entry.status === 'FAIL').length, skip: results.filter((entry) => entry.status === 'SKIP').length },
  results,
};
await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
await appendEvent('run_end', { manifestPath, counts: manifest.counts });
console.log(`S10 summary: ${manifest.counts.pass} pass / ${manifest.counts.fail} fail / ${manifest.counts.skip} skip -> ${manifestPath}`);
if (manifest.counts.fail) process.exitCode = 1;
