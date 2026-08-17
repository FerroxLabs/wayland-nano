---
phase: 03-wp-0.3-pdf-intake
plan: 03
subsystem: model
tags: [pdf, anthropic-messages, provider-catalog, provenance]
requires:
  - phase: 03-01
    provides: DocumentRef and InputBlock::DocumentRef durable contracts
  - phase: 03-09
    provides: WP-0.3 ownership control and D9 preflight
provides:
  - Provider-neutral PDF document content and ordered TurnInput projections
  - Exact Anthropic base64 document wire block
  - Canonical Flux Anthropic provider catalog binding with generated golden
affects: [03-04, 03-05, 03-06, pdf-intake, model-routing]
tech-stack:
  added: []
  patterns: [digest-only durable manifest with live base64 transport, fail-loud wrong-codec invariant]
key-files:
  created: []
  modified:
    - crates/nano-model/src/types.rs
    - crates/nano-agent/src/turn_input.rs
    - crates/nano-agent/src/compact.rs
    - crates/nano-model/src/anthropic_messages.rs
    - crates/nano-model/src/flux_responses.rs
    - crates/nano-model/data/providerCatalog.vendored.json
    - crates/nano-model/tests/provider_catalog.rs
    - crates/nano-model/tests/golden/provider_catalog.golden.rs
    - UPSTREAM.md
key-decisions:
  - "Document request-byte accounting uses the exact base64 data length without changing compaction policy."
  - "A document reaching the Flux Responses codec panics loudly; Plan 03-05 owns the typed pre-dispatch zero-call refusal."
patterns-established:
  - "One TurnBlock::Document drives display projection, digest-only manifest, and live provider content."
requirements-completed: [PDF-02, PDF-03]
coverage:
  - id: D1
    description: Ordered document projection, digest-only manifest, and live base64 content
    requirement: PDF-02
    verification:
      - kind: unit
        ref: "crates/nano-agent/src/turn_input.rs#mixed_duplicate_documents_preserve_all_three_views"
        status: pass
    human_judgment: false
  - id: D2
    description: Exact Anthropic base64 PDF document wire shape
    requirement: PDF-03
    verification:
      - kind: unit
        ref: "crates/nano-model/src/anthropic_messages.rs#document_source_shape_is_exact"
        status: pass
    human_judgment: false
  - id: D3
    description: Canonical Flux Anthropic catalog endpoint, SHA pin, and generated golden
    requirement: PDF-03
    verification:
      - kind: integration
        ref: "cargo test -p nano-model --test provider_catalog"
        status: pass
    human_judgment: false
duration: 1h15m
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 03: Model Document Codec Summary

**Provider-neutral PDF blocks now preserve ordered durable/live projections, emit the exact Anthropic document source shape, and route through a drift-pinned canonical Flux Anthropic catalog binding.**

## Performance

- **Duration:** 1h 15m
- **Completed:** 2026-08-17
- **Tasks:** 3
- **Files modified:** 9 implementation/provenance files

## Accomplishments

- Added `ContentBlock::Document` and `TurnBlock::Document` with mixed-order duplicate-aware projection, manifest, and live-content tests.
- Added exact Anthropic `{type: document, source: {type: base64, media_type: application/pdf, data}}` encoding with no extra fields.
- Added the proven `flux-router-anthropic` endpoint, normalized SHA drift pin, build-generated golden, endpoint assertions, and provenance ledger row.
- Kept wrong-codec behavior fail-loud and counted document base64 bytes without changing compaction policy.

## Task Commits

No commits were created by this executor; the parent integrator explicitly owns atomic commits.

## Decisions Made

- Document size estimation counts `data.len()` exactly; MIME metadata is not counted because the existing heuristic primarily accounts for payload bytes.
- The Responses codec uses an explicit `unreachable!` invariant rather than silently filtering documents. The typed zero-egress refusal remains assigned to Plan 03-05.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added explicit Flux Responses document invariant**
- **Found during:** Task 1 compilation
- **Issue:** The additive enum variant made an exhaustive codec match fail compilation.
- **Fix:** DEV-WP-0.3D expanded ownership; an explicit fail-loud arm and panic regression were added.
- **Files modified:** `crates/nano-model/src/flux_responses.rs`
- **Verification:** `cargo test -p nano-model`

**2. [Rule 3 - Blocking] Added document request-byte accounting**
- **Found during:** Task 1 compilation after the codec fix
- **Issue:** The compaction estimator's exhaustive match lacked the new variant.
- **Fix:** DEV-WP-0.3E expanded ownership; exact base64 payload length and a monotonic regression were added without changing compaction policy.
- **Files modified:** `crates/nano-agent/src/compact.rs`
- **Verification:** `cargo test -p nano-agent document_payload_contributes_monotonically`

**3. [Review Fix - High] Closed mixed image/document Responses bypass**
- **Found during:** Post-implementation review round 1
- **Issue:** The image fast path returned after filtering unsupported blocks, so a mixed Image+Document message could silently omit the document before reaching the fail-loud guard.
- **Fix:** Reject any document before selecting the image fast path and pin mixed Image+Document behavior with a panic regression.
- **Files modified:** `crates/nano-model/src/flux_responses.rs`
- **Verification:** `cargo test -p nano-model`

**Total deviations:** 2 blocking compile fixes explicitly authorized by integrator rulings, plus 1 bounded high-severity review fix.

## Known Stubs

None.

## Issues Encountered

- D9 external-tree hashing exceeded two initial command timeouts; it was rerun unchanged with a longer timeout rather than bypassed.
- Review/fix rounds: 1; the high-severity mixed-media silent-drop path was closed and reverified.

## User Setup Required

None.

## Next Phase Readiness

- ACP intake and dispatch gating can consume the provider-neutral document block.
- Plan 03-05 must enforce typed zero-call refusal before non-Anthropic codecs.

## Self-Check: PASSED

- All declared implementation and provenance files exist.
- No task commits were expected from this executor; changes remain uncommitted for the parent integrator.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
