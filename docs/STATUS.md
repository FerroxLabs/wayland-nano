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
- B-FLX-02 `8f31d76`: streaming/tools/thinking/cache probes; wire-2 gate FAILS
  (recommend Completions single v1 wire).
- B-VND-01 `9ce256b`: closure analysis; vendor stays reference-only; port
  strategy fixed (otel→facade, protocol→extract-types, state/net→reject).
- B-SBX-01 (this commit): `nano-sandbox` port increment 1 — telemetry facade
  (replaces codex-otel seam, `MetricsSink` trait), `winutil.rs` +
  `path_normalization.rs` ported with donor tests; 5 tests green on native
  Windows, workspace clippy `-D warnings` clean. Ledger updated.

## Task cards

### B-ENV-01 — clean-VM rig
- Owner: K3 mainline | Files: `scripts/clean-sandbox/`
- Prerequisites: ~~Windows Sandbox feature~~ **Windows Sandbox client binary not
  present on host (2026-08-09); optional-feature query needs admin.**
  Fallback options: (a) owner enables Windows Sandbox or Hyper-V (admin +
  possible reboot), (b) local standard-user account (`net user`) as the
  clean-profile context, (c) repro on a second physical Windows machine.
- Work: clean execution profile + bootstrap; upstreams read-only, scratch RW,
  Defender on, no dev tools
- Commands: `scripts/clean-sandbox/launch.ps1`
- Evidence: boot log + `whoami`/`ver` capture in `artifacts/evidence/env/`
- Rollback: delete `scripts/clean-sandbox/` (no host state)
- Status: **blocked on environment choice** — recommend (b) standard-user
  account for B-ENV-02 speed, (a) for the real §8G gate later. Owner
  preference needed only if admin rights required; will proceed with (b) by
  default next turn.

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
- Status: **done 2026-08-09** — all probes 200; wire-2 gate FAILED (thinking
  dropped even on pinned claude-sonnet-5; no cache_control creation);
  recommendation: Completions as single v1 production wire; alias rotation
  documented (use `flux-pinned-*` for deterministic fixtures); `/mcp/` catalog
  empty (`tools:[]`). `Retry-After` not observed (no 429 induced — deferred to
  adversarial batch).

### B-VND-01 — vendored closure analysis
- Owner: K3 mainline | Files: `vendor/`, `docs/spikes/donor-closure.md`
- Prerequisites: none
- Work: resolve the 12 first-party codex deps pulled by vendored crates;
  keep/port/reject per crate; binary-size and build-time delta
- Evidence: machine-readable `cargo metadata` closure + decision table
- Rollback: none (analysis only)
- Status: **done 2026-08-09** — transitive closures measured: sandbox 18
  crates/86k lines, rollout 23/113k, skills 14/65k; zero optional edges.
  All sandbox heavies flow through one `codex-otel` edge. Decisions:
  PORT sandbox semantics w/ otel replaced by facade; EXTRACT-TYPES from
  codex-protocol; REJECT codex-state + networking stack (nano-egress owns);
  rollout reference-only. Vendor dir stays out of the build (D3 confirmed
  by measurement). Binary-size delta pending `nano-sandbox` port.

## External prerequisites

- Flux live key: **present** (`.secrets/`, owner-provided, rotate-on-transcript caveat)
- macOS runners: not registered (mercy rule: deferred, not lost)
- Windows ARM64 hardware: not present (compile-gate only until available)

## Next action

B-ENV-01 (clean-VM rig), then B-FLX-02 in parallel with B-ENV-02.

### B-SBX-02 — token.rs port (done this turn)
- token.rs + token_tests.rs ported near-verbatim (windows-sys pinned 0.52 to
  match donor HANDLE semantics; two `# Safety` doc sections added for clippy)
- 6 tests green incl. real `CreateRestrictedToken` round-trip on this host
- B-SBX-03 done: acl.rs + live deny-write test (077a30d)
## B-CORE-01 (this commit): nano-core permission type layer — extracted from
  codex-protocol (serde-compatible); AbsolutePathBuf minimal port; 9 tests green

- B-SBX-06 done: job.rs extraction + process.rs port — LIVE contained spawn +
  whole-tree kill proven on this host (15 tests green)
- B-SBX-05 done: logging + proc_thread_attr + desktop ported (14 tests green)
- B-SBX-10 done: elevated backend arm live (runner IPC wired into
  unified_exec router; Elevated/proxy/SID routes active; ConPTY still
  deferred per D8)
- B-SBX-09 done: unified_exec legacy backend — END-TO-END containment proven
  (restricted spawn echoes; inside-root write ok; outside-root write DENIED;
  3 tests through full ported stack, ~30s each from ACL/token work)
- C1.3 done: nano-session journal — append-only Ops, torn-tail
  tolerance, unknown-skip, idempotent ids, stranded-phase reset, compaction
  equivalence (10 tests; design bug caught: compaction carries effect inventory)
- B-SBX-04 done: env.rs + wfp.rs + filter_specs ported (9 tests green); workspace
  edition bumped to 2024 to match donor (let-chains); unsafe_op_in_unsafe_fn
  allow carried at crate root

### B-SBX-01 — nano-sandbox port increment 1 (foundation)
- Owner: K3 mainline | Files: `crates/nano-sandbox/src/{lib,telemetry,winutil,path_normalization}.rs`
- Work: telemetry facade + leaf modules, donor tests retained
- Status: done (see Completed evidence)
- Next: **B-SBX-02** — port `token.rs` (restricted token, 510 lines) and
  `acl.rs` (DACL allow/deny, 802 lines); both depend only on winutil.
