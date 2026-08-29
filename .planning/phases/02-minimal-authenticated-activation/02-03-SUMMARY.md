---
phase: 02-minimal-authenticated-activation
plan: 03
subsystem: authentication
tags: [authority, sqlite, journal, ed25519, acl, tty]
requires:
  - phase: 02-minimal-authenticated-activation
    provides: frozen signed admin contract and exact Nano implementation worktree from Plans 02-01 and 02-11
provides:
  - immutable issuer, subject, principal, project, key, epoch, revocation and retirement authority state
  - fsync-before-projection closed authority journal with deterministic SQLite rebuild
  - signed local-admin lifecycle with distinct recovery root and durable nonce transactions
  - owner-only key-reference loading with Unix and Windows path, owner, ACL and alias defenses
affects: [02-04, 02-05, 02-09, 02-15]
tech-stack:
  added: [rusqlite 0.37, nano-session journal lock reuse]
  patterns: [journal-is-truth, disposable-projection, immutable-authority-reducer, role-separated-key-references]
key-files:
  created:
    - crates/nano-activation/src/admin.rs
    - crates/nano-activation/src/authority.rs
    - crates/nano-activation/src/journal.rs
    - crates/nano-activation/src/key_provider.rs
    - crates/nano-activation/src/store.rs
    - crates/nano-activation/tests/admin_crash_rebuild.rs
    - crates/nano-activation/tests/admin_lifecycle.rs
  modified:
    - crates/nano-activation/Cargo.toml
    - crates/nano-activation/src/lib.rs
    - Cargo.lock
key-decisions:
  - "Admin command and nonce tombstone are one closed journal transaction so a crash cannot authorize an operation without consuming its replay nonce."
  - "Root recovery verifies against a separately provisioned recovery public key; the current or lost admin root cannot self-recover."
  - "Windows key references permit writable ACEs only for current SID, SYSTEM and Builtin Administrators and reject mapped drives, UNC, reparse points and writable unrelated principals."
patterns-established:
  - "Every authority mutation is reduced against a clone, flushed to the closed journal under one writer lock, then projected to SQLite."
  - "Unknown journal records remain observable in state but never enter authorization decisions."
requirements-completed: [REQ-POL-01]
coverage:
  - id: D1
    description: Immutable authority records, revocation, retirement, durable nonces, writer exclusion and DB-loss rebuild fail closed.
    requirement: REQ-POL-01
    verification:
      - kind: integration
        ref: "cargo test -p nano-activation --test admin_crash_rebuild -- --test-threads=1"
        status: pass
    human_judgment: false
  - id: D2
    description: TTY-gated bootstrap, signed digest/epoch/nonce lifecycle, distinct recovery root and owner-only key references are enforced.
    requirement: REQ-POL-01
    verification:
      - kind: integration
        ref: "cargo test -p nano-activation --test admin_lifecycle -- --test-threads=1"
        status: pass
      - kind: other
        ref: "cargo clippy -p nano-activation --all-targets -- -D warnings; cargo fmt --all -- --check"
        status: pass
    human_judgment: false
duration: 47min
completed: 2026-08-30
status: complete
---

# Phase 2 Plan 03: Local Authority Lifecycle Summary

**A locked journal-first authority reducer now independently governs immutable identity/project grants, revocation, recovery and replay-safe local administration.**

## Performance

- **Duration:** 47 min
- **Started:** 2026-08-29T23:39:00+07:00
- **Completed:** 2026-08-30T00:26:17+07:00
- **Tasks:** 2
- **Files modified:** 10

## Accomplishments

- Added a closed authority vocabulary and deterministic reducer covering issuer enrollment, immutable subject/principal bindings, project grants, key rotation/revocation, issuer revocation, identifier retirement, admin epochs, recovery and nonce tombstones.
- Added a single-writer fsync-first authority journal whose SQLite projection is disposable and query-equivalent after projection loss or an injected journal-to-DB crash.
- Added attached-TTY and explicit-confirmation bootstrap, signed admin envelope validation, before/after state digests, distinct recovery-key verification and atomic command-plus-nonce journal transactions.
- Added role-bound opaque key-reference loading that never returns private material and enforces no-follow, stable file identity, secure parents, Unix effective-owner/0600 and Windows current-owner/restricted-DACL/local-drive rules.
- Added real child-process alias rejection plus focused lifecycle, crash, rebuild, contention, remap, retirement and revocation coverage.

## Task Commits

1. **TDD RED: define authority lifecycle security gates** - `19074ac`
2. **Task 1 GREEN: add journal-first authority store** - `d31cf7e`
3. **Task 2 GREEN: enforce authenticated authority lifecycle** - `3a46a10`
4. **Security hardening: remove unchecked bootstrap surface** - `6a42b8f`
5. **Custody hardening: require role-bound bootstrap references** - `d785d9e`

Planning metadata is left for the parent orchestrator because the summary lives in the separate planning worktree.

## Files Created/Modified

- `crates/nano-activation/src/authority.rs` - Closed commands, snapshots, immutable reducer, epochs and nonce tombstones.
- `crates/nano-activation/src/journal.rs` - Sequence-checked JCS JSONL records and atomic admin transaction replay.
- `crates/nano-activation/src/store.rs` - Lifetime writer lock, fsync-first commits, authorization query and disposable SQLite projection.
- `crates/nano-activation/src/admin.rs` - TTY bootstrap, signed admin validation, recovery-key selection and lifecycle application.
- `crates/nano-activation/src/key_provider.rs` - Opaque role-bound reference loading and cross-platform custody/path checks.
- `crates/nano-activation/tests/admin_crash_rebuild.rs` - Crash boundary, DB loss, contention, immutability, revocation and nonce proof.
- `crates/nano-activation/tests/admin_lifecycle.rs` - Signed lifecycle, recovery, TTY, ACL, role and real child-process alias negatives.
- `crates/nano-activation/src/lib.rs`, `Cargo.toml`, `Cargo.lock` - Public authority modules and exact existing-stack dependencies.

## Decisions Made

- The authority journal uses its own closed vocabulary rather than reusing session operations; unknown future records are retained as non-authorizing evidence.
- Exact duplicate operation IDs are idempotent only when their immutable command digest matches; changed content is a typed conflict.
- Recovery has separate public-key custody from the active admin root, preventing a missing/compromised root from authorizing its own recovery.
- Local Windows extended drive paths are accepted, while UNC and mapped remote drives are rejected before open.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Made admin command and nonce one durable transaction**
- **Found during:** Task 2 completion audit
- **Issue:** Two independent journal appends could crash after authority mutation but before replay nonce consumption.
- **Fix:** Added one closed `transaction` record reduced atomically on live commit and rebuild.
- **Files modified:** `authority.rs`, `journal.rs`, `store.rs`, `admin.rs`
- **Verification:** Full focused crate suite and crash/rebuild tests pass.
- **Committed in:** `3a46a10`

**2. [Rule 2 - Missing Critical] Separated recovery from active admin-root verification**
- **Found during:** Task 2 lifecycle audit
- **Issue:** A lost root could not recover if recovery required its own signature.
- **Fix:** Bootstrap provisions a distinct recovery public key and only `recover_root` selects it.
- **Files modified:** `authority.rs`, `admin.rs`, `admin_lifecycle.rs`
- **Verification:** Recovery succeeds under the recovery signer; the old admin signer fails immediately after epoch/root replacement.
- **Committed in:** `3a46a10`

**3. [Rule 1 - Bug] Distinguished local extended paths from network paths**
- **Found during:** Windows owner-only key-reference test
- **Issue:** Canonical `\\?\F:\...` local paths were initially grouped with UNC paths, and mapped drive detection was missing.
- **Fix:** Allowed only extended drive syntax, rejected extended UNC/ordinary UNC, and checked `GetDriveTypeW` for remote mappings.
- **Files modified:** `key_provider.rs`, `admin_lifecycle.rs`
- **Verification:** Owner-only and real child-process alias tests pass on Windows.
- **Committed in:** `3a46a10`

**4. [Rule 2 - Missing Critical] Removed the public unchecked bootstrap seam**
- **Found during:** Final API-surface audit
- **Issue:** An integration-test convenience constructor could initialize authority without the production TTY/owner/confirmation ceremony.
- **Fix:** Restricted initialization and journal mutation to the crate; integration tests now write a closed bootstrap-record fixture and enter only through normal `AuthorityStore::open`.
- **Files modified:** `admin.rs`, `journal.rs`, `store.rs`, `admin_crash_rebuild.rs`, `admin_lifecycle.rs`
- **Verification:** Full crate suite, clippy and formatting pass after the public surface was removed.
- **Committed in:** `6a42b8f`

**5. [Rule 2 - Missing Critical] Required approved role-bound custody at bootstrap**
- **Found during:** Final contract-trace audit
- **Issue:** The public ceremony proved TTY/owner/confirmation but accepted public keys without first opening the four approved owner-only role references.
- **Fix:** The sole public bootstrap now requires and validates distinct admin-root, recovery-root, receipt-signer and local-CLI key references, while the journal retains only public verification keys.
- **Files modified:** `admin.rs`, `authority.rs`, `admin_lifecycle.rs`
- **Verification:** Full crate suite, clippy and formatting pass; confirmation/TTY fail before any missing reference can be trusted.
- **Committed in:** `d785d9e`

**Total deviations:** 5 auto-fixed (4 missing critical security semantics, 1 path-classification bug)
**Impact on plan:** All fixes close explicit D2-08/D2-15 lifecycle requirements; no admission, ACP, runtime, Desktop or Phase 3 surface was added.

## Issues Encountered

- Windows default temp-file ACLs include writable Authenticated Users. The test proves rejection, then provisions an owner-only test fixture; production validation still rejects the broad inherited ACE.
- Shared Cargo compilation briefly waited on other Phase 2 executors. No test semantics or dependency versions changed.

## User Setup Required

None.

## Known Stubs

None.

## Threat Flags

| Flag | File | Description |
|---|---|---|
| threat_flag: auth-path | `crates/nano-activation/src/admin.rs` | New local administrator trust transition validates root/recovery signatures, epochs, time, nonce and state digests. |
| threat_flag: file-access | `crates/nano-activation/src/key_provider.rs` | New key-reference custody path opens only stable owner-controlled local files and returns opaque references. |

## Next Phase Readiness

- Admission/replay plans can query one immutable Nano-local authority source and cannot treat Desktop signatures as authorization by themselves.
- Receipt/admin tooling can consume public authority snapshots and typed failures without accessing private key material.
- No ACP integration, admission runtime, persistence quarantine, Desktop source, Phase 3 memory, push, merge or tag occurred.

## Self-Check: PASSED

- All seven planned source/test files and dependency updates exist.
- TDD and hardening commits `19074ac`, `d31cf7e`, `3a46a10`, `6a42b8f` and `d785d9e` exist on `feat/p2-minimal-authenticated-activation`.
- `cargo test -p nano-activation -- --test-threads=1` passes all 15 unit/integration/contract tests.
- `cargo clippy -p nano-activation --all-targets -- -D warnings` and `cargo fmt --all -- --check` pass.
- Nano implementation worktree is clean at `d785d9e0ec0df1bb4e4f0ad54d8643e9c18750dc`.

---
*Phase: 02-minimal-authenticated-activation*
*Completed: 2026-08-30*
