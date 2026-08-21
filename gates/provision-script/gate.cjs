#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { GateContract } = require('../lib/contract.cjs');
const { extractFramedJson } = require('./fixtures/generators/generators.cjs');

const INVENTORY = [['PV-01', 'structure'], ['PV-02', 'security'], ['PV-03', 'relation'], ['PV-04', 'security'], ['PV-05', 'value'], ['PV-06', 'relation']];
const KEYS = ['version', 'offline_username', 'online_username', 'nano_home', 'command_cwd', 'read_roots', 'write_roots', 'deny_read_paths', 'deny_write_paths', 'deny_write_paths_no_create', 'proxy_ports', 'allow_local_binding', 'otel', 'real_user', 'mode', 'refresh_only', 'cancellation_path'];
const IDENTITIES = new Set(['NanoSandboxOffline', 'NanoSandboxOnline']);
const PATH_LISTS = ['read_roots', 'write_roots', 'deny_read_paths', 'deny_write_paths', 'deny_write_paths_no_create'];

function externalSnapshot(nanoHome) {
  const script = `$ErrorActionPreference='Stop'; $u=@(Get-LocalUser -ErrorAction SilentlyContinue | Where-Object Name -Like 'NanoSandbox*' | Select-Object -ExpandProperty Name | Sort-Object); $f=@(Get-NetFirewallRule -ErrorAction SilentlyContinue | Where-Object { $_.Name -Like 'nano_sandbox_*' -or $_.DisplayName -Like 'NanoSandbox*' } | Select-Object Name,DisplayName,Enabled | Sort-Object Name | ConvertTo-Json -Compress); $m=Test-Path -LiteralPath '${String(nanoHome).replaceAll("'", "''")}\\.sandbox\\setup-marker.json'; [pscustomobject]@{users=$u;firewall=$f;marker=$m}|ConvertTo-Json -Compress`;
  const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8', windowsHide: true });
  if (result.status !== 0) throw new Error('SNAPSHOT_FAILED');
  return crypto.createHash('sha256').update(result.stdout.trim()).digest('hex');
}

function validate(payload, live = null) {
  const gate = new GateContract(INVENTORY);
  const keys = Object.keys(payload).sort();
  const allowed = payload.uninstall === true ? [...KEYS, 'uninstall'].sort() : [...KEYS].sort();
  if (JSON.stringify(keys) !== JSON.stringify(allowed) || payload.mode !== 'provision-only' || payload.refresh_only !== false) gate.fail('PV-01', 'structure');
  if (!IDENTITIES.has(payload.offline_username) || !IDENTITIES.has(payload.online_username) || payload.offline_username === payload.online_username || /CodexSandbox|NanoK3/.test(JSON.stringify(payload))) gate.fail('PV-02', 'security');
  const operations = [`user:${payload.offline_username}`, `user:${payload.online_username}`, ...PATH_LISTS.flatMap((name) => Array.isArray(payload[name]) ? payload[name].map((value) => `${name}:${String(value).toLowerCase()}`) : [])];
  if (operations.length !== new Set(operations).size || PATH_LISTS.some((name) => !Array.isArray(payload[name])) || !Array.isArray(payload.proxy_ports) || new Set(payload.proxy_ports).size !== payload.proxy_ports.length) gate.fail('PV-03', 'relation');
  if (live) {
    const before = externalSnapshot(payload.nano_home);
    const encoded = Buffer.from(JSON.stringify(payload), 'utf8').toString('base64');
    const result = spawnSync(live.setup, [encoded], { encoding: 'utf8', windowsHide: true });
    const after = externalSnapshot(payload.nano_home);
    if (result.status === 0 || before !== after) gate.fail('PV-04', 'security');
  } else {
    const home = String(payload.nano_home || '').replaceAll('\\', '/').toLowerCase();
    const cancellation = String(payload.cancellation_path || '').replaceAll('\\', '/').toLowerCase();
    if (!home || !cancellation.startsWith(`${home}/.sandbox/cancel-`) || !/^cancel-[0-9a-f]{32}$/.test(path.posix.basename(cancellation))) gate.fail('PV-04', 'security');
  }
  if (!Number.isInteger(payload.version) || payload.version < 5) gate.fail('PV-05', 'value');
  const paths = PATH_LISTS.flatMap((name) => Array.isArray(payload[name]) ? payload[name] : []);
  if (paths.some((value) => /[*?]/.test(String(value))) || (payload.uninstall === true && (!IDENTITIES.has(payload.offline_username) || !IDENTITIES.has(payload.online_username) || payload.offline_username === payload.online_username))) gate.fail('PV-06', 'relation');
  return gate;
}

function malfunction() { process.stdout.write('gate: 0/6\n'); process.exitCode = 2; }
function main(argv) {
  const packetAt = argv.indexOf('--packet'); const liveAt = argv.indexOf('--live');
  if ((packetAt < 0) === (liveAt < 0)) return malfunction();
  try {
    let payload; let live = null;
    if (packetAt >= 0) {
      if (argv.length !== 2 || packetAt !== 0) return malfunction();
      payload = JSON.parse(fs.readFileSync(argv[1], 'utf8'));
    } else {
      if (argv.length !== 1 || process.platform !== 'win32') return malfunction();
      const dryRun = process.env.NANO_DRY_RUN_BIN; const setup = process.env.NANO_SETUP_BIN;
      if (!dryRun || !setup || !fs.existsSync(dryRun) || !fs.existsSync(setup)) return malfunction();
      const produced = spawnSync(dryRun, [], { encoding: 'utf8', windowsHide: true });
      if (produced.status !== 0) return malfunction();
      payload = extractFramedJson(produced.stdout); live = { setup };
    }
    const gate = validate(payload, live); gate.write(); process.exitCode = gate.failures.size ? 1 : 0;
  } catch { malfunction(); }
}

module.exports = { INVENTORY, KEYS, validate, externalSnapshot };
if (require.main === module) main(process.argv.slice(2));
