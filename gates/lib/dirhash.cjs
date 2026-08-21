#!/usr/bin/env node
'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

function sha256(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

function enumerate(root, current = root, seen = new Set(), files = []) {
  for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
    const absolute = path.join(current, entry.name);
    const stat = fs.lstatSync(absolute);
    if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) {
      throw new Error(stat.isSymbolicLink() ? 'DIRHASH_INVALID symbolic link' : 'DIRHASH_INVALID special file');
    }
    if (stat.isDirectory()) {
      enumerate(root, absolute, seen, files);
      continue;
    }
    const relative = path.relative(root, absolute).split(path.sep).join('/').normalize('NFC');
    if (!relative || relative.startsWith('../') || path.posix.isAbsolute(relative)) {
      throw new Error('DIRHASH_PATH_ESCAPE');
    }
    if (seen.has(relative)) throw new Error('DIRHASH_INVALID NFC collision');
    seen.add(relative);
    files.push({ relative, absolute });
  }
  return files;
}

function digestDirectory(root) {
  const stat = fs.lstatSync(root);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('DIRHASH_ROOT_NOT_DIRECTORY');
  const files = enumerate(path.resolve(root));
  files.sort((a, b) => Buffer.compare(Buffer.from(a.relative), Buffer.from(b.relative)));
  const entries = files.map(({ relative, absolute }) => [relative, sha256(fs.readFileSync(absolute))]);
  const manifest = entries.map(([relative, digest]) => `${relative}  ${digest}\n`).join('');
  return { digest: sha256(Buffer.from(manifest, 'utf8')), manifest, entries };
}

const hashDirectory = (root) => digestDirectory(root).digest;
const directorySeal = (root) => `sealed:dir-sha256:${hashDirectory(root)}`;

module.exports = { digestDirectory, hashDirectory, directorySeal, sha256 };

if (require.main === module) {
  if (process.argv.length !== 3) {
    process.stderr.write('usage: node gates/lib/dirhash.cjs <directory>\n');
    process.exitCode = 2;
  } else {
    try {
      process.stdout.write(`${hashDirectory(process.argv[2])}\n`);
    } catch {
      process.stderr.write('DIRHASH_FAILED\n');
      process.exitCode = 1;
    }
  }
}
