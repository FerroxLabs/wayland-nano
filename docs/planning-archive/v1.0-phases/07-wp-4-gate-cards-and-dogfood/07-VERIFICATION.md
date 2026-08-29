---
phase: 07-wp-4-gate-cards-and-dogfood
verified: 2026-08-21T00:00:00+07:00
status: gaps_found
score: 4/5 roadmap must-haves verified
behavior_unverified: 0
overrides_applied: 0
gaps:
  - truth: "Dogfood CI exercises the cards through WP-3 and demonstrates bad blocked and good accepted"
    status: partial
    reason: "Exact-product local WP-3-only dogfood is proved, but the integrator-owned gate-cards workflow job has not yet been added, landed, or run."
    artifacts:
      - path: ".github/workflows/gate.yml"
        issue: "Builder correctly left this file unchanged; the requested top-level gate-cards job is pending integrator ownership."
    missing:
      - "Integrator-only gate-cards workflow commit and post-merge rerun"
  - truth: "Owner receives final canary-clean, exact-SHA CI-backed WP-4 evidence and autonomous execution stops"
    status: failed
    reason: "WP-4 has not been no-ff merged, pushed/fetched, or confirmed green in the seven required CI jobs, so the external final evidence is intentionally absent."
    artifacts:
      - path: "F:/Development/waylandnano/shared/reviews/verified-change/WP4-FINAL-EVIDENCE.md"
        issue: "Not present; plan 07-09 forbids creation before exact-SHA CI succeeds."
    missing:
      - "Detached no-ff merge of feat/wp-4"
      - "Integration just gate-all and WP-3 good/bad dogfood rerun"
      - "Push/fetch equality and exact-SHA seven-job CI success"
      - "Final canary scan and WP4-FINAL-EVIDENCE.md owner handoff"
---

# Phase 7: WP-4 Gate Cards and Dogfood Verification Report

**Phase Goal:** Operators can use three sealed, mutant-proven Gate Card packs through the WP-3 verifier, block deliberately bad changes, accept good changes, and hand a fully evidenced build state to the owner.
**Verified:** 2026-08-21
**Status:** gaps_found — builder product is promotion-ready; phase landing/final evidence remain integrator-owned
**Re-verification:** No — initial verification

## Verification Boundary

The submitted clean branch tip is `41f35e80863e77dcc24b8e7518cd833352e08517`. The audited product is `42d2417e1b053ea8c06be5504670267892fcc8c8` with tree `d458e6df443a27c8e3517dd53a7cfc0caa0db841`. The branch history keeps the audited product, builder evidence, handoff, and request in that order. `07-PROMOTION-REQUEST.json` requests integration but truthfully records no `.github` modification, merge, push, or CI result.

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|---|---|---|
| 1 | All three packs resolve from canonical registry closures and run through WP-3 `verify --gate ... --run-only`. | VERIFIED | `gates/registry.json` has exactly `install-payload`, `provision-script`, and `config-schema`; registry closure tests passed independently. The dogfood evidence records the exact WP-3 argv and the validator independently executes it against a detached exact-product worktree. |
| 2 | Every reference scores 6/6; at least five fluent mutants per pack are documented and all are caught; protocol/meta defects fail closed. | VERIFIED | The three cards each declare six checks and six mutants (`ip-m1..6`, `pv-m1..6`, `cf-m1..6`), exactly 18 total. Builder evidence fixes the exact 13-name battery and seeds 41041/41042/41043. Shared named schema/seal/meta/writer tests passed in this verification; the controlled validator executes the complete battery from the audited product. |
| 3 | Packaging, provision no-mutation/idempotence, and config/catalog strictness are verified without producer mutation and with cleanup. | VERIFIED | Exact-product audit and dogfood validators enforce producer diffs empty, detached worktree identity, external provision state equality, cleanup attempts plus post-enumeration, and exact five-file owner deviations. No Phase-7 diff exists under `packaging/**` or `scripts/provision/**`. |
| 4 | Dogfood blocks prescribed bad arms and accepts good arms through WP-3, with provenance. | PARTIAL / BLOCKER | Local exact-product evidence proves all three good arms Green and `ip-m1`, `pv-m2`, `cf-m3` Red through WP-3 only; `UPSTREAM.md` provenance validates. The CI half of the roadmap criterion is pending integrator promotion. |
| 5 | Owner receives final canary-clean evidence with merge/gates/CI and explicit stop. | FAILED / BLOCKER | The request correctly leaves merge SHA, run ID, workflow SHA, and CI pending. No final external evidence exists yet. |

**Score:** 4/5 roadmap truths materially implemented; 3/5 fully verified, 1/5 locally verified but awaiting CI landing, 1/5 pending integration/finalization.

### Required Artifacts and Wiring

| Artifact | Status | Details |
|---|---|---|
| `gates/lib/{card,contract,dirhash,artifact-writer}.cjs` | VERIFIED | Substantive, imported by pack/generator/test/evidence paths, and covered by fail-closed tests. |
| `gates/lib/atomic-replace-win32.ps1` | VERIFIED | Uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`; no copy/pre-delete fallback. Real PowerShell 5.1 junction smoke is bound by the audit. |
| Three cards, gates, generators, and sealed corpora | VERIFIED | Closed cards each expose six checks, six documented mutants, pins, seals, known gaming modes, and rotation `k=2`. |
| `gates/registry.json` | VERIFIED | Closed three-gate registry with canonical closures/digests and CARD-05..07 mappings; consumed by WP-3. |
| `gates/tests/validate-seeded.cjs` | VERIFIED | Binds exact seed, base/cards/gates/fixtures/selections/observations/cleanup; three requested seeds are fixed in controlled evidence. |
| `07-DOGFOOD-EVIDENCE.json` | VERIFIED | Product/registry bound, exact three good plus prescribed three bad arms, identifiers-only JSON results, cleanup claims independently re-executed by validator. |
| `07-REVIEW.{md,json}` and `07-FIX-RECHECK.md` | VERIFIED | Exact product/tree/diff binding, independent roles, one fix round, computed zero open Critical/High. |
| `07-BUILDER-EVIDENCE.json` and `07-PROMOTION-REQUEST.json` | VERIFIED | Hash-linked request-only handoff with exact nine-command inventory, 13 names, three seeds, seven requested CI jobs, and no authority overclaim. |
| `.github/workflows/gate.yml` WP-4 job | MISSING BY OWNERSHIP / BLOCKER | Correctly absent from builder diff; plan assigns it to integrator after builder verification. |
| `WP4-FINAL-EVIDENCE.md` | MISSING / BLOCKER | Correctly deferred until merge, push/fetch, exact-SHA CI, and final canary succeed. |

### Key Link Verification

| From | To | Via | Status |
|---|---|---|---|
| Cards | WP-3 verifier | canonical `gates/registry.json` closures | VERIFIED |
| Pack generators | persistent fixtures/cards | shared `artifact-writer.cjs` | VERIFIED by source/test coverage and audit inventory |
| Config mutants | shipped CLI | detached exact-base worktrees and private targets | VERIFIED |
| Dogfood evidence | audited product | detached-product validator plus commit/tree and registry digest checks | VERIFIED |
| Builder evidence | promotion request | SHA-256 links and request-tip ancestry | VERIFIED |
| Promotion request | master/CI/final evidence | integrator-only plan 07-09 | NOT YET WIRED |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|---|---|---|---|
| Shared cards, registry, seals, protocol and atomic writer | `node --test gates/tests/*.test.cjs` (observed initial shared tranche) | 12 named/shared/writer tests passed before the long authentic pack tranche; run stopped to avoid duplicating the already controlled multi-hour battery | PASS (focused) |
| Evidence anti-spoofing and cleanup | `node --test gates/tests/validate-evidence-adversarial.test.cjs` | 11/11 passed in 31.9 s | PASS |
| Audit identity/diff/deviation binding | `node gates/tests/validate-evidence.cjs audit .../07-REVIEW.json .../07-FIX-RECHECK.md` | validator accepts exact audit binding | PASS |
| Provenance completeness | `node gates/tests/validate-evidence.cjs provenance UPSTREAM.md` | validator accepts exact owned inventory | PASS |

The controlled builder validator was inspected before relying on its retained handoff. It creates a detached `F:\\w4e-product` at the asserted product SHA, checks HEAD/tree identity, executes the exact build, complete Node battery, seeds 41041–41043, independent dogfood, provenance, `just gate-all`, and `cargo deny check`, enforces output bounds and the 13 names exactly once, runs the canary, then removes tools/worktree registrations and residue. Adversarial tests cover forged dogfood, claimed cleanup, mutable-product substitution, junction identity/removal, registration normalization, audit drift, authority spoofing, CI job/SHA spoofing, and final-stop spoofing.

### Requirements Coverage

| Requirement | Status | Evidence |
|---|---|---|
| CARD-01 | SATISFIED | Canonical three-gate registry closures, digests, run artifacts, and mappings. |
| CARD-02 | SATISFIED | Closed cards, pins, invocation/seal/validation/gaming fields; drift tests pass. |
| CARD-03 | SATISFIED | Exactly 18 documented mutants; exhaustive plus three seeded rotations retained. |
| CARD-04 | SATISFIED | Three 6/6 references and fail-closed seal/protocol/meta behavior. |
| CARD-05 | SATISFIED | Authentic staged npm payload and helpers; packaging producers unchanged. |
| CARD-06 | SATISFIED | Real dry-run serializer plus live non-elevated refusal/state-equality oracle; producer unchanged. |
| CARD-07 | SATISFIED | Public rules CLI/catalog checks, sealed patches, detached-worktree cleanup. |
| CARD-08 | PARTIAL / BLOCKER | Local WP-3 good/bad dogfood is proved; CI `gate-cards` job is not yet promoted or run. |
| PROV-03 | SATISFIED | Provenance validator accepts one exact row for every adapted owned file. |
| EVID-01 | PARTIAL / BLOCKER | Builder branch/request are exact; WP-4 merge, integration gate, push/fetch, and CI link are pending. |
| EVID-02 | FAILED / BLOCKER | Final shared evidence must not be written until exact-SHA CI succeeds. |
| EVID-03 | PARTIAL | No WP-5/WP-6/Phase-8 or `.github` builder diff exists, but terminal owner handoff awaits plan 07-09. |

### Anti-Patterns and Disconfirmation Pass

No unreferenced `TBD`, `FIXME`, or `XXX` debt marker was found in Phase-7 product paths (the `mktemp ... XXXXXX` template is not a debt marker). No forbidden Phase-8/WP-5/WP-6 or `.github` path appears in the builder diff. The only `crates/**` changes are the five exact paths recorded as two owner deviations.

Disconfirmation findings: the local dogfood proof is not CI proof; a passing card unit test alone would not establish WP-3 wiring, so the independently executing dogfood validator is required; and the builder branch cannot prove final evidence because those facts do not yet exist. These are reflected above rather than softened into a pass.

## Gaps Summary and Pending Integrator Actions

The audited WP-4 product and builder handoff are ready for promotion, with zero open Critical/High findings. The full Phase-7 goal is not yet achieved because plan 07-09 remains intentionally outside builder authority. The integrator must:

1. Independently accept this verification and create the dedicated `.github/workflows/gate.yml` `gate-cards` promotion commit.
2. Merge `feat/wp-4` through detached `.tmp-wt-integ` using `--no-ff`.
3. Rerun the complete integration `just gate-all` and WP-3 good/bad dogfood checks.
4. Push `HEAD:master`, verify fetch equality, and require all six platform jobs plus `gate-cards` Green at the exact merge SHA.
5. Run the final canary and write `F:/Development/waylandnano/shared/reviews/verified-change/WP4-FINAL-EVIDENCE.md`, explicitly recording WP-0.1, WP-5, and WP-6 unexecuted, then stop.

---

_Verifier: independent ferrox-verifier_

