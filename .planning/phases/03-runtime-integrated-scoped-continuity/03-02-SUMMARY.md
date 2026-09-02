# 03-02 Summary: host MemoryPolicy source + MemoryRecall admission

## Scope and base

- Base: `7b71f32b79c2d25384d2727b3725c952294d8901` (`origin/master`, includes
  the merged 03-01 mem-sec pack and the PR #21 planning amendment).
- Worktree: `D:/Development/waylandnano/wayland-nano/.tmp-wt-p3-admission`.
- Branch: `feat/p3-memory-policy`.
- Anti-scope preserved (D3-08 verbatim): no KG pass-3, hosted memory, MCP
  memory exposure, extraction, global reads, schema rename,
  scheduler/registry/UI/provider work. This plan enables a continuity MODE;
  it wires no retrieval seam (03-03) and measures nothing (03-05).

## Policy-source decision record (owner-approved 2026-09-02, PR #21 amendment)

- DECISION: the host MemoryPolicy source is strict
  `$NANO_HOME/memory-policy.toml` with `deny_unknown_fields`, parsed into the
  frozen nano-memory `MemoryPolicy` type, plus the MEMORY-CONTRACT §6.8
  config-file agent registry (`$NANO_HOME/agents/*.agent.toml`, filename stem
  must equal the declared id, `main` implicit-reserved).
- REJECTED: extending the `NANO_MEMORY_*` env surface. Those variables remain
  exclusively the quarantined legacy Markdown store behavior (03-04's
  migration target) and are untouched by this plan.
- Posture: absent/empty source resolves `enabled: false` (default-off);
  every knob is mandatory so an omitted field is a typed error rather than
  inheriting the widened `MemoryPolicy::default()` (tighten-only — there is
  no widen path); `read_scope = "Global"`, unknown `min_tier`/`source_trust`/
  `agent_scope` values, unknown fields, and malformed/duplicate/
  stem-mismatched agent files are all typed errors, never coerced.
- Boundary: this module resolves only. `MemoryStore::validate_policy`
  (nano-memory store.rs) remains the real fail-closed store-open gate, and
  `Op::MemoryPolicyResolved` journaling with explicit attribution is 03-03's
  seam obligation after its D3-06 topology decision. No journal or store is
  opened here.

## Commit boundaries

1. `eecc0d7` — `feat(cli): strict host memory-policy source and agent registry`
   (Task 1: module, manifest/lock edges, MemoryHostConfig + shared resolver
   wiring, harness call-site updates).
2. `6142ac7` — `feat(activation): admit memory_recall with pinned fallback
   semantics` (Task 2: admission arm flip, typed `AdmittedContinuity` on
   `AdmittedToken`, typed-negative matrix, CLI mint vocabulary pin).
3. The summary + `.continue-here.md` resolution note ride the final commit;
   its SHA is the PR head and is intentionally not self-recorded here.

## Failing-first evidence

- Task 1: with `resolve` stubbed to default-off, 18 of 20
  `memory_policy` tests failed (every parse-error, registry, and
  exact-resolution row); after implementation all 20 pass.
- Task 2: against the unconditional `ContinuityNotEnabled` arm, the new rows
  failed to compile (`AdmittedContinuity`/`continuity()` absent) and the
  admitted `memory_recall + fallback:none` row could not exist. One real
  behavioral surprise surfaced and was fixed by strengthening the test, not
  the gate: `session_resume` without an explicit binding fails closed in the
  binding lookup (`ResumeDrift`) BEFORE the fallback check runs, so the
  widening rows supply an exact binding to isolate `FallbackUnauthorized`.
- CLI mint vocabulary test pins `fresh`/`session_resume` with
  `fallback: none` over a real enrolled local-CLI issuer keyref.

## Evidence (exact commands, this worktree, CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano-p3-admission)

- `cargo test -p nano-cli memory_policy -- --test-threads=1`: 20 passed, 0 failed.
- `cargo test -p nano-cli --test activation_cli -- --test-threads=1`: 7 passed, 0 failed.
- `cargo test -p nano-activation --test admission_matrix -- --test-threads=1`: 4 passed, 0 failed (includes the new typed negatives).
- `cargo test -p nano-activation -- --test-threads=1`: all targets green, 0 failures.
- `cargo test -p nano-cli -- --test-threads=1`: 414 passed, 0 failed, 1 ignored (live-gated PDF test self-skips without `FLUX_API_KEY_FILE`, per the live-gate rule).
- `cargo clippy -p nano-cli -p nano-activation --all-targets -- -D warnings`: green.
- `cargo fmt --all --check`: green.

## Typed-negative coverage (Task 2 matrix rows)

- `memory_recall + fallback:none` → admitted; token retains
  `AdmittedStrategy::MemoryRecall` / `AdmittedFallback::None`.
- `memory_recall + fallback:fresh` → admitted; retains `Fresh`.
- `memory_recall + fallback:memory_recall` → `FallbackUnauthorized` (self-fallback).
- `fresh + fallback:fresh|memory_recall` → `FallbackUnauthorized`.
- `session_resume + fallback:fresh|memory_recall` → `FallbackUnauthorized`
  (exact binding supplied so the fallback check is isolated).
- `session_resume` drift mismatch → `ResumeDrift` (template byte-unchanged;
  regression row).
- `memory_recall` under expired carrier → `AssertionExpired`; malformed
  resume fingerprint → `ResumeDrift`; revoked issuer key → `RevokedKey`.
- Recorded, never faked (03-03 seam-time rows): policy-disabled/unavailable
  memory at session bootstrap refuses `fallback:none` with typed
  `ContinuityNotEnabled` and degrades `fallback:fresh` with one journaled
  receipt. Admission cannot observe the seam; the rows are documented in the
  matrix test header.

## Round-trip / schema notes

- The signed carrier schema and canonical bytes are unchanged; the admitted
  continuity value is derived post-validation and carried typed on
  `AdmittedToken` (signed bytes → validated typed value round-trip asserted
  in the matrix). `AdmittedFallback::Fresh` is constructible only beside
  `MemoryRecall` — the type encodes the validation result.
- The new config formats are read-only host sources; same-commit parse
  coverage asserts exact field-for-field resolution of a complete
  `memory-policy.toml` and the registry reader's strict accept/refuse rows.

## Deviations and notes for the verifier

- `crates/nano-activation/src/lib.rs` was NOT touched: the signed carrier
  vocabulary stays `pub(crate)`; the typed read-only value lives in
  `admission.rs` as `AdmittedContinuity`/`AdmittedStrategy`/`AdmittedFallback`
  (lane table assigns only `admission.rs` in nano-activation).
- 17 nano-cli test harnesses construct `MemoryHostConfig` literally; each
  gained `policy: ResolvedMemoryPolicy::disabled()` (the default-off
  resolution) — mechanical field additions only, no behavior change.
- `Cargo.lock` delta is purely additive (`nano-memory`, `toml` edges on
  nano-cli); no version changes (T-03-SC).
- `ConfiguredAgents` duplicate detection: with stem==id enforced, duplicate
  ids are checked before stem matching so two files declaring one id fail as
  `DuplicateAgent` regardless of filenames.
- Worth a second look: the `session_resume` bind-lookup ordering note above
  (refusal happens before `validate_continuity` when no binding is passed) —
  pre-existing behavior, unchanged, but the new widening rows depend on it.
