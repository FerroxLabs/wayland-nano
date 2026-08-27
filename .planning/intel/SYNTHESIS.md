# Program Document Synthesis

## Input

- Documents synthesized: 4
- SPEC: 3 (`MEMORY-CONTRACT.md`, `PROFILES-CONTRACT.md`, `NANO-MODULE-CONTRACT.md`)
- PRD: 1 (`NANO-PROGRAM-PLAN.md`)
- ADR: 0
- DOC: 0
- UNKNOWN: 0
- Resolvable cross-reference cycles: 0

## Decisions

- Locked ADR decisions: 0
- The source set contains fixed owner decisions inside the PRD, including memory-primary continuity and never-recycle agent IDs; these remain traceable through the requirements and constraints rather than being reclassified as ADRs.

## Requirements

- Requirements extracted: 16
- IDs: REQ-wp-0-contract-freeze, REQ-p-mem-1-core-memory-store, REQ-p-mem-sec-gate-pack, REQ-p-prof-profiles, REQ-p-mod-gap-manifest-integrity, REQ-p-bot-5a-agent-composition, REQ-p-bot-5b-persistence, REQ-p-bot-5c-proactivity, REQ-p-exe-1-browser-backend, REQ-p-exe-2-desktop-backend, REQ-p-cons-memory-compaction, REQ-p-proc-procedure-extraction, REQ-p-graph-1-code-index, REQ-p-graph-2-blast-radius, REQ-p-mem-kg-retrieval, REQ-p-xproj-opt-in

## Constraints

- Constraints extracted: 19
- Type breakdown: 4 api-contract, 4 schema, 9 protocol, 2 nfr

## Context

- Context topics: 0

## Conflicts

- Blockers: 0
- Competing acceptance variants: 0
- Auto-resolved: 2
- Detail: `.planning/INGEST-CONFLICTS.md`

## Intel files

- `.planning/intel/decisions.md`
- `.planning/intel/requirements.md`
- `.planning/intel/constraints.md`
- `.planning/intel/context.md`
