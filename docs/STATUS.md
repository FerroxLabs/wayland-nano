# NanoK3 (Track B) status

## Current position

### 48H SPRINT STATE (2026-08-10, swarm landing: unix parity + adversarial + CI + packaging DONE)
- **UNIX PARITY LANDED** (b68e7a9): macOS seatbelt + Linux landlock/seccomp ported
  into nano-sandbox; cargo check + clippy clean on linux-gnu + apple-darwin targets;
  22 builder tests run on Windows host. bwrap deferred (UPSTREAM.md TODO).
- **ADVERSARIAL SUITE** (6e44921): 31 tests; found + fixed 6 real holes (egress
  redirect bypass, 3 credential-leak displays, junction/symlink write escape).
  nano-model transport leak closed via shared sanitizer. No test weakened.
- **CI MATRIX AUTHORED** (0ecf88a): 6 targets (win x64/arm64, macos-13/14,
  ubuntu x64/arm64); unix legs go green on first hosted run. Not yet pushed to a remote.
- **NPM SCAFFOLD** (f387c03): zero-dep installer + launcher, win32-x64 staged,
  win32-arm64 runtime-rejected per compile-gate-only rule.
- **COMPLIANCE CATALOG** (973f5b7): docs/compliance/SCENARIO_CATALOG.md — 290-test
  inventory keyed to COMP-* IDs + 16-gap register. NOTE: shared/contracts/ is EMPTY
  (Track A freeze never landed, gap G-CTR-1) — catalog keys off SCORECARD §2 instead.
- Workspace: 290+ tests green, clippy -D warnings clean, HEAD = 973f5b7.

### Previous state (2026-08-10, post-compaction: Desktop registration DONE)
- **ACP ADAPTER LIVE** (commit 2eddab7): `nanok3 acp-host` speaks ACP; slice proves
  initialize v1 → session/new → prompt with streamed updates → end_turn → canary
  clean → zero orphans. Release binary: target/release/nanok3.exe (6.4MB).
- **DESKTOP REGISTRATION DONE**: `acp.customAgents` entry `nanok3` written to
  `%APPDATA%/Wayland/config/wayland-config.txt` (backup: `../../.tmp/wayland-config-backup-20260810-071442.txt`).
  Launch: `target/release/nanok3.exe acp-host`, env `FLUX_API_KEY_FILE` →
  `../../.secrets/flux-test-key` (new `flux_key.rs` resolver: env first, then
  key-file path — secret stays out of the config blob). Exact launch config
  smoke-verified: initialize handshake clean with file-only credential.
  **Awaiting owner: fully quit + restart Desktop, pick "Nano K3", run one
  real prompt.** (Desktop was running during the edit; if the entry vanished,
  re-apply after quit.)
- **NEXT (in order)**: (a) OWNER live conversation in Desktop UI;
  (b) OWNER provisioning → C1.2 → C1 claim; (c) Unix containment port
  (seatbelt/landlock from Codex); (d) 6-target CI matrix; (e) compliance matrix;
  (f) adversarial formalization; (g) NPM packaging (signing via NPM per owner).
- ARM64 Windows: compile-gate only, not claimed without hardware.
- User directive: NOTHING cut — parity/signing/compliance/adversarial all required.

### Original position

- Goal: active (K3 native goal mode; objective = C1→C2→C3 per `../../shared/SCORECARD.md`)
- Phase: **C1 code-complete; C2 + C3 CLAIMED (shared/reviews/C2,C3)**; awaiting owner: C2/C3 promotion decisions + elevated provisioning for C1.2 + C1 claim
- Checkpoint: C1 ready-to-prove (no claim posted); C2 in progress
- Last green: workspace-wide tests + clippy `-D warnings` clean, commit HEAD
- Track A (observed read-only): G0-era sandbox corrections ongoing

## C1 scoreboard

| Leg | Status | Evidence |
|---|---|---|
| C1.1 Flux fixtures | ✅ | all six endpoints live-verified, 2 batches + client smoke; wire-2 gate failed → Completions = v1 wire (`shared/fixtures/flux/FINDINGS.md`) |
| C1.2 containment mechanisms | ✅ ported + unit/integration proven | 40+ files ported; live proofs: outside-root write DENIED (unified_exec + capture), DACL deny blocks write, Job tree-kill 3–4ms no survivors |
| C1.2 full criterion | ⏳ **awaiting owner elevated provisioning** | `scripts/provision/README.md`; proof harness green: `scripts/c12-proof/` (8 pass / 0 fail / 2 provisioning-gated skips) |
| C1.3 session journal | ✅ | 10 kill-boundary tests; torn-tail, unknown-skip, idempotence, compaction equivalence |
| C1.4/5/6 metrics | ✅ | `docs/metrics/C1-metrics.md`: bins ~1MB, 26s release build, 71-crate production closure |

## Capabilities flipped after slice proof (2026-08-10)

- `mcp: true` — proven: model called `mcp__fake__probe` through the registry in the live slice; marker returned and reported.
- `skills: true` (extensions) — proven: skill instruction (SKILLCONFIRMED) visible in model reply in the live slice.
- The honesty rule held: flags flipped only AFTER end-to-end proof.

## C3 (vertical slice passing)

- `nanok3` binary: `doctor` (real self-diagnostics, exit 0) + `protocol-host`.
- `nano-protocol`: NDJSON wire, honest capabilities, malformed-tolerant codec, ready-first host loop.
- Vertical slice (`nano-cli/tests/vertical_slice.rs`): simulated Desktop host spawns real binary — ready-first, ping→pong, live framed turn (model read fixture file), clean exit, **zero orphans**.

## C2 foundations (landed after C1 code-complete)

- `nano-egress`: deny-by-default policy chokepoint, redaction-proof errors, flux_only preset.
- `nano-model`: neutral types, SSE parser, **Flux Completions client (v1 primary wire)**, Kimi retry policy, fixture-replay tests + **live smoke verified against real Flux** (egress → client → API → parsed events).
- `nano-tools`: fs (policy-enforced read/write/edit, sensitive defaults, bounded reads) + search (glob/content, deny-invisible, cycle-safe, bounded).

## Next actions (in order)

1. **OWNER: C2 promotion decision** (claim posted) + **elevated provisioning** (dry-run review first: `cargo run -p nano-sandbox --bin nanok3-provision-dry-run`).
2. C1.2 full proof → C1 claim to `shared/reviews/C1/`.
3. C2: shell tool (via sandbox SpawnSpec), agent loop (turn state machine + loop protection), journal integration.
