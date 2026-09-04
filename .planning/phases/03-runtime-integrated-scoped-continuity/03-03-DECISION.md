# Phase 3 Plan 03: Memory journal topology

## Decision status

**DECISION: use the dedicated memory journal for memory mutations.** The existing
`MemoryStore` remains the sole writer of `MemoryWrite*` and
`MemoryWriteReceipt` records. Each persistent runtime session separately appends
exactly one attributed `MemoryPolicyResolved` audit record through its already-owned
session `JournalCoordinator`, after `SessionBegin` and before memory effects. Store
open must therefore stop appending `MemoryPolicyResolved` itself.

This decision does not rename the schema or journal vocabulary. The legacy
`MemoryWrite*` and `MemoryPolicyResolved` variants remain replay-neutral in session
folding (`crates/nano-session/src/replay.rs:639-644`).

## Current invariants

- `MemoryStore::open*` owns a `JournalWriter`, derives the next `memory-N` id from
  the selected journal, and holds the database writer `FileLock`
  (`crates/nano-memory/src/store.rs:10-17`, `:52-66`, `:100-114`, `:862-868`).
- A memory mutation constructs its full-content operation, synchronously appends it,
  and only then applies the SQLite transaction; the fault hook is deliberately
  between those two actions (`crates/nano-memory/src/store.rs:125-181`).
- `rebuild_from_journals` already accepts a journal-path slice, acquires the target
  database lock, and replays every supplied journal into the replacement database
  (`crates/nano-memory/src/store.rs:871-943`).
- The store currently appends `MemoryPolicyResolved` during both `open` paths
  (`crates/nano-memory/src/store.rs:66`, `:114`, `:400-412`). That duplicate
  authority is removed by Task 2; admission never appends a journal record.
- The session `JournalCoordinator` serializes every append through its owned writer
  (`crates/nano-session/src/coordinator.rs:32-67`).

## Consequence matrix

| Consequence | Option A: per-session `JournalCoordinator` for memory mutations | Option B: dedicated memory journal (chosen) |
|---|---|---|
| Journal-first / kill-mid-write | Would require `MemoryStore` to surrender or abstract its writer so the coordinator append completes before each DB commit. Unique `memory-N` allocation would have to coexist with session operation ids under the coordinator lock. | Preserves the proven ordering and fault seam: a full `MemoryWrite*` record is durable before SQLite mutation (`store.rs:125-181`). Unique `memory-N` ids remain derived from the one memory journal (`store.rs:371-383`, `:862-868`). |
| Writer locking and contention | The session coordinator's writer lock serializes one session journal, but the database `FileLock` must still be held by the store. Multiple session journals could be writable while contending for the same DB unless activation ownership supplies the outer single-live-agent invariant. | The store holds the DB `FileLock` and its dedicated writer for the activation lifetime (`store.rs:52-64`, `:100-112`). Under one live activation per `agent_id`, all memory writes serialize through that store; another store opener fails closed on contention. |
| Rebuild scan | `rebuild_from_journals` must discover and scan every session journal that may contain memory records, in deterministic order, including retired sessions. Missing one journal loses authoritative writes. | `rebuild_from_journals` scans the single dedicated memory journal supplied in its existing slice API. Session journals remain audit sources, not mutation authority. |
| 03-04 query-equivalence proof | Must prove deterministic merge ordering across all session journals, de-duplication of colliding operation ids, and equivalence when session journals are missing or reordered. | Must delete the DB, replay the dedicated memory journal, and prove identical ordered hit ids and currently-valid rows including project, agent_id, tier and validity. It must separately prove that session audit records grant no memory authority. |
| Policy audit | One attributed record still belongs in each already-owned session coordinator. Store-open emission would create a second record and is forbidden. | Same. The dedicated mutation journal contains no authoritative policy audit; every persistent session coordinator appends exactly one attributed record after `SessionBegin`. |

## Rejected option

Option A is **rejected** for Phase 3. It broadens the runtime and rebuild topology:
the memory store would need an injected journal-writer abstraction, global discovery
and ordering of all session journals, and collision handling across independently
numbered streams. None is needed to satisfy scoped continuity, and each creates a new
03-04 equivalence obligation. Option B already satisfies journal-first durability and
the single-writer constraint with the shipped store implementation.

## Binding implementation and test obligations

1. Remove the store's `append_resolved_policy` call and helper. Store open validates
   policy and configured-agent membership but emits no policy audit record.
2. Extend `MemoryPolicyResolved` additively with optional project and agent
   attribution while retaining legacy deserialization. Every new runtime record must
   contain project, agent, and the actual runtime session id.
3. ACP new/load, protocol-host, and exec fresh/resume append exactly one policy audit
   through their already-owned coordinator after `SessionBegin` and before effects.
4. Dedicated-journal kill-mid-write and `rebuild_from_journals` tests remain green.
   03-04 must prove dedicated-journal DB-drop/query equivalence and prove session
   audit records are replay-neutral.
5. Legacy `MemoryWrite*`, `Cron*`, and unattributed policy records remain readable
   without granting session-fold or retrieval authority (`replay.rs:639-644`).
