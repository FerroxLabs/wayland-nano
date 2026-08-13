#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const PACKAGE_ROOT = path.resolve(__dirname, '..');
const MANIFEST_PATH = path.join(PACKAGE_ROOT, 'binaries-manifest.json');
const SUPPORTED = new Set([
  'win32-x64',
  'darwin-arm64',
  'darwin-x64',
  'linux-x64',
  'linux-arm64',
]);

class PackagingError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'WaylandNanoPackagingError';
    this.code = code;
  }
}

function resolveNanoBinary(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  if (!SUPPORTED.has(key)) {
    throw new PackagingError(
      'WAYLAND_NANO_UNSUPPORTED_PLATFORM',
      `no prebuilt binary for ${key}; supported platforms: ${[...SUPPORTED].join(', ')}`,
    );
  }
  const filename = platform === 'win32' ? 'wayland-nano.exe' : 'wayland-nano';
  return {
    key,
    filename,
    binaryPath: path.join(PACKAGE_ROOT, 'binaries', key, filename),
  };
}

function verifyNanoBinary(resolved) {
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(MANIFEST_PATH, 'utf8'));
  } catch (error) {
    throw new PackagingError(
      'WAYLAND_NANO_INTEGRITY_MANIFEST',
      `cannot read integrity manifest ${MANIFEST_PATH}: ${error.message}; reinstall the package`,
    );
  }

  const expected = manifest.platforms && manifest.platforms[resolved.key];
  if (!expected || expected.file !== resolved.filename) {
    throw new PackagingError(
      'WAYLAND_NANO_INTEGRITY_MANIFEST',
      `integrity metadata for ${resolved.key} is absent or invalid; reinstall the package`,
    );
  }

  let bytes;
  try {
    bytes = fs.readFileSync(resolved.binaryPath);
  } catch (error) {
    throw new PackagingError(
      'WAYLAND_NANO_BINARY_MISSING',
      `cannot read ${resolved.binaryPath}: ${error.message}; reinstall the package`,
    );
  }
  const actualDigest = crypto.createHash('sha256').update(bytes).digest('hex');
  if (bytes.length !== expected.size || actualDigest !== expected.sha256) {
    throw new PackagingError(
      'WAYLAND_NANO_INTEGRITY_MISMATCH',
      `integrity check failed for ${resolved.key} (expected ${expected.size} bytes / ${expected.sha256}, got ${bytes.length} bytes / ${actualDigest}); reinstall from a trusted source`,
    );
  }
}

function main() {
  try {
    const resolved = resolveNanoBinary();
    verifyNanoBinary(resolved);
    if (process.platform !== 'win32') {
      fs.chmodSync(resolved.binaryPath, 0o755);
      // P4 PTY: the unix host-death sentinel ships beside the host binary
      // and needs the same exec bit (npm only preserves it for `bin`
      // entries). Absent ⇒ PTY fails closed at spawn (never unwatched).
      const guard = path.join(path.dirname(resolved.binaryPath), 'wayland-nano-pty-guard');
      if (fs.existsSync(guard)) {
        fs.chmodSync(guard, 0o755);
      }
    }
    console.log(`wayland-nano: verified prebuilt binary for ${resolved.key}`);
  } catch (error) {
    const code = error.code || 'WAYLAND_NANO_INSTALL_FAILED';
    console.error(`wayland-nano [${code}]: ${error.message}`);
    process.exitCode = 1;
  }
}

if (require.main === module) {
  main();
}

module.exports = { PackagingError, resolveNanoBinary, verifyNanoBinary };
