#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const SHELL_SHA256 = 'ac133a6c5d7addac4f4acd248b45d57b336ec4df22b0e6d8b94cecb3ff987c43';

function failClosed() {
  process.stdout.write('gate: 0/6\n');
  process.exit(2);
}

function bashArtifact(raw) {
  if (process.platform !== 'win32') return raw;
  let native = raw;
  if (/^\\\\\?\\[A-Za-z]:\\/.test(native)) native = native.slice(4);
  if (!/^[A-Za-z]:\\/.test(native)) return raw;
  const converted = spawnSync('cygpath.exe', ['-u', native], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (converted.status !== 0) failClosed();
  return converted.stdout.replace(/[\r\n]+$/g, '');
}

if (process.argv.length !== 3) failClosed();
const shell = path.join(__dirname, 'gate.sh');
const actual = crypto.createHash('sha256').update(fs.readFileSync(shell)).digest('hex');
if (actual !== SHELL_SHA256) failClosed();
const result = spawnSync('bash', [shell, bashArtifact(process.argv[2])], {
  cwd: process.cwd(),
  env: process.env,
  encoding: 'utf8',
  windowsHide: true,
});
if (result.error || result.status === null) failClosed();
process.stdout.write(result.stdout || '');
process.stderr.write(result.stderr || '');
process.exit(result.status);
