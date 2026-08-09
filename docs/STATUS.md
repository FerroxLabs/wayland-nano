# NanoK3 (Track B) status

## Current position

- Goal: active (K3 native goal mode, created 2026-08-09; objective = C1→C2→C3
  per `../../shared/SCORECARD.md`)
- Phase: foundation complete; wave 1 (environment + fixtures + closure) ready
- Checkpoint: C1 in progress — **no claims posted**
- Last green: skeleton compile + clippy `-D warnings` clean, commit `b4ab8a0`
- Track A (observed read-only, `../nano/docs/STATUS.md`): P0, G0 pending on 4
  upstream Windows sandbox failures; clean-VM reproduction needed by both tracks

## Completed task evidence

- Scaffold `b4ab8a0`: 12-crate workspace, Rust 1.95.0 pinned, egress clippy ban
  active, constitution + provenance committed, 3 Codex crates vendored
  reference-only.
- Flux fixture batch 1 → `../../shared/fixtures/flux/FINDINGS.md`: all six DoD
  endpoints live-verified; 3 quirks recorded (per-surface alias routing,
  reasoning-eats-budget, `/mcp` trailing-slash).

## Task cards

### B-ENV-01 — clean-VM rig
- Owner: K3 mainline | Files: `scripts/clean-sandbox/`
- Prerequisites: Windows Sandbox feature present (Win11 Pro)
- Work: `.wsb` profile + bootstrap script; upstreams mapped read-only, scratch
  RW, Defender on, no dev tools
- Commands: `scripts/clean-sandbox/launch.ps1`
- Evidence: sandbox boot log + `whoami`/`ver` capture in `artifacts/evidence/env/`
- Rollback: delete `scripts/clean-sandbox/` (no host state)
- Status: pending

### B-ENV-02 — reproduce Track A baseline failures
- Owner: K3 mainline | Files: `vendor/codex-windows-sandbox-rs`, `artifacts/evidence/env/`
- Prerequisites: B-ENV-01
- Work: run vendored `sandbox_smoketests.py` + the 4 failing nextest cases
  (cancellation timeout, `CreateProcessWithLogonW: 2`, 2 legacy timeouts) on the
  clean VM
- Evidence: pass/fail matrix vs Track A run `c59e02e0`; environmental-vs-real
  classification → `shared/reviews/` note for Track A's G0 (read-only input to
  their review, not a claim against them)
- Rollback: none (read-only tests)
- Status: pending

### B-FLX-02 — Flux fixture batch 2
- Owner: K3 mainline | Files: `../shared/fixtures/flux/`, `scripts/flux-probe/`
- Prerequisites: `.secrets/flux-test-key` (present)
- Work: SSE streaming on all 3 inference wires; tool calls (single/parallel);
  thinking-block + `cache_control` pass-through on `/anthropic`; omit-`max_tokens`
  on completions; `Retry-After` observation; `/mcp/` `tools/list`
- Evidence: scrubbed bodies + updated `FINDINGS.md`; pass-through verdict
  (gates wire-2 policy per plan v3 §5)
- Rollback: delete new fixture files
- Status: pending

### B-VND-01 — vendored closure analysis
- Owner: K3 mainline | Files: `vendor/`, `docs/spikes/donor-closure.md`
- Prerequisites: none
- Work: resolve the 12 first-party codex deps pulled by vendored crates;
  keep/port/reject per crate; binary-size and build-time delta
- Evidence: machine-readable `cargo metadata` closure + decision table
- Rollback: none (analysis only)
- Status: pending

## External prerequisites

- Flux live key: **present** (`.secrets/`, owner-provided, rotate-on-transcript caveat)
- macOS runners: not registered (mercy rule: deferred, not lost)
- Windows ARM64 hardware: not present (compile-gate only until available)

## Next action

B-ENV-01 (clean-VM rig), then B-FLX-02 in parallel with B-ENV-02.
