'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const { CATEGORIES, ID_RE } = require('./contract.cjs');

const TOP_FIELDS = new Set(['card', 'gate_id', 'domain', 'tier', 'gate_script_hash', 'relational_target', 'disclosure_default', 'checks', 'wrapped_tools', 'validation', 'gamed_modes', 'escape_hatch_bans']);
const HEX64 = /^[0-9a-f]{64}$/;
const SEAL_RE = /^sealed:dir-sha256:[0-9a-f]{64}$/;

function scalar(raw) {
  const value = raw.trim().replace(/\s+#.*$/, '');
  if (value === 'null') return null;
  if (/^[0-9]+$/.test(value)) return Number(value);
  if (value.startsWith('[') && value.endsWith(']')) return split(value.slice(1, -1)).map(scalar);
  return value.replace(/^(["'`])|(["'`])$/g, '');
}

function split(text) {
  const parts = []; let start = 0; let depth = 0; let quote = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) { if (c === quote) quote = null; continue; }
    if ('"\'`'.includes(c)) { quote = c; continue; }
    if (c === '[' || c === '{') depth++;
    else if (c === ']' || c === '}') depth--;
    else if (c === ',' && depth === 0) { parts.push(text.slice(start, i)); start = i + 1; }
  }
  parts.push(text.slice(start));
  return parts.map((part) => part.trim()).filter(Boolean);
}

function inlineMap(text) {
  const body = text.trim().replace(/^\{/, '').replace(/\}$/, '');
  const out = {};
  for (const part of splitMap(body)) {
    const colon = part.indexOf(':');
    if (colon < 1) throw new Error('CARD_INVALID inline map');
    const key = part.slice(0, colon).trim();
    if (Object.hasOwn(out, key)) throw new Error(`CARD_INVALID duplicate field ${key}`);
    out[key] = scalar(part.slice(colon + 1));
  }
  return out;
}

function splitMap(text) {
  const parts = []; let start = 0; let depth = 0; let quote = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) { if (c === quote) quote = null; continue; }
    if ('"\'`'.includes(c)) { quote = c; continue; }
    if (c === '[' || c === '{') depth++;
    else if (c === ']' || c === '}') depth--;
    else if (c === ',' && depth === 0 && /^\s*[a-z_]+\s*:/.test(text.slice(i + 1))) {
      parts.push(text.slice(start, i)); start = i + 1;
    }
  }
  parts.push(text.slice(start));
  return parts.map((part) => part.trim()).filter(Boolean);
}

function parseCard(text) {
  if (typeof text !== 'string') throw new Error('CARD_INVALID machine block');
  const lines = text.split(/\r?\n/);
  const fences = lines.map((line, index) => line === '---' ? index : -1).filter((index) => index >= 0);
  if (fences.length !== 2 || fences[1] <= fences[0] + 1) throw new Error('CARD_INVALID machine block');
  const block = lines.slice(fences[0] + 1, fences[1]);
  const card = { checks: [], wrapped_tools: [], validation: { mutants: [] }, gamed_modes: [], escape_hatch_bans: [], relational_target: {} };
  const topSeen = new Set();
  let section = null;
  let listObject = null;
  let mutantObject = null;
  for (const line of block) {
    if (!line.trim()) continue;
    const top = line.match(/^([a-z_]+):(?:\s*(.*))?$/);
    if (top) {
      const [, key, raw = ''] = top;
      if (!TOP_FIELDS.has(key)) throw new Error(`CARD_INVALID unknown field ${key}`);
      if (topSeen.has(key)) throw new Error(`CARD_INVALID duplicate field ${key}`);
      topSeen.add(key); section = key; listObject = null; mutantObject = null;
      if (raw) card[key] = scalar(raw);
      continue;
    }
    const item = line.trim().match(/^-\s+(\{.*\})$/);
    if (item && ['checks', 'wrapped_tools', 'gamed_modes', 'escape_hatch_bans'].includes(section)) {
      if (!Array.isArray(card[section])) card[section] = [];
      card[section].push(inlineMap(item[1]));
      continue;
    }
    const blockItem = line.match(/^\s{2}-\s+([a-z_]+):\s*(.*)$/);
    if (blockItem && ['checks', 'wrapped_tools', 'gamed_modes', 'escape_hatch_bans'].includes(section)) {
      if (!Array.isArray(card[section])) card[section] = [];
      listObject = { [blockItem[1]]: scalar(blockItem[2]) };
      card[section].push(listObject);
      continue;
    }
    const blockItemField = line.match(/^\s{4}([a-z_]+):\s*(.*)$/);
    if (blockItemField && listObject) {
      listObject[blockItemField[1]] = scalar(blockItemField[2]);
      continue;
    }
    if (section === 'relational_target') {
      const field = line.match(/^\s{2}([a-z_]+):\s*(.*)$/);
      if (field) {
        if (Object.hasOwn(card.relational_target, field[1])) throw new Error(`CARD_INVALID duplicate field ${field[1]}`);
        card.relational_target[field[1]] = scalar(field[2]);
        continue;
      }
    }
    if (section === 'validation') {
      const field = line.match(/^\s{2}([a-z_]+):\s*(.*)$/);
      if (field && field[1] === 'mutants') continue;
      if (field) {
        if (Object.hasOwn(card.validation, field[1])) throw new Error(`CARD_INVALID duplicate field ${field[1]}`);
        card.validation[field[1]] = scalar(field[2]);
        continue;
      }
      const mutant = line.match(/^\s{4}-\s+(\{.*\})$/);
      if (mutant) { card.validation.mutants.push(inlineMap(mutant[1])); continue; }
      const mutantStart = line.match(/^\s{4}-\s+([a-z_]+):\s*(.*)$/);
      if (mutantStart) {
        mutantObject = { [mutantStart[1]]: scalar(mutantStart[2]) };
        card.validation.mutants.push(mutantObject);
        continue;
      }
      const mutantField = line.match(/^\s{6}([a-z_]+):\s*(.*)$/);
      if (mutantField && mutantObject) { mutantObject[mutantField[1]] = scalar(mutantField[2]); continue; }
    }
    throw new Error('CARD_INVALID machine block');
  }
  for (const required of ['card', 'gate_id', 'domain', 'tier', 'gate_script_hash', 'checks', 'validation', 'gamed_modes']) {
    if (!topSeen.has(required)) throw new Error(`CARD_INVALID missing field ${required}`);
  }
  if (card.card !== 1 || card.domain !== 'repo-deliverable' || card.tier !== 1 || card.disclosure_default !== 'opaque' || !HEX64.test(card.gate_script_hash)) throw new Error('CARD_INVALID scalar');
  assertFields(card.relational_target, ['artifact', 'relation'], 'relational_target');
  if (!card.relational_target.artifact || !card.relational_target.relation) throw new Error('CARD_INVALID relational_target');
  const ids = new Set();
  for (const check of card.checks) {
    assertFields(check, ['id', 'category', 'desc', 'measures'], 'check');
    if (!ID_RE.test(check.id || '') || ids.has(check.id)) throw new Error('CARD_INVALID check id');
    if (!CATEGORIES.includes(check.category)) throw new Error('CARD_INVALID category');
    if (!check.desc || !check.measures) throw new Error('CARD_INVALID check');
    ids.add(check.id);
  }
  if (card.checks.length === 0) throw new Error('CARD_INVALID checks');
  assertFields(card.validation, ['reference', 'pool_min', 'pool_status', 'mutants', 'rotation_k', 'last_validated'], 'validation');
  if (!SEAL_RE.test(card.validation.reference || '')) throw new Error('CARD_INVALID seal');
  if (!Number.isInteger(card.validation.pool_min) || card.validation.pool_min < 1 || !['full', 'partial'].includes(card.validation.pool_status) || card.validation.mutants.length < card.validation.pool_min) throw new Error('CARD_INVALID validation');
  const mutantIds = new Set();
  for (const mutant of card.validation.mutants) {
    assertFields(mutant, ['id', 'class', 'why_fluent', 'expected_drop', 'must_fail', 'fixture'], 'mutant');
    if (!/^[a-z0-9-]+$/.test(mutant.id || '') || mutantIds.has(mutant.id) || mutant.class !== 'fluent-but-wrong' || !mutant.why_fluent || !Number.isInteger(mutant.expected_drop) || mutant.expected_drop < 1 || !Array.isArray(mutant.must_fail) || mutant.must_fail.length === 0 || mutant.must_fail.some((id) => !ids.has(id))) throw new Error('CARD_INVALID mutant');
    if (!SEAL_RE.test(mutant.fixture || '')) throw new Error('CARD_INVALID seal');
    mutantIds.add(mutant.id);
  }
  for (const tool of card.wrapped_tools) {
    assertFields(tool, ['name', 'version', 'license', 'role'], 'wrapped_tool');
    if (!tool.name || !tool.version || !tool.license || !tool.role) throw new Error('CARD_INVALID wrapped_tool');
  }
  for (const mode of card.gamed_modes) {
    assertFields(mode, ['mode', 'status', 'note'], 'gamed_mode');
    if (!mode.mode || !['sealed', 'mitigated'].includes(mode.status) || !mode.note) throw new Error('CARD_INVALID gamed mode');
  }
  for (const ban of card.escape_hatch_bans) {
    assertFields(ban, ['ban', 'check'], 'escape_hatch_ban');
    if (!ban.ban || !ids.has(ban.check)) throw new Error('CARD_INVALID escape_hatch_ban');
  }
  if (!Number.isInteger(card.validation.rotation_k) || card.validation.rotation_k < 1 || card.gamed_modes.length < 1) throw new Error('CARD_INVALID validation');
  card.validationCurrent = (actualHash) => card.validation.last_validated !== null && card.validation.last_validated === actualHash && card.gate_script_hash === actualHash;
  return card;
}

function assertFields(value, allowed, label) {
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(value)) if (!allowedSet.has(key)) throw new Error(`CARD_INVALID unknown field ${label}.${key}`);
  for (const key of allowed) if (!Object.hasOwn(value, key)) throw new Error(`CARD_INVALID missing field ${label}.${key}`);
}

function loadCard(cardPath) { return parseCard(fs.readFileSync(cardPath, 'utf8')); }
function scriptHash(scriptPath) { return crypto.createHash('sha256').update(fs.readFileSync(scriptPath)).digest('hex'); }

module.exports = { parseCard, loadCard, scriptHash };
