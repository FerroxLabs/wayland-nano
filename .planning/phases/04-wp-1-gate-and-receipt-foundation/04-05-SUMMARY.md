---
phase: 04-wp-1-gate-and-receipt-foundation
plan: 05
subsystem: verification
tags: [rust, canonical-json, atomic-replace, receipt, fail-closed]
requires:
  - phase: 04-wp-1-gate-and-receipt-foundation
    provides: nano-verify crate seam and pinned windows-sys 0.52 storage API
provides:
  - Standalone schema-1 canonical red-green receipts
  - Bounded exclusive receipt writer lock
  - Same-directory fsynced atomic replacement and corruption-intolerant reader
affects: [wp-1-receipt-preflight, wp-3-final-verdict]
tech-stack:
  added: []
  patterns: [RAII lockfile release, platform-authoritative atomic replacement, one-retry fail-closed reader]
key-files:
  created: []
  modified: [crates/nano-verify/src/receipt.rs]
key-decisions:
  - "WP-1 receipt APIs expose Ready but cannot produce final VerifyVerdict::Valid."
  - "Production storage uses exact 50 ms retry, 10 s deadline, and greater-than-60 s stale-lock policy."
patterns-established:
  - "Persist canonical bytes through a synced same-directory tempfile under create_new lock ownership."
requirements-completed: [RCPT-01, RCPT-02, RCPT-04]
coverage:
  - id: D1
    description: "Standalone canonical schema-1 receipts reject malformed or green-only red evidence"
    requirement: RCPT-01
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/receipt.rs#canonical_schema_and_mint_validation"
        status: pass
    human_judgment: false
  - id: D2
    description: "Receipt writes are bounded, locked, synced, and atomically replace existing documents"
    requirement: RCPT-02
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/receipt.rs#store_lock_contention_is_bounded; store_replace_overwrites_existing_atomically"
        status: pass
    human_judgment: false
  - id: D3
    description: "Corrupt receipt reads retry once after 100 ms and then fail closed"
    requirement: RCPT-04
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/receipt.rs#store_reader_retry_then_corruption"
        status: pass
    human_judgment: false
duration: 18min
completed: 2026-08-17
status: complete
---

# Phase 4 Plan 05: Receipt Foundation Summary

**Canonical standalone red-green receipts with bounded writer exclusion and platform-authoritative atomic replacement**

## Performance

- **Duration:** 18 min
- **Completed:** 2026-08-17
- **Tasks:** 2
- **Files modified:** 1 code file plus this summary

## Accomplishments

- Added the IFACE receipt, failing-run, verdict, and preflight types with strict decoding and no WP-1 path to final `Valid`.
- Added canonical NFC JSON serialization and mint validation for genuine red evidence, SHA shapes, required identities, and RFC-3339 UTC timestamps.
- Added bounded `create_new` locking, same-directory tempfile fsync, Windows `MoveFileExW(REPLACE_EXISTING)`, Unix atomic rename plus directory fsync, and exactly one corrupt-read retry.

## Behavioral RED Evidence

### Task 1 — reader retry and corruption

- **Command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify receipt::tests::store_reader_retry_then_corruption`
- **Exit code:** `101`
- **Failing test:** `receipt::tests::store_reader_retry_then_corruption`
- **Assertion excerpt:** `reader returned before the required retry delay`
- **RED quality:** The crate and test compiled; failure came from the behavioral assertion for the absent 100 ms retry, not setup or fixture creation.

### Task 2 — bounded lock and atomic replacement

- **Command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify receipt::tests::store_`
- **Exit code:** `101`
- **Failing tests:** `receipt::tests::store_lock_contention_is_bounded`, `receipt::tests::store_replace_overwrites_existing_atomically`
- **Assertion excerpts:** `assertion failed: matches!(result, Err(VerifyError::LockHeld(_)))`; `called Result::unwrap() on an Err value: Registry("receipt storage not implemented")`
- **RED quality:** All three store tests compiled and the corruption/retry test passed; the two failures were behavioral assertions for missing bounded contention and replacement behavior.

## Task Commits

1. **Task 1 RED: receipt contract and failing reader behavior** — `b08dcee`
2. **Task 1 GREEN: canonical receipt validation and reader retry** — `3fddc55`
3. **Task 2 RED: bounded lock and replacement tests** — `fe21f30`
4. **Task 2 GREEN: authoritative atomic receipt store** — `e5f56a0`

## Files Created/Modified

- `crates/nano-verify/src/receipt.rs` — receipt types, canonicalization, validation, reader, atomic writer, and focused tests.

## Decisions Made

- Used an RAII lock guard so every success and error path attempts lockfile release.
- Kept production timing constants exact while injecting shorter policies only through the module-private test seam.
- Accepted only confined ASCII requirement slugs for receipt filenames, rejecting separators and traversal.

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

- Clippy identified `u32::is_multiple_of` as newer than the workspace MSRV; RFC-3339 leap-year validation uses modulo arithmetic instead.

## Known Stubs

None found in the modified code.

## Threat Mitigations

- **T-04-13:** exclusive lock, synced same-directory tempfile, and authoritative platform replacement; no remove-before-rename fallback.
- **T-04-14:** exact bounded writer retry policy with typed contention failure.
- **T-04-15:** deny-unknown decoding and strict schema/red-evidence validation before mint or write.

## Verification

- `cargo test -p nano-verify` — 16 passed, 0 failed.
- `cargo test -p nano-verify receipt::tests::store_` — 3 passed, 0 failed.
- `cargo fmt --all -- --check` — passed.
- `cargo clippy -p nano-verify --all-targets -- -D warnings` — passed.

## Self-Check: PASSED

- Modified code file and canonical summary exist.
- All four TDD commits exist in history.
- No stub patterns or new unmodeled trust boundary were found.

---
*Phase: 04-wp-1-gate-and-receipt-foundation*
*Completed: 2026-08-17*
