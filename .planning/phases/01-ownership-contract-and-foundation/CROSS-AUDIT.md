# Phase 1 Authority Amendment Cross-Audit

**Date:** 2026-08-27
**Artifact:** `D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md`
**Candidate SHA-256:** Dynamically discovered immediately before the bound audit. Every candidate SHA previously printed here is historical/superseded and cannot be used after signature.

## Reviewer convergence

| Reviewer | Result |
|---|---|
| Ferrox security auditor | Contract threats resolved after revision; planning alignment corrected. |
| Ferrox integration checker | PASS after exact artifact/build boundary correction. |
| Ferrox contract checker | Contract content passes; one live governance blocker remains. |
| External Codex | Contract findings corrected; stale both-stack wording corrected afterward. |
| Gemini CLI | Disqualified: `IneligibleTierError`; not counted as agreement. |

## Corrections made

- Detached final-byte ratification receipt; no impossible embedded self-hash.
- Mandatory signed opaque product subject, byte-identical `principal_id == agent_id`, local project grants, and forced Own scope.
- One Nano admission gate below both Desktop ACP stacks with Nano-only carrier.
- Canonical assertion/admin/receipt cryptography, journal-first replay/idempotency, crash ambiguity and reconciliation.
- Resume safety, legacy filesystem/T2-memory and cron quarantine before enablement, default-off promotion and fail-closed rollback.
- Exact source commit + `Cargo.lock` + Desktop-built executable artifact triple.
- Accurate source signature states, named fixture owner, live governance verification, and strict detached-receipt fields.
- Active roadmap/requirements/context aligned so bypass quarantine occurs in Phase 2 before persistence enablement.

## Remaining blocker

PR #8 cannot yet satisfy the ratified compensated-control model:

- `@FerroxLabs` is the PR author/current CODEOWNER. `@TradeCanyon` is the designated non-author approval/merge account, but collaborator invitation/acceptance is not yet evidenced.
- GitHub does not allow a PR author to approve their own PR.
- `master` currently has no active protection/ruleset.

PR #10 at `f23eed5fe195b76a862f87c4808a35d4a83448aa` is the one-time unprotected bootstrap for all three CODEOWNERS rules. After its exact seven CI pass, TradeCanyon may interactively merge it with explicit bootstrap evidence; then protection must be installed/verified. The separately reviewed fixture-correction PR must land and the P-MEM adversarial-audit fixes must pass before PR #8 proceeds. PR #8 must then synchronize with protected master, preserve the exact landed three-rule CODEOWNERS blob, and rerun all seven CI legs. Only the dynamically discovered post-synchronization head/run may populate regenerated receipts and the bounded audit. Every older PR #8 head/run is historical/superseded. The governance model is exactly `single-human-distinct-account`: `same_human_controller=true`, `independent_human_review=false`; it is separation of account credentials and ceremony, not human judgment. Cross-AI review is adversarial advisory evidence. Owner residual acceptance is mandatory.

## Proof boundary

- Live GitHub/API evidence can prove account, collaborator, reviewer, merger, PR head/base/review commit, CI, CODEOWNERS blob, ruleset, merge commit, and ancestry facts.
- Interactive ceremony, MFA/passkey use, credential custody/inaccessibility to agents or automation, same-human control, residual Sybil/collusion-risk acceptance, and executor non-switch/non-review/non-merge are owner-signed attestations. Verifiers validate their presence, exact binding, hashes, timestamps, and consistency; they do not claim GitHub proves them.

## Verdict

**NOT YET APPROVED.** Remaining sequence: fixture-correction merge and P-MEM audit-fix pass; TradeCanyon invite/accept; PR #10 CI and interactive bootstrap merge receipt; install/verify ruleset; synchronize PR #8 with protected master and exact landed CODEOWNERS; rerun seven CI; dynamically regenerate PR #8 receipts; strict bound-artifact audit; signed residual acceptance; protected TradeCanyon PR #8 review/merge.
