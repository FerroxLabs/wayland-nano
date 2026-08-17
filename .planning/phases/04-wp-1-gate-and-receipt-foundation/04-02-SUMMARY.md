---
phase: 04-wp-1-gate-and-receipt-foundation
plan: 02
subsystem: verification
tags: [rust, serde, canonical-json, sha256, unicode-nfc, fail-closed]
requires:
  - phase: 04-wp-1-gate-and-receipt-foundation
    provides: nano-verify crate seam and VerifyError taxonomy
provides:
  - Schema-1 gate registry authority with strict deserialization
  - NFC canonical closure digests and closure-pin verification
  - Repo-confined card, script, and run-artifact resolution
  - Requirement and Gate Card check-inventory resolution
affects: [wp-1-gate-runner, wp-1-receipts, wp-2-engine, wp-3-cli]
tech-stack:
  added: []
  patterns: [canonical NFC JSON before SHA-256, canonical containment after lexical path rejection, fail-closed registry parsing]
key-files:
  created: []
  modified: [crates/nano-verify/src/registry.rs]
key-decisions:
  - "Normalize every canonical JSON object key and string value to NFC before lexicographic serialization."
  - "Require paths to pass both lexical relative-path checks and canonical repository containment."
patterns-established:
  - "Registry inputs reject schema skew, unknown fields, dangling mappings, closure drift, and ambiguous invocation shapes."
requirements-completed: [GATE-02, GATE-03, RCPT-03, RCPT-04]
coverage:
  - id: D1
    description: Canonical schema-1 registry loading and closure-pin authority
    requirement: GATE-02
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/registry.rs#closure_digest_is_canonical"
        status: pass
      - kind: unit
        ref: "crates/nano-verify/src/registry.rs#registry_loads_closures_requirements_and_rejects_drift"
        status: pass
    human_judgment: false
  - id: D2
    description: Strict registry shape, mapping, path, invocation, and card inventory validation
    requirement: GATE-03
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/registry.rs#registry_rejects_unknown_fields"
        status: pass
      - kind: other
        ref: "cargo clippy -p nano-verify --all-targets -- -D warnings"
        status: pass
    human_judgment: false
duration: 16min
completed: 2026-08-17
status: complete
---

# Phase 04 Plan 02: Canonical Registry Authority Summary

**Schema-1 registry loading with NFC canonical SHA-256 pins, complete requirement resolution, confined artifacts, invocation validation, and Gate Card inventories**

## Performance

- **Duration:** 16 min active execution
- **Completed:** 2026-08-17T16:07:27Z
- **Tasks:** 2
- **Files modified:** 1 production file

## Accomplishments

- Implemented the contract registry types and strict schema-1 envelope loading with unknown-field rejection at every persisted layer.
- Implemented integer-only NFC canonical JSON and pinned lowercase SHA-256 closure digests.
- Fail closed on dangling mappings, digest drift, ambiguous direct/interpreter shapes, missing or escaping paths, and malformed or empty card inventories.

## TDD RED Evidence

- **Command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify registry::tests`
- **Exit code:** `101`
- **Bounded result:** `1 passed; 2 failed; 0 ignored`
- **Expected failing tests:** `registry::tests::closure_digest_is_canonical`, `registry::tests::registry_loads_closures_requirements_and_rejects_drift`
- **Assertion excerpts:** canonical bytes were `left: []` against the pinned UTF-8 byte vector; registry loading unwrapped `Err(Registry("not implemented"))`.
- **Validity:** compilation and fixtures succeeded; `registry_rejects_unknown_fields` passed. Failures were caused only by deliberately absent canonicalization and registry behavior.

## Task Commits

1. **Tasks 1-2: Lock RED tests and implement canonical registry behavior** - `4e60b4b` (feat)

## Files Created/Modified

- `crates/nano-verify/src/registry.rs` - Registry contract types, canonical hashing, validation, resolution, card inventory parsing, and named tests.

## Decisions Made

- Canonicalization constructs a normalized value tree first, detecting NFC key collisions before compact serialization.
- Path trust uses lexical rejection before filesystem canonicalization, then checks canonical containment under the repository root.

## Deviations from Plan

None - plan executed exactly as written. The orchestrator paused execution until the prerequisite gate `FailCategory` type landed; no invalid compile failure was accepted as RED evidence.

## Known Stubs

None.

## Verification

- `cargo fmt --all --check` - passed.
- `cargo test -p nano-verify registry::tests` - 3 passed.
- `cargo test -p nano-verify` - 12 passed, including all registry and gate unit tests.
- `cargo clippy -p nano-verify --all-targets -- -D warnings` - passed.

## Self-Check: PASSED

- `crates/nano-verify/src/registry.rs` exists.
- Production commit `4e60b4b` exists.
- No product stubs or untracked generated artifacts remain.

## Next Phase Readiness

Gate execution and receipt preflight can resolve independently reconstructable, drift-checked registry entries and authoritative card inventories.

---
*Phase: 04-wp-1-gate-and-receipt-foundation*
*Completed: 2026-08-17*
