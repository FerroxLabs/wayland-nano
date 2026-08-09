# B-VND-01 — vendored donor closure analysis

**Date:** 2026-08-09 · **Donor pin:** `646f7c0a` · **Method:** recursive
first-party BFS over `[dependencies]` (dev-deps excluded; **zero** optional
first-party edges exist — every edge below is really built).

## Measured closure (transitive)

| Vendored crate | First-party closure | Rust size | Heavy members |
|---|---|---|---|
| `codex-windows-sandbox-rs` | 18 crates | 242 files / 85,902 lines | codex-protocol 23.9k, codex-network-proxy 17.4k, codex-api 14.3k, codex-http-client 7.7k, codex-otel 7.4k |
| `codex-rollout` | 23 crates | 302 files / 113,091 lines | + codex-state 20.5k |
| `codex-skills` | 14 crates | 153 files / 64,982 lines | codex-shell-command 6.8k + shared heavies |

The explore-stage claim "self-contained, minimal upward coupling" holds only
for **direct** deps. Transitively, verbatim linking is entanglement: the
original no-fork rationale confirmed by measurement.

## The poisoned edges (shortest chains)

```text
sandbox:  codex-otel ─→ codex-api ─→ codex-protocol ─→ codex-network-proxy
                                     ├→ codex-execpolicy
                                     └→ codex-http-client ─→ codex-websocket-client
skills:   codex-protocol ─→ (same heavies)
rollout:  codex-protocol + codex-state + codex-otel
```

All heavy weight in the sandbox closure flows through **one** edge:
`codex-otel` (telemetry). The sandbox's other seeds (utils-pty 4.7k,
utils-absolute-path 872, utils-string 560) are clean.

## Decisions

| Component | Decision | Rationale |
|---|---|---|
| `windows-sandbox-rs` src (tokens, ACLs, Job Objects, WFP, dual-identity) | **PORT** into `nano-sandbox`, Nano-owned seams | the prize; ~15k lines of irreplaceable Windows containment semantics |
| `codex-otel` (7.4k) | **REPLACE** with ~100-line Nano telemetry facade | it's the single poison edge; Nano wants structured events, not OTel product wiring |
| `codex-utils-{pty,absolute-path,string,path,path-uri,cache,home-dir,async-utils}` (~10k total) | **VENDOR/port piecemeal** as needed | genuinely small, no heavies |
| `codex-protocol` (23.9k) | **EXTRACT-TYPES** — lift only the wire types a consumer actually names into `nano-types` | do not link; pulls execpolicy/http-client/network-proxy |
| `codex-state` (20.5k) | **REJECT** | Nano sessions = Kimi journal model (D-ref: plan v3 §6.5), not Codex SQLite state |
| `codex-rollout` crate as a unit | **REFERENCE-ONLY** — JSONL writer mechanics inform `nano-session` | its own closure is the heaviest of the three |
| `codex-shell-command` (6.8k) | **REFERENCE** — quoting/escaping semantics for `nano-tools` shell | will re-derive with tests |
| `codex-http-client` / `codex-network-proxy` / `codex-api` / `codex-websocket-client` (46k+) | **REJECT from build** | `nano-egress` owns networking; network-proxy kept as the managed-proxy *pattern* reference for spike 2 |
| `codex-execpolicy` (2.9k) | **REFERENCE** for policy-revision semantics | Nano defines its own policy contract against the shared P1 freeze |

## Consequences

1. `vendor/` stays **reference-only** — never a workspace member (D3 stands,
   now evidence-backed).
2. `nano-sandbox` becomes the first real port target (Track B spike-2
   equivalent): port sandbox semantics, stub telemetry, cut otel/protocol
   edges, prove against the vendored `sandbox_smoketests.py` as the oracle.
3. Effort shape: ~15k lines to port (sandbox) + ~2k (skills parser) +
   mechanics-only reading (rollout). No 65–113k-line transitive ballast.
4. Pending measurement: binary-size/build-time delta lands when the
   `nano-sandbox` port compiles — recorded here when it exists.
