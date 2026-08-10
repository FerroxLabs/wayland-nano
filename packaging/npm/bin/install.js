#!/usr/bin/env node
'use strict';

// Postinstall: resolve the prebuilt `nanok3` binary for this platform/arch.
//
// Pattern choice — single package with an in-tree `binaries/` layout,
// NOT per-platform optionalDependencies packages:
//   * optionalDependencies (@nanok3/win32-x64, @nanok3/darwin-arm64, ...)
//     is the right pattern for a *published* package (per-platform tarballs
//     shrink installs and let npm skip unsupported platforms), but it
//     requires publishing N+1 packages and keeping their versions in lockstep.
//   * This package is `private: true` for the alpha: we distribute one
//     tarball (npm pack / GitHub release asset), so a single package with
//     `binaries/<platform>-<arch>/nanok3[.exe]` is simpler to build, sign,
//     and audit, and bin/install.js + bin/nanok3.js do the selection
//     locally with zero third-party dependencies (pure node stdlib).
//   * If/when we publish to the public registry, we can flip to
//     optionalDependencies without changing the launcher contract
//     (resolveNanok3Binary below stays the single source of truth).

const fs = require('node:fs');
const path = require('node:path');

const PACKAGE_ROOT = path.resolve(__dirname, '..');

// Compile-gate only: the aarch64-pc-windows-msvc binary is built in CI to
// keep the target compiling, but it is NOT supported at runtime yet.
const UNSUPPORTED_RUNTIME = new Set(['win32-arm64']);

function resolveNanok3Binary(platform, arch) {
  const key = `${platform}-${arch}`;
  const exe = platform === 'win32' ? 'nanok3.exe' : 'nanok3';
  return { key, binaryPath: path.join(PACKAGE_ROOT, 'binaries', key, exe) };
}

function main() {
  const { key, binaryPath } = resolveNanok3Binary(process.platform, process.arch);

  if (!fs.existsSync(binaryPath)) {
    const binariesDir = path.join(PACKAGE_ROOT, 'binaries');
    const available = fs.existsSync(binariesDir)
      ? fs
          .readdirSync(binariesDir)
          .filter((d) => !d.startsWith('.'))
          .sort()
      : [];
    console.error(
      `nanok3: no prebuilt binary for ${key}.\n` +
        `  This alpha ships binaries for: ${available.join(', ') || '(none — run scripts/pack.ps1 first)'}`,
    );
    process.exit(1);
  }

  if (process.platform !== 'win32') {
    fs.chmodSync(binaryPath, 0o755);
  }

  if (UNSUPPORTED_RUNTIME.has(key)) {
    console.warn(
      `nanok3: WARNING — ${key} is compile-gated only and not supported at runtime in this alpha.`,
    );
  }

  console.log(`nanok3: using prebuilt binary for ${key}`);
}

main();
