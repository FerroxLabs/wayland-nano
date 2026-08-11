#!/usr/bin/env node
'use strict';

// Postinstall: resolve the prebuilt `wayland-nano` binary for this platform/arch.
//
// Pattern choice — single package with an in-tree `binaries/` layout,
// NOT per-platform optionalDependencies packages:
//   * optionalDependencies (waylandnano-win32-x64, waylandnano-darwin-arm64, ...)
//     is the right pattern for a *published* package (per-platform tarballs
//     shrink installs and let npm skip unsupported platforms), but it
//     requires publishing N+1 packages and keeping their versions in lockstep.
//   * For the alpha we distribute one package with
//     `binaries/<platform>-<arch>/wayland-nano[.exe]` — simpler to build, sign,
//     and audit, and bin/install.js + bin/wayland-nano.js do the selection
//     locally with zero third-party dependencies (pure node stdlib).
//   * If/when we outgrow the single-tarball distribution, we can flip to
//     optionalDependencies without changing the launcher contract
//     (resolveNanoBinary below stays the single source of truth).

const fs = require('node:fs');
const path = require('node:path');

const PACKAGE_ROOT = path.resolve(__dirname, '..');

// Compile-gate only: the aarch64-pc-windows-msvc binary is built in CI to
// keep the target compiling, but it is NOT supported at runtime yet.
const UNSUPPORTED_RUNTIME = new Set(['win32-arm64']);

function resolveNanoBinary(platform, arch) {
  const key = `${platform}-${arch}`;
  const exe = platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano';
  return { key, binaryPath: path.join(PACKAGE_ROOT, 'binaries', key, exe) };
}

function main() {
  const { key, binaryPath } = resolveNanoBinary(process.platform, process.arch);

  if (!fs.existsSync(binaryPath)) {
    const binariesDir = path.join(PACKAGE_ROOT, 'binaries');
    const available = fs.existsSync(binariesDir)
      ? fs
          .readdirSync(binariesDir)
          .filter((d) => !d.startsWith('.'))
          .sort()
      : [];
    console.error(
      `wayland-nano: no prebuilt binary for ${key}.\n` +
        `  This alpha ships binaries for: ${available.join(', ') || '(none — run scripts/pack.ps1 first)'}`,
    );
    process.exit(1);
  }

  if (process.platform !== 'win32') {
    fs.chmodSync(binaryPath, 0o755);
  }

  if (UNSUPPORTED_RUNTIME.has(key)) {
    console.warn(
      `wayland-nano: WARNING — ${key} is compile-gated only and not supported at runtime in this alpha.`,
    );
  }

  console.log(`wayland-nano: using prebuilt binary for ${key}`);
}

main();
