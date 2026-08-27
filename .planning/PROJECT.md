# Wayland Nano

## Core Value

A change earns trust only through independently rerunnable machine evidence; model confidence and self-report never substitute for a green gate.

## Validated Foundation

- Native cross-platform Rust runtime with fail-closed containment, typed policy seams, append-only journals, and provider-neutral execution.
- Attested multi-target release pipeline consumed by Wayland Desktop.
- `just gate-all`, generated-artifact drift checks, `wayland-nano verify`, red-green receipts, gated climb, and sealed Gate Card dogfood were completed in archived milestone v1.0.
- The old seven phase directories and their evidence are archived under `docs/planning-archive/v1.0-phases/`; they are outside Ferrox's active `.planning/` discovery tree and cannot contaminate v1.1 progress.

## Current Milestone: v1.1 Persistent Agent Program

**Goal:** A named Wayland Nano agent can be configured and safely activated repeatedly with memory-primary continuity, project- and agent-scoped accumulation, bounded proactive routines/escalations, and hardened browser and desktop execution backends. Every promoted phase has executable acceptance evidence and seven-leg CI green.

**Target features:**

- Safe memory/profile/module composition substrate
- Named agents with never-recycled identity and receipt-bound modules
- One shared memory-primary runtime bootstrap across interactive hosts
- Bounded routines, escalation, retry, and pause controls
- Hardened browser and desktop execution behind one backend seam
- Mediated compaction and procedure extraction
- Measured code structure, blast-radius suggestions, and gated KG retrieval
- Explicit host-authorized cross-project reads built last

## Authority

This file is a Ferrox navigation projection, not a replacement for the governing sources. Precedence for v1.1 is:

1. `.planning/sources/MEMORY-CONTRACT.md` v1.2 for memory, identity, persona, activation, and security semantics
2. `.planning/sources/NANO-PROGRAM-PLAN.md` for owner decisions, package scope, evidence, dependencies, and tripwires
3. `.planning/sources/PROFILES-CONTRACT.md` and `.planning/sources/NANO-MODULE-CONTRACT.md`, narrowed by the higher authorities
4. The active `.planning/` navigation artifacts

MEMORY-CONTRACT v1.2 is owner-signed as of 2026-08-25. Any conflict stops execution and corrects the projection.

## Current Foundation Gate

- WP-0 mechanical CODEOWNERS protection and P-MEM-1 are implemented on open PR #8 (`feat/p-mem-1-core`). Reported evidence is recall@10 1.000, zero partition leakage, kill recovery, mediated writes, and seven green CI legs.
- PR #8 is not merged. Phase 1 awaits only human review, merge, and fresh-checkout evidence.

## Locked Decisions

| Decision | Locked outcome |
|---|---|
| Contract precedence | MEMORY-CONTRACT v1.2 governs memory, persona, identity, and activation over lower sources. |
| Continuity | All interactive and routine activations are memory-primary: fresh context plus scoped recall; journals are audit/fallback only. |
| Agent identity | IDs are never recycled; retired IDs remain tombstoned and reuse is a typed error. |
| Build versus activation | Packages are built thin and measured; weak evidence may keep a capability dark but never erases the package. |
| Execution | Browser and desktop containers ship behind one `host | browser-container | desktop-container` seam. |
| Procedure learning | Learn-from-doing is mediated procedure extraction, never automatic execution. |
| Blast radius | Suggested heuristic labeled with measured confidence, never asserted fact. |
| Cross-project reads | Explicit host-authorized per-query opt-in, promoted last; no sticky Global or cross-agent widening. |

## Dependency DAG and Promotion Policy

Hard dependencies determine technical eligibility. The numbered roadmap is the owner/user's one-active-goal promotion policy and does not invent extra dependencies.

- P-MEM-SEC and P-PROF require P-MEM-1; P-MOD-GAP requires WP-0.
- P-BOT-5a requires P-PROF and P-MOD-GAP; P-BOT-5b requires P-BOT-5a and P-MEM-1; P-BOT-5c requires P-BOT-5b.
- P-EXE-1 is eligible after P-BOT-5a; P-EXE-2 requires P-EXE-1.
- P-CONS and P-PROC are eligible after P-MEM-1.
- P-GRAPH-1 is eligible after WP-0; P-GRAPH-2 requires P-GRAPH-1; P-MEM-KG requires P-MEM-1, the extended fixture, and graph-lane ordering after P-GRAPH-2.
- P-XPROJ requires P-MEM-1 and P-MEM-SEC and is promoted last.

## Execution Discipline

- One active phase goal, isolated worktree/branch, and promotion PR at a time; internal parallel waves only where the roadmap explicitly permits them.
- Carry forward no-side-quests, three-strikes, tripwire, secrets, human-review, and handoff-on-stop discipline.
- Never self-merge or push tags. `gates/**` and `agents/**` changes require human review.
- Touched-crate fmt, Clippy, and tests pass locally; promoted work requires all seven CI legs including Windows ARM64.
- Never read or print `.secrets`; Flux tests use only `FLUX_API_KEY_FILE` and self-skip when absent.

## Product Boundary and Out of Scope

Nano remains the existing Rust engine driven through ACP/TUI/Desktop/config. Desktop owns roster UI, group chats, creation flows, and marketplace UX. Excluded: replacement architecture, hosted memory/embeddings, runtime plugin ABI, hidden installers, registry-governance design, webhook platform, teach-by-demonstration, procedure auto-execution, remote/cloud backends, new policy language, dynamic-language blast radius, LSP/SCIP, community detection, LLM entity resolution, cross-agent reads, sticky Global scope, unrelated cleanup, and dependency upgrades.

---
*Milestone v1.1 installed 2026-08-27; v1.0 history retained in the milestone archive.*
