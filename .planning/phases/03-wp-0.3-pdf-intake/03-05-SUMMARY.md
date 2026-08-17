---
phase: 03-wp-0.3-pdf-intake
plan: 05
subsystem: acp-runtime
tags: [rust, acp, pdf, attachment-store, provider-routing]
requires:
  - phase: 03-03
    provides: Provider-neutral Document content and canonical Anthropic binding
  - phase: 03-04
    provides: Validated inline and confined-path PDF intake
  - phase: 03-09
    provides: Durable DocumentRef and attachment-store reachability
provides:
  - Lease-backed PDF publication and digest-verified resume reconstruction
  - Resolved-leaf ModelLacksPdf refusal before driver construction or dispatch
affects: [03-06, pdf-live-proof, acp-host]
tech-stack:
  added: []
  patterns: [manifest-only durable PDF state, resolved-wire pre-dispatch refusal]
key-files:
  created: [.planning/phases/03-wp-0.3-pdf-intake/03-05-SUMMARY.md]
  modified: [crates/nano-cli/src/acp_mode.rs]
key-decisions:
  - "PDFs share the existing attachment-store lease and verified replay path; projection placeholders are never payload authority."
  - "Client-side Auto refuses on its first successfully resolved incompatible leaf and never searches for a document-aware replacement."
requirements-completed: [PDF-01, PDF-03, PDF-04, PDF-06]
coverage:
  - id: D1
    description: PDF bytes persist as digest-only DocumentRef manifests and rehydrate in mixed order through read_verified.
    requirement: PDF-04
    verification:
      - kind: unit
        ref: "crates/nano-cli/src/acp_mode.rs#document_manifest_kill_resume_rehydrates_verified_bytes_in_mixed_order"
        status: pass
      - kind: unit
        ref: "crates/nano-cli/src/acp_mode.rs#document_resume_missing_and_corrupt_degrade_without_placeholder_reconstruction"
        status: pass
    human_judgment: false
  - id: D2
    description: Resolved OpenAiCompletions leaves refuse PDF input with ModelLacksPdf and zero calls while AnthropicMessages sends once.
    requirement: PDF-03
    verification:
      - kind: unit
        ref: "crates/nano-cli/src/acp_mode.rs#pdf_resolved_leaf_gate_refuses_completions_with_exactly_zero_calls"
        status: pass
    human_judgment: false
  - id: D3
    description: Ignored active-leaf runtime live proof through ACP serve frames.
    requirement: PDF-06
    verification:
      - kind: integration
        ref: "cargo test -p nano-cli pdf_live_active_leaf_runtime_path -- --ignored --list"
        status: pass
    human_judgment: false
duration: 55min
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 05: ACP PDF Runtime Summary

**PDF attachments now publish under the existing GC lease, resume only from digest-verified bytes, and refuse incompatible resolved leaves before driver construction or network dispatch.**

## Accomplishments

- Extended attachment publication to validated document blocks while preserving the digest match and lease-through-journal invariant.
- Reconstructed `DocumentRef` blocks in original mixed order through `AttachmentStore::read_verified`; missing, corrupt, or malformed blobs become the canonical document warning and never trust placeholder text.
- Added a resolved-wire gate for pinned and client-side Auto paths. The first resolved incompatible Auto leaf returns `ModelLacksPdf` without constructing its driver or considering a replacement.
- Added focused mixed-order, kill/resume, missing/corrupt, and zero-call regressions.
- Added the ignored live harness over `serve`: initialize/session-new, explicit canonical model selection, identical text control, real `document_path`, journal-derived usage delta, exact oracle assertion, and sanitized evidence output.
- Passed D9 ownership and protected-tree inventory checks on the final product diff.

## Task Commits

No commits were created. The parent integrator explicitly owns commits on `feat/wp-03`.

## Files Created/Modified

- `crates/nano-cli/src/acp_mode.rs` — document publication, verified replay, resolved-wire refusal, and focused tests.
- `.planning/phases/03-wp-0.3-pdf-intake/03-05-SUMMARY.md` — execution evidence and remaining live-harness gap.

## Decisions Made

- Reused the image attachment-store gate rather than creating a parallel PDF store or persistence channel.
- Kept image influence bookkeeping image-only while broadening only the store-open predicate to all attachment manifests.
- Applied PDF compatibility after concrete binding resolution; Auto does not reroute based on document capability.

## Deviations from Plan

### Review Fixes

**1. [HIGH - Pre-refusal persistence] Deferred publication until resolved-wire acceptance**
- Moved attachment-store publication after pinned/Auto binding resolution and the PDF compatibility gate.
- Added actual `serve` recording coverage proving pinned and Auto incompatibility produces `ModelLacksPdf`, zero driver calls, no reroute, and no blob; the compatible Anthropic leaf publishes and calls once.

**2. [HIGH - Replay metadata trust] Validated reconstructed PDF bytes**
- After `read_verified`, replay now requires exact `DocumentRef.bytes`, at most 20 MiB, and `%PDF-` at offset zero before base64 emission.
- Added wrong-magic, wrong-length, and independent failure-ordinal regressions.

**3. [HIGH - Live endpoint] Pinned the canonical API path**
- Corrected the harness assertion/evidence from `/v1/messages` to `/anthropic/v1/messages` at that review point; DEV-WP-0.3N records the later live-evidence authority reversal back to `/v1/messages`.

**4. [HIGH - Worktree fixture resolution] Added validated monorepo-root discovery**
- Fixture resolution now walks ancestors until both the canonical shared GOALS marker and `wayland-nano/AGENTS.md` exist; it no longer assumes a nested worktree depth.

**5. [HIGH - Document ordinals] Counted every manifest entry**
- A dedicated document ordinal increments before rehydration, so failures do not collapse later numbering.

**6. [HIGH - Dispatch proof] Exercised the actual serve/router path**
- Channel-backed pinned, Auto, and compatible cases prove call counts and persistence behavior through `serve` rather than a predicate-only fake.

**7. [HIGH - Evidence completeness] Prepared the paired Plan 07 scanner inputs**
- On explicit live invocation only, the harness writes six payload pairs plus `evidence-manifest.json` as the seventh file in both Plan 07 roots.
- Under DEV-WP-0.3F, the non-self-referential manifest describes exactly the six payload pairs with `repo_path`, `shared_path`, `sha256`, and `bytes`; the harness reopens both roots and verifies byte/hash equality after writing.
- Plan 07 remains responsible for adding the manifest as scanner input seven and creating/validating the canary receipt. Plan 05 invokes neither scanner nor receipt writer.
- A network-free schema/path-set regression pins the six manifest entries. No capture is created during ordinary tests.

**Review/fix rounds:** 1

## Known Stubs

None.

## Verification

- `cargo test -p nano-cli document` — 2 passed.
- `cargo test -p nano-cli pdf` — zero-call test passed; ignored live test remained network-free.
- `cargo test -p nano-cli pdf_live_active_leaf_runtime_path -- --ignored --list` — exact live harness discovered without executing it.
- `cargo test -p nano-cli acp_mode` — 63 passed, 1 ignored.
- `cargo test -p nano-session attachment_store` — passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `03-OWNERSHIP-PREFLIGHT.ps1 -Mode Check` — `WP-0.3 ownership Check PASS` after the bounded review round (918.4s).

## Threat Flags

None. The file-store and egress-driver trust boundaries are the declared plan surfaces and are covered by the implemented mitigations.

## Self-Check: PASSED

- Both declared files exist and the product diff is ownership-clean.
- Durable replay and resolved-leaf refusal verification pass.
- The ignored live runtime harness compiles, is discoverable, and contains no direct provider-client construction or curl path.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
