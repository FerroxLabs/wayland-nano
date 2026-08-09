# NanoK3 (Track B) status

## Current position

- Goal: active (K3 native goal mode; objective = C1→C2→C3 per `../../shared/SCORECARD.md`)
- Phase: **C1 code-complete; C2 CLAIMED (shared/reviews/C2/trackb-claim.md, 2 deferrals)**; awaiting owner elevated provisioning for C1.2 + C2 promotion decision
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

## C2 foundations (landed after C1 code-complete)

- `nano-egress`: deny-by-default policy chokepoint, redaction-proof errors, flux_only preset.
- `nano-model`: neutral types, SSE parser, **Flux Completions client (v1 primary wire)**, Kimi retry policy, fixture-replay tests + **live smoke verified against real Flux** (egress → client → API → parsed events).
- `nano-tools`: fs (policy-enforced read/write/edit, sensitive defaults, bounded reads) + search (glob/content, deny-invisible, cycle-safe, bounded).

## Next actions (in order)

1. **OWNER: C2 promotion decision** (claim posted) + **elevated provisioning** (dry-run review first: `cargo run -p nano-sandbox --bin nanok3-provision-dry-run`).
2. C1.2 full proof → C1 claim to `shared/reviews/C1/`.
3. C2: shell tool (via sandbox SpawnSpec), agent loop (turn state machine + loop protection), journal integration.
