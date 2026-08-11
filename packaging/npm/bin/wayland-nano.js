#!/usr/bin/env node
'use strict';

// wayland-nano launcher shim.
// Resolves the prebuilt native binary for this platform/arch (see
// bin/install.js for the packaging-pattern rationale) and replaces this
// process's stdio with the child's, passing process.argv through verbatim.
// Exit codes and termination signals are forwarded so shells and CI see the
// native binary's real status, not the shim's.

const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');

const PACKAGE_ROOT = path.resolve(__dirname, '..');

// Compile-gate only — see bin/install.js.
const UNSUPPORTED_RUNTIME = new Set(['win32-arm64']);

function resolveBinary() {
  const key = `${process.platform}-${process.arch}`;
  const exe = process.platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano';
  return { key, binaryPath: path.join(PACKAGE_ROOT, 'binaries', key, exe) };
}

function main() {
  const { key, binaryPath } = resolveBinary();

  if (UNSUPPORTED_RUNTIME.has(key)) {
    console.error(
      `wayland-nano: ${key} is not supported at runtime in this alpha.\n` +
        '  The ARM64 Windows build is compile-gated only. Use win32-x64, darwin-x64, darwin-arm64, linux-x64, or linux-arm64.',
    );
    process.exit(1);
  }

  if (!fs.existsSync(binaryPath)) {
    console.error(
      `wayland-nano: no prebuilt binary for ${key} at ${binaryPath}.\n` +
        '  Reinstall the package, or build from source (see packaging/npm/README.md).',
    );
    process.exit(1);
  }

  const child = spawn(binaryPath, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: true,
  });

  // Forward termination signals to the child so Ctrl-C / kill behave as if
  // the native binary were invoked directly.
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGBREAK']) {
    process.on(signal, () => {
      child.kill(signal);
    });
  }

  child.on('error', (err) => {
    console.error(`wayland-nano: failed to start ${binaryPath}: ${err.message}`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      // Re-raise so our exit status reflects the signal (128+N) like the
      // native binary would. Guarded: on win32 self-signalling throws for
      // some signals.
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
