---
phase: 03-wp-0.3-pdf-intake
plan: 09
subsystem: session-storage
tags: [rust, serde, journal, attachments, gc, pdf]
requires:
  - phase: 03-wp-0.3-pdf-intake
    provides: D9 ownership preflight and WP-0.3 narrow nano-session grant
provides:
  - Digest-only closed DocumentRef journal contract
  - Fail-closed PDF attachment reachability and retention
  - Document-specific unavailable replay placeholder
affects: [pdf-intake, context-replay, attachment-gc]
tech-stack:
  added: []
  patterns: [additive journal variant, typed serde validation, fail-closed reachability]
key-files:
  created: [.planning/phases/03-wp-0.3-pdf-intake/03-09-SUMMARY.md]
  modified: [crates/nano-session/src/op.rs, crates/nano-session/src/attachment_store.rs]
key-decisions:
  - "Validate DocumentRef digest and MIME during deserialization, then reject malformed document manifests again at the GC boundary."
  - "Add a parallel document-unavailable placeholder so ImageRef fallback text remains byte-stable."
patterns-established:
  - "Document manifests contain metadata and a lowercase SHA-256 digest only; bytes/base64 never enter the journal."
  - "Document reference scan errors abort GC rather than returning a partial live set."
requirements-completed: [PDF-02, PDF-04]
coverage:
  - id: D1
    description: "Closed digest-only DocumentRef round-trips without changing ImageRef wire shape."
    requirement: PDF-02
    verification:
      - kind: unit
        ref: "crates/nano-session/src/op.rs#document_ref_tests"
        status: pass
    human_judgment: false
  - id: D2
    description: "Journaled PDFs remain reachable while orphans are swept and malformed references abort scanning."
    requirement: PDF-04
    verification:
      - kind: integration
        ref: "cargo test -p nano-session attachment_store"
        status: pass
      - kind: other
        ref: "03-OWNERSHIP-PREFLIGHT.ps1 -Mode Check"
        status: pass
    human_judgment: false
duration: 1h06m
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 09: Document Manifest and GC Safety Summary

**Closed digest-only PDF journal manifests with fail-closed GC reachability, orphan collection, and document-specific unavailable replay text.**

## Performance

- **Duration:** 1h 06m
- **Completed:** 2026-08-17
- **Tasks:** 2
- **Files modified:** 2 product files plus this summary

## Accomplishments

- Added additive `DocumentRef` and `InputBlock::DocumentRef` types with deny-unknown deserialization, exact `application/pdf`, lowercase 64-hex digest validation, and no byte/base64 field.
- Extended journal reachability to retain valid document blobs while aborting GC on malformed document manifests; existing image and tool-result behavior is unchanged.
- Proved referenced retention, orphan sweeping, malformed rejection, `AttachmentMissing`, and a loud document-specific fallback placeholder.

## Task Commits

Commits are intentionally deferred to the parent integrator, which owns atomic commits on the authorized `feat/wp-03` worktree.

## Files Created/Modified

- `crates/nano-session/src/op.rs` - Adds the closed `DocumentRef` journal contract and byte-compatibility tests for `ImageRef`.
- `crates/nano-session/src/attachment_store.rs` - Adds document reachability, fail-closed malformed handling, retention/orphan tests, and document fallback text.
- `.planning/phases/03-wp-0.3-pdf-intake/03-09-SUMMARY.md` - Records lifecycle evidence for Plan 09.

## Decisions Made

- Enforced digest and MIME invariants at deserialization and retained an explicit GC-boundary check as defense in depth against live-data deletion.
- Kept `attachment_unavailable_placeholder` unchanged and introduced `document_unavailable_placeholder` in parallel, preserving the locked image contract.

## Deviations from Plan

### Review Fixes

**1. [HIGH - Locked D3 fallback text] Corrected the document replay instruction**
- **Found during:** Independent review after initial Plan 09 completion
- **Issue:** The new document fallback said `do not describe it from memory`; locked D3 requires the exact text `do not answer from memory`.
- **Fix:** Replaced only the document-specific instruction and updated its exact assertion. The existing image fallback remains byte-unchanged.
- **Files modified:** `crates/nano-session/src/attachment_store.rs`
- **Verification:** DocumentRef tests 2/2, attachment-store tests 15/15, format/diff checks, and post-fix D9 ownership Check all passed.
- **Commit:** Deferred to the parent integrator as requested.

**Review/fix rounds:** 1

The plan otherwise executed within the exact two-file product scope. The parallel document fallback helper is the smallest required implementation of the planned missing/corrupt replay behavior.

## Issues Encountered

- The first D9 invocation exceeded its 20-minute process ceiling without a verdict. It was not counted; a later exact invocation completed successfully.
- The first document fallback assertion exposed that the existing helper is intentionally image-specific. A parallel document helper resolved the mismatch without modifying image output.
- Independent review found one HIGH exact-text mismatch in that new helper; one bounded fix round corrected it to locked D3 text and reran all focused and ownership gates.

## Known Stubs

None. Placeholder references are durable manifest/replay contract fields, not implementation stubs.

## Threat Flags

None. The changes implement the plan's journal-to-GC trust-boundary mitigations and add no new endpoint, auth path, file-access pattern, or schema trust boundary beyond the declared additive journal variant.

## Verification

- `cargo test -p nano-session document_ref_tests` — 2 passed.
- `cargo test -p nano-session attachment_store` — 15 passed.
- `cargo fmt` completed; `git diff --check` passed before final summary creation.
- Exact D9 Check completed with `WP-0.3 ownership Check PASS` after the final product bytes.
- Post-review exact D9 Check completed again with `WP-0.3 ownership Check PASS` after the corrected D3 text.

## User Setup Required

None.

## Next Phase Readiness

- Turn projection and ContextFold plans can consume `InputBlock::DocumentRef` and `document_unavailable_placeholder` without changing image behavior.
- No blocker remains in Plan 09 scope.

## Self-Check: PASSED

- Both declared product files and this summary exist.
- Focused journal and attachment-store suites pass.
- Final D9 ownership check passes on the exact uncommitted product diff.
- One review/fix round is recorded, with no unresolved finding.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
