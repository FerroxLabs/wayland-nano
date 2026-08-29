# Codebase Concerns

**Analysis Date:** 2026-08-27

## Tech Debt

**Two independent memory implementations:**
- Issue: The shipped agent-facing memory tools still use the filesystem-backed `nano_agent::memory::MemoryStore`, while the new persistent T2 store is a separate `nano_memory::MemoryStore`. The CLI constructs the former and no runtime crate depends on `nano-memory`.
- Files: `crates/nano-agent/src/memory.rs`, `crates/nano-cli/src/host_mode.rs`, `crates/nano-cli/src/acp_mode.rs`, `crates/nano-memory/src/lib.rs`, `Cargo.toml`
- Impact: P-MEM-1 is a proven substrate, but interactive activations do not get project/agent-partitioned FTS5/KNN recall, mediated proposals, or T2 retention semantics.
- Fix approach: Implement only the planned integration in P-BOT-5b: bind a configured `agent_id` and project to `nano-memory`, expose scoped recall and mediated proposals, and preserve the existing journal-first authority. Do not casually merge or rewrite the old C5 store.

**Deferred engine and UX debt is centralized but sizable:**
- Issue: The follow-up ledger records open engine/host issues including uncapped composed tool results, manual task-directory GC, missing Desktop presentation surfaces, untyped task failures, and low-severity merge-review debt.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-agent/src/turn.rs`, `crates/nano-protocol/src/acp.rs`, `crates/nano-agent/src/tasks.rs`
- Impact: These do not invalidate the stable coding-agent engine, but new tool and persistent-agent surfaces can amplify context flooding, storage accumulation, and weak operator diagnostics.
- Fix approach: Treat `docs/FOLLOWUPS.md` as the authoritative queue. Close entries in dedicated packages; do not fold them into persistent-agent milestones without an explicit dependency.

**Large orchestration modules concentrate change risk:**
- Issue: `nano-cli`, `nano-agent`, and `nano-sandbox` contain the largest Rust surfaces, while central runtime wiring remains concentrated in files such as `crates/nano-cli/src/acp_mode.rs` and `crates/nano-agent/src/turn.rs`.
- Files: `crates/nano-cli/src/acp_mode.rs`, `crates/nano-agent/src/turn.rs`, `crates/nano-agent/src/wiring.rs`, `crates/nano-sandbox/src/lib.rs`
- Impact: Named-agent, recall, routine, and backend changes all cross high-churn seams where an unrelated behavioral regression is easy to introduce.
- Fix approach: Add narrow adapters at existing construction and journal seams, keep package ownership explicit, and require focused contract tests plus `just gate-all` for every landing.

## Known Bugs

**Permission-parked ACP turn can starve control traffic:**
- Symptoms: A turn waiting on permission has been observed to leave fork, a second prompt, and cancel unanswered for at least 15 seconds.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-cli/src/acp_mode.rs`
- Trigger: Park an ACP turn on a permission request, then issue a control request during the wait.
- Workaround: Avoid overlapping control requests while a permission card is unresolved; F-7 requires a scripted reproduction before changing the wait architecture.

**AGENTS.md edits are applied one ACP turn late:**
- Symptoms: A mid-session edit affects turn N+2 rather than the contractually expected next turn N+1 on the ACP path; host mode re-reads per turn.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-cli/src/acp_mode.rs`, `crates/nano-cli/src/host_mode.rs`
- Trigger: Edit `AGENTS.md` between ACP turns and inspect the next generated prefix.
- Workaround: Restart/resume the ACP session after changing instructions when immediate effect matters.

**Residual host-path retained growth remains unresolved:**
- Symptoms: Long soak measurements retain roughly 8 KB per turn after the earlier whole-journal rebuild leak was fixed; the one-hour acceptance receipt was not completed.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-cli/src/acp_mode.rs`, `crates/nano-agent/src/turn.rs`
- Trigger: Run the scaled B1/B5 host-turn soak described under F-45 in `docs/FOLLOWUPS.md`.
- Workaround: Bound session length or restart between long runs until F-45 has a passing long-duration receipt.

## Security Considerations

**Memory adversarial gates are not yet landed:**
- Risk: P-MEM-1 has partition and mediation unit/acceptance evidence, but the required six-card `mem-sec` pack has not independently exercised poisoned supersession, extraction laundering, removed-scope escapes, and cross-agent leakage at every retrieval checkpoint.
- Files: `crates/nano-memory/src/store.rs`, `crates/nano-memory/src/mediation.rs`, `gates/`, `contracts/`
- Current mitigation: `nano-memory` validates project/agent partitions, applies deterministic tier-aware resolution, requires host mediation for model writes, and filters retrieval in both candidate passes and assembled output.
- Recommendations: Land P-MEM-SEC before persistent memory is treated as safe for named agents; preserve human review ownership for both `gates/**` and `agents/**` via `CODEOWNERS`.

**Agent identity has grammar but no configured registry:**
- Risk: `nano-memory` validates the syntax of `agent_id`, but the planned fail-closed configured-ID check cannot exist until the named-agent registry lands. A syntactically valid, unconfigured namespace must not become writable through runtime integration.
- Files: `crates/nano-memory/src/types.rs`, `crates/nano-memory/src/store.rs`, `CODEOWNERS`
- Current mitigation: No runtime T2 memory surface is wired, so the incomplete registry check is not currently externally reachable through the agent.
- Recommendations: P-BOT-5a must establish the trusted registry; P-BOT-5b must reject unknown identities at activation and write time before exposing memory tools.

**Module provenance is insufficient for agent composition:**
- Risk: Agent files referencing modules would currently lack the planned contract version, digest-pinned source verification, install receipt, and registry-kind refusal.
- Files: `crates/nano-plugins/src/manifest.rs`, `crates/nano-plugins/src/source.rs`, `crates/nano-plugins/src/fetch.rs`, `crates/nano-session/src/op.rs`
- Current mitigation: Named-agent module composition is not implemented, so this gap is not yet a reachable agent-registry path.
- Recommendations: Complete P-MOD-GAP before P-BOT-5a permits `[modules]`; fail closed on digest mismatch and unsupported registry sources.

**Blob write leases are not store-bound:**
- Risk: The open F-26 audit finding states a `WriteLease` can be consumed by the wrong attachment store when names collide.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-session/src/attachment_store.rs`
- Current mitigation: Lease validation and namespace controls exist, but the store identity is not part of the lease invariant.
- Recommendations: Bind leases to canonical store identity and add an adversarial cross-store test in a dedicated attachment-store hardening change.

## Performance Bottlenecks

**Per-turn retained growth limits very long sessions:**
- Problem: F-45 records residual retained growth of about 8 KB per host turn and no successful 3,600-second acceptance receipt.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-cli/src/acp_mode.rs`
- Cause: The prior whole-journal rebuild source is fixed, but the remaining owner is not proven.
- Improvement path: Reproduce with the recorded B1/B5 isolation battery, attribute allocations before changing code, and rerun the long receipt.

**Tool output lacks a global context ceiling:**
- Problem: Per-tool caps do not protect model history or ACP frames from a future or composed MCP tool returning an oversized result.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-agent/src/turn.rs`, `crates/nano-protocol/src/acp.rs`
- Cause: `ToolOutcome.output` is cloned into history and emitted without an engine-wide bound.
- Improvement path: Add a typed, deterministic ceiling at both the history-append and ACP emission seams, with an oversized MCP adversarial test.

**Retention cleanup for task artifacts is manual:**
- Problem: Completed task directories retain journals, workspace copies, and reports indefinitely.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-agent/src/tasks.rs`
- Cause: Auditability was prioritized and no explicit retention command exists.
- Improvement path: Add an explicit age/completion-aware GC command; never silently reap audit artifacts.

## Fragile Areas

**Journal/SQLite dual-write boundary:**
- Files: `crates/nano-memory/src/store.rs`, `crates/nano-memory/src/mediation.rs`, `crates/nano-session/src/op.rs`, `crates/nano-session/src/replay.rs`
- Why fragile: The journal is authority and SQLite is rebuildable; changing operation order, receipt binding, or replay neutrality can create acknowledged-but-unrecoverable memory or change old-session replay.
- Safe modification: Append the memory op before committing the index, keep new op variants additive and replay-neutral, and verify kill-after-journal recovery by dropping and rebuilding the DB.
- Test coverage: P-MEM-1 covers core hard-kill recovery; each new runtime write surface and new op family still needs its own mediation and replay test.

**SQLite native extension matrix:**
- Files: `crates/nano-memory/src/lib.rs`, `crates/nano-memory/Cargo.toml`, `.github/workflows/ci.yml`
- Why fragile: `sqlite-vec` introduces native C compilation and explicit SQLite extension registration across seven CI targets, including Windows ARM64.
- Safe modification: Preserve the proven dependency versions and registration pattern; exercise all target legs for schema or build changes.
- Test coverage: P-MEM-1 is green at the recorded HEAD, but no runtime consumer currently exercises startup/registration through CLI binaries.

**Identity/composition resume boundary:**
- Files: `crates/nano-session/src/fork.rs`, `crates/nano-session/src/op.rs`, `crates/nano-agent/src/tasks.rs`, `crates/nano-cli/src/acp_mode.rs`
- Why fragile: P-BOT-5a/5b must combine immutable identity, current ceilings, persona/module hashes, fork digests, and re-derived postures without restoring revoked authority.
- Safe modification: Journal resolved composition in `SessionBegin`, require digest and identity checks on resume, and re-derive current posture rather than replaying prior grants.
- Test coverage: Existing fork and task tests cover ephemeral sessions; named-agent mismatch, rekey, revoked-module, and `AgentBusy` scenarios are missing until P-BOT-5a/5b.

**Container supervisor boundary:**
- Files: `crates/nano-cua/src/backend.rs`, `crates/nano-cua/src/backends/`, `crates/nano-sandbox/src/`, `crates/nano-platform/src/lib.rs`
- Why fragile: The planned browser/desktop backends combine container-runtime authority, validated names, bind mounts, image digests, viewer ports, capability intersection, and per-action evidence.
- Safe modification: P-EXE-1 must expose only ensure/stop/reset/list by validated `agent_id`, re-inspect hardening before every use, and refuse mismatches; P-EXE-2 must extend the same seam rather than create a second architecture.
- Test coverage: Current `nano-cua` live desktop proofs remain capability-gated in `docs/FOLLOWUPS.md`; browser/desktop container tamper, Docker/Podman parity, and frame-receipt replay do not exist yet.

## Scaling Limits

**Memory writer concurrency:**
- Current capacity: One `memory.db` under the session ownership lock; agent activations are sequential for writes.
- Limit: Concurrent writers for the same agent are not a v1 claim and must produce typed contention/`AgentBusy`, not queue silently.
- Scaling path: Keep P-BOT-5b to one live activation per `agent_id`; treat concurrent multi-agent writers as a later contract change, not an optimization.

**Local memory retention:**
- Current capacity: Defaults in `crates/nano-memory/src/types.rs` cap each `(project, agent_id)` at 10,000 episodes, 50,000 facts, and 256 MiB across retained memory.
- Limit: Caps are local and enforced synchronously; there is no hosted tier, cross-project global read, or concurrent shared service.
- Scaling path: Preserve partition-local retention first. P-XPROJ is explicitly last, and hosted storage requires a separate security and durability contract.

**Routine execution:**
- Current capacity: Existing cron infrastructure schedules ephemeral work, but no per-agent routine activation or ledger exists.
- Limit: Without P-BOT-5c caps, persistent routines could spin, retain unbounded run history, or repeatedly escalate.
- Scaling path: Enforce per-agent routine/run-record caps, rate-limited attention requests, typed failure handling, and a global pause control in P-BOT-5c.

## Dependencies at Risk

**`sqlite-vec` native extension:**
- Risk: Native builds and SQLite extension ABI/registration are platform-sensitive, especially on Windows ARM64.
- Impact: A failure prevents `nano-memory` from compiling or opening its vector table on a supported target.
- Migration plan: No migration is currently justified; retain the exact proven stack and let the seven-leg CI matrix gate every change.

**Container runtime assumptions are not yet contracted in code:**
- Risk: Docker and Podman differ in inspect output, security defaults, networking, and lifecycle behavior.
- Impact: A backend may appear available while violating cap-drop, namespace, mount, or loopback-only requirements.
- Migration plan: P-EXE-1 must normalize only the four supervisor verbs and verify derived posture on both runtimes; unsupported states return typed refusal.

## Missing Critical Features

**P-MEM-SEC — adversarial memory certification:**
- Problem: The six contract gate cards and independently owned fixtures are absent from the current gate registry.
- Blocks: Treating runtime persistent memory as poisoning- and partition-resistant.

**P-PROF — profiles and merge math:**
- Problem: Closed profile TOML, narrow-only merge behavior, `Op::ProfileSet`, resume-narrows, and shipped profiles are not implemented.
- Blocks: Safe policy composition and the P-BOT-5a ceiling chain.

**P-MOD-GAP — digest-verified modules:**
- Problem: Manifest contract versioning, digest source pins, install receipts, provenance, and typed registry refusal are missing.
- Blocks: Allowing named-agent files to reference modules.

**P-BOT-5a — named-agent composition:**
- Problem: There is no `agents/*.agent.toml` registry, trusted agent selection, persona overlay, composition hash, named spawn, or agent-attributed usage rollup.
- Blocks: Persistent named identities and backend selection.

**P-BOT-5b — recall-driven continuity:**
- Problem: Runtime activations do not use T2 scoped recall, mediated proposals, memory-primary resume, per-agent ledgers, identity-checked fork chains, or surgical rollback.
- Blocks: The product claim that a named agent accumulates and safely recalls experience across activations.

**P-BOT-5c — proactive routines and escalation:**
- Problem: Cron cannot activate a named agent with bounded routine receipts, typed activation failures, attention requests, or per-agent pause semantics.
- Blocks: Safe proactive agents.

**P-EXE-1/P-EXE-2 — browser and desktop computers:**
- Problem: No hardened per-agent container supervisor, browser flavor, desktop flavor, image/composition digest binding, or frame-receipt evidence exists.
- Blocks: Agents safely operating isolated browser and desktop environments.

## Test Coverage Gaps

**Runtime T2 memory integration:**
- What's not tested: CLI startup/open, scoped recall injection, proposal mediation, visible receipts, configured-agent rejection, and kill/resume through an actual agent activation.
- Files: `crates/nano-cli/src/acp_mode.rs`, `crates/nano-cli/src/host_mode.rs`, `crates/nano-agent/src/memory.rs`, `crates/nano-memory/src/store.rs`
- Risk: The store can remain correct in isolation while the host wires identity, trust tier, or journaling incorrectly.
- Priority: High

**Cross-process attachment-store safety:**
- What's not tested: F-25 records that the claimed cross-process GC battery is single-process, and F-26 lacks a wrong-store lease rejection test.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-session/src/attachment_store.rs`
- Risk: Concurrent cleanup or store confusion can delete or authorize the wrong blob.
- Priority: Medium

**Environment-sensitive Linux sandbox probe:**
- What's not tested: F-38 records a `bwrap` probe whose CI result depends on runner environment rather than a fully controlled oracle.
- Files: `docs/FOLLOWUPS.md`, `crates/nano-sandbox/src/`
- Risk: CI can be flaky or misclassify platform availability.
- Priority: Medium

**Persistent-agent end-to-end acceptance:**
- What's not tested: Named composition, memory-primary repeated activations, revocation across resume, one-live-activation locking, routine spin caps, container tamper refusal, and browser/desktop evidence replay.
- Files: `crates/nano-agent/`, `crates/nano-cli/`, `crates/nano-session/`, `crates/nano-memory/`, `crates/nano-cua/`
- Risk: Individual packages may pass locally while identity, authority, memory, scheduling, and computer control fail at their integration boundaries.
- Priority: High

---

*Concerns audit: 2026-08-27*
