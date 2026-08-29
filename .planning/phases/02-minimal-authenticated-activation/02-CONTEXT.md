# Phase 2 Context: Minimal Authenticated Activation

**Authority:** Signed `WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md`, ROADMAP Phase 2, REQ-ACT-01, REQ-POL-01, Phase 1 verification, Phase 2 research and pattern map.

## Goal

Establish the smallest fail-closed authenticated activation boundary shared by both Desktop ACP stacks and direct CLI, with canonical receipts and legacy persistence quarantine. Stop before Phase 3 memory-runtime integration.

## Locked Decisions

- D2-01: Authentication starts on raw inbound ACP bytes before `serde_json::Value`, request routing, session creation/load, journaling, memory, tools or effects. Duplicate-key and noncanonical evidence must remain observable.
- D2-02: The Nano-only carrier is `_meta.waylandNanoActivation`, schema `wayland.nano.activation/v1`; both legacy `AcpConnection` and newer `AcpRuntime`/`AcpSession` converge through one Nano admission gate.
- D2-03: Direct CLI uses the same admission gate through a separately enrolled local issuer and explicit `main` subject/principal; it is not a bypass.
- D2-04: One Nano trusted constructor/library owns JCS validation, Ed25519 verification, issuer/project/subject/principal grants, replay/idempotency state, policy intersection, controls and receipts. Existing NFC-normalizing canonical JSON helpers are forbidden for this protocol.
- D2-05: `principal_id` equals physical/journal `agent_id` byte-for-byte under existing grammar. `product_subject_id` is mandatory, opaque, signed and immutably bound. Asserted project is accepted only with an active Nano-local grant.
- D2-06: Runtime agent scope is `AgentScope::Own`; no cross-agent/project/global read is introduced. Phase 2 does not wire T2 recall into runtime.
- D2-07: Persistent activation is default-off. Before enablement, every unauthenticated filesystem/T2-memory route and Nano cron/routine firing or model-visible scheduler surface is quarantined. Existing jobs never auto-fire or auto-migrate; legacy journal vocabulary stays replay-readable.
- D2-08: Admission/replay intent is journal-first and crash-rebuildable. Exact replay returns stored receipt without redispatch; changed immutable content conflicts; ambiguous external effects become durable `unknown_outcome` requiring reconciliation.
- D2-09: Canonical activation/control receipts are offline-verifiable and bind assertion, identity/scope, effective authority, epochs, build identity, decision/result and journal positions.
- D2-10: Nano lands first. Desktop implementation uses a separately authorized worktree/branch/PR and builds the exact Nano source commit + Cargo.lock pair, recording executable SHA. Both ACP stacks test the resulting triple.
- D2-11: Desktop product IDs such as `agentId`, `presetAssistantId`, `customAgentId` and conversation ID are not authority principals. Desktop owns a separate durable opaque product-subject→principal binding.
- D2-12: No Nano product registry, persona/team/model catalog, scheduler, UI, provider, Phase 3 memory integration, graph/KG, extraction or cross-project scope.
- D2-13: Worktree identity is a gate, not executor discretion. Nano implementation uses only `D:/Development/waylandnano/wayland-nano/.tmp-wt-phase2` on `feat/p2-minimal-authenticated-activation`, created after planning PR #9 merges from the then-current exact `origin/master` SHA. Desktop implementation uses only `D:/Development/waylandnano/desktop/.tmp-wt-phase2` on `feat/nano-activation-boundary` from an owner-authorized exact base branch/SHA. Every receipt records path, branch, base and repository identity; primary/dirty worktrees are never implementation targets.
- D2-14: Desktop binding authority is a dedicated main-process, owner-provisioned, atomic owner-only file under Electron `userData/wayland-nano/activation-bindings.json`. It contains explicit opaque `product_subject_id`, `principal_id`, `project_id`, issuer/key reference and immutable retirement tombstones. No conversation, assistant, backend, cwd, persona or display field is a source or fallback. Desktop owns this product binding; Nano independently owns its enrollment/grant mirror.
- D2-15: Nano production admin-root, receipt-signer and local-CLI issuer providers use explicit owner-only key-reference files, never environment/project files. Unix requires regular non-symlink file, owning effective UID, mode 0600, safe parents and same-device open/fstat. Windows requires regular non-reparse file, canonical local path, owner=current SID and DACL granting only that SID/SYSTEM/Administrators, with no inherited broad write. Bootstrap proves attached controlling TTY plus effective owner. Desktop issuer custody accepts only Electron `safeStorage` `enc:v1:` on Windows DPAPI, macOS Keychain, or Linux non-`basic_text` Secret Service; it rejects `fenc:v1:`, legacy prefixes and unavailable OS storage without changing the global wrapper.
- D2-16: `protocol-host` uses the same separately enrolled local CLI issuer and shared admission gate before its journal; it has no unauthenticated durable mode. Old `exec` compatibility is process-local/in-memory only, cannot resume, and leaves Nano home unchanged. Activation nonce uniqueness is durable across all activation/idempotency variants, concurrency, crash/rebuild and bounded tombstone retention. Cancel/pause uses signed `_meta.waylandNanoControl`, schema `wayland.nano.control/v1`, checked by the raw reader before flags/races and emitted by both Desktop stacks.
- D2-17: Persistent activation remains disabled unless an authenticated local-admin journal operation explicitly enables an exact Nano `(source_commit_sha,Cargo.lock_sha256,executable_sha256)` for a bounded compatibility-window expiry and authority epochs. Missing, expired, revoked, drifted, rolled-back or crash-ambiguous enablement refuses typed. There is no environment/config toggle; tests explicitly journal enablement in temporary homes.
- D2-18: Nano build identity is compiled from the clean checkout commit plus build-time SHA-256 of the exact workspace `Cargo.lock`, verified by the artifact runner rather than trusted from runtime input. Desktop resolves the Nano executable to an absolute canonical file, verifies SHA-256 and file identity against the signed/committed manifest immediately before spawn, holds a nonreplaceable/open identity where supported, and rechecks identity across spawn. PATH, symlink/reparse, replacement/TOCTOU and stale-manifest variants fail before process creation.
- D2-19: Phase 1 compensated governance is reused exactly for both implementation PRs: FerroxLabs author; TradeCanyon owner-directed agent-operated review/merge; same human controller; no claim of independent/human-interactive review; exact head/check/review/merge/ancestry/hash/default-off receipts and no bypass. Desktop execution also requires `WL_LANE=desktop`, `wl queue`, an owned open `area:desktop-ui` issue, architecture/testing/oss-pr rules, and the repository pre-PR gates.

## Acceptance

- REQ-ACT-01 and REQ-POL-01 are mapped exactly once to executable plans.
- Positive Desktop legacy/new-stack and CLI admissions pass against an exact artifact triple.
- Tamper, duplicate/unknown/noncanonical fields, algorithm/key substitution, time bounds, replay/conflict, revoked issuer, project/subject/principal substitution/remap, widening, controls/races, resume drift and legacy persistence bypass fail typed.
- Local Nano and Desktop governing tests/CI are green; receipts and raw vectors are committed and independently verifiable.

## Discipline

- Separate Nano and Desktop worktrees/PRs; Nano repository agents do not edit Desktop without explicit authorized worktree.
- No `.secrets` reads. No tags or hidden merge/bypass.
- Three strikes per repeated failure; exact handoff on stop.
- Any expansion into Phase 3 or product control-plane work is a stop/replan tripwire.
