# Requirements: Wayland Nano Verified Change

**Defined:** 2026-08-16
**Core Value:** A change earns trust only through independently rerunnable machine evidence.
**Authority:** This is a traceability projection of `../shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md`; the canonical interface contract and WP specs remain authoritative.

## Current Program Requirements

### Program Control

- [x] **CTRL-01**: Every WP reads and obeys `AGENTS.md`, the master frame, its full WP spec, the canonical interface contract where applicable, and its GOALS card before implementation.
- [x] **CTRL-02**: Every WP changes only files inside its explicit OWNS boundary and records a deviation instead of routing around a boundary conflict.
- [x] **CTRL-03**: Before every WP, current `origin/master` is fetched and resolved to a recorded SHA; a clean isolated worktree and canonical `feat/wp-<id>` branch are created at that SHA, and builders never merge or push WP branches.
- [x] **CTRL-04**: Every WP completes one Critical/High audit, at most one fix round, fix verification, and `just gate-all` (formatting, clippy with warnings denied, workspace tests, and `gate-gen-check`) before integration.
- [x] **CTRL-05**: The integrator merges one WP at a time with `--no-ff`, re-runs `just gate-all`, pushes detached integration HEAD to `master`, and confirms CI green before the next promotion.
- [x] **CTRL-06**: Captured evidence is canary-scanned and contains no Flux key value; the key is referenced by path only.
- [x] **CTRL-07**: Generated artifacts are regenerated through their generator and never hand-edited; new dependencies require cargo-deny-clean justification.
- [x] **CTRL-08**: Completion reporting distinguishes Implemented, Reachable, and Live-proven claims and reports WP, commits, gate, and CI in one line.

### WP-0.1 Owner Gate

- [x] **HOST-01**: WP-0.1 remains owner/host-run because its CUA evidence requires an interactive Windows desktop and manual 100%/150% display scaling; autonomous execution records the handoff and does not simulate proof.

### WP-0.2 Memory Hardening

- [x] **MEM-01**: A feature-gated `NANO_MEM_STATS=<path>` NDJSON reporter emits the exact retained-structure schema at the specified cadence without contaminating ACP stdout or stderr protocols.
- [x] **MEM-02**: A recorded short soak identifies whether fold auxiliaries, tool-definition clones, or neither are the dominant retained-growth source using measured per-structure and PWS deltas.
- [x] **MEM-03**: Only the profile-selected fix arm is implemented; if neither suspect dominates, the WP stops with evidence and an open follow-up rather than applying a speculative fix.
- [x] **MEM-04**: Any landed fix preserves the incremental-fold/full-rebuild equivalence oracle and adds the spec-required size/bounding assertion.
- [x] **MEM-05**: A one-hour receipt soak satisfies the locked B1 budget, including slope at or below 16 MiB/hour, before F-45 can be marked fixed.

### WP-0.3 PDF Intake

- [x] **PDF-01**: Inline and confined-path PDF intake validates `%PDF-` magic, MIME/extension agreement, one-document-per-message, and the 20 MiB cap with typed refusals.
- [x] **PDF-02**: PDF content uses additive `DocumentRef` and document block types while leaving the existing image/attachment journal contract intact.
- [x] **PDF-03**: Anthropic Messages emits the exact base64 `document` source block, while an OpenAI-completions-bound leaf is refused before network I/O and is never silently dropped or rerouted.
- [x] **PDF-04**: A PDF-bearing session kill/resume rehydrates the digest-verified document through the existing attachment store.
- [x] **PDF-05**: A live Flux probe demonstrates correct quoted content plus the prompt-token jump, archives a canary-clean fixture, and records the page/token metering limitation.
- [x] **PDF-06**: The new typed error is added through the canonical error table and all generated mirrors are regenerated only after the WP-0.3 ownership card explicitly grants the exact source and generated-artifact surfaces.

### WP-0.4 Frozen Contracts

- [x] **CTR-01**: Four byte-canonical JSON artifacts exist for capability profile, journal semantics, Flux endpoint contract, and event-type vocabulary with freeze and change-control metadata.
- [x] **CTR-02**: Capability, journal vocabulary, and event vocabulary/count artifacts derive deterministically from current source or corpus inputs and regenerate byte-identically.
- [x] **CTR-03**: The Flux endpoint artifact contains all six evidence-backed endpoints and schema validation verifies required fields and fixture paths.
- [x] **CTR-04**: Generator `--check` and workspace schema tests fail closed on deliberately stale or tampered contract artifacts.
- [x] **CTR-05**: The WP-0.4 ownership card explicitly grants the additive `nano-session/src/op.rs` and `nano-protocol` contract-test changes required by its spec before implementation; the executor does not infer broader crate ownership.
- [x] **CTR-06**: The owner-managed G-CTR-1 catalog closure is reported as ready but is not self-approved by the executor.

### WP-1 Gate and Receipt Foundation

- [ ] **GATE-01**: `nano-verify` runs argv-only gate invocations with the canonical environment allowlist, bounded output, timeout/process-tree termination, and no network in the decision core.
- [ ] **GATE-02**: Gate parsing uses the last valid summary, reconstructs the full card inventory, treats exit code as non-authoritative, and fails closed on missing output, timeout, spawn failure, unknown check IDs, or inconsistent totals.
- [ ] **GATE-03**: Gate outcomes expose canonical scores and failure keys without leaking gate source, command, expected values, or ambient secrets to builders.
- [ ] **RCPT-01**: Receipts are standalone schema-1 canonical JSON documents containing the canonical red evidence, gate identity/digest, observed commit, fix commit, mint time, and producer.
- [ ] **RCPT-02**: Receipt writes use the canonical writer lock and true platform atomic replacement, with bounded contention and fail-closed reader retry behavior.
- [ ] **RCPT-03**: WP-1 receipt preflight proves schema, genuine red evidence, commit existence, ancestry/test existence, and the registry pin, returning `Ready` only when those read-only checks pass; WP-3 `CLI-04` exclusively owns the bounded fix-commit gate rerun and canonical final verdict.
- [ ] **RCPT-04**: The complete named parser, subprocess, registry, receipt-preflight, ancestry/test-path, atomic-write contention/retry, and corruption tests pass against materialized fixture repositories.
- [ ] **PROV-01**: Every donor-adapted file in `nano-verify` has an exact transformation entry in `UPSTREAM.md`.

### WP-2 Gated Climb

- [ ] **CLIMB-01**: The pure climb implements probe, ensemble, per-check surgical escalation, and consolidation-on-plateau through the canonical injected `Effects` seam.
- [ ] **CLIMB-02**: Candidate acceptance requires strict score improvement or a strict subset of canonical failures and never uses failure count alone.
- [ ] **CLIMB-03**: Builders and reviewers receive opaque failing-check identifiers only; prompts never expose gate internals.
- [ ] **CLIMB-04**: The default call budget is 12, every model call consumes budget, escalation is typed, and all exits map to the complete canonical terminal/stop enums.
- [ ] **CLIMB-05**: Driver-stub tests cover probe and ensemble wins, surgical rejects/accepts, consolidation, plateau, no-cheap-model, budget exhaustion, trust-boundary leakage, oscillation regression, and terminal completeness.

### WP-3 Verify CLI and CI Surface

- [ ] **CLI-01**: `wayland-nano verify` exposes the exact argv and exit-code contract for minting, `--run-only`, and `--verify-receipt` modes, including repo-confined run-only artifact resolution, with registration limited to the owned CLI surfaces.
- [ ] **CLI-02**: The production adapter resolves requirement-to-gate mappings and canonical closure bodies from the registry, materializes invocations, runs the climb, and mints receipts only after verified closure.
- [ ] **CLI-03**: JSONL v1 emits the closed verify event vocabulary with deny-unknown persisted artifacts and no gate-command/source leakage.
- [ ] **CLI-04**: Offline verification materializes a temporary worktree at the receipt fix commit, performs bounded Git probes and gate rerun, cleans up, and fails closed on tampering or unverifiable state.
- [ ] **CLI-05**: End-to-end fixture tests prove authored-defect rejection, identifiers-only feedback, repair, receipt roundtrip, tamper rejection, and the complete exit-code matrix.
- [ ] **CLI-06**: The documented CI consumer is version-pinned under `docs/verify/ci/`; promotion into `.github/` remains an integrator decision.
- [ ] **PROV-02**: Every donor-adapted WP-3 CLI or verifier file has an exact transformation entry in `UPSTREAM.md`.

### WP-4 Gate Cards and Dogfood

- [ ] **CARD-01**: The registry contains canonical closure bodies/digests, direct-or-pinned-interpreter script shapes, repo-confined run-only artifacts, and requirement-to-gate mappings for install-payload, provision-script, and config-schema packs.
- [ ] **CARD-02**: Each Gate Card declares a closed inventory, categories, wrapped-tool pins, invocation policy, sealed fixture digest, validation status, and known gamed modes.
- [ ] **CARD-03**: Each pack has at least five documented fluent-but-wrong mutants with `why_fluent` and `expected_drop`, and every mutant is caught.
- [ ] **CARD-04**: Every sealed fixture passes all checks, while missing/tampered digests, unknown IDs, and inconsistent summaries fail closed.
- [ ] **CARD-05**: Install-payload verifies the actual packaged payload and helpers without changing packaging sources.
- [ ] **CARD-06**: Provision-script verifies the Windows dry-run payload, operation/idempotence semantics, and no-mutation oracle without changing provisioning sources.
- [ ] **CARD-07**: Config-schema verifies the pinned catalog/schema contract and sealed source-patch mutants in throwaway worktrees with complete cleanup.
- [ ] **CARD-08**: Dogfood CI exercises cards only through the WP-3 verifier surface and demonstrates a bad change blocked and a good change accepted.
- [ ] **PROV-03**: Every donor-adapted WP-4 Gate Card or helper file has an exact transformation entry in `UPSTREAM.md`.

### Final Program Evidence

- [ ] **EVID-01**: WP-0.2 through WP-4 are merged in dependency order with branch and merge commit IDs, local/integration gate results, and green CI links.
- [ ] **EVID-02**: A final canary-clean evidence summary is appended under `../shared/reviews/verified-change/` and explicitly records WP-0.1, WP-5, and WP-6 as owner-led/not executed.
- [ ] **EVID-03**: Autonomous execution stops after WP-4 and hands the verified build state to the owner without starting the demo/partner or decision-gate programs.

## Deferred Requirements

- **DEMO-01**: Package the canonical demo and onboard design partners — WP-5, owner-led after WP-4.
- **ADOPT-01**: Obtain two external required-check adoptions and record the decision — WP-6, owner-led.
- **MEMORY-01**: Build P-MEM only after ADOPT-01 succeeds.
- **PROFILE-01**: Build profiles only after the memory roadmap unlocks.
- **MODULE-01**: Build module/MCP-server surfaces only after profiles.

## Out of Scope

| Feature | Reason |
|---------|--------|
| WP-0.1 autonomous execution | Requires the owner's interactive desktop and manual display scaling |
| WP-5 and WP-6 autonomous execution | Owner-led product/adoption work; explicit stop boundary |
| P-MEM, P-PROF, P-MOD | Frozen until external adoption earns the roadmap |
| Self-evolution and kernel self-modification | Constitutionally excluded from the current product |
| Subscription bridging | Later convenience tier; default-off and not foundational |
| Unnamed cleanup or refactoring | No traceability to a WP completion contract |

## Traceability

Roadmap generation must map every current-program requirement to exactly one
phase. Deferred and out-of-scope requirements must not be scheduled.

---
*Requirements defined: 2026-08-16 from the authoritative verified-change program*
