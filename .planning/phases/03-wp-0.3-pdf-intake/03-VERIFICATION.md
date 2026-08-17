---
phase: 03-wp-0.3-pdf-intake
verified: 2026-08-17T20:30:00+07:00
status: passed
score: 6/6 requirements verified
behavior_unverified: 0
overrides_applied: 0
re_verification:
  previous_status: gaps_found
  previous_score: 5/6
  gaps_closed:
    - "F-P2B-4 is authoritatively marked FIXED with local implementation, live proof, audit, D9, and full-gate closure."
    - "03-CONTROL.json durably records the complete just gate-all command, exit 0, commit/tree binding, duration, timestamp, and absent isolated Desktop sink."
  gaps_remaining: []
  regressions: []
---

# Phase 3: WP-0.3 PDF Intake Verification Report

**Phase Goal:** Users can submit a valid PDF, preserve it across resume, and receive model-grounded handling only on a compatible Anthropic Messages leaf; close F-P2B-4 with live Flux, canary, audit, D9, and full-gate evidence.

**Status:** passed
**Re-verification:** Yes — after gap closure commit `7befc8ebe9be9ee971916950ccc2b9d4d7f697d0`

## Goal Achievement

### Roadmap Observable Truths

| # | Truth | Status | Evidence |
|---|---|---|---|
| 1 | Inline/confined-path PDF intake enforces magic, MIME/extension, one-document count, and 20 MiB ceiling with typed refusals. | VERIFIED | `crates/nano-protocol/src/acp.rs` performs strict pre-persistence checks. Initial focused invalid-matrix test passed; exact ceiling, sensitive path, link/reparse, authorize/open swap, and Windows path regressions exist. |
| 2 | Compatible turns emit the exact Anthropic document block; completions leaves refuse before I/O without drop/reroute. | VERIFIED | Exact structural codec test passed. The active binding gates documents on `WireKind::AnthropicMessages`; zero-call refusal and actual-serve dispatch regressions passed. The actual-serve test passed again during re-verification. |
| 3 | Kill/resume rehydrates a digest-verified document through the existing store without changing image behavior. | VERIFIED | Additive `DocumentRef` replay calls `read_verified`, checks digest/length/MIME/magic, and preserves mixed ordering. The focused kill/resume test passed again during re-verification. |
| 4 | Canonical error table contains the typed refusal and generated mirrors match generator output. | VERIFIED | `ModelLacksPdf`/`model_lacks_pdf` source and generated contracts agree; the pinned contract test and isolated mandatory-mirror generator check passed during initial verification. |
| 5 | Canary-clean live Flux evidence proves quoted content/token jump, documents limits, and closes F-P2B-4. | VERIFIED | Exact oracle retained; control 38 tokens vs PDF 13,969, delta 13,931. Six repo/shared pairs rehash byte-identically. External receipt remains SHA-256 `949a38c71320db0506ba9a2b1925d0d44bc993038c22ab15e44e7bf375635c50`, 1,878 bytes, seven files, zero hits. `docs/FOLLOWUPS.md:800` now marks F-P2B-4 FIXED. |

**Score:** 5/5 roadmap truths; 6/6 PDF requirements.

## Requirement Coverage

| Requirement | Status | Actual evidence |
|---|---|---|
| PDF-01 | SATISFIED | Inline and path intake validation plus typed boundary/path regressions. |
| PDF-02 | SATISFIED | Additive `ContentBlock::Document`, `TurnBlock::Document`, and strict digest-only `DocumentRef`; existing image/tool variants remain separate. |
| PDF-03 | SATISFIED | Exact Anthropic shape, canonical `/v1/messages` catalog binding, and pre-network zero-call completions refusal are tested. |
| PDF-04 | SATISFIED | Digest-verified kill/resume and document GC/rejection behavior are implemented and tested. |
| PDF-05 | SATISFIED | Live oracle/token proof, six mirrored payloads, seven-file zero-hit receipt, metering limitation, and authoritative FIXED disposition exist. |
| PDF-06 | SATISFIED | Typed error/count table and mandatory generated mirrors are fresh under exact D9 ownership. |

## Artifact, Wiring, and Data Flow

| Artifact/link | Status | Details |
|---|---|---|
| ACP document → `TurnBlock::Document` | VERIFIED | Validation precedes construction/persistence; failures are bounded and typed. |
| `DocumentRef` journal → attachment store → resumed content | VERIFIED | Journal is digest-only; `read_verified` supplies real bytes and replay rebuilds live base64 content. |
| Active leaf → wire gate → model driver | VERIFIED | Resolved binding controls admission; incompatible wires return `ModelLacksPdf` before driver/network calls. |
| Anthropic content → `/v1/messages` | VERIFIED | Catalog, generated golden, provenance, and exact codec agree. |
| Live fixture → manifest → shared mirror → canary receipt | VERIFIED | Evidence commit `0eb5098426f95ee8d8e33bb4c35d370d399ea6b4` contains exactly seven expected repo paths; manifest has six non-self rows; all current repo/shared hashes and bytes match; receipt excludes itself. |

## Audit, History, D9, and Full Gate

- `03-AUDIT.json` remains valid `wp03_audit_v2`: one audit, one fix round, eight findings, seven final detached recheck commands, final status PASS.
- Canonical product commit `5040293cf4de8467555f4c74b46b34a91d6939d7` and tree `be34bb63f58cacd64bdab3a073f17fa5d4088719` resolve and remain in current history.
- `03-CONTROL.json` remains valid `wp03_control_v1`; D9 closure is PASS with protected nano/resources equality, valid shared delta, and valid evidence pairs.
- New durable full-gate receipt records `just gate-all`, exit `0`, duration `124165ms`, timestamp `2026-08-17T13:14:26.5384816Z`, and `desktop_sink_absent: true`.
- Receipt head `e5dd301c296317f6070f1f7381454d5b1ebd75fe` exists, is an ancestor of current HEAD, and independently resolves to the recorded tree `4c4303f8cd9b39a4bb5d8d3dad33642a4439202d`.
- Gap-closure commit `7befc8ebe9be9ee971916950ccc2b9d4d7f697d0` changes only `03-CONTROL.json`, `03-08-SUMMARY.md`, and `docs/FOLLOWUPS.md`.
- Worktree was clean except for this untracked verification report; `git diff --check` passed. No paid live call was repeated and no key file was read.

## Behavioral Spot-Checks

| Behavior | Result | Status |
|---|---|---|
| Actual serve pinned/auto/compatible PDF dispatch | 1 passed, 0 failed | PASS |
| PDF kill/resume through verified store | 1 passed, 0 failed | PASS |
| Retained live evidence repo/shared equality | 6/6 pairs, hashes and bytes equal | PASS |
| Canary receipt integrity | Exact hash/bytes; seven files; zero hits; PASS | PASS |
| Full gate receipt binding | Real commit; exact tree; ancestor; exit 0 | PASS |

## Anti-Patterns and Notes

No phase-owned product TODO/FIXME/XXX debt marker or user-visible stub was found. No secret value appears in retained evidence according to the current exact-list receipt.

Non-blocking documentation note: `03-08-SUMMARY.md` now echoes the durable gate receipt, but two older narrative lines still say F-P2B-4 is pending live closure. The authoritative `docs/FOLLOWUPS.md` entry and current control receipt supersede those stale summary statements; summaries are not verification authority.

## Gaps Summary

No remaining phase-goal gaps. Merge, push, CI, and final branch promotion remain later promotion operations explicitly outside this local phase verification and are not claimed here.

---

_Verified: 2026-08-17T20:30:00+07:00_
_Verifier: ferrox-verifier_
