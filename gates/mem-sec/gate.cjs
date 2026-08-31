#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
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

const target = path.join(os.tmpdir(), 'wayland-nano-p3-mem-sec-target');
const result = spawnSync(
  'cargo',
  [
    'test',
    '--locked',
    '--target-dir',
    target,
    '-p',
    'nano-memory',
    '--test',
    'mem_sec_cards',
    'mem_sec_gate_summary',
    '--',
    '--exact',
    '--nocapture',
    '--test-threads=1',
  ],
  { cwd: process.cwd(), env: process.env, encoding: 'utf8', windowsHide: true },
);
if (result.error || result.status !== 0) failAll();
const lines = (result.stdout || '').split(/\r?\n/).filter((line) =>
  /^FAIL MS-0[1-6] (structure|value|relation|grounding|execution|security)$/.test(line)
  || /^gate: \d+\/6$/.test(line),
);
if (lines.filter((line) => /^gate: /.test(line)).length !== 1) failAll();
process.stdout.write(`${lines.join('\n')}\n`);
