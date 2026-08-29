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

PR #10 and fixture PR #11 were owner-directed agent-operated bootstrap merges through TradeCanyon before protection was installed. They are exactly attributed exceptions, not human-interactive or independent review. Classic branch protection was then installed. PR #8 synchronized with protected master, preserved the three-rule CODEOWNERS blob, and reran seven CI legs. Only its final synchronized head/run may populate receipts and the bounded audit; every older head/run is historical.

## Proof boundary

- Live GitHub/API evidence can prove account, collaborator, reviewer, merger, PR head/base/review commit, CI, CODEOWNERS blob, ruleset, merge commit, and ancestry facts.
- Receipts require `owner_directed_agent_operated_review=true`, `executor_did_switch_review_merge=true`, same-human control, residual-risk acceptance, and exact account-action attribution. Superseded interactive/MFA/credential-isolation/executor-nonparticipation claims are rejected.

## Verdict

**IN PROGRESS.** PR #10/#11 and P-MEM corrective work landed; classic branch protection is installed; PR #8 is synchronized at its exact final head with local acceptance green. Remaining sequence: final seven-leg CI; exact receipts; bounded disposition audit; signed owner override; protected owner-directed TradeCanyon PR #8 review/merge; fresh-master verification.
