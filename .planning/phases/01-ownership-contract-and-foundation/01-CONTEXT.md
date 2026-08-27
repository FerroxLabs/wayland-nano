# Phase 1 Context: Ownership Contract and Foundation

**Source:** Owner-approved revised roadmap, ADR-001, and governing source snapshots.

## Goal

Ratify the Desktop-control-plane/Nano-kernel authority and compatibility boundary, human-merge PR #8, and freshly verify the landed P-MEM-1 foundation. Stop before Phase 2 implementation.

## Locked Decisions

- D-01: Desktop is the sole product control plane for bot identity/CRUD, persona, team, backend/model selection, scheduling, approvals, and UI.
- D-02: Nano is the security, continuity, and bounded-execution kernel. Minimal trust-root and anti-remap enrollment state is permitted; a second product registry is forbidden.
- D-03: Wire `principal_id` is a 1:1 semantic alias of immutable physical/schema/journal `agent_id` in v1.1. No storage or journal rename occurs.
- D-04: Nano verifies a trusted issuer assertion, not the real-world truth of Desktop's bot-to-principal mapping. Ratification pins issuer provisioning, rotation, overlap, revocation, recovery, immutable binding, and reassignment refusal.
- D-05: Direct CLI admission uses a locally enrolled issuer and explicit `main` compatibility principal under the same trust, replay, and identity rules.
- D-06: Both Desktop ACP stacks—the legacy `AcpConnection` path and newer `AcpRuntime`/`AcpSession` path—converge through one shared Nano admission gate using the Nano-only `_meta.waylandNanoActivation` carrier; carrier/version/downgrade/replay behavior is pinned and only fields with a current enforcement consumer are bound.
- D-07: PR #10 is the one-time evidenced unprotected CODEOWNERS bootstrap, interactively merged by TradeCanyon only after exact CI. Protection is installed immediately afterward. The separately reviewed fixture correction and every P-MEM audit fix must land/pass; PR #8 is then synchronized with protected master, preserves the exact landed three-rule CODEOWNERS blob, and reruns all seven CI legs before evidence/audit. PR #8 uses the honest `single-human-distinct-account` compensated-control model: FerroxLabs and TradeCanyon are distinct GitHub accounts controlled by the same human owner; `same_human_controller=true`, `independent_human_review=false`; this is separation of account credentials and ceremony, not separation of human judgment. Cross-AI review is adversarial advisory evidence, not human review. Executor never switches account, reviews, merges, bypasses, or pushes tags.
- D-08: Existing hash-verified source snapshots remain byte-identical. Ratification uses a new versioned amendment/manifest with an explicit precedence table; it does not silently rewrite history.
- D-09: Only MEMORY-CONTRACT v1.2 is represented as owner-signed. NANO-PROGRAM-PLAN is governing/pending sign-off; PROFILES-CONTRACT and NANO-MODULE-CONTRACT are unsigned drafts.
- D-10: The MEM-SEC fixture owner, exact Nano artifact SHA, cross-repo merge order, compatibility window, and exit criteria are explicit before Phase 2.

## Success Criteria

- REQ-FOUND-01: PR #8 is human-merged and a fresh checkout reproduces recall@10 >= 0.90, zero cross-project/agent leakage, query-equivalent kill/rebuild including identity, mediated writes, and seven green CI legs.
- REQ-ARCH-01: A signed/versioned owner amendment enumerates every source artifact/version/signature/disposition/precedence and pins the complete authority, identity, issuer, fixture, carrier, merge-order, and compatibility contract.

## Scope Fence

- No Phase 2 implementation.
- No Desktop code changes from this repository/worktree.
- No browser/desktop providers, compaction, procedure extraction, graph/KG, cross-project reads, refactors, upgrades, or cleanup.
- No `.secrets` access. Flux remains path-reference-only through `FLUX_API_KEY_FILE`.
- Three strikes per repeated failure; on stop, write an exact continuation handoff.

## Execution Notes

- Planning happens on `plan/persistent-agent-program`; implementation evidence for P-MEM-1 is PR #8 at the head dynamically discovered only after the fixture correction, audit fixes, PR #10, protected-master synchronization, and seven-leg rerun. Every printed candidate/head/run SHA in historical summaries is superseded and non-authorizing.
- GitHub live evidence proves repository/account facts only: account identity, collaborator/reviewer/merger, PR head/base/review commit, CI, CODEOWNERS blob, ruleset, merge, and ancestry. The owner's interactive ceremony, MFA/passkey use, credential custody/inaccessibility to agents, same-human status, residual-risk acceptance, and executor nonparticipation are signed attestations checked for binding/consistency; no verifier may describe them as GitHub-proven.
- Contract amendment may be prepared under the authorized `../shared` planning/review surface, but owner signature remains a human checkpoint.
- Desktop implementation needs a separately authorized Desktop worktree/branch/PR in Phase 2 and is not part of this phase.
