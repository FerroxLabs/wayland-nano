# Codebase Concerns

**Analysis Date:** 2026-08-16

## Tech Debt

**Residual per-turn retained memory (F-45):**
- Issue: The live ACP host retains approximately 8–10 KiB per completed turn after the larger whole-journal rebuild leak was fixed. The retaining structure is not identified; the current hypotheses are per-turn tool-definition rebuilding, engine construction, or an accumulating registry.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/docs/STATUS.md`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-agent/src/turn.rs`
- Impact: At the measured maximum soak cadence this is approximately 50 MiB/hour. The 1.5 GiB harness cap gives operational headroom, but long-running sessions remain O(turns) in retained memory and the owner explicitly accepted this sev-2 risk for 0.1.0/0.1.1.
- Fix approach: Heap-profile the ACP host under the recorded soak workload, identify the retaining owner, fix it without weakening the memory oracle, and require a one-hour max-cadence run at no more than 16 MiB/hour as specified by `wayland-nano/docs/FOLLOWUPS.md`.

**Monolithic ACP orchestration:**
- Issue: `acp_mode.rs` is about 10,154 lines and owns startup, session lifecycle, model routing, rules, MCP, hooks, checkpoints, approval, streaming, and notices. Several shipped-but-unreachable defects (hooks F-46 and checkpoints F-47) arose because feature crates exposed integration seams without production callers in this file.
- Files: `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`, `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/docs/FOLLOWUPS.md`
- Impact: Cross-cutting features can compile and pass crate tests while remaining inert on the primary Desktop/TUI surface. Review and merge conflict cost is high, and it is easy for exec, protocol-host, and ACP behavior to drift.
- Fix approach: Keep a mandatory production-call-site matrix for every capability and add wire-level tests through each advertised surface. Extract cohesive bootstrap/registration units only when a locked requirement touches them; preserve the existing fail-closed gates and avoid a broad rewrite.

**Open low-severity register is broad and partly reference-only:**
- Issue: Numerous sev-3 items remain open, including typed task-spawn errors, backend attribution fidelity, cross-process blob GC proof, lease/store identity binding, dispatcher/OAuth/rules/PTY/session-browser robustness, and platform test gaps. Some P3/P4 entries are recorded only as lists by reference rather than independently closeable findings.
- Files: `wayland-nano/docs/SEVERITY-MAP.md`, `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-session/src/attachment_store.rs`, `wayland-nano/crates/nano-mcp/src/dispatcher.rs`, `wayland-nano/crates/nano-mcp/src/oauth/`, `wayland-nano/crates/nano-core/src/execrules.rs`
- Impact: Small correctness and proof debts accumulate around security-sensitive machinery, and reference-only rows are difficult to assign, verify, and close without omission.
- Fix approach: Split aggregate rows into one ID per independently testable behavior, attach current file/test evidence, and prioritize items that affect journal integrity, cancellation, containment, or credential handling before cosmetic fidelity.

**Hook behavior is intentionally incomplete across child turns:**
- Issue: Hooks are wired on the primary ACP surface, but C6 child task turns remain hook-free on every surface. In addition, an invalid `hooks.toml` produces warnings and zero hooks rather than aborting the host.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-hooks/src/lib.rs`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-agent/src/tasks.rs`
- Impact: Policy or audit expectations encoded as hooks do not apply to delegated child work. A malformed hook configuration can silently remove expected enforcement for clients that do not surface host stderr prominently.
- Fix approach: Treat hooks as advisory unless their contract is strengthened. If hooks are used for enforcement, propagate them into child-engine construction and convert invalid configured state into a typed unavailable startup/session error rather than a warning-only zero-hook fallback.

**Toolchain metadata disagrees:**
- Issue: The repository pins Rust 1.95.0, while the workspace package metadata declares `rust-version = "1.85"`.
- Files: `wayland-nano/rust-toolchain.toml`, `wayland-nano/Cargo.toml`, `wayland-nano/AGENTS.md`
- Impact: Downstream tooling can infer an unsupported MSRV, while contributors and CI use the newer pinned compiler. This weakens reproducibility and can make compatibility claims inaccurate.
- Fix approach: Decide whether 1.85 is a tested MSRV or stale metadata. Either add an explicit 1.85 build gate and document it, or align `rust-version` with the pinned 1.95.0 toolchain.

## Known Bugs

**Image-bearing turns under-report cost:**
- Symptoms: Flux usage reports do not include image-token cost on the OpenAI wire; recorded image turns can report fewer prompt tokens than text-only baselines. Attachments are re-sent on later turns, so the under-count compounds.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-model/src/pricing.rs`, `wayland-nano/crates/nano-agent/src/turn.rs`, `shared/fixtures/flux/vision/`
- Trigger: Run an image-bearing Flux turn and compare returned prompt usage with the bytes sent and the text-only baseline.
- Workaround: Treat image-bearing session cost as incomplete/unpriced; do not use the reported token total as a hard spend oracle.

**Remote image URLs are unsupported:**
- Symptoms: ACP intake rejects remote HTTP(S) image URLs with a typed error rather than fetching and inlining them.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-egress/src/client.rs`
- Trigger: Submit an image input whose source is an HTTP(S) URL.
- Workaround: Provide an inline/local image through the supported bounded intake path.

**Multiple images per message are rejected:**
- Symptoms: More than one image in a message is typed-refused because live Flux probing miscounted a two-image request.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `shared/fixtures/flux/vision/`
- Trigger: Send a single message containing two image blocks.
- Workaround: Use one image per message until the upstream behavior is re-probed and the count guard can be safely removed.

**Flux MCP invocation is externally blocked:**
- Symptoms: `/mcp/` initialization and `tools/list` work, but the live Flux catalog is empty, so no `tools/call` proof is possible.
- Files: `wayland-nano/docs/compliance/SCENARIO_CATALOG.md`, `shared/fixtures/flux/mcp/`, `shared/fixtures/flux/FINDINGS.md`
- Trigger: Query the current Flux MCP tool catalog used by the recorded compatibility proof.
- Workaround: Use configured external MCP servers; keep the Flux MCP invocation capability unclaimed until an invocable upstream tool exists.

## Security Considerations

**Fail-closed invariants span many integration points:**
- Risk: Sandbox, egress, tool policy, journal integrity, rules, hooks, MCP, and checkpoints are composed across large orchestration modules. A missing call site can ship an advertised control as inert, while warning-to-empty fallback can remove expected controls.
- Files: `wayland-nano/AGENTS.md`, `wayland-nano/ARCHITECTURE.md`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`, `wayland-nano/crates/nano-egress/`, `wayland-nano/crates/nano-sandbox/`
- Current mitigation: The codebase uses deny-by-default egress, OS containment, policy-gated tools, append-only journaling, capability honesty, adversarial suites, and external filesystem/process/network oracles.
- Recommendations: Require a negative test and a real production-surface call-site test for every security control. Never accept crate-local availability as proof of activation; preserve `SANDBOX_UNAVAILABLE` and typed refusal on uncertainty.

**Attachment-store cross-process safety is not fully proven:**
- Risk: The GC race battery is single-process, and `WriteLease` is not bound to a specific store identity. These are currently classified sev-3 because no production cross-process failure or multi-store configuration is demonstrated.
- Files: `wayland-nano/crates/nano-session/src/attachment_store.rs`, `wayland-nano/docs/SEVERITY-MAP.md`, `wayland-nano/docs/FOLLOWUPS.md`
- Current mitigation: Production uses one store and the current lease discipline; attachment references are scanned from manifests and tool-result image references.
- Recommendations: Add a true cross-process writer/GC battery and bind leases to store identity before supporting multiple stores or concurrent maintenance processes.

**Remote image fetching must not bypass egress policy:**
- Risk: Adding convenience URL intake could create SSRF/private-range access or unbounded binary downloads if it reuses a generic HTTP client or widens the text fetcher.
- Files: `wayland-nano/crates/nano-egress/src/client.rs`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/docs/FOLLOWUPS.md`
- Current mitigation: Remote image URLs are rejected; the existing bounded egress fetcher denies `image/*`.
- Recommendations: Keep rejection until a dedicated bounded image-fetch operation has an explicit host allowlist, private-range denial, content-type/size limits, redirect policy, and adversarial probes.

## Performance Bottlenecks

**ACP host retained-memory slope:**
- Problem: The host retains about 8–10 KiB per turn even after incremental journal folding removed the dominant leak.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/scripts/soak/`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`
- Cause: Unknown; A/B evidence exonerates repo-map, and the remaining candidates are in turn construction and registries.
- Improvement path: Use a heap profiler against the reproducible soak workload, retain the strengthened slope/absolute-cap oracle, and verify no throughput decay after the fix.

**Large high-churn modules:**
- Problem: Several production files are very large: `acp_mode.rs` (~10.1k lines), sandbox ACL code (~3.1k), agent tasks/wiring/turn (~2.1–2.5k each), auto-routing (~2.3k), and MCP dispatcher (~1.8k).
- Files: `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-sandbox/src/acl.rs`, `wayland-nano/crates/nano-agent/src/tasks.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`, `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/crates/nano-cli/src/auto_routing.rs`, `wayland-nano/crates/nano-mcp/src/dispatcher.rs`
- Cause: Successive capability waves converge in shared orchestration and platform-specific security code.
- Improvement path: Extract only proven cohesive boundaries with characterization tests. Prefer registries/builders that make activation enumerable, while keeping approval and journal ordering explicit.

## Fragile Areas

**Primary surface wiring:**
- Files: `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-cli/src/exec_mode.rs`, `wayland-nano/crates/nano-cli/src/host_mode.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`
- Why fragile: Hooks and checkpoints both shipped as implemented crates but were unreachable on the ACP surface until post-stable fixes. Every new capability must be registered, approved, journaled, advertised honestly, and resumed consistently on three surfaces.
- Safe modification: Enumerate all production modes and add end-to-end tests that invoke the feature through each claimed mode. Keep capability flags false until those tests and live proof exist.
- Test coverage: Wire-level batteries now cover hooks and checkpoints, but there is no general automated assertion that every advertised definition has a live production caller.

**Journal-first state transitions:**
- Files: `wayland-nano/crates/nano-session/`, `wayland-nano/crates/nano-checkpoints/src/lib.rs`, `wayland-nano/crates/nano-agent/src/cron.rs`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`
- Why fragile: Previous findings included checkpoint events appended before reachable state, cron double-fire windows, swallowed append failures, and competing writers. Ordering failures create replay claims that external state cannot satisfy.
- Safe modification: Preserve one append authority, acquire ownership before idempotency checks, persist/sync external state before the final journal claim where required, and abort on append failure.
- Test coverage: Existing kill-resume and adversarial tests are strong, but each new stateful feature needs crash points before and after every durable boundary plus an external oracle.

**Unix sandbox wrapper:**
- Files: `wayland-nano/crates/nano-sandbox/src/bin/linux_sandbox/main.rs`, `wayland-nano/crates/nano-sandbox/src/linux_landlock.rs`, `wayland-nano/crates/nano-sandbox/src/linux_bwrap.rs`, `wayland-nano/crates/nano-mcp/tests/unix_contained_spawn.rs`
- Why fragile: It combines fork/exec, signal forwarding, process groups, bubblewrap, Landlock, seccomp, protected-create monitoring, and parent-death behavior. A recent contained stdio MCP regression showed containment can remain present while function breaks.
- Safe modification: Run the full hosted Linux/macOS containment and stdio dispatcher batteries; never replace failure with an uncontained fallback.
- Test coverage: Hosted platform legs exist, but architecture-specific seccomp code still contains an explicit unsupported-architecture `unimplemented!` in `wayland-nano/crates/nano-sandbox/src/bin/linux_sandbox/landlock.rs` and Windows cannot exercise the Unix runtime locally.

## Scaling Limits

**Long-lived session memory:**
- Current capacity: The soak harness caps host memory at 1.5 GiB and the recorded workload has about 12× headroom; measured retained growth is about 50 MiB/hour at maximum cadence.
- Limit: A sufficiently long uninterrupted high-cadence session approaches the absolute cap because retained state grows with turn count.
- Scaling path: Remove F-45's retaining structure, keep compaction/restart as secondary bounds, and enforce slope plus absolute limits in release soaks.

**Tool and hydration bounds:**
- Current capacity: MCP/tool outputs, schemas, hydrated names, hook output, and HTTP bodies have explicit bounds; hydration over the 64-name carry cap degrades to digest/summary form.
- Limit: Large tool inventories intentionally lose hydrated-name carry across compaction and require rehydration; exceeding bounds fails or degrades rather than scaling indefinitely.
- Scaling path: Preserve global byte/count budgets, use deferred tool search, and improve rehydration UX without raising bounds or placing full schemas/results into model history.

## Dependencies at Risk

**Pinned platform/security dependencies:**
- Risk: The workspace requires Rust 1.95.0 and pins `windows-sys` 0.52; security and FFI behavior is tested against those exact constraints. The workspace metadata currently advertises Rust 1.85.
- Impact: Uncoordinated compiler or Windows binding changes can alter ACL, job-object, credential-manager, or process-containment behavior and invalidate certified evidence.
- Migration plan: Change pins only as a dedicated compatibility phase with hosted platform matrices, containment adversarial tests, provenance updates, and refreshed evidence.

**Upstream Flux behavior:**
- Risk: Flux routing/model catalogs, image accounting, multi-image behavior, and MCP inventory are external and partially unstable.
- Impact: Provider payload quirks have previously caused startup availability failures and typed-error misclassification; current MCP invocation and image cost completeness remain blocked.
- Migration plan: Keep the vendored provider catalog as endpoint authority, reject malformed entries individually where safe, retain recorded fixtures, and require live probes before changing capability flags.

## Missing Critical Features

**PDF input:**
- Problem: PDF/document blocks are not supported; all Flux bindings currently use the OpenAI-completions path while the proven PDF contract requires an Anthropic document block or safe pinned-id alternative.
- Blocks: PDF-based analysis and multi-page document workflows.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-model/src/provider_router.rs`, `shared/fixtures/flux/`

**Complete image cost accounting:**
- Problem: Provider usage omits image tokens and there is no client-side byte-to-token estimate.
- Blocks: Trustworthy spend caps and cost reports for image-heavy sessions.
- Files: `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/crates/nano-model/src/pricing.rs`, `wayland-nano/crates/nano-cli/src/auto_routing.rs`

## Test Coverage Gaps

**Cross-process attachment GC:**
- What's not tested: A real second process racing attachment writes against garbage collection.
- Files: `wayland-nano/crates/nano-session/src/attachment_store.rs`
- Risk: A race could delete or misclassify a live blob without being caught by the single-process battery.
- Priority: Medium

**Hook enforcement on delegated tasks:**
- What's not tested: Hook behavior on C6 child task turns, because those engines do not receive hooks.
- Files: `wayland-nano/crates/nano-agent/src/tasks.rs`, `wayland-nano/crates/nano-hooks/src/lib.rs`, `wayland-nano/docs/FOLLOWUPS.md`
- Risk: Delegation bypasses user expectations if hooks are treated as enforcement or comprehensive audit policy.
- Priority: High if hooks are policy; Medium while explicitly advisory

**Host-independent and architecture-edge containment:**
- What's not tested: All Unix fork/signal/seccomp paths on the Windows development host; unsupported seccomp architectures terminate via `unimplemented!`.
- Files: `wayland-nano/crates/nano-sandbox/src/bin/linux_sandbox/landlock.rs`, `wayland-nano/crates/nano-sandbox/src/bin/linux_sandbox/main.rs`, `wayland-nano/crates/nano-mcp/tests/unix_contained_spawn.rs`
- Risk: Platform-only regressions appear only in hosted CI, and a newly targeted architecture can panic rather than return a typed unavailable error.
- Priority: High before expanding supported architectures

**Capability-to-production-call-site coverage:**
- What's not tested: A generic invariant that every advertised capability and tool definition has a consumer on each claimed production surface.
- Files: `wayland-nano/crates/nano-agent/src/wiring.rs`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-cli/src/exec_mode.rs`, `wayland-nano/crates/nano-cli/src/host_mode.rs`
- Risk: Implemented crates can ship inert again, as occurred with hooks and checkpoints.
- Priority: High

## Planning and Specification Inconsistencies

**Severity map is stale relative to follow-ups and release status:**
- Issue: `docs/SEVERITY-MAP.md` still lists F-1, F-8 data integrity, F-17 latency, F-18, F-19, F-27 item 6, F-P3-5/6/8/11/12, and F-P5-3 as open sev-1/2 even though `docs/FOLLOWUPS.md` records fixes with commits and `docs/STATUS.md` declares the stable gate met. It also says owner signature is pending while status says owner-signed.
- Files: `wayland-nano/docs/SEVERITY-MAP.md`, `wayland-nano/docs/FOLLOWUPS.md`, `wayland-nano/docs/STATUS.md`, `shared/reviews/stable-wave/SEVERITY-SIGNOFF-2026-08-14.md`
- Impact: A planner can prioritize already-fixed work, misreport release risk, or distrust the stable claim. F-45 is the actual explicit accepted exception and must remain visible.
- Fix approach: Rebuild the severity map from current HEAD, retain code/test evidence for each transition, and make one artifact authoritative for open severity.

**Contract gap register contradicts the filesystem:**
- Issue: The scenario catalog says the frozen artifacts in `shared/contracts/` have not been produced, and an older status section says the directory is empty. The active tree contains `capability-profile.md`, `journal-semantics.md`, `flux-endpoint-contract.md`, `event-types.md`, and `nano-error-codes.json`.
- Files: `wayland-nano/docs/compliance/SCENARIO_CATALOG.md`, `wayland-nano/docs/STATUS.md`, `shared/contracts/capability-profile.md`, `shared/contracts/journal-semantics.md`, `shared/contracts/flux-endpoint-contract.md`, `shared/contracts/event-types.md`
- Impact: Compliance tooling and future plans can key off obsolete SCORECARD prose instead of the frozen contracts.
- Fix approach: Close G-CTR-1 with provenance/evidence and label old status sections historical so they cannot be read as current state.

**Build-plan mirror path is stale, though current files match:**
- Issue: `NANO-BUILD-PLAN-V3.md` warns about drift between `shared/contracts/nano-error-codes.json` and an unspecified in-repo copy. The actual in-repo mirror is `crates/nano-session/contracts/nano-error-codes.json`; both currently have the same SHA-256, so the risk is procedural rather than an active mismatch.
- Files: `shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md`, `shared/contracts/nano-error-codes.json`, `wayland-nano/crates/nano-session/contracts/nano-error-codes.json`
- Impact: Future error-kind edits can update the wrong assumed path or skip regeneration.
- Fix approach: Name the exact mirror path in the plan and enforce byte equality in the existing error-table generation/check gate.

---

*Concerns audit: 2026-08-16*
