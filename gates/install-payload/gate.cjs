#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { loadCard } = require('../lib/card.cjs');
const { GateContract } = require('../lib/contract.cjs');
const { directorySeal } = require('../lib/dirhash.cjs');

const INVENTORY = [
  ['IP-01', 'execution'], ['IP-02', 'structure'], ['IP-03', 'value'],
  ['IP-04', 'security'], ['IP-05', 'execution'], ['IP-06', 'structure'],
];
const PLATFORMS = new Set(['win32-x64', 'darwin-arm64', 'darwin-x64', 'linux-x64', 'linux-arm64']);
const HEX64 = /^[0-9a-f]{64}$/;
const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

function failAll(contract) {
  for (const [id, category] of INVENTORY) contract.fail(id, category);
}

function safeEntries(directory) {
  const names = [];
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('unsafe binary entry');
    names.push(entry.name);
  }
  return names;
}

function validateManifest(manifest) {
  if (!manifest || Object.getPrototypeOf(manifest) !== Object.prototype
      || manifest.schema !== 1 || manifest.algorithm !== 'sha256'
      || !manifest.platforms || Object.getPrototypeOf(manifest.platforms) !== Object.prototype
      || Object.keys(manifest.platforms).length !== PLATFORMS.size
      || Object.keys(manifest.platforms).some((key) => !PLATFORMS.has(key))) return false;
  for (const entry of Object.values(manifest.platforms)) {
    if (!entry || Object.getPrototypeOf(entry) !== Object.prototype
        || Object.keys(entry).sort().join(',') !== 'file,helpers,sha256,size'
        || typeof entry.file !== 'string' || entry.file.length === 0
        || !Number.isSafeInteger(entry.size) || entry.size < 0 || !HEX64.test(entry.sha256)
        || !Array.isArray(entry.helpers)) return false;
    for (const helper of entry.helpers) {
      if (!helper || Object.getPrototypeOf(helper) !== Object.prototype
          || Object.keys(helper).sort().join(',') !== 'file,sha256,size'
          || typeof helper.file !== 'string' || helper.file.length === 0
          || !Number.isSafeInteger(helper.size) || helper.size < 0 || !HEX64.test(helper.sha256)) return false;
    }
  }
  return true;
}

function hostKey() {
  return `${process.platform}-${process.arch}`;
}

function runGate(source, expectedSeal, contract) {
  const resolved = path.resolve(source);
  if (!expectedSeal || !/^sealed:dir-sha256:[0-9a-f]{64}$/.test(expectedSeal)
      || !fs.statSync(resolved).isDirectory() || directorySeal(resolved) !== expectedSeal) {
    failAll(contract);
    return;
  }

  const tempRoot = path.resolve(process.env.NANO_WP4_TEMP_ROOT
    || path.join(__dirname, '..', '..', 'target', 'wp4-install-gate'));
  if (path.parse(tempRoot).root.toUpperCase() !== 'F:\\') throw new Error('scratch must be on F:');
  fs.mkdirSync(tempRoot, { recursive: true });
  const scratch = fs.mkdtempSync(path.join(tempRoot, 'run-'));
  const packageRoot = path.join(scratch, 'package');
  try {
    fs.cpSync(resolved, packageRoot, { recursive: true, errorOnExist: true });
    if (directorySeal(packageRoot) !== expectedSeal) {
      failAll(contract);
      return;
    }
    let manifest;
    try {
      manifest = JSON.parse(fs.readFileSync(path.join(packageRoot, 'binaries-manifest.json'), 'utf8'));
      if (!validateManifest(manifest)) throw new Error('invalid manifest');
    } catch { contract.fail('IP-06', 'structure'); }

    const binaryRoot = path.join(packageRoot, 'binaries');
    const onDisk = new Set();
    try {
      for (const platform of fs.readdirSync(binaryRoot)) {
        const platformRoot = path.join(binaryRoot, platform);
        if (!fs.statSync(platformRoot).isDirectory()) throw new Error('not directory');
        for (const file of safeEntries(platformRoot)) onDisk.add(`${platform}/${file}`);
      }
    } catch { contract.fail('IP-02', 'structure'); }
    const declared = new Set();
    for (const [platform, entry] of Object.entries(manifest?.platforms || {})) {
      declared.add(`${platform}/${entry.file}`);
      for (const helper of entry.helpers || []) declared.add(`${platform}/${helper.file}`);
    }
    if (onDisk.size !== declared.size || [...onDisk].some((file) => !declared.has(file))
        || [...declared].some((file) => !onDisk.has(file))) contract.fail('IP-02', 'structure');

    for (const [platform, entry] of Object.entries(manifest?.platforms || {})) {
      for (const item of [entry, ...(entry.helpers || [])]) {
        try {
          const bytes = fs.readFileSync(path.join(binaryRoot, platform, item.file));
          if (bytes.length !== item.size || sha256(bytes) !== item.sha256) contract.fail('IP-03', 'value');
        } catch { contract.fail('IP-03', 'value'); }
      }
    }

    const install = path.join(packageRoot, 'bin', 'install.js');
    const installed = spawnSync(process.execPath, [install], { cwd: packageRoot, encoding: 'utf8', timeout: 15_000, windowsHide: true });
    if (installed.status !== 0 || !/verified prebuilt binary/.test(installed.stdout)) contract.fail('IP-01', 'execution');

    const host = manifest?.platforms?.[hostKey()];
    if (!host) contract.fail('IP-04', 'security');
    else {
      try {
        const primary = path.join(binaryRoot, hostKey(), host.file);
        const original = fs.readFileSync(primary);
        if (original.length === 0) throw new Error('empty host binary');
        const tampered = Buffer.from(original); tampered[0] ^= 0xff;
        fs.writeFileSync(primary, tampered);
        const refused = spawnSync(process.execPath, [install], { cwd: packageRoot, encoding: 'utf8', timeout: 15_000, windowsHide: true });
        if (refused.status === 0 || !/WAYLAND_NANO_INTEGRITY_MISMATCH/.test(refused.stderr)) contract.fail('IP-04', 'security');
        fs.writeFileSync(primary, original);
      } catch { contract.fail('IP-04', 'security'); }
    }

    const wrapper = spawnSync(process.execPath, [path.join(packageRoot, 'bin', 'wayland-nano.js'), '--version'], {
      cwd: packageRoot, encoding: 'utf8', timeout: 15_000, windowsHide: true,
    });
    if (wrapper.status !== 0 || !/\b\d+\.\d+\.\d+\b/.test(`${wrapper.stdout}${wrapper.stderr}`)) contract.fail('IP-05', 'execution');
    if (process.platform !== 'win32' && host) {
      for (const item of [host, ...(host.helpers || [])]) {
        try { if ((fs.statSync(path.join(binaryRoot, hostKey(), item.file)).mode & 0o111) === 0) contract.fail('IP-05', 'execution'); }
        catch { contract.fail('IP-05', 'execution'); }
      }
    }
    const modeFixture = path.join(packageRoot, '.nano-fixture-modes.json');
    if (fs.existsSync(modeFixture)) {
      try {
        const modes = JSON.parse(fs.readFileSync(modeFixture));
        if (Object.values(modes).some((mode) => !Number.isInteger(mode) || (mode & 0o111) === 0)) contract.fail('IP-05', 'execution');
      } catch { contract.fail('IP-05', 'execution'); }
    }
  } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
}

function main(argv) {
  const contract = new GateContract(INVENTORY);
  try {
    if (argv.length < 1 || argv.length > 2) failAll(contract);
    else {
      // WP3 appends only registry.run_artifact. In that canonical production
      // form the card, not candidate-controlled manifest bytes, is the seal
      // authority. The explicit second argument remains an author-test seam.
      const expectedSeal = argv[1]
        || loadCard(path.join(__dirname, 'card.md')).validation.reference;
      runGate(argv[0], expectedSeal, contract);
    }
  } catch { failAll(contract); }
  contract.write();
  process.exitCode = contract.failures.size === 0 ? 0 : 1;
}

module.exports = { INVENTORY, runGate, validateManifest };

if (require.main === module) main(process.argv.slice(2));
