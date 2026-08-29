'use strict';

const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const test = require('node:test');

const ROOT = path.resolve(__dirname, '..', '..');

test('memory-retrieval-recall-v1 fixture is structurally honest', () => {
  const result = spawnSync(process.execPath, ['gates/validate-memory-recall-fixture.cjs'], {
    cwd: ROOT,
    encoding: 'utf8',
    timeout: 30_000,
    windowsHide: true,
  });
  assert.equal(result.status, 0, `${result.stderr}\n${result.stdout}`);
  assert.match(result.stdout, /fixture honesty: PASS/u);
});
