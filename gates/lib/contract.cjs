'use strict';

const CATEGORIES = Object.freeze(['structure', 'value', 'relation', 'grounding', 'execution', 'security']);
const ID_RE = /^[A-Z]{2,4}-[0-9]{2}$/;
const SUMMARY_RE = /^(?:[a-z0-9-]*gate|gate):\s*(\d+)\s*\/\s*(\d+)$/;

function normalizeInventory(inventory) {
  if (!Array.isArray(inventory) || inventory.length === 0) throw new Error('INVALID_INVENTORY');
  const seen = new Set();
  return inventory.map((check) => {
    const id = typeof check === 'string' ? check : Array.isArray(check) ? check[0] : check && check.id;
    const category = typeof check === 'string' ? undefined : Array.isArray(check) ? check[1] : check && check.category;
    if (!ID_RE.test(id || '') || (category !== undefined && !CATEGORIES.includes(category)) || seen.has(id)) {
      throw new Error('INVALID_INVENTORY');
    }
    seen.add(id);
    return { id, category };
  });
}

function renderGateOutput(inventory, failures = [], options = {}) {
  const checks = normalizeInventory(inventory);
  if (options.malfunction) return `gate: 0/${checks.length}\n`;
  const known = new Map(checks.map((check) => [check.id, check]));
  const failed = new Map();
  for (const failure of failures) {
    const id = typeof failure === 'string' ? failure : failure && failure.id;
    const suppliedCategory = typeof failure === 'string' ? undefined : failure && failure.category;
    const check = known.get(id);
    const category = suppliedCategory || (check && check.category);
    if (!check || !category || !CATEGORIES.includes(category) || (check.category && check.category !== category)) {
      throw new Error('INVALID_FAILURE');
    }
    failed.set(id, category);
  }
  const lines = [...failed].map(([id, category]) => `FAIL ${id} ${category}`);
  lines.push(`gate: ${checks.length - failed.size}/${checks.length}`);
  return `${lines.join('\n')}\n`;
}

class GateContract {
  constructor(inventory) {
    this.inventory = normalizeInventory(inventory);
    this.failures = new Map();
    this.broken = false;
  }
  fail(id, category) {
    const check = this.inventory.find((item) => item.id === id);
    if (!check || !CATEGORIES.includes(category) || (check.category && check.category !== category)) {
      this.broken = true;
      return;
    }
    this.failures.set(id, category);
  }
  malfunction() { this.broken = true; }
  render() {
    return renderGateOutput(this.inventory, [...this.failures].map(([id, category]) => ({ id, category })), { malfunction: this.broken });
  }
  write(stream = process.stdout) { stream.write(this.render()); }
}

function parseGateOutput(stdout, inventory) {
  let checks;
  const closed = (reason, passed = 0, total = checks ? checks.length : 0) => ({ ok: false, passed, total, failures: [], failClosed: reason });
  try { checks = normalizeInventory(inventory); } catch { return closed('InvalidInventory'); }
  if (typeof stdout !== 'string' || stdout.length === 0) return closed('NoGateOutput');
  const lines = stdout.split(/\r?\n/).filter((line) => line.length > 0);
  const summaries = lines.map((line) => line.match(SUMMARY_RE)).filter(Boolean);
  if (summaries.length === 0) return closed('NoGateOutput');
  const summary = summaries[summaries.length - 1];
  const known = new Map(checks.map((check) => [check.id, check]));
  const failed = new Set();
  for (const line of lines) {
    if (SUMMARY_RE.test(line) || !line.startsWith('FAIL ')) continue;
    const match = line.match(/^FAIL ([A-Z]{2,4}-[0-9]{2}) (structure|value|relation|grounding|execution|security)$/);
    if (!match) return closed('MalformedOutput');
    const check = known.get(match[1]);
    if (!check) return closed('UnknownCheckId');
    if (check.category && check.category !== match[2]) return closed('InconsistentSummary');
    if (failed.has(match[1])) return closed('InconsistentSummary');
    failed.add(match[1]);
  }
  const passed = Number(summary[1]);
  const total = Number(summary[2]);
  if (total !== checks.length || passed !== total - failed.size) {
    return closed('InconsistentSummary', passed, total);
  }
  return { ok: failed.size === 0, passed, total, failures: [...failed].map((id) => ({ id, category: known.get(id).category })), failClosed: null };
}

function canonicalJson(value) {
  function normalize(item) {
    if (item === null || typeof item === 'boolean' || typeof item === 'string') return typeof item === 'string' ? item.normalize('NFC') : item;
    if (typeof item === 'number') { if (!Number.isSafeInteger(item)) throw new Error('CANONICAL_JSON_INTEGER'); return item; }
    if (Array.isArray(item)) return item.map(normalize);
    if (item && Object.getPrototypeOf(item) === Object.prototype) {
      const out = {};
      const keys = Object.keys(item).map((key) => key.normalize('NFC')).sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
      if (new Set(keys).size !== keys.length) throw new Error('CANONICAL_JSON_NFC_COLLISION');
      for (const key of keys) out[key] = normalize(item[Object.keys(item).find((raw) => raw.normalize('NFC') === key)]);
      return out;
    }
    throw new Error('CANONICAL_JSON_TYPE');
  }
  return JSON.stringify(normalize(value));
}

function scoreMutants(gateId, mutants, inventory) {
  const defects = mutants.filter(({ output }) => parseGateOutput(output, inventory).ok).map(({ id }) => `GATE_DEFECT ${gateId} ${id}`);
  return { ok: defects.length === 0, defects };
}

module.exports = { CATEGORIES, ID_RE, SUMMARY_RE, GateContract, renderGateOutput, parseGateOutput, normalizeInventory, canonicalJson, scoreMutants };
