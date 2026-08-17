---
phase: 03-wp-0.3-pdf-intake
plan: 10
subsystem: testing
tags: [rust, acp, pdf, checkpoints, audit-fix]
requires:
  - phase: 03-wp-0.3-pdf-intake
    provides: audited PDF intake implementation and WP03-AUDIT-HIGH-001
provides:
  - immutable group-A fix for the ACP PDF dispatch regression fixture
  - machine-readable group-A fix candidate for Plan 13
affects: [03-11-independent-recheck, 03-13-fix-metadata]
tech-stack:
  added: []
  patterns: [checkpoint-ready Git test workspaces, provider-isolated ACP fixtures]
key-files:
  created: [.planning/phases/03-wp-0.3-pdf-intake/03-10-SUMMARY.md]
  modified: [crates/nano-cli/src/acp_mode.rs]
key-decisions:
  - "Initialize the regression workspace as a real Git root with a committed baseline so the production checkpoint guard remains exercised."
  - "Use catalog-backed OpenAI and Anthropic test providers to isolate the regression from the FLUX_API_KEY unit-test race."
patterns-established:
  - "ACP checkpoint-path regressions create a valid Git repository instead of bypassing checkpoint availability."
requirements-completed: [PDF-01, PDF-02, PDF-03, PDF-04, PDF-06]
coverage:
  - id: D1
    description: "ACP PDF serve regression reaches the model_lacks_pdf wire gate through a checkpoint-ready workspace."
    requirement: PDF-04
    verification:
      - kind: integration
        ref: "cargo test -p nano-cli pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded -- --nocapture"
        status: pass
    human_judgment: false
duration: 29min
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 10: Bounded Critical/High Fix Round Summary

**Checkpoint-ready ACP PDF regression fixture that reaches the typed `model_lacks_pdf` refusal without weakening checkpoint or persistence assertions**

## Performance

- **Duration:** 29 min
- **Completed:** 2026-08-17
- **Tasks:** 1
- **Product files modified:** 1

## Accomplishments

- Initialized the regression's temporary workspace as a canonical Git root with a tracked baseline commit, satisfying the real checkpoint-store prerequisites.
- Preserved the production checkpoint guard and the exact `model_lacks_pdf`, zero-driver-call, and no-blob-publication assertions.
- Committed the sole group-A product path as immutable commit `18d57a6724637f597883685749583253613a0884` with tree `c2dfe7aac460dd7cfe30084859d26eb2a4145403`.

## Group A Candidate Record

Plan 13 may copy this record into canonical `fix.commits[]` only after independently validating it.

```json
{
  "group": "A",
  "pre_commit": "3fde7c507b151411996210d159ccb4b5a3122a69",
  "commit": "18d57a6724637f597883685749583253613a0884",
  "tree": "c2dfe7aac460dd7cfe30084859d26eb2a4145403",
  "paths": [
    "crates/nano-cli/src/acp_mode.rs"
  ],
  "finding_ids": [
    "WP03-AUDIT-HIGH-001"
  ],
  "commands": [
    {
      "id": "FIX-A-CMD-001",
      "command": "cargo fmt --all -- --check",
      "exit": 0
    },
    {
      "id": "FIX-A-CMD-002",
      "command": "cargo test -p nano-cli pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded -- --nocapture",
      "exit": 0
    },
    {
      "id": "FIX-A-CMD-003",
      "command": "powershell -NoProfile -ExecutionPolicy Bypass -File .planning/phases/03-wp-0.3-pdf-intake/03-OWNERSHIP-PREFLIGHT.ps1 -Mode Check",
      "exit": 0
    },
    {
      "id": "FIX-A-CMD-004",
      "command": "git diff --check 3fde7c507b151411996210d159ccb4b5a3122a69 18d57a6724637f597883685749583253613a0884 -- crates/nano-cli/src/acp_mode.rs",
      "exit": 0
    }
  ]
}
```

## Task Commit

1. **Task 1: Commit bounded group-A Critical/High fix** — `18d57a6724637f597883685749583253613a0884` (`fix`)

## Files Created/Modified

- `crates/nano-cli/src/acp_mode.rs` — creates a committed Git fixture and uses isolated catalog providers so the actual serve regression reaches the PDF wire gate.
- `.planning/phases/03-wp-0.3-pdf-intake/03-10-SUMMARY.md` — durable group-A candidate record; intentionally excluded from the product fix commit.

## Independent Candidate Validation

- Commit object exists and resolves to tree `c2dfe7aac460dd7cfe30084859d26eb2a4145403`.
- The per-commit diff from `3fde7c507b151411996210d159ccb4b5a3122a69` contains exactly `crates/nano-cli/src/acp_mode.rs`.
- The sole finding ID is unique and resolves to the sole declared audit finding on the changed path.
- Every command in the candidate record has a unique ID and exit `0`.

## Non-Candidate Evidence

The broader audit command `cargo test -p nano-protocol -p nano-agent -p nano-model -p nano-session -p nano-cli` was run after the fix. The targeted test `acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded` passed, but the command exited `101` on the unrelated pre-existing `nano-session::tests::p2a_op_never_denies_unknown_fields` failure. Because candidate command records require exit `0`, this run is documented here and is not included in the machine-validated candidate commands.

## Deviations from Plan

None — the fix remained on the single authorized group-A surface and did not alter audit metadata.

## Known Stubs

None.

## Self-Check: PASSED

The product file, summary, fix commit, tree, exact path set, unique finding ID, and exit-zero candidate commands were independently checked.

## Next Phase Readiness

Group A is immutable and ready for Plan 13 transcription. The independent recheck remains pending.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
