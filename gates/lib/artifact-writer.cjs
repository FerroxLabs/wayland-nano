'use strict';

const fs = require('node:fs');
const fsp = fs.promises;
const path = require('node:path');
const crypto = require('node:crypto');
const { execFile } = require('node:child_process');
const { promisify } = require('node:util');

const execFileAsync = promisify(execFile);
const LOCK_RETRY_MS = 50;
const LOCK_TIMEOUT_MS = 10_000;
const LOCK_STALE_MS = 60_000;
const READ_RETRY_MS = 100;

class ArtifactError extends Error {
  constructor(code) {
    super(code);
    this.name = 'ArtifactError';
    this.code = code;
  }
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function lockPathFor(target) {
  return `${target}.lock`;
}

function temporaryPathFor(target) {
  const nonce = crypto.randomBytes(16).toString('hex');
  return path.join(path.dirname(target), `.${path.basename(target)}.${process.pid}.${nonce}.tmp`);
}

function wrap(code) {
  return new ArtifactError(code);
}

async function removeOwnedFile(file, io) {
  try {
    await io.unlink(file);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}

async function acquireLock(lockPath, options) {
  const { io, sleep, now } = options;
  const started = now();

  for (;;) {
    try {
      const handle = await io.open(lockPath, 'wx', 0o600);
      const token = crypto.randomBytes(16).toString('hex');
      try {
        await handle.writeFile(`${now()} ${token}\n`, { encoding: 'ascii' });
        await handle.sync();
      } catch (error) {
        await handle.close().catch(() => {});
        await removeOwnedFile(lockPath, io).catch(() => {});
        throw error;
      }
      return { handle, token };
    } catch (error) {
      if (error.code !== 'EEXIST') throw wrap('LOCK_CREATE_FAILED');

      let stale = false;
      try {
        const stat = await io.stat(lockPath);
        stale = now() - stat.mtimeMs > options.staleLockMs;
      } catch (statError) {
        if (statError.code !== 'ENOENT') throw wrap('LOCK_INSPECTION_FAILED');
      }

      if (stale) {
        let removed = false;
        try {
          await io.unlink(lockPath);
          removed = true;
        } catch (unlinkError) {
          if (unlinkError.code === 'ENOENT') removed = true;
        }
        if (removed) continue;
        if (now() - started >= options.lockTimeoutMs) throw wrap('ARTIFACT_LOCK_TIMEOUT');
        await sleep(options.retryMs);
        continue;
      }

      if (now() - started >= options.lockTimeoutMs) throw wrap('ARTIFACT_LOCK_TIMEOUT');
      await sleep(options.retryMs);
    }
  }
}

async function windowsReplace(temp, target, options) {
  if (options.replace) return options.replace(temp, target);
  const helper = path.join(__dirname, 'atomic-replace-win32.ps1');
  try {
    await execFileAsync(options.powershell || 'powershell.exe', [
      '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
      '-File', helper, '-Source', temp, '-Destination', target,
    ], { windowsHide: true, timeout: LOCK_TIMEOUT_MS });
  } catch (_) {
    throw wrap('ATOMIC_REPLACE_FAILED');
  }
}

async function unixReplace(temp, target, options) {
  try {
    if (options.replace) await options.replace(temp, target);
    else await options.io.rename(temp, target);
  } catch (_) {
    throw wrap('ATOMIC_REPLACE_FAILED');
  }

  let directory;
  try {
    directory = await options.io.open(path.dirname(target), 'r');
    await directory.sync();
  } catch (_) {
    throw wrap('DIRECTORY_SYNC_FAILED');
  } finally {
    if (directory) await directory.close().catch(() => {});
  }
}

function normalizedOptions(options = {}) {
  // The optional seams are test-only fault/clock/I/O controls. Production callers
  // omit them and therefore cannot weaken the locked 50 ms/10 s/60 s/100 ms policy.
  return {
    io: options.io || fsp,
    sleep: options.sleep || delay,
    now: options.now || Date.now,
    platform: options.platform || process.platform,
    replace: options.replace,
    powershell: options.powershell,
    lockTimeoutMs: options.lockTimeoutMs ?? LOCK_TIMEOUT_MS,
    retryMs: options.retryMs ?? LOCK_RETRY_MS,
    staleLockMs: options.staleLockMs ?? LOCK_STALE_MS,
    read: options.read,
    readRetryMs: options.readRetryMs ?? options.retryMs ?? READ_RETRY_MS,
    injectFailure: options.injectFailure,
  };
}

async function writeArtifact(target, payload, suppliedOptions = {}) {
  if (typeof target !== 'string' || target.length === 0) throw wrap('INVALID_TARGET');
  const bytes = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
  const targetPath = path.resolve(target);
  const lockPath = lockPathFor(targetPath);
  const tempPath = temporaryPathFor(targetPath);
  const options = normalizedOptions(suppliedOptions);
  let lock;
  let temp;
  let primaryError;

  try {
    lock = await acquireLock(lockPath, options);
    try {
      temp = await options.io.open(tempPath, 'wx', 0o600);
      await temp.writeFile(bytes);
      if (options.injectFailure === 'sync') throw new Error('injected sync failure');
      await temp.sync();
      await temp.close();
      temp = undefined;
    } catch (_) {
      throw wrap('TEMP_WRITE_FAILED');
    }

    if (options.injectFailure === 'replace') throw wrap('ARTIFACT_WRITE_FAILED');
    if (options.platform === 'win32') await windowsReplace(tempPath, targetPath, options);
    else await unixReplace(tempPath, targetPath, options);
  } catch (error) {
    primaryError = error instanceof ArtifactError ? error : wrap('WRITE_FAILED');
  } finally {
    if (temp) await temp.close().catch(() => {});
    try {
      await removeOwnedFile(tempPath, options.io);
    } catch (_) {
      if (!primaryError) primaryError = wrap('TEMP_CLEANUP_FAILED');
    }
    if (lock) await lock.handle.close().catch(() => {});
    if (lock) {
      try {
        // A stale breaker may have replaced our pathname while the write was in
        // flight. Never unlink a successor's create-new lock.
        const contents = await options.io.readFile(lockPath, 'ascii');
        if (contents.endsWith(` ${lock.token}\n`)) await removeOwnedFile(lockPath, options.io);
      } catch (_) {
        if (!primaryError) primaryError = wrap('LOCK_RELEASE_FAILED');
      }
    }
  }

  if (primaryError) {
    if (primaryError.code === 'ARTIFACT_LOCK_TIMEOUT') throw primaryError;
    throw wrap('ARTIFACT_WRITE_FAILED');
  }
}

async function readArtifact(target, parserOrOptions, maybeOptions) {
  const parser = typeof parserOrOptions === 'function' ? parserOrOptions : (bytes) => bytes;
  const suppliedOptions = typeof parserOrOptions === 'function' ? maybeOptions : parserOrOptions;
  const options = normalizedOptions(suppliedOptions);
  const targetPath = path.resolve(target);

  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const bytes = options.read ? await options.read(targetPath) : await options.io.readFile(targetPath);
      return await parser(bytes);
    } catch (_) {
      if (attempt === 0) await options.sleep(options.readRetryMs);
    }
  }
  throw wrap('ARTIFACT_UNVERIFIABLE');
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks);
}

async function main(argv) {
  if (argv.length !== 2 || !['write', 'read'].includes(argv[0])) throw wrap('USAGE');
  if (argv[0] === 'write') await writeArtifact(argv[1], await readStdin());
  else process.stdout.write(await readArtifact(argv[1]));
}

module.exports = {
  ArtifactError,
  writeArtifact,
  readArtifact,
  write: writeArtifact,
  read: readArtifact,
  constants: Object.freeze({ LOCK_RETRY_MS, LOCK_TIMEOUT_MS, LOCK_STALE_MS, READ_RETRY_MS }),
};

if (require.main === module) {
  main(process.argv.slice(2)).catch((error) => {
    const code = error instanceof ArtifactError ? error.code : 'INTERNAL_ERROR';
    process.stderr.write(`artifact writer: ${code}\n`);
    process.exitCode = 1;
  });
}
