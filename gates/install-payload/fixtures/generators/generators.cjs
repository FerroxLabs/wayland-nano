#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { writeArtifact } = require('../../../lib/artifact-writer.cjs');
const { directorySeal } = require('../../../lib/dirhash.cjs');

const ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const PRODUCER = path.join(ROOT, 'packaging', 'npm');
const OUTPUT = path.join(ROOT, 'gates', 'fixtures', 'install-payload');
const CARD = path.join(ROOT, 'gates', 'install-payload', 'card.md');
const PLATFORMS = ['win32-x64', 'darwin-arm64', 'darwin-x64', 'linux-x64', 'linux-arm64'];
const SHA_RE = /^[0-9a-f]{64}$/;
const scriptBinary = Buffer.from('#!/usr/bin/env node\nprocess.stdout.write("wayland-nano 0.1.1\\n");\n');
const guardBinary = Buffer.from('#!/usr/bin/env node\nprocess.exit(0);\n');

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
const posix = (...parts) => path.posix.join(...parts);

function producerBytes(relative) {
  return fs.readFileSync(path.join(PRODUCER, ...relative.split('/')));
}

function baseFiles(hostBinary) {
  const files = new Map([
    ['package.json', producerBytes('package.json')],
    ['README.md', producerBytes('README.md')],
    ['bin/install.js', producerBytes('bin/install.js')],
    ['bin/wayland-nano.js', producerBytes('bin/wayland-nano.js')],
  ]);
  for (const platform of PLATFORMS) {
    const primary = platform === 'win32-x64' ? 'wayland-nano.exe' : 'wayland-nano';
    files.set(posix('binaries', platform, primary), platform === 'win32-x64' ? hostBinary : scriptBinary);
    if (platform !== 'win32-x64') files.set(posix('binaries', platform, 'wayland-nano-pty-guard'), guardBinary);
  }
  rebuildManifest(files);
  files.set('.nano-fixture-modes.json', Buffer.from(`${JSON.stringify({
    'darwin-arm64/wayland-nano': 493, 'darwin-arm64/wayland-nano-pty-guard': 493,
    'darwin-x64/wayland-nano': 493, 'darwin-x64/wayland-nano-pty-guard': 493,
    'linux-x64/wayland-nano': 493, 'linux-x64/wayland-nano-pty-guard': 493,
    'linux-arm64/wayland-nano': 493, 'linux-arm64/wayland-nano-pty-guard': 493,
  }, null, 2)}\n`));
  return files;
}

function rebuildManifest(files) {
  const platforms = {};
  for (const platform of PLATFORMS) {
    const primary = platform === 'win32-x64' ? 'wayland-nano.exe' : 'wayland-nano';
    const primaryBytes = files.get(posix('binaries', platform, primary));
    const helpers = [];
    if (platform !== 'win32-x64') {
      const helper = 'wayland-nano-pty-guard';
      const bytes = files.get(posix('binaries', platform, helper));
      helpers.push({ file: helper, size: bytes.length, sha256: sha256(bytes) });
    }
    platforms[platform] = { file: primary, size: primaryBytes.length, sha256: sha256(primaryBytes), helpers };
  }
  files.set('binaries-manifest.json', Buffer.from(`${JSON.stringify({ schema: 1, algorithm: 'sha256', platforms }, null, 2)}\n`));
}

function cloneFiles(files) {
  return new Map([...files].map(([name, bytes]) => [name, Buffer.from(bytes)]));
}

function mutate(reference, id) {
  const files = cloneFiles(reference);
  const manifest = JSON.parse(files.get('binaries-manifest.json'));
  if (id === 'ip-m1') files.set('binaries/linux-x64/wayland-nano', Buffer.from(files.get('binaries/win32-x64/wayland-nano.exe')));
  else if (id === 'ip-m2') { delete manifest.platforms['linux-arm64']; files.set('binaries-manifest.json', Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`)); }
  else if (id === 'ip-m3') { manifest.platforms['darwin-x64'].sha256 = '0'.repeat(64); files.set('binaries-manifest.json', Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`)); }
  else if (id === 'ip-m4') {
    for (const name of [...files.keys()]) if (name.startsWith('binaries/darwin-arm64/')) files.delete(name);
  } else if (id === 'ip-m5') {
    const modes = JSON.parse(files.get('.nano-fixture-modes.json'));
    modes['linux-x64/wayland-nano-pty-guard'] = 420;
    files.set('.nano-fixture-modes.json', Buffer.from(`${JSON.stringify(modes, null, 2)}\n`));
  } else if (id === 'ip-m6') files.set('bin/install.js', Buffer.from('process.exit(0);\n'));
  else throw new Error(`unknown mutant ${id}`);
  return files;
}

async function publish(directory, files) {
  const stageRoot = process.env.NANO_WP4_TEMP_ROOT;
  if (!stageRoot || path.parse(path.resolve(stageRoot)).root.toUpperCase() !== 'F:\\') throw new Error('NANO_WP4_TEMP_ROOT must be on F:');
  const stage = fs.mkdtempSync(path.join(path.resolve(stageRoot), 'ip-publish-'));
  try {
    for (const [relative, bytes] of files) {
      const staged = path.join(stage, ...relative.split('/'));
      fs.mkdirSync(path.dirname(staged), { recursive: true });
      fs.writeFileSync(staged, bytes);
    }
    const previous = [];
    const walk = (current) => {
      if (!fs.existsSync(current)) return;
      for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
        const absolute = path.join(current, entry.name);
        if (entry.isDirectory()) walk(absolute);
        else previous.push(path.relative(directory, absolute).split(path.sep).join('/'));
      }
    };
    walk(directory);
    for (const [relative] of files) {
      const target = path.join(directory, ...relative.split('/'));
      fs.mkdirSync(path.dirname(target), { recursive: true });
      await writeArtifact(target, fs.readFileSync(path.join(stage, ...relative.split('/'))));
    }
    for (const relative of previous) {
      if (!files.has(relative)) fs.rmSync(path.join(directory, ...relative.split('/')), { force: true });
    }
    if (process.platform !== 'win32' && files.has('.nano-fixture-modes.json')) {
      const modes = JSON.parse(files.get('.nano-fixture-modes.json'));
      for (const [relative, mode] of Object.entries(modes)) {
        fs.chmodSync(path.join(directory, 'binaries', ...relative.split('/')), mode);
      }
    }
  } finally { fs.rmSync(stage, { recursive: true, force: true }); }
}

function inspectFixture(directory) {
  const failures = new Set();
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(path.join(directory, 'binaries-manifest.json'), 'utf8'));
    if (manifest.schema !== 1 || manifest.algorithm !== 'sha256' || !manifest.platforms || Object.keys(manifest.platforms).length !== PLATFORMS.length) throw new Error('schema');
    for (const entry of Object.values(manifest.platforms)) {
      if (!entry.file || !Number.isSafeInteger(entry.size) || entry.size < 0 || !SHA_RE.test(entry.sha256 || '') || !Array.isArray(entry.helpers)) throw new Error('entry');
      for (const helper of entry.helpers) if (!helper.file || !Number.isSafeInteger(helper.size) || helper.size < 0 || !SHA_RE.test(helper.sha256 || '')) throw new Error('helper');
    }
  } catch { failures.add('IP-06'); }
  const disk = new Set();
  const binaryRoot = path.join(directory, 'binaries');
  if (fs.existsSync(binaryRoot)) for (const platform of fs.readdirSync(binaryRoot)) {
    const platformRoot = path.join(binaryRoot, platform);
    if (fs.statSync(platformRoot).isDirectory()) for (const file of fs.readdirSync(platformRoot)) disk.add(`${platform}/${file}`);
  }
  const declared = new Set();
  for (const [platform, entry] of Object.entries(manifest?.platforms || {})) {
    declared.add(`${platform}/${entry.file}`);
    for (const helper of entry.helpers || []) declared.add(`${platform}/${helper.file}`);
  }
  if (disk.size !== declared.size || [...disk].some((file) => !declared.has(file))) failures.add('IP-02');
  for (const [platform, entry] of Object.entries(manifest?.platforms || {})) for (const item of [entry, ...(entry.helpers || [])]) {
    try {
      const bytes = fs.readFileSync(path.join(binaryRoot, platform, item.file));
      if (bytes.length !== item.size || sha256(bytes) !== item.sha256) failures.add('IP-03');
    } catch { failures.add('IP-03'); }
  }
  try { if (!fs.readFileSync(path.join(directory, 'bin', 'install.js')).equals(producerBytes('bin/install.js'))) failures.add('IP-04'); } catch { failures.add('IP-04'); }
  try {
    const modes = JSON.parse(fs.readFileSync(path.join(directory, '.nano-fixture-modes.json')));
    if (Object.values(modes).some((mode) => (mode & 0o111) === 0)) failures.add('IP-05');
  } catch { failures.add('IP-05'); }
  if (!manifest?.platforms?.['win32-x64'] || failures.has('IP-06')) failures.add('IP-01');
  return { failures: [...failures].sort() };
}

function cardText(seals) {
  const gatePath = path.join(ROOT, 'gates', 'install-payload', 'gate.cjs');
  const gateHash = fs.existsSync(gatePath) ? sha256(fs.readFileSync(gatePath)) : 'a'.repeat(64);
  const rows = [
    ['ip-m1', 'the win32-x64 executable is staged under linux-x64 with the expected filename, so the tree looks complete', 'IP-03'],
    ['ip-m2', 'the valid manifest quietly omits the least-used linux-arm64 lane while its directory remains', 'IP-02'],
    ['ip-m3', 'a well-formed 64-hex digest pins previous bytes and passes superficial schema review', 'IP-03'],
    ['ip-m4', 'the darwin-arm64 directory is absent while the complete-looking manifest still declares it', 'IP-02'],
    ['ip-m5', 'the PTY guard is present and hashed but recorded non-executable while the primary smoke path remains green', 'IP-05'],
    ['ip-m6', 'postinstall is a successful no-op and the shipped wrapper and binaries still look runnable', 'IP-04'],
  ];
  const mutants = rows.map(([id, why, check]) => `    - { id: ${id}, class: fluent-but-wrong, why_fluent: ${why}, expected_drop: 1, must_fail: [${check}], fixture: ${seals[id]} }`).join('\n');
  return `---\ncard: 1\ngate_id: install-payload\ndomain: repo-deliverable\ntier: 1\ngate_script_hash: ${gateHash}\nrelational_target:\n  artifact: the staged npm package tree\n  relation: every staged binary resolves against the integrity manifest and install refuses tampering\ndisclosure_default: opaque\nchecks:\n  - { id: IP-01, category: execution, desc: postinstall completes on a clean prefix, measures: copied package install verifies the host platform }\n  - { id: IP-02, category: structure, desc: manifest and directory sets match, measures: bidirectional primary and helper inventory }\n  - { id: IP-03, category: value, desc: payload bytes match metadata, measures: independently recomputed size and sha256 for every binary }\n  - { id: IP-04, category: security, desc: tampered payload is refused, measures: one-byte host tamper returns WAYLAND_NANO_INTEGRITY_MISMATCH }\n  - { id: IP-05, category: execution, desc: wrapper and executable modes work, measures: wrapper emits semver and unix binaries are executable }\n  - { id: IP-06, category: structure, desc: manifest is well formed, measures: exact schema 1 sha256 entry and helper fields }\nwrapped_tools:\n  - { name: node, version: 20, license: MIT, role: stdlib-only gate and package lifecycle runtime }\nvalidation:\n  reference: ${seals.reference}\n  pool_min: 6\n  pool_status: full\n  mutants:\n${mutants}\n  rotation_k: 2\n  last_validated: null\ngamed_modes:\n  - { mode: hardcoded hashes over swapped bytes, status: sealed, note: ip-m1 and ip-m3 require independent whole-pool rehashing }\n  - { mode: host-only inspection, status: mitigated, note: IP-02 and IP-03 traverse every platform and helper }\nescape_hatch_bans:\n  - { ban: skipping postinstall because wrapper verification exists, check: IP-01 }\n  - { ban: treating the tamper probe failure as ignorable, check: IP-04 }\n---\n\n## Intent\n\nVerify the copied npm install payload without modifying or repairing packaging producers.\n`;
}

async function generate() {
  const hostPath = process.env.NANO_WP4_HOST_BINARY;
  if (!hostPath || !fs.statSync(hostPath).isFile()) throw new Error('NANO_WP4_HOST_BINARY is required');
  fs.mkdirSync(process.env.NANO_WP4_TEMP_ROOT, { recursive: true });
  const reference = baseFiles(fs.readFileSync(hostPath));
  const pools = new Map([['reference', reference]]);
  for (const id of ['ip-m1', 'ip-m2', 'ip-m3', 'ip-m4', 'ip-m5', 'ip-m6']) pools.set(id, mutate(reference, id));
  const seals = {};
  for (const [id, files] of pools) {
    const output = id === 'reference' ? path.join(OUTPUT, id) : path.join(OUTPUT, 'mutants', id);
    await publish(output, files);
    seals[id] = directorySeal(output);
  }
  await writeArtifact(CARD, cardText(seals));
}

function check() {
  const card = require('../../../lib/card.cjs').loadCard(CARD);
  if (card.validation.reference !== directorySeal(path.join(OUTPUT, 'reference'))) throw new Error('reference seal drift');
  for (const mutant of card.validation.mutants) if (mutant.fixture !== directorySeal(path.join(OUTPUT, 'mutants', mutant.id))) throw new Error(`${mutant.id} seal drift`);
}

async function repin() {
  const seals = { reference: directorySeal(path.join(OUTPUT, 'reference')) };
  for (const id of ['ip-m1', 'ip-m2', 'ip-m3', 'ip-m4', 'ip-m5', 'ip-m6']) {
    seals[id] = directorySeal(path.join(OUTPUT, 'mutants', id));
  }
  const gateHash = sha256(fs.readFileSync(path.join(ROOT, 'gates', 'install-payload', 'gate.cjs')));
  await writeArtifact(CARD, cardText(seals).replace('  last_validated: null', `  last_validated: ${gateHash}`));
}

module.exports = { generate, inspectFixture };

if (require.main === module) {
  const work = process.argv.includes('--check') ? Promise.resolve().then(check)
    : process.argv.includes('--repin') ? repin() : generate();
  work.catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
