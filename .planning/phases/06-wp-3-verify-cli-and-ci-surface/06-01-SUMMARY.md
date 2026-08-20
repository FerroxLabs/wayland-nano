---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 01
subsystem: cli
tags: [rust, nano-verify, argv-parser, registry, tdd]
requires:
  - phase: 05-wp-2-gated-climb
    provides: landed nano_verify contract and climb engine
provides:
  - Closed mint, receipt-check, and run-only CLI mode parser
  - Thin wayland-nano verify binary registration
  - Byte-canonical empty production gate registry bootstrap
  - Compile-time probe of the landed nano_verify public contract
affects: [06-02, 06-03, 06-04, 06-05, 06-06]
tech-stack:
  added: [nano-verify path dependency]
  patterns: [closed argv mode enum, canonical registry delegation]
key-files:
  created: [crates/nano-cli/src/verify_cmd.rs, gates/registry.json]
  modified: [Cargo.lock, crates/nano-cli/Cargo.toml, crates/nano-cli/src/lib.rs, crates/nano-cli/src/main.rs]
key-decisions:
  - "Preserve SPEC-WP3's exact registry JSON literal; its encoded length is 41 bytes despite the plan prose saying 45."
  - "Keep the production run entry fail-closed at exit 2 until later WP-3 plans wire effects."
patterns-established:
  - "All verify argv is reduced to exactly one closed VerifyMode before effects."
  - "Only the exact empty bootstrap bypasses nano_verify::load_registry; all populated registries use the canonical loader."
requirements-completed: [CLI-01, CLI-02, CLI-03]
coverage:
  - id: D1
    description: Closed parser represents all three verify modes and rejects invalid combinations before effects.
    requirement: CLI-01
    verification:
      - kind: unit
        ref: "crates/nano-cli/src/verify_cmd.rs#parse tests"
        status: pass
    human_judgment: false
  - id: D2
    description: nano-cli imports the landed nano_verify contract without changing or redeclaring verifier behavior.
    requirement: CLI-02
    verification:
      - kind: unit
        ref: "crates/nano-cli/src/verify_cmd.rs#landed_contract_import_probe"
        status: pass
    human_judgment: false
  - id: D3
    description: Empty production registry is byte-canonical and populated registries delegate to nano_verify.
    requirement: CLI-03
    verification:
      - kind: unit
        ref: "crates/nano-cli/src/verify_cmd.rs#bootstrap tests"
        status: pass
    human_judgment: false
duration: 24min
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 1: Verify CLI Contract Summary

**Closed verify argv parsing, landed verifier imports, thin binary dispatch, and a canonical empty registry bootstrap.**

## Performance

- **Duration:** 24 min
- **Started:** 2026-08-21T02:18:00+07:00
- **Completed:** 2026-08-21T02:42:00+07:00
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments

- Added exact mint, receipt-check, and run-only mode values with defaults, caps, duplicate rejection, mutual exclusion, and the fixed usage exit code.
- Registered `wayland-nano verify` through a thin current-thread runtime arm and compiled against nano_verify's public receipt, registry, gate, artifact, manifest, and climb contracts.
- Created the exact empty registry literal and proved requested work fails closed while populated fixture registries use the canonical verifier loader.

## Task Commits

1. **Task 1 RED: Define verify parser contract** - `ca59fb5` (test)
2. **Task 1 GREEN: Add closed verify CLI parser** - `dbfc613` (feat)
3. **Task 2: Enforce empty registry bootstrap** - `0c5f247` (feat)

## Files Created/Modified

- `crates/nano-cli/src/verify_cmd.rs` - Closed parser types, fail-closed entry stub, registry recognizer, contract probe, and focused tests.
- `gates/registry.json` - Exact empty schema-1 production bootstrap with no newline.
- `crates/nano-cli/src/main.rs` - Thin verify registration and top-level usage update.
- `crates/nano-cli/src/lib.rs` - Exports the verify module.
- `crates/nano-cli/Cargo.toml` - Adds the exact nano-verify path dependency.
- `Cargo.lock` - Records nano-cli's dependency on the existing workspace package.

## Decisions Made

- The exact JSON literal is authoritative. UTF-8 encoding proves it is 41 bytes; the plan's executable comparison also uses that literal, so the contradictory prose count of 45 was not fabricated into the file.
- The generic-free `run` entry returns usage 2 without effects until later plans wire real registry, climb, receipt, and verification behavior.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Resolved contradictory registry byte count**
- **Found during:** Task 2
- **Issue:** The plan calls the exact literal 45 bytes, but the required UTF-8 literal is mathematically and mechanically 41 bytes.
- **Fix:** Preserved the exact literal and no trailing newline, matching the plan's own byte-comparison command.
- **Files modified:** `gates/registry.json`
- **Verification:** Byte-for-byte PowerShell comparison passed with `BYTE_EXACT=41`; both bootstrap tests passed.
- **Committed in:** `0c5f247`

---

**Total deviations:** 1 auto-fixed (1 blocking authority inconsistency)
**Impact on plan:** No scope expansion; the authoritative content and executable acceptance check are satisfied.

## Issues Encountered

- The first non-interactive repository gate process outlived its output capture; it was stopped and rerun in an interactive tracked session. The rerun completed successfully.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Later WP-3 plans can build production adapters and receipt verification on one compiled closed parser boundary.
- The production registry remains intentionally empty; WP-4 owns population.

## Self-Check: PASSED

- All six changed product files and this summary exist.
- Commits `ca59fb5`, `dbfc613`, and `0c5f247` exist on `worktree-agent-wp3-01`.
- Focused parser/bootstrap/import tests, `cargo check -p nano-cli`, strict clippy, and `just gate-all` passed with F:-only TEMP/TMP/CARGO_TARGET_DIR.

---
*Phase: 06-wp-3-verify-cli-and-ci-surface*
*Completed: 2026-08-21*
