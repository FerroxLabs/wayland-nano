---
phase: 03-runtime-integrated-scoped-continuity
plan: 04
base_sha: 628901ab28409499ce0ac15e0264178e08c18af1
subsystem: memory-migration
tags: [memory, migration, journal, durability, quarantine]
status: complete
requires:
  - 03-01 mem-sec gate-card pack
  - 03-03 dedicated memory-journal topology and runtime seam
provides:
  - explicit legacy Markdown migration command
  - journal-first ModelInference ingestion with per-entry receipts
  - live-DB versus journal-rebuild equivalence including agent_id
  - post-migration legacy authority closure
affects:
  - 03-06 phase closure evidence
tech-stack:
  added: []
  patterns:
    - resolver-backed shadow evaluation before authoritative journal append
    - stable content-addressed migration identities
    - strict typed migration receipts and failures
key-files:
  created:
    - crates/nano-cli/src/memory_migrate.rs
    - crates/nano-cli/tests/memory_migration.rs
  modified:
    - crates/nano-cli/src/main.rs
    - crates/nano-cli/src/memory_seam.rs
    - crates/nano-cli/tests/activation_quarantine.rs
    - crates/nano-memory/src/lib.rs
    - crates/nano-memory/src/store.rs
    - crates/nano-memory/tests/corrective_regressions.rs
    - .planning/phases/03-runtime-integrated-scoped-continuity/03-04-PLAN.md
    - .planning/phases/03-runtime-integrated-scoped-continuity/03-VALIDATION.md
key-decisions:
  - Migrate the quarantined Markdown store through an explicit operator command.
  - Treat every legacy entry as ModelInference because its original source is ambiguous.
  - Preserve the dedicated memory journal as the only mutation authority and refuse migration after its completion receipt.
metrics:
  tasks: 3
  files: 11
  completed: 2026-09-05
---

# Phase 3 Plan 04: Legacy Memory Migration Summary

Explicit legacy Markdown migration now derives resolver outcomes through the existing mediation boundary, appends attributed operations and receipts to the dedicated memory journal, and rebuilds SQLite only from that journal.

## Decision and Evidence

The selected disposition is **migrate**. The legacy directory can contain useful operator history, and its pinned filename supplies deterministic validity time, so abandoning it was not justified. Migration is available only through:

`wayland-nano memory migrate --project <project> --agent-id <id> --session-id <id>`

No session-open path invokes migration. Each plain UTF-8 Markdown entry receives a stable identity from its filename and bytes, a SHA-256 receipt, the supplied project and configured agent, and `ModelInference` trust. Invalid timestamps, unreadable content, non-plain files, secret-screening failures, and unconfigured agents fail closed. A completion receipt permanently refuses reruns, leaving later filesystem edits invisible to runtime memory.

The migration computes the deterministic contradiction outcome in an isolated store rebuilt from the current dedicated journal. It then writes the resulting `MemoryWriteFact` with the explicit migration session id and its receipt to the authoritative journal before rebuilding `memory.db`. This preserves the exact tier-aware resolver rule without granting the shadow database authority.

## Task Commits

| Task | Commit | Result |
|---|---|---|
| Task 1 RED | `6352dde` | Specified explicit migration, attribution, receipt, malformed metadata, crash recovery, and closure behavior. |
| Task 1 GREEN | `e9fd8b2` | Added the CLI command and initial journal-first migration. |
| Task 2 | `bc11d3b` | Proved sealed-corpus live/rebuild equivalence for facts, ordered queries, attribution, and receipts. |
| Task 3 RED | `46e0d5d` | Exposed lower-tier authority regain and added legacy/unconfigured/explicit-invocation negatives. |
| Task 3 GREEN | `85a4a95` | Routed resolver decisions through mediation and retained explicit migration-session attribution. |
| Crash-pair RED | `68889d9` | Reproduced interruption between an authoritative write and its receipt. |
| Crash-pair GREEN | `f22eec6` | Made retry repair the missing receipt before rebuild. |
| Collision RED | `459b3d1` | Proved a reused fact id cannot attach a migration receipt to substituted journal payload. |
| Collision GREEN | `c72b6ef` | Bound interrupted-write recovery to an exact operation and receipt match. |
| Corrective RED | `6b482b8` | Added resolved-policy, session, completion, strict-wire, real migration/rebuild, valid legacy-op, filename, and partial-refusal rows. |
| Corrective GREEN | `9a5e149` | Closed all eight authority and evidence gaps. |
| Recovery RED | `9641225` | Isolated forged resolver outcome, reserved op-id collision, mixed partial retry, and real child-process kill behavior. |
| Recovery GREEN | `c3cb4c6` | Recomputed recovery at the journal position, strictly scanned legacy entries, preflighted op ids, and added the SIGKILL fault point. |
| Transaction RED | `6305b45` | Exposed fully receipted outcome forgery, foreign/reserved receipt identities, and live-writer journal mutation. |
| Transaction GREEN | `fe3dad6` | Moved migration snapshot, resolver, append, rebuild, and completion under canonical nano-memory writer ownership. |
| Governance | `86aa734` | Recorded the approved store/lib transaction ownership and wave serialization. |
| Causality RED | `224b224` | Exposed foreign candidate authority, noncausal receipt ordering, and unstable per-entry result mapping. |
| Causality GREEN | `535e122` | Required canonical candidate envelopes and causal receipts, bound results to source entries, and proved migrated data through the real scoped runtime seam. |
| Torn-tail RED | `b935800` | Exposed recovery across an intervening row and sorted-candidate append before torn-write repair. |
| Torn-tail GREEN | `ba7a03a` | Restricted repairable unreceipted authority to the journal tail and repaired it before every new candidate. |
| Test-only ownership | `4e3b4f8` | Assigned the cfg(test) runtime-seam proof to 03-04 while retaining all production seam ownership in 03-03. |
| Reserved-ID RED | `1aa287a` | Exposed completion-receipt authorization of colliding Fact, Decision, Episode, and Procedure IDs before and after migration completion. |
| Reserved-ID GREEN | `cdfcdfb` | Reserved the completion ID across live writes, migration preflight, and replay while preserving the normal completion receipt. |

## Verification

- `cargo test -p nano-cli --test memory_migration --test activation_quarantine -- --test-threads=1`: 36 passed.
- `cargo test -p nano-cli --bin wayland-nano memory_migrate::tests -- --test-threads=1`: strict receipt/failure round-trip, unknown-field rejection, and canonical legacy filename grammar passed.
- `cargo test -p nano-cli --lib memory_seam::tests::migrated_fact_is_visible_through_the_real_scoped_runtime_seam -- --exact`: 1 passed; the migrated row was present only for its owning project/agent and absent for a foreign project, foreign agent, and a higher minimum tier.
- `cargo test -p nano-memory --test corrective_regressions --test durability --test mem_sec_cards -- --test-threads=1`: 23 passed, including four-family reserved-ID live/replay rejection, the child-process kill-mid-write test, and all six mem-sec cards plus summary.
- `cargo test -p nano-session --test activation_legacy_replay -- --test-threads=1`: 1 passed; the legacy replay target was unchanged.
- The sealed recall fixture proof compares all 20 ordered query result lists and all currently-valid facts after rebuilding from the dedicated journal, explicitly including validity, trust tier, project, and `agent_id`.
- The CLI-level equivalence proof seeds the sealed fixture, invokes the real migration command, captures the resulting live database and migration receipts, deletes `memory.db`, rebuilds from `memory.jsonl`, and repeats every comparison including the migrated ModelInference row.
- Mediation receipts are present before rebuild and byte-for-field identical afterward because rebuild never rewrites the authoritative journal.
- A migration-specific child process is killed after the authoritative write and receipt are synced but before `memory.db` exists; rebuilding its journal produces the same facts and per-entry receipts as a completed control migration.
- `just gate-all` passed at branch head `cdfcdfb` in the fresh isolated target `F:/CargoTarget/wayland-nano-p3-migration-final-reserved`, including workspace format, clippy, tests, error-table generation checks, and contract generation checks.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Prevented lower-tier migration rows from remaining current beside User truth**

- **Found during:** Task 3 failing-first authority test
- **Issue:** A fixed `coexist` outcome bypassed the tier-aware resolver during rebuild.
- **Fix:** Use an isolated resolver-backed store to derive the authoritative operation before appending it to the dedicated journal.
- **Files modified:** `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `46e0d5d`, `85a4a95`

**2. [Rule 1 - Bug] Repaired a write interrupted before its receipt**

- **Found during:** Final crash-window review
- **Issue:** Retry recognized the existing write but did not add its missing receipt, so ModelInference recovery would ignore it.
- **Fix:** Retry idempotently appends the stable per-entry receipt before rebuilding.
- **Files modified:** `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `68889d9`, `f22eec6`

**3. [Rule 2 - Missing critical validation] Bound recovery to exact source bytes**

- **Found during:** Final trust-boundary review
- **Issue:** An existing journal row could reuse the deterministic fact id with different content and receive the migration receipt.
- **Fix:** Require exactly one stable write envelope whose complete payload, partition, tier, validity, session id, and resolver outcome match; also reject receipt-id collisions with different fields.
- **Files modified:** `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `459b3d1`, `c72b6ef`

**4. [Rule 1/2 - Corrective audit] Closed policy, identity, completion, evidence, and receipt gaps**

- **Found during:** Independent corrective audit after the first candidate
- **Issue:** Migration used default policy authority, accepted invalid session ids, did not verify a completion-id collision, completed despite retryable refused entries, enumerated names outside the legacy grammar, emitted loose/free-text receipts, and lacked a real command-to-rebuild sealed-fixture proof. The legacy session-op negative also relied on invalid tier spelling.
- **Fix:** Consume the resolved policy and configured agents; refuse disabled/write-Off before append; enforce the canonical session grammar; require a newly appended or byte-identical completion record; leave partial refusal incomplete and retryable; enumerate via the legacy store's validator; serialize strict typed global/entry/failure receipts; execute actual migration before sealed-fixture DB-drop equivalence; and prove a valid session-journal memory op stays outside dedicated-journal authority.
- **Files modified:** `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `6b482b8`, `9a5e149`

**5. [Rule 1/2 - Corrective audit] Closed recovery-prefix and enumeration gaps**

- **Found during:** Second independent corrective audit
- **Issue:** An unreceipted write could carry a forged resolver outcome; an unrelated op could occupy the stable authoritative envelope id; filename enumeration delegated to a fail-open listing helper; mixed retry accounting and a physical kill were not directly proven.
- **Fix:** Recompute the expected operation against the exact authoritative journal prefix and compare the operation byte-for-field; preflight every stable write id and require fresh append success; perform one strict `read_dir` pass using the canonical legacy grammar while propagating iterator/name errors; prove mixed retry has exact counts and no duplicate writes/receipts; and kill a child at the post-sync/pre-DB fault point before independent rebuild comparison.
- **Files modified:** `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `9641225`, `c3cb4c6`

**6. [Rule 2 - Missing critical transaction boundary] Serialized migration under canonical store ownership**

- **Found during:** Final independent transaction audit
- **Issue:** Fully receipted existing migration writes were trusted without resolver recomputation, matching receipts could use foreign envelope ids, and the CLI appended journal rows before discovering a live canonical writer lock.
- **Fix:** Add one nano-memory transaction API that acquires `memory.memory.lock` before journal inspection and retains it through exact-prefix recomputation of every existing migration write, exact receipt/id validation, journal-first appends, projection replacement, and completion. The CLI prepares candidates and renders receipts but never opens a bare journal writer.
- **Files modified:** `crates/nano-memory/src/store.rs`, `crates/nano-memory/src/lib.rs`, `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `6305b45`, `fe3dad6`

**7. [Rule 1/2 - Corrective audit] Closed candidate causality and runtime-consumption gaps**

- **Found during:** Final independent causality audit
- **Issue:** A foreign envelope could reuse a migration candidate fact id, receipts could retroactively authorize reordered writes, positional result mapping could relabel a refused entry, and migrated data had not been exercised through the actual runtime executor.
- **Fix:** Reject every candidate fact id unless its sole existing write uses the canonical migration envelope; require an exact receipt immediately after its authoritative write; retain the exact receipt index for each prepared source entry; and retrieve a migrated ModelInference fact through `MemorySeamExecutor`, proving own-scope visibility plus foreign-project, foreign-agent, and minimum-tier exclusion.
- **Files modified:** `crates/nano-memory/src/store.rs`, `crates/nano-cli/src/memory_migrate.rs`, `crates/nano-cli/src/memory_seam.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `224b224`, `535e122`

**8. [Rule 1/2 - Corrective audit] Restricted torn-write repair to a causal journal tail**

- **Found during:** Final causal recovery audit
- **Issue:** An unreceipted canonical migration write could be repaired after an unrelated intervening row, and a newly discovered earlier-sorted candidate could be appended before the torn candidate's receipt.
- **Fix:** Preflight every candidate before mutation; permit exactly one unreceipted existing migration write only when it is the current journal tail; append its exact receipt immediately; then process new candidates. Intervening rows now return typed `JournalInvalid` with byte-identical journal and absent projection.
- **Files modified:** `crates/nano-memory/src/store.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `b935800`, `ba7a03a`

**9. [Rule 1/2 - Corrective audit] Reserved the migration completion ID from memory data authority**

- **Found during:** Final reserved-identifier authority audit
- **Issue:** The normal completion receipt targets `legacy-migration-complete`; a ModelInference Fact, Decision, Episode, or Procedure with the same record ID could therefore become replay-authorized, including when appended after a valid completion.
- **Fix:** Validate the reserved ID before every live store write, before migration completion inspection, and before replay builds its receipt authority set. Four-family live/replay matrices and pre-/post-completion migration rows now fail closed with typed invalid-ID/journal errors and no journal mutation; the normal completion and idempotent rerun rows remain green.
- **Files modified:** `crates/nano-memory/src/store.rs`, `crates/nano-memory/tests/corrective_regressions.rs`, `crates/nano-cli/tests/memory_migration.rs`
- **Commits:** `1aa287a`, `cdfcdfb`

## Completion Receipt

- Base: `628901ab28409499ce0ac15e0264178e08c18af1`
- Implementation head: `cdfcdfb`
- Governance head: `4e3b4f8`
- Final gate head: `cdfcdfb`
- Changed files from base: 11
- Final gate: GREEN in `F:/CargoTarget/wayland-nano-p3-migration-final-reserved`

## Desktop Generator Isolation

The final local gate command is `CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano-p3-migration-final-reserved NANO_ERROR_TABLE_DESKTOP_DIR=<worktree>/.tmp-desktop-absent just gate-all`. The lane-specific F-drive target prevents another worktree from replacing the test executable. The Desktop override omits only optional sibling-repository mirrors while continuing to require and compare the Nano and shared canonical error tables.

## Known Stubs

None.

## Self-Check: PASSED

All 11 changed files and twenty-one implementation/test commits exist. The implementation and approved ownership amendments remain within 03-04 scope, and no tracked file was deleted.
