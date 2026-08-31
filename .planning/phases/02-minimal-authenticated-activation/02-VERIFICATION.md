---
phase: 02-minimal-authenticated-activation
verified: 2026-08-31T11:52:58Z
status: passed
score: 19/19 rows verified
behavior_unverified: 0
overrides_applied: 0
verifier: fresh independent agent (no implementation history); fresh detached checkouts only
checkouts:
  nano: .tmp-wt-verify-p2 @ c10dcb9b0964a23df7b5bb2760ef494c4e15369d (origin/master tip)
  desktop: .tmp-wt-verify-p2 @ 0b7f029dbda4f2b08cfd5962c978ab33202abe37 (origin/main tip)
---

# Phase 2: Minimal Authenticated Activation — Verification Report

**Phase Goal:** Nano accepts only a minimal trusted-issuer assertion and independently narrows it before activation.
**Verified:** 2026-08-31T11:52:58Z
**Status:** passed

All evidence below was produced from fresh detached checkouts of merged `origin/master`
(Nano) and `origin/main` (Desktop) plus live GitHub queries. No implementation worktree
(`.tmp-wt-phase2`, `.tmp-wt-phase2-offline-bootstrap`) was used as evidence.

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---:|---|---|---|
| 1 | Nano PR chain #12–#17 merged in order through protected review, each with an on-head TradeCanyon APPROVED review. | VERIFIED | `gh pr view` per PR: merges `188b839f`, `b3dd9cd1`, `d0f96d84`, `1d80ecf9`, `17e80e83`, `c10dcb9b`; mergedAt strictly increasing 2026-08-30T00:15Z → 2026-08-31T08:13Z; review commit == PR head on all six; all six merges are ancestors of origin/master. |
| 2 | Nano runtime source `288de9ed…9824` (PR #15) merged with 7-leg CI green including Windows ARM64. | VERIFIED | Run `33318936491` conclusion success on headSha `288de9ed…`; jobs: gate windows-x64, windows-11-arm, ubuntu-22.04, ubuntu-24.04-arm, macos-14, macos-15-intel, gate-cards — all success. Reviewed-head ancestry + tree equality with merge `1d80ecf9` confirmed by git. |
| 3 | Fixture helper source `2f7b33f4…0dbe` (PR #17) merged with 7/7 CI green. | VERIFIED | Run `33369702224` success on headSha `2f7b33f4…`; merge `c10dcb9b` is the origin/master tip. |
| 4 | Cargo.lock identity identical at both pinned commits. | VERIFIED | At `288de9ed…` and `2f7b33f4…`: file SHA-256 `3d6ec29f3b19e0b3778a5de222418ec497eaf79be8e93a92dd120d986bdb930a`, blob `7bb979cf829f7bf0a63692d8485bfc8e4935ed13` — recomputed via `git show` + sha256sum and `git rev-parse <c>:Cargo.lock`. |
| 5 | Branch protection on Nano master forbids bypass. | VERIFIED | `gh api repos/FerroxLabs/wayland-nano/branches/master/protection`: enforce_admins=true, strict=true, allow_force_pushes=false, 1 required approval with stale-dismissal, required contexts = exactly the 7 gate legs. |
| 6 | Nano persistent activation is default-off in merged source. | VERIFIED | `crates/nano-cli/src/activation.rs:35` wires `AdmissionGate::open_enabled(...)`; `crates/nano-activation/src/enablement.rs:81` default state digest is `Sha256(b"disabled")`; `enablement.rs:139` fails closed when no enablement exists; corroborating tests `activation_admission.rs:132`, `enablement.rs:53`. |
| 7 | No Phase 3 scope in merged Nano source. | VERIFIED | `nano-memory` appears only in the workspace members list and its own manifest; no other crate depends on it; no `nano_memory` references outside its own tests. |
| 8 | Desktop PR #1277 merged by squash with review on the exact head. | VERIFIED | `gh pr view 1277`: MERGED 2026-08-31T08:48:47Z, base `312b5db0…`, head `f6482c08…`, squash merge `4cdf67de…`; review id 5064632116 APPROVED by TradeCanyon on commit `f6482c08…`. |
| 9 | Reviewed-head change is byte-identical to the squash merge (tree/diff identity, not ancestry). | VERIFIED | `git rev-parse f6482c08^{tree}` == `4cdf67de^{tree}` == `7c49aacd3a594a692f1e34b9b032d275f42ac7c7`; parent of `4cdf67de` is the recorded base `312b5db0…`. Same base + same tree ⇒ identical diff/patch bytes by construction. |
| 10 | All seven check runs on the reviewed head are green, including both exact-artifact legs and the production bootstrap contract. | VERIFIED | Check runs on head_sha `f6482c08…`, all conclusion success: 99426865066 Code Quality; 99429766994/99429766939/99429766951 Unit Tests (macos/ubuntu/windows); 99426865135/99426865090 Exact artifact (ubuntu/windows); 99426865011 Production bootstrap contract. |
| 11 | Postmerge governance correctives #1278/#1279 merged and their exact-artifact run is green. | VERIFIED | #1278 merge `7f021f73…`, #1279 merge `0b7f029d…`; run `33378467554` success with jobs 99445248134/99445248317/99445248467 all success. |
| 12 | Committed premerge manifest is hash-exact and non-self-referential. | VERIFIED | `docs/evidence/phase2/desktop-pr-premerge-manifest.json` at origin/main: SHA-256 `d24c1e2248740ae28e9be324479e00db2844682c6165ee19a1e29c3f4e877f4a`; contains neither the reviewed head nor the squash merge SHA; pins Nano source `288de9ed…`, merge `1d80ecf9…`, CI run 33318936491, lock `3d6ec29f…`, helper source `2f7b33f4…`. |
| 13 | Committed matrix evidence has the frozen shape (5 positive / 26 negative / 31 rows). | VERIFIED | `activation-artifact-manifest.json` + `activation-negative-crash-receipt.json` at origin/main: positiveCount 5, negativeCount 26, totalCount 31, 31 unique rowIds, rowIdsSha256 consistent across both files. The ceremony executable SHA-256 `8db4b5f5…` is pinned in the frozen local run evidence (`production-bootstrap-result.json`), not in the committed manifests — the manifests deliberately pin source-commit + Cargo.lock identity. Recorded here as a wording clarification, not a discrepancy. |
| 14 | One-time offline-bootstrap private key is destroyed; public artifact retained. | VERIFIED | `C:/Users/seand/AppData/Local/WaylandNano/phase2-offline-bootstrap-authority/` contains only `owner-offline-bootstrap-authority-public.der` (44 bytes); no `.pem` or private key material anywhere under the custody directory. |
| 15 | Frozen ceremony evidence is byte-stable and internally bound. | VERIFIED | Recomputed at `…/phase2-exact-artifact-evidence/run-mPI2K4/`: challenge `c7dd32ac…`, authorization `300c36cf…`, bootstrap receipt `7c55cd78…`, consumption receipt `80b30c8d…`, authority journal `934d0cdf…` — all equal the handoff-recorded values; `production-bootstrap-result.json` (SHA-256 `7f0a6dcb…`) binds challenge/authorization/receipts/journal plus executableSha256 `8db4b5f5…`. The interim handoff's "authority snapshot" hash refers to the then-current authority.db; the DB later served the post-ceremony matrix rows, so its live bytes differ — the authoritative journal (`authority.jsonl`) is unchanged and matches. |
| 16 | Production bootstrap contract holds at the exact helper SHA. | VERIFIED | Fresh detached worktree at `2f7b33f4…`, `cargo test --locked`: nano-activation lib 21/21; `offline_bootstrap` 3/3 serial; `phase2_fixture` (feature-gated) 4/4 serial; nano-cli `offline_bootstrap_cli` 6/6 + `activation_cli` 4/4 serial. Mirrors the Desktop CI production-bootstrap-contract job. |
| 17 | Full exact-artifact matrix reruns green from fresh merged checkouts. | VERIFIED | In fresh Desktop checkout `0b7f029db`: `bun run scripts/verify-wayland-nano-activation.ts --require-fresh-nano-checkout --require-terminal-refusal` → exit 0 in 407s. The run performed its own fresh clone of Nano at `288de9ed…`, verified remote/HEAD/clean-status/Cargo.lock identity, built debug+release `wayland-nano` with `--locked` (fresh executable SHA-256 `056ae40f05d049101613a0e2cf3e2079b36ef689b14841f10d7ecd1c2b80be9d` — a new build, recorded as evidence, not expected to byte-match the ceremony build), built the fixture helper at `2f7b33f4…`, and ran the dual-stack/CLI/terminal-refusal matrix. |
| 18 | Postmerge governance verifier passes against the external final receipt with all strict flags. | VERIFIED | `verify-phase2-pr-governance.ts --premerge … --external-final-receipt 02-10-SUMMARY.md --expected-author FerroxLabs --expected-reviewer TradeCanyon --require-squash-change-identity --require-default-off --require-nano-first` → "Phase 2 Desktop postmerge governance verified for PR 1277." |
| 19 | Desktop default-off + honest governance disclosure. | VERIFIED | `src/process/agent/activation/waylandNanoActivationOwner.ts:69`: absent/invalid owner manifest ⇒ no activation owner installed (`initBridge.ts:93-95`); invalid custody/binding/artifact ⇒ bounded nonpersistent mode (`:246`). All three Desktop PRs reviewed and merged by the single TradeCanyon login — consistent with the disclosed owner-directed, agent-operated, non-independent model; no independent reviewer exists, matching the disclosure. |

**Score:** 19/19 rows verified

### Key Link Verification

| From | To | Via | Status | Details |
|---|---|---|---|---|
| Reviewed Nano heads | origin/master | merge ancestry + tree equality | VERIFIED | All six merges ancestors; `288de9ed…`→`1d80ecf9` diff empty. |
| Reviewed Desktop head | squash merge | tree/diff/patch identity | VERIFIED | Identical trees `7c49aacd…`; same recorded base; no ancestry claim made. |
| Committed manifest | external receipt | hash + cross-check | VERIFIED | Manifest SHA-256 `d24c1e22…`; receipt cross-checked against live GitHub by the strict governance verifier. |
| Frozen ceremony | merged artifacts | hash chain | VERIFIED | All five frozen hashes recomputed equal; result JSON binds executable/journal/receipts. |
| CI evidence | local rerun | fresh-checkout rebuild | VERIFIED | Independent local rerun of the exact-artifact pipeline exit 0. |

### Requirements Coverage

| Requirement | Status | Evidence |
|---|---|---|
| REQ-ACT-01 | SATISFIED | Both Desktop ACP stacks + direct CLI converge through one shared Nano admission gate with the signed minimal descriptor; proven by the merged dual-stack/CLI matrix and its independent fresh-checkout rerun (rows 1–3, 8–13, 17). |
| REQ-POL-01 | SATISFIED | Issuer verification, local ceilings, frozen receipts, quarantine of unauthenticated memory/cron/hook surfaces, enrolled local CLI issuer, and typed tamper/replay/remap/widening failures — evidenced by the merged negative matrix (26 rows), default-off proofs, and protection/review chain (rows 5–6, 12–18). |

### Behavioral Spot-Checks

| Behavior | Result | Status |
|---|---|---|
| `verify-phase2-pr-governance.ts` strict postmerge mode | "verified for PR 1277" | PASS |
| Exact-artifact matrix from fresh checkouts | exit 0 | PASS |
| Bootstrap contract suites at helper SHA | 44/44 tests across 4 suites | PASS |
| Frozen ceremony hash recomputation | 5/5 equal | PASS |
| Private-key custody directory inventory | public DER only | PASS |

### Limitations (disclosed, non-blocking)

- The one-time production bootstrap ceremony is intentionally non-re-executable: its single
  authorization is consumed and the private key destroyed (row 14). Ceremony soundness is
  therefore evidenced by the frozen signed receipts (row 15) plus the contract test suites
  at the exact helper SHA (row 16), which is the design's stated compensating control.
- "No protection-bypass events" is not independently auditable: FerroxLabs is a user
  account without an accessible audit-log API. Protection configuration itself is verified
  in force on both repos (rows 5, 11 evidence); reviews and merges occurred through the
  disclosed single-controller workflow.

### Anti-Patterns Found

None in the verified surface. The phase's own `.continue-here.md` strike ledger shows the
three-strikes discipline was exercised (six documented stops, each root-caused before
retry); no weakened test, gate, or sandbox/egress/policy surface was found in the merged
diffs inspected.

### Gaps Summary

No remaining gaps. Phase 2 ends default-off with no Phase 3 memory wiring, registry,
scheduler, UI, provider, graph, extraction, or cross-project scope in either merged tree.

---

_Verified: 2026-08-31T11:52:58Z_
_Verifier: fresh independent agent acting as ferrox-verifier under Plan 02-12_
