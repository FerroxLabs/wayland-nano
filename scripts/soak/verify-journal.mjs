#!/usr/bin/env node
import { readFile } from 'node:fs/promises';

const path = process.argv[2];
if (!path) throw new Error('usage: verify-journal.mjs <journal.jsonl>');
const bytes = await readFile(path, 'utf8');
const lines = bytes.split('\n');
let tornTail = 0;
let tornMiddle = 0;
let parsedOps = 0;
for (let index = 0; index < lines.length; index += 1) {
  const line = lines[index];
  if (!line.trim()) continue;
  try { JSON.parse(line); parsedOps += 1; }
  catch {
    const laterContent = lines.slice(index + 1).some((candidate) => candidate.trim());
    if (laterContent) tornMiddle += 1; else tornTail += 1;
  }
}
const result = { path, parsedOps, tornMiddle, tornTail, replayClean: tornMiddle === 0 };
process.stdout.write(`${JSON.stringify(result)}\n`);
if (tornMiddle > 0) process.exitCode = 1;
