---
phase: 03-wp-0.3-pdf-intake
plan: 04
subsystem: protocol
tags: [rust, acp, pdf, validation, confinement]
requires:
  - phase: 03-03
    provides: TurnBlock::Document and provider-neutral document content
  - phase: 03-09
    provides: DocumentRef durable manifest contract
provides:
  - Fail-closed inline ACP PDF validation
  - Handle-verified confined document_path intake
affects: [03-05, pdf-intake, acp-host]
tech-stack:
  added: []
  patterns: [bounded pre-publication validation, handle-verified confined read]
key-files:
  created: [.planning/phases/03-wp-0.3-pdf-intake/03-04-SUMMARY.md]
  modified: [crates/nano-protocol/src/acp.rs]
key-decisions:
  - "Reject every document input before publication unless MIME, extension where applicable, offset-zero magic, count, and decoded size agree."
  - "Reuse the existing verified-handle open and translate its bounded path diagnostics to the document_path vocabulary."
requirements-completed: [PDF-01, PDF-02]
duration: 35m
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 04: ACP PDF Intake Summary

**ACP now accepts exactly one valid inline or confined-path PDF through a bounded, race-resistant validation boundary.**

## Accomplishments

- Added strict `application/pdf` inline decoding with exact `%PDF-` offset-zero magic, non-empty input, one-document count, saturating accounting, and the exact 20 MiB ceiling.
- Added `.pdf`-only confined path intake using lexical no-link/reparse checks before canonicalization plus the existing canonical-root, sensitive-subtree, verified-handle, and authorize/open identity checks.
- Minted digest-only `DocumentRef` metadata and the locked inline/canonical-path placeholders without routing document bytes through image decoding.
- Added adversarial coverage for malformed base64, MIME/magic mismatches, zero/over-cap input, second documents, extension mismatch, root escape, sensitive paths, and authorize/open swaps.

## Task Commits

Commits are intentionally deferred to the parent integrator, which owns atomic commits on `feat/wp-03`.

## Decisions Made

- Kept SHA-256 construction local to the owned converter because the exact D9 grant excludes dependency-manifest changes.
- Bounded path reads to 20 MiB plus one rejection sentinel before base64 encoding or document publication.

## Deviations from Plan

### Review Fixes

**1. [HIGH - Pre-decode allocation] Bounded encoded inline input**
- **Issue:** The decoded cap was correct, but an arbitrarily large base64 string could allocate during decode before rejection.
- **Fix:** Added an overflow-safe maximum STANDARD encoded length check before decode while retaining the authoritative decoded-byte cap.
- **Verification:** Huge encoded regression plus exact decoded boundary tests pass.

**2. [HIGH - Pre-existing link traversal] Rejected every link/reparse path component**
- **Issue:** Canonical-root and handle identity checks stopped escapes and swaps, but a pre-existing in-root link could still be accepted when its resolved target remained allowed.
- **Fix:** Inspect every lexical component with no-follow metadata before canonicalization; reject Unix symlinks and Windows reparse points, then retain canonical-root and opened-handle identity checks. Both raw and canonical extensions must be `.pdf`.
- **Verification:** Direct file symlink, directory symlink/Windows junction, and authorize/open swap regressions pass where the host supports link creation.

**3. [HIGH - Strict lint gate] Replaced boolean equality assertion**
- **Issue:** One test used `assert_eq!(condition, true)`, which violates strict Clippy.
- **Fix:** Replaced it with `assert!(condition)` and ran the all-target strict lint gate.

**Review/fix rounds:** 1

## Known Stubs

None. Document placeholders are locked projection fields, not implementation stubs.

## Verification

- `cargo test -p nano-protocol acp::tests` — 25 passed.
- `cargo clippy -p nano-protocol --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `03-OWNERSHIP-PREFLIGHT.ps1 -Mode Check` — `WP-0.3 ownership Check PASS`.

## Threat Flags

None. The file-access and untrusted-byte surfaces are the declared plan boundaries and their mitigations are implemented and tested.

## Self-Check: PASSED

- The declared product file and this summary exist.
- Focused ACP tests, format/diff checks, and D9 ownership verification pass.
- No forbidden external tree or credential was read or modified.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
