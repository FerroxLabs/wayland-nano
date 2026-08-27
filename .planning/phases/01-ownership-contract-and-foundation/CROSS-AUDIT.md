# Phase 1 Authority Amendment Cross-Audit

**Date:** 2026-08-27
**Artifact:** `D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md`
**Candidate SHA-256:** `A541BE1D80EC0E85855FA4E6C8D6004B825A14BC459630EA784B32A084275FCC` (informational; changes on signature)

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

PR #8 cannot currently satisfy independent CODEOWNER approval:

- `@FerroxLabs` is the PR author, only collaborator, and only CODEOWNER.
- GitHub does not allow a PR author to approve their own PR.
- `master` currently has no active protection/ruleset.

Before approval/merge, an actual independent human or team must be granted appropriate repository access and added to the relevant `CODEOWNERS` entries. That changes PR #8 head, so the immutable head, CI run, and Plan 01-02 evidence must then be regenerated. No reviewer identity or access grant is inferred by the agent.

## Verdict

**NOT YET APPROVED.** The amendment is ready for owner ratification on content, but Phase 1 remains blocked on establishing real independent server-side review governance for PR #8.
