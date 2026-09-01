#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const crypto = require('node:crypto');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const INVENTORY = [
  ['MS-01', 'security'],
  ['MS-02', 'relation'],
  ['MS-03', 'security'],
  ['MS-04', 'security'],
  ['MS-05', 'security'],
  ['MS-06', 'security'],
];
const CHILD_ENV_KEYS = process.platform === 'win32'
  ? ['PATH', 'HOME', 'TMPDIR', 'TEMP', 'TMP', 'SYSTEMROOT', 'PATHEXT', 'USERPROFILE', 'COMSPEC']
  : ['PATH', 'HOME', 'TMPDIR', 'TEMP', 'TMP'];

function childEnv() {
  const env = {};
  for (const key of CHILD_ENV_KEYS) {
    if (process.env[key] !== undefined) env[key] = process.env[key];
  }
  return env;
}

function failAll() {
  for (const [id, category] of INVENTORY) process.stdout.write(`FAIL ${id} ${category}\n`);
  process.stdout.write('gate: 0/6\n');
  process.exit(2);
}

if (process.argv.length !== 3) failAll();
let artifactPath = String(process.argv[2]);
if (process.platform === 'win32') artifactPath = artifactPath.replace(/^\\\\\?\\/, '');
let artifact;
let expected;
let artifactStat;
try {
  artifact = fs.realpathSync.native(artifactPath).toLowerCase();
  expected = fs.realpathSync.native(path.resolve('gates', 'fixtures', 'mem-sec')).toLowerCase();
  artifactStat = fs.lstatSync(artifactPath);
} catch {
  failAll();
}
if (!artifactStat.isDirectory() || artifactStat.isSymbolicLink() || artifact !== expected) failAll();

const harness = path.resolve('crates', 'nano-memory', 'tests', 'mem_sec_cards.rs');
if (!fs.existsSync(harness)) failAll();

const testArgs = ['mem_sec_gate_summary', '--exact', '--nocapture', '--test-threads=1'];
const restrictedWindows = process.platform === 'win32'
  && /^f:\\tmp\\wngc[0-9a-f]{12}-home$/i.test(process.env.USERPROFILE || '');
function checkedHarness(expectedPath, expectedHashPath) {
  let resolved;
  let stat;
  try {
    resolved = fs.realpathSync.native(expectedPath).replace(/^\\\\\?\\/, '').toLowerCase();
    stat = fs.lstatSync(expectedPath);
  } catch {
    failAll();
  }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size === 0 || stat.size > 256 * 1024 * 1024
      || resolved !== expectedPath.toLowerCase()) failAll();
  if (expectedHashPath) {
    let expectedHash;
    try {
      const hashStat = fs.lstatSync(expectedHashPath);
      if (!hashStat.isFile() || hashStat.isSymbolicLink() || hashStat.size !== 64) failAll();
      expectedHash = fs.readFileSync(expectedHashPath, 'utf8');
    } catch {
      failAll();
    }
    if (!/^[0-9a-f]{64}$/.test(expectedHash)) failAll();
    const actualHash = crypto.createHash('sha256').update(fs.readFileSync(expectedPath)).digest('hex');
    if (actualHash !== expectedHash) failAll();
  }
  return expectedPath;
}

let result;
if (restrictedWindows) {
  const protectedHarness = String.raw`F:\gate-cards-bin\mem_sec_cards.exe`;
  result = spawnSync(checkedHarness(protectedHarness), testArgs, {
    cwd: process.cwd(), env: childEnv(), encoding: 'utf8', windowsHide: true,
  });
} else if (process.platform === 'win32') {
  const prebuiltHarness = path.resolve('target', 'mem_sec_cards.exe');
  const prebuiltHash = path.resolve('target', 'mem_sec_cards.sha256');
  const hasHarness = fs.existsSync(prebuiltHarness);
  const hasHash = fs.existsSync(prebuiltHash);
  if (hasHarness !== hasHash) failAll();
  if (hasHarness) {
    result = spawnSync(checkedHarness(prebuiltHarness, prebuiltHash), testArgs, {
      cwd: process.cwd(), env: childEnv(), encoding: 'utf8', windowsHide: true,
    });
  }
}
if (!result) {
  const target = path.join(os.tmpdir(), 'wayland-nano-p3-mem-sec-target');
  result = spawnSync('cargo', [
    'test',
    '--locked',
    '--target-dir',
    target,
    '-p',
    'nano-memory',
    '--test',
    'mem_sec_cards',
    '--',
    ...testArgs,
  ], { cwd: process.cwd(), env: childEnv(), encoding: 'utf8', windowsHide: true });
}
if (result.error) failAll();
const lines = (result.stdout || '').split(/\r?\n/).filter((line) =>
  /^FAIL MS-0[1-6] (structure|value|relation|grounding|execution|security)$/.test(line)
  || /^gate: \d+\/6$/.test(line),
);
if (lines.filter((line) => /^gate: /.test(line)).length !== 1) failAll();
process.stdout.write(`${lines.join('\n')}\n`);
if (result.status !== 0) process.exit(2);
