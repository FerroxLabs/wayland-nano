---
phase: 02-minimal-authenticated-activation
plan: 01
subsystem: authentication
tags: [raw-json, jcs, ed25519, contracts, vectors]
requires:
  - phase: 02-minimal-authenticated-activation
    provides: exact worktree/base authorization from Plan 02-11
provides:
  - closed activation, control, admin-request, and receipt schemas
  - hash-bound positive and negative raw-byte contract vectors
  - bounded duplicate-preserving whole-frame parser and private trusted constructors
  - RFC 8785 JCS and RFC 8032 Ed25519 verification profile
affects: [02-03, 02-04, 02-05, 02-08, 02-09, 02-15]
tech-stack:
  added: [serde_jcs 0.2.0, ed25519-dalek 3.0.0]
  patterns: [raw-bytes-before-value, closed-wire-types, domain-separated-signatures, manifest-bound-fixtures]
key-files:
  created:
    - contracts/activation/wayland.nano.activation-v1.schema.json
    - contracts/activation/wayland.nano.activation-receipt-v1.schema.json
    - contracts/activation/wayland.nano.admin-request-v1.schema.json
    - contracts/activation/wayland.nano.control-v1.schema.json
    - contracts/activation/vectors/manifest.json
    - contracts/activation/vectors/positive.json
    - contracts/activation/vectors/negative.json
    - crates/nano-activation/src/lib.rs
    - crates/nano-activation/src/raw.rs
    - crates/nano-activation/tests/contract_vectors.rs
  modified: [Cargo.toml, Cargo.lock]
key-decisions:
  - "Raw whole-frame validation precedes serde_json::Value and rejects duplicate decoded keys, invalid I-JSON numbers, trailing bytes, excessive depth/properties/arrays, and frames over 32 KiB."
  - "Only private fields behind verify_activation_frame, verify_control, and verify_admin_request can construct trusted contract types."
patterns-established:
  - "Canonical carrier bytes must occur exactly inside the raw frame; no Unicode normalization, key folding, trimming, or lossy preparse is permitted."
  - "Activation, control, admin, and receipt signatures use separate fixed domain tags and unpadded 64-byte Ed25519 signatures."
requirements-completed: [REQ-ACT-01]
coverage:
  - id: D1
    description: Four closed schemas and five positive plus 26 negative contract subjects are hash-bound by an independently recomputed manifest.
    requirement: REQ-ACT-01
    verification:
      - kind: integration
        ref: "PowerShell schema/id/additionalProperties and SHA-256 inventory gate from 02-01-PLAN.md"
        status: pass
    human_judgment: false
  - id: D2
    description: The Rust contract consumer preserves raw ambiguity and verifies JCS/Ed25519 activation, control, admin, receipt, RFC, crypto-negative, and resource-boundary cases.
    requirement: REQ-ACT-01
    verification:
      - kind: integration
        ref: "cargo test -p nano-activation --test contract_vectors (two clean external target directories)"
        status: pass
      - kind: unit
        ref: "cargo clippy -p nano-activation --all-targets -- -D warnings; cargo fmt --all -- --check"
        status: pass
    human_judgment: false
duration: 26min
completed: 2026-08-29
status: complete
---

# Phase 2 Plan 01: Byte-Frozen Activation Contract Summary

**Closed wire schemas, independently hash-bound raw vectors, and a bounded pre-parse JCS/Ed25519 verifier now define the sole trusted activation contract surface.**

## Performance

- **Duration:** 26 min
- **Started:** 2026-08-29T16:17:00Z
- **Completed:** 2026-08-29T16:42:54Z
- **Tasks:** 2
- **Files modified:** 13

## Accomplishments

- Froze activation, control, admin-request, and receipt schemas with closed top-level vocabulary, ASCII identifier grammar, UTC-second timestamps, safe integers, capability/control enums, and signature bounds.
- Added five positive subjects (RFC 8032, activation, receipt, control, admin) plus 26 raw negative vectors across eight mandatory subject families, all SHA-256 bound by the manifest.
- Added a leaf `nano-activation` crate that rejects ambiguity before ordinary serde parsing and exposes only verified trusted types with private fields.
- Verified wrong key/domain/signature, noncanonical bytes, excessive frame/depth/property count, duplicate decoded/escaped keys, invalid UTF-8/I-JSON, and trailing frames fail typed.

## Task Commits

1. **Task 1 / TDD RED: freeze schemas and independent vectors** - `dd53eb8`
2. **Task 2 / TDD GREEN: implement bounded raw/JCS/Ed25519 verifier** - `3cec905`
3. **Task 2 / REFACTOR: remove unnecessary contract dependencies** - `f9d4b45`
4. **Task 2 / verification hardening: crypto and resource boundaries** - `3e3057c`

Planning metadata commit is intentionally left to the parent orchestrator because this summary lives in the planning worktree on `plan/persistent-agent-program`, outside the executor branch.

## Files Created/Modified

- `contracts/activation/*.schema.json` - Four versioned closed contract schemas.
- `contracts/activation/vectors/*.json` - Positive/negative vectors and exact inventory hashes.
- `crates/nano-activation/src/raw.rs` - Bounded duplicate-aware whole-frame deserializer retaining raw-byte evidence.
- `crates/nano-activation/src/lib.rs` - Closed types, validation, JCS, domain separation, Ed25519 verification, and private trusted constructors.
- `crates/nano-activation/tests/contract_vectors.rs` - Independent manifest/RFC/raw/crypto/resource consumer gate.
- `Cargo.toml`, `Cargo.lock` - Leaf workspace member and exact approved crypto/JCS pins.

## Fixture Generator Provenance

The non-secret fixture key is the deterministic 32-byte sequence `01 02 ... 20`. `python 3` with `cryptography==46.0.5` constructed ASCII-only dictionaries, serialized them with `json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False)`, and signed `DOMAIN || canonical_bytes` using `Ed25519PrivateKey.from_private_bytes(bytes(range(1, 33)))`. The exact domains were `WAYLAND-NANO-ACTIVATION\0v1\0`, `WAYLAND-NANO-CONTROL\0v1\0`, `WAYLAND-NANO-ADMIN\0v1\0`, and `WAYLAND-NANO-RECEIPT\0v1\0`. Rust and Desktop code were not used to generate fixture bytes. RFC 8032 section 7.1 test 1 remains a separately published oracle.

## Decisions Made

- Used `serde_json::DeserializeSeed` only as a lexical, duplicate-aware bounded parser. No ordinary `Value` exists until duplicate/depth/count/number/trailing checks finish.
- Required the JCS serialization of the carrier to occur byte-for-byte inside the raw frame, preserving ordering and escaping evidence while freezing the whole JSON-RPC carrier representation.
- Kept runtime admission, authority lookup, replay state, quarantine, Desktop integration, and Phase 3 memory out of this crate and plan.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Corrected the approved Ed25519 feature profile**
- **Found during:** TDD RED dependency resolution
- **Issue:** `ed25519-dalek` 3.0.0 has no `std` feature; the initial explicit feature spelling could not resolve.
- **Fix:** Retained the exact approved `=3.0.0` pin and used its default verification profile, as required by research.
- **Files modified:** `Cargo.toml`, `Cargo.lock`
- **Verification:** Exact `cargo tree -p nano-activation --depth 1` and focused compile/test passed.
- **Committed in:** `dd53eb8`

**2. [Rule 1 - Bug] Distinguished a malformed present carrier from an absent carrier**
- **Found during:** negative-vector execution
- **Issue:** Chained object conversion collapsed the wrong-shaped carrier into `carrier_missing`; seven schema/crypto vectors also placed `_meta` at the wrong JSON-RPC level.
- **Fix:** Preserved key presence before the object-shape check and moved only those vector carriers under `params._meta`.
- **Files modified:** `crates/nano-activation/src/raw.rs`, `contracts/activation/vectors/negative.json`, manifest hash
- **Verification:** Isolated location-only reproduction followed by all 26 typed negative cases passing on attempt 3.
- **Committed in:** `3cec905`

**3. [Rule 2 - Missing Critical] Added explicit control/admin contract consumers and resource/crypto negatives**
- **Found during:** completion audit
- **Issue:** Schemas existed, but executable positive consumers and wrong-domain/resource-boundary tests were required to prove all four frozen signed document families.
- **Fix:** Added strict private control/admin types and verifiers plus exact fixtures and negative tests without adding runtime admission.
- **Files modified:** positive vector/manifest, `lib.rs`, `contract_vectors.rs`
- **Verification:** both clean external-target runs and clippy/fmt passed.
- **Committed in:** `3cec905`, `3e3057c`

**Total deviations:** 3 auto-fixed (1 blocking, 1 bug, 1 missing critical)
**Impact on plan:** All changes are required contract correctness/security work; no runtime or cross-repository scope was added.

## Issues Encountered

- Concurrent workspace compilation held shared package-cache locks briefly. External plan-specific target directories prevented artifact interference; no retry changed test semantics.
- The execution branch is the exact Plan 02-11 authorized feature branch, not the generic `worktree-agent-*` namespace. The four implementation commits remain on that exact branch; planning metadata is left for parent integration.

## User Setup Required

None.

## Known Stubs

None.

## Next Phase Readiness

- Authority-store and admission plans can consume one frozen contract and cannot fabricate trusted activation/control/admin types.
- Desktop can consume the same manifest/RFC/raw bytes after its package checkpoint.
- No admission runtime, authority store, quarantine, Desktop source, memory integration, push, merge, tag, or secret was touched.

## Self-Check: PASSED

- All 13 planned implementation files exist and the implementation worktree is clean.
- Commits `dd53eb8`, `3cec905`, `f9d4b45`, and `3e3057c` exist on `feat/p2-minimal-authenticated-activation`.
- Schema/inventory verifier passed with exact recomputed SHA-256 hashes.
- Focused vector suite passed twice from clean external target directories, then clippy and fmt passed on the final exact source.

---
*Phase: 02-minimal-authenticated-activation*
*Completed: 2026-08-29*
