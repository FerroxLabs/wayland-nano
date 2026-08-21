#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { writeArtifact } = require('../../../lib/artifact-writer.cjs');

const ROOT = path.resolve(__dirname, '..', '..', '..', '..');
// Producer-owner repair layered on the locked D-04 base. Mutants bind to the
// actual audited producer bytes under test, never to the mutable builder tree.
const BASE = '30dbe9d8311f1d2192774f04788f1107b6cbd631';
const OUT = path.join(ROOT, 'gates', 'fixtures', 'config-schema');
const sha = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
const gitShow = (rel) => {
  const r = spawnSync('git', ['show', `${BASE}:${rel}`], { cwd: ROOT, encoding: 'utf8' });
  if (r.status !== 0) throw new Error(`BASE_READ_FAILED ${rel}`);
  return r.stdout.replace(/\r\n/g, '\n');
};

function patchFor(rel, before, after) {
  const scratchRoot = process.env.NANO_CF_TEMP_ROOT || path.join(ROOT, 'target', 'cf-generator');
  fs.mkdirSync(scratchRoot, { recursive: true });
  const dir = fs.mkdtempSync(path.join(scratchRoot, 'diff-'));
  try {
    const old = path.join(dir, 'old'); const next = path.join(dir, 'new');
    fs.writeFileSync(old, before, 'utf8'); fs.writeFileSync(next, after, 'utf8');
    const r = spawnSync('git', ['diff', '--no-index', '--no-ext-diff', '--unified=3', '--', old, next], { encoding: 'utf8' });
    if (r.status !== 1) throw new Error(`DIFF_FAILED ${rel}`);
    const lines = r.stdout.replace(/\r\n/g, '\n').split('\n');
    lines[0] = `diff --git a/${rel} b/${rel}`;
    lines[2] = `--- a/${rel}`;
    lines[3] = `+++ b/${rel}`;
    return lines.join('\n');
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
}

function replaceOnce(text, from, to, id) {
  if (text.split(from).length !== 2) throw new Error(`ANCHOR_INVALID ${id}`);
  return text.replace(from, to);
}

function replaceFirst(text, from, to, id) {
  if (!text.includes(from)) throw new Error(`ANCHOR_INVALID ${id}`);
  return text.replace(from, to);
}

async function write(rel, content) {
  const target = path.join(OUT, rel);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  await writeArtifact(target, Buffer.from(content, 'utf8'));
}

async function main() {
  const valid = `[[rule]]\npattern = ["git", "status"]\nexact = true\ndecision = "allow"\n\n[[rule]]\npattern = ["rm", "-rf"]\nexact = false\ndecision = "deny"\n\n[[rule]]\npattern = ["cargo", "test"]\nexact = false\ndecision = "prompt"\n`;
  const deny = `[[rule]]\npattern = ["rm"]\nexact = false\ndecision = "deny"\n\n[[rule]]\npattern = ["git", "push"]\nexact = true\ndecision = "deny"\n\n[[rule]]\npattern = ["cargo"]\nexact = false\ndecision = "allow"\n`;
  const probes = {
    'valid.toml': valid,
    'unknown_top.toml': `${valid}\nunknown = true\n`,
    'unknown_rule.toml': valid.replace('exact = true', 'exact = true\nunknown = true'),
    'type_exact_string.toml': valid.replace('exact = true', 'exact = "yes"'),
    'type_decision_int.toml': valid.replace('decision = "allow"', 'decision = 3'),
    'type_pattern_string.toml': valid.replace('pattern = ["git", "status"]', 'pattern = "git"'),
    'deny_heavy.toml': deny,
    'overlong_command.toml': `[[rule]]\npattern = ["${'x'.repeat(4097)}"]\nexact = false\ndecision = "allow"\n`,
    'too_many_tokens.toml': `[[rule]]\npattern = [${Array.from({ length: 65 }, (_, i) => `"t${i}"`).join(', ')}]\nexact = false\ndecision = "allow"\n`,
  };
  for (const [name, body] of Object.entries(probes)) await write(`probes/${name}`, body);

  const core = gitShow('crates/nano-core/src/execrules.rs');
  const cli = gitShow('crates/nano-cli/src/rules_cmds.rs');
  const catalog = gitShow('crates/nano-model/data/providerCatalog.vendored.json');
  const mutations = {
    'cf-m1': ['crates/nano-core/src/execrules.rs', core, replaceOnce(core, '#[derive(Debug, Serialize, Deserialize)]\n#[serde(deny_unknown_fields)]\nstruct RuleFile', '#[derive(Debug, Serialize, Deserialize)]\nstruct RuleFile', 'cf-m1')],
    'cf-m2': ['crates/nano-core/src/execrules.rs', core, replaceOnce(core, '    #[serde(default)]\n    pub exact: bool,', '    #[serde(default, deserialize_with = "deserialize_exact_flexible")]\n    pub exact: bool,', 'cf-m2').replace('impl PrefixRule {', 'fn deserialize_exact_flexible<\'de, D: serde::Deserializer<\'de>>(d: D) -> Result<bool, D::Error> {\n    let value = toml::Value::deserialize(d)?;\n    Ok(matches!(value, toml::Value::Boolean(true)) || matches!(&value, toml::Value::String(s) if s == "yes"))\n}\n\nimpl PrefixRule {')],
    'cf-m3': ['crates/nano-cli/src/rules_cmds.rs', cli, replaceOnce(cli, '            for (index, rule) in rules.rules().iter().enumerate() {', '            for (index, rule) in rules.rules().iter().enumerate() {\n                if rule.decision == RuleDecision::Deny { continue; }', 'cf-m3')],
    'cf-m4': ['crates/nano-core/src/execrules.rs', core, replaceOnce(core, 'const MAX_COMMAND_BYTES: usize = 4 * 1024;', 'const MAX_COMMAND_BYTES: usize = 64 * 1024;', 'cf-m4')],
    'cf-m5': ['crates/nano-model/data/providerCatalog.vendored.json', catalog, replaceFirst(catalog, 'https://api.fluxrouter.ai', 'https://api2.fluxrouter.ai', 'cf-m5')],
    'cf-m6': ['crates/nano-core/src/execrules.rs', core, replaceOnce(core, 'pub enum RuleDecision {\n    Allow,\n    Prompt,\n    Deny,\n}', 'pub enum RuleDecision {\n    Allow,\n    Deny,\n    #[serde(other)]\n    Prompt,\n}', 'cf-m6')],
  };
  for (const [id, [rel, before, after]] of Object.entries(mutations)) await write(`mutants/${id}/mutant.diff`, patchFor(rel, before, after));

  const manifest = {
    schema: 1,
    locked_base: '05637086c81e88550edb002a916a80aff4b278dc',
    producer_owner_repair: BASE,
    base: BASE,
    parser_anchor: {
      'crates/nano-core/src/execrules.rs': sha(Buffer.from(core)),
      'crates/nano-cli/src/rules_cmds.rs': sha(Buffer.from(cli)),
    },
    catalog_anchor: {
      file: 'crates/nano-model/data/providerCatalog.vendored.json',
      pin: 'crates/nano-model/tests/provider_catalog.rs::RECORDED_SHA256',
    },
    mutants: Object.keys(mutations),
  };
  await write('manifest.json', `${JSON.stringify(manifest, null, 2)}\n`);
}

main().catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
