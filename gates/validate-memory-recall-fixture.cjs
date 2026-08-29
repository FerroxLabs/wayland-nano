#!/usr/bin/env node
'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const fixturePath = path.join(__dirname, 'fixtures', 'memory-retrieval-recall-v1', 'fixture.json');
const fixture = JSON.parse(fs.readFileSync(fixturePath, 'utf8'));
const projects = ['project-a', 'project-b'];
const agents = ['bot-a', 'bot-b'];
const trusts = ['User', 'ToolOutput', 'ModelInference'];

function exactKeys(value, keys, label) {
  assert.deepEqual(Object.keys(value).sort(), [...keys].sort(), `${label}: unexpected or missing field`);
}

function nonEmptyString(value, label) {
  assert.equal(typeof value, 'string', `${label}: must be a string`);
  assert.ok(value.trim().length > 0, `${label}: must not be blank`);
}

function partition(row, label) {
  assert.ok(projects.includes(row.project), `${label}: unknown project`);
  assert.ok(agents.includes(row.agent_id), `${label}: unknown agent_id`);
}

function factSemanticKey(row) {
  return JSON.stringify([
    row.subject, row.predicate, row.object, row.confidence, row.source_episode,
    row.valid_from, row.valid_to, row.source_trust,
  ]);
}

function decisionSemanticKey(row) {
  return JSON.stringify([
    row.summary, row.why, row.how_to_apply, row.tags, row.source_episode, row.source_trust,
  ]);
}

function groups(rows, keyOf) {
  const result = new Map();
  for (const row of rows) {
    const key = keyOf(row);
    if (!result.has(key)) result.set(key, []);
    result.get(key).push(row);
  }
  return result;
}

function assertOpposingPairs(rows, keyOf, expectedGroups, label) {
  const semanticGroups = groups(rows, keyOf);
  assert.equal(semanticGroups.size, expectedGroups, `${label}: wrong semantic group count`);
  let projectOnly = 0;
  let agentOnly = 0;
  let both = 0;
  for (const pair of semanticGroups.values()) {
    assert.equal(pair.length, 2, `${label}: every semantic group must contain exactly two rows`);
    const projectDiffers = pair[0].project !== pair[1].project;
    const agentDiffers = pair[0].agent_id !== pair[1].agent_id;
    assert.ok(projectDiffers || agentDiffers, `${label}: duplicate pair must challenge a partition`);
    if (projectDiffers && agentDiffers) both += 1;
    else if (projectDiffers) projectOnly += 1;
    else agentOnly += 1;
  }
  if (label === 'facts') {
    assert.ok(projectOnly > 0, `${label}: no pair independently challenges project filtering`);
    assert.ok(agentOnly > 0, `${label}: no pair independently challenges agent filtering`);
  }
  assert.ok(both > 0, `${label}: no pair challenges combined filtering`);
  return semanticGroups;
}

exactKeys(fixture, ['version', 'facts', 'decisions', 'queries'], 'fixture');
assert.equal(fixture.version, 'memory-retrieval-recall-v1');
assert.ok(Array.isArray(fixture.facts));
assert.ok(Array.isArray(fixture.decisions));
assert.ok(Array.isArray(fixture.queries));
assert.equal(fixture.facts.length, 50, 'fixture must contain exactly 50 facts');
assert.equal(fixture.decisions.length, 10, 'fixture must contain exactly 10 decisions');
assert.equal(fixture.queries.length, 20, 'fixture must contain exactly 20 queries');

const ids = new Set();
for (const [index, fact] of fixture.facts.entries()) {
  const label = `fact[${index}]`;
  exactKeys(fact, [
    'id', 'subject', 'predicate', 'object', 'confidence', 'source_episode', 'valid_from',
    'valid_to', 'source_trust', 'project', 'agent_id',
  ], label);
  for (const field of ['id', 'subject', 'predicate', 'object', 'valid_from']) nonEmptyString(fact[field], `${label}.${field}`);
  assert.match(fact.id, /^f\d{2}$/u, `${label}: malformed id`);
  assert.equal(ids.has(fact.id), false, `${label}: duplicate id`);
  ids.add(fact.id);
  assert.equal(typeof fact.confidence, 'number', `${label}: confidence must be numeric`);
  assert.ok(fact.confidence >= 0 && fact.confidence <= 1, `${label}: confidence out of range`);
  assert.ok(fact.source_episode === null || typeof fact.source_episode === 'string', `${label}: malformed source_episode`);
  assert.ok(fact.valid_to === null || typeof fact.valid_to === 'string', `${label}: malformed valid_to`);
  assert.ok(trusts.includes(fact.source_trust), `${label}: unknown source_trust`);
  partition(fact, label);
}

for (const [index, decision] of fixture.decisions.entries()) {
  const label = `decision[${index}]`;
  exactKeys(decision, [
    'id', 'summary', 'why', 'how_to_apply', 'tags', 'source_episode', 'source_trust',
    'project', 'agent_id',
  ], label);
  for (const field of ['id', 'summary', 'why', 'how_to_apply']) nonEmptyString(decision[field], `${label}.${field}`);
  assert.match(decision.id, /^d\d{2}$/u, `${label}: malformed id`);
  assert.equal(ids.has(decision.id), false, `${label}: duplicate id`);
  ids.add(decision.id);
  assert.ok(Array.isArray(decision.tags) && decision.tags.length > 0, `${label}: tags must be non-empty`);
  for (const tag of decision.tags) nonEmptyString(tag, `${label}.tag`);
  assert.equal(new Set(decision.tags).size, decision.tags.length, `${label}: duplicate tag`);
  assert.ok(decision.source_episode === null || typeof decision.source_episode === 'string', `${label}: malformed source_episode`);
  assert.ok(trusts.includes(decision.source_trust), `${label}: unknown source_trust`);
  partition(decision, label);
}

const rows = [...fixture.facts, ...fixture.decisions];
for (const project of projects) {
  assert.equal(rows.filter((row) => row.project === project).length, 30, `${project}: must own exactly 30 rows`);
}
for (const agent of agents) {
  assert.equal(rows.filter((row) => row.agent_id === agent).length, 30, `${agent}: must own exactly 30 rows`);
}

const factGroups = assertOpposingPairs(fixture.facts, factSemanticKey, 25, 'facts');
const decisionGroups = assertOpposingPairs(fixture.decisions, decisionSemanticKey, 5, 'decisions');
const rowById = new Map(rows.map((row) => [row.id, row]));
const labels = new Set();
const queryPartitions = new Set();
let projectFilterChallenges = 0;
let agentFilterChallenges = 0;
let combinedFilterChallenges = 0;

for (const [index, query] of fixture.queries.entries()) {
  const label = `query[${index}]`;
  exactKeys(query, ['label', 'text', 'project', 'agent_id', 'relevant_ids'], label);
  nonEmptyString(query.label, `${label}.label`);
  nonEmptyString(query.text, `${label}.text`);
  assert.match(query.label, /^q\d{2}-[a-z0-9-]+$/u, `${label}: malformed label`);
  assert.equal(labels.has(query.label), false, `${label}: duplicate label`);
  labels.add(query.label);
  partition(query, label);
  queryPartitions.add(`${query.project}/${query.agent_id}`);
  assert.ok(Array.isArray(query.relevant_ids), `${label}: relevant_ids must be an array`);
  assert.equal(query.relevant_ids.length, 1, `${label}: exactly one human-readable relevance label is required`);
  const relevant = rowById.get(query.relevant_ids[0]);
  assert.ok(relevant, `${label}: relevant id does not exist`);
  assert.equal(relevant.project, query.project, `${label}: relevant row belongs to wrong project`);
  assert.equal(relevant.agent_id, query.agent_id, `${label}: relevant row belongs to wrong agent`);
  const semanticGroups = relevant.id.startsWith('f') ? factGroups : decisionGroups;
  const key = relevant.id.startsWith('f') ? factSemanticKey(relevant) : decisionSemanticKey(relevant);
  const counterpart = semanticGroups.get(key).find((row) => row.id !== relevant.id);
  assert.ok(counterpart, `${label}: missing identical wrong-partition competitor`);
  const projectDiffers = counterpart.project !== query.project;
  const agentDiffers = counterpart.agent_id !== query.agent_id;
  assert.ok(projectDiffers || agentDiffers, `${label}: competitor is not in a wrong partition`);
  if (projectDiffers && agentDiffers) combinedFilterChallenges += 1;
  else if (projectDiffers) projectFilterChallenges += 1;
  else agentFilterChallenges += 1;
}

assert.deepEqual([...queryPartitions].sort(), [
  'project-a/bot-a', 'project-a/bot-b', 'project-b/bot-a', 'project-b/bot-b',
], 'queries must exercise all project/agent partitions');
assert.ok(projectFilterChallenges > 0, 'queries must expose an omitted project filter while agent filtering remains');
assert.ok(agentFilterChallenges > 0, 'queries must expose an omitted agent filter while project filtering remains');
assert.ok(combinedFilterChallenges > 0, 'queries must expose omission of both filters');

process.stdout.write('memory-retrieval-recall-v1 fixture honesty: PASS (50 facts, 10 decisions, 20 queries, 30/30 project, 30/30 agent, 30 opposing semantic pairs)\n');
