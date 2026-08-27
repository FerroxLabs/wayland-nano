---
phase: 03-wp-0.3-pdf-intake
plan: 07
subsystem: live-evidence
tags: [pdf, flux, live-proof, canary, d9]
requires:
  - phase: 03-wp-0.3-pdf-intake
    provides: canonical endpoint and live harness at f1372da6336f7bacad95b2c460c7f9ff1d4fcaf5
provides:
  - immutable seven-file PDF live-evidence commit
  - six-pair non-self-referential evidence manifest
  - exact seven-file zero-hit canary receipt
affects: [03-08-closure]
tech-stack:
  added: []
  patterns: [path-only credential resolution, non-self-referential evidence manifests, exact-list canary receipts]
key-files:
  created:
    - crates/nano-model/fixtures-flux/pdf/control-request.json
    - crates/nano-model/fixtures-flux/pdf/document-request.json
    - crates/nano-model/fixtures-flux/pdf/document-response.json
    - crates/nano-model/fixtures-flux/pdf/evidence-manifest.json
    - crates/nano-model/fixtures-flux/pdf/known-quote.pdf
    - crates/nano-model/fixtures-flux/pdf/session-transcript.json
    - crates/nano-model/fixtures-flux/pdf/usage-summary.json
  modified: []
key-decisions:
  - "Keep live evidence outside the audited product history and bind it to its own immutable seven-path commit."
  - "Keep the receipt external and exclude it from its own exact seven-file scan."
requirements-completed: [PDF-05]
duration: 975.2s D9 plus live execution
completed: 2026-08-17
status: complete
---

# Phase 03 Plan 07: Active PDF Live Evidence Summary

**The canonical Flux Anthropic runtime returned the exact PDF oracle with a 13,931-token anti-blind delta, backed by six byte-identical evidence pairs and a zero-hit exact-seven canary receipt.**

## Machine-Readable Live Evidence Ledger

product_commit: f1372da6336f7bacad95b2c460c7f9ff1d4fcaf5
product_tree: 5ff1ea037d604c273095b5303062a68e936d83df
evidence_commit: 0eb5098426f95ee8d8e33bb4c35d370d399ea6b4
receipt_sha256: 949a38c71320db0506ba9a2b1925d0d44bc993038c22ab15e44e7bf375635c50
receipt_bytes: 1878
files_scanned: 7
canary_hits: 0
canary_verdict: PASS
implemented_status: PASS
reachable_status: PASS
live_proven_status: PASS

## Live Runtime Result

- Provider/model: `flux-router-anthropic:flux-auto` through the canonical `/v1/messages` binding.
- Oracle: `WAYLAND NANO PDF ORACLE 7F3A: copper owls navigate by moonlit checksum.`
- Control input tokens: `38`.
- PDF input tokens: `13,969`.
- Same-path token delta: `13,931`, exceeding the required `1,000` minimum.
- Credential handling used only the absolute `FLUX_API_KEY_FILE` path. No key value, authorization header, or credential diagnostic entered evidence.

## Immutable Live-Evidence Commit

Commit `0eb5098426f95ee8d8e33bb4c35d370d399ea6b4` contains exactly these seven paths:

1. `crates/nano-model/fixtures-flux/pdf/control-request.json`
2. `crates/nano-model/fixtures-flux/pdf/document-request.json`
3. `crates/nano-model/fixtures-flux/pdf/document-response.json`
4. `crates/nano-model/fixtures-flux/pdf/evidence-manifest.json`
5. `crates/nano-model/fixtures-flux/pdf/known-quote.pdf`
6. `crates/nano-model/fixtures-flux/pdf/session-transcript.json`
7. `crates/nano-model/fixtures-flux/pdf/usage-summary.json`

The evidence manifest contains exactly six payload-pair rows and excludes itself. Every repo/shared payload pair matched its recorded full SHA-256 and byte count.

## Deterministic PDF Provenance

The fixture is an original deterministic browser-generated one-page PDF containing the oracle once. It has SHA-256 `d15785e2ebdaa5658aadc490e78ccc8858b7ca90b88086a6af90178c78176a39` and is `87,132` bytes. No upstream donor, timestamp, random identifier, or secret material was used.

## Canary Receipt

The canonical external receipt at `D:/Development/waylandnano/shared/fixtures/flux/pdf/canary-receipt.json` is `1,878` bytes with SHA-256 `949a38c71320db0506ba9a2b1925d0d44bc993038c22ab15e44e7bf375635c50`. It covers exactly the seven in-repo evidence paths, excludes itself, reports `files_scanned=7`, `hits=0`, and `PASS`, and matches every current file hash and byte count.

## Ownership and Temporary Checkpoint Setup

- D9 ownership Check passed in `975.2s` against the protected external inventories.
- The canonical shared fixture directory was temporarily initialized as a local Git repository so the runtime checkpoint path had a clean baseline containing only `known-quote.pdf`.
- The temporary nested `.git` directory was removed after the test with exact-target containment and non-reparse validation; no nested repository remains.

## Deviations from Plan

The live service and harness required operational retries before the immutable successful run. No proof threshold, endpoint assertion, canary rule, ownership rule, or secret-handling rule was weakened.

## Known Stubs

None.

## Self-Check: PASSED

The product commit/tree and evidence commit resolve in Git; the evidence commit changes exactly seven expected paths; the manifest contains six non-self rows; the canonical receipt hash, bytes, file count, zero-hit verdict, oracle, token counts, and D9 result match immutable audit/evidence authority. This summary intentionally records no future summary commit or tree.
