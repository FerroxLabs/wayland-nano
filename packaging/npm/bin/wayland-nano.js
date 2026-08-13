#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { resolveNanoBinary, verifyNanoBinary } = require('./install.js');

function main() {
  let resolved;
  try {
    resolved = resolveNanoBinary();
    verifyNanoBinary(resolved);
  } catch (error) {
    const code = error.code || 'WAYLAND_NANO_LAUNCH_FAILED';
    console.error(`wayland-nano [${code}]: ${error.message}`);
    process.exit(1);
  }

  const child = spawn(resolved.binaryPath, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: true,
  });

  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGBREAK']) {
    process.on(signal, () => child.kill(signal));
  }
  child.on('error', (error) => {
    console.error(`wayland-nano [WAYLAND_NANO_SPAWN_FAILED]: ${error.message}`);
    process.exit(1);
  });
  child.on('exit', (code, signal) => {
    if (signal) {
      try {
        process.kill(process.pid, signal);
      } catch {
        process.exit(1);
      }
    } else {
      process.exit(code === null ? 1 : code);
    }
  });
}

main();
