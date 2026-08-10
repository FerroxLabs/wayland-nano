# C2 metrics — Track B (nano-k3)

Measured 2026-08-10 on the primary truth machine (Win11 Pro, i9-13900KF,
128 GB RAM, Rust 1.95.0 MSVC, release profile: lto=thin, codegen-units=1,
strip=symbols). Agent-path numbers; no live model calls anywhere (G-C2-4).

Reproduce:

```
cargo build --release -p nano-cli
./target/release/nanok3-acp-profile.exe   # locates the sibling nanok3.exe; NANOK3_EXE overrides
                                          # `--json <path>` also writes a machine-readable report
```

The profiler spawns `nanok3 acp-host` with a placeholder `FLUX_API_KEY`
(the host refuses to start without one; `initialize`/`session/new` never
touch the network and no prompt is ever sent) and replays a scripted fixture
turn through the real NDJSON host loop.

## G-C2-4 — full perf set: cold start, idle RSS, active RSS, task wall time

Re-measured 2026-08-10 with the extended profiler (`--json` machine-readable
report). All numbers from one local release run on the truth machine; the
same profiler runs in CI (see below).

| Metric | Value | Method |
|---|---|---|
| acp-host spawn→ready (cold process, "cold start") | **median 5.62 ms** (mean 5.99, min 5.38, max 9.55; n=10) | spawn → first `initialize` response |
| — first-ever run cold effect | max 257.5 ms on the first spawn of a run (AV/DLL warm-up), subsequent spawns ~6 ms | same |
| initialize handshake (warm process) | **median 0.02 ms** (mean 0.02, max 0.04; n=50) | `initialize` round trip on a live host |
| fixture-replayed turn frame throughput | **~3.26 M frames/sec** (5,101 frames / 588,745 bytes in 1.57 ms) | 50 turns × 100 scripted events through `run_host_loop` (encode + frame + flush) into a sink |
| task wall time (1 fixture turn, end-to-end) | **median 0.03 ms** (mean 0.03, max 0.05; n=50) | message frame in → terminal `stream_end` out through `run_host_loop` |
| idle RSS | **7.67 MiB** (8,040,448 bytes, median of 3 samples after 300 ms settle) | spawned `acp-host`, initialized, no turn running; read externally via `Get-Process` WorkingSet64 |
| active RSS | **5.20 MiB** (5,447,680 bytes peak working set) | profiler process peak across a 51,001-frame fixture turn storm through the real host loop |

Notes:

- "Spawn→ready" is defined on the ACP wire as spawn → first `initialize`
  response; the ACP agent emits nothing unsolicited, so that response is the
  first sign of life.
- Frame throughput and task wall time measure the codec + host-loop framing
  path only (`encode_event` → write → flush per frame), deliberately NOT
  live-model timing — model latency is a Flux property, not an agent-path
  property.
- RSS is read externally, never self-reported by the measured process:
  Windows `Get-Process` `(Peak)WorkingSet64` via a PowerShell one-liner
  (unix: `ps` RSS / VmHWM). The profiler is a dev bin, not shipped runtime,
  so the shell-out is acceptable.
- Active-RSS scope, stated exactly: the load is generated IN the profiler
  process — a spawned `acp-host` cannot run turns without a live model
  (live turns never run in CI, and the profiler makes no network calls).
  The number is the peak working set of the real codec/host-loop/framing
  code path under a turn storm, not of a model-driven `acp-host`.
- Steady-state throughput is ~2.6–3.3 M frames/sec across runs; the
  flush-per-frame cadence enforced by the G-C2-1 test costs nothing
  measurable at this scale.

### CI measurement (the scorecard C2.4 oracle)

`.github/workflows/gate.yml` runs a `gate-perf (C2.4, advisory)` step on the
windows-latest leg (`continue-on-error: true` — perf never fails the gate):
`cargo build --release -p nano-cli` then
`./target/release/nanok3-acp-profile.exe --json nano-k3/artifacts/evidence/ci/perf-<os>.json`.
The leg's evidence-manifest step merges that JSON as a `perf` key into
`nano-k3/artifacts/evidence/ci/<run-id>-manifest.json` and uploads it with
the `ci-manifest-<os>` artifact, so every CI run records the full perf set
on a hosted runner.

## G-C2-1 — frame cadence (conformance test, not a number)

`nano-protocol/src/host.rs::tests::streamed_turn_cadence_orders_frames_and_flushes_between_them`
drives `run_host_loop` with a scripted event sequence into a write/flush
recording probe and asserts: `ready` first, per-event frames in scripted
order, terminal `stream_end` last, and a flush boundary between every pair
of frames (no two frames coalesce unflushed). Runs in `cargo test
-p nano-protocol`, no live model calls.

## ARM64 Windows compile-gate (local cross-check)

Validated 2026-08-10 on the primary truth machine (Win11 Pro x64 host,
i9-13900KF, Rust 1.95.0 pinned toolchain). Per the standing rule this is a
**compile-gate only** — no ARM64 binaries were executed and no ARM64 hardware
claim is made.

| Command | Result |
|---|---|
| `rustup target add --toolchain 1.95.0 aarch64-pc-windows-msvc` | PASS (component installed from network) |
| `cargo check --workspace --target aarch64-pc-windows-msvc` | **PASS** (exit 0, all 12 workspace crates) |
| `cargo clippy --workspace --target aarch64-pc-windows-msvc --all-targets -- -D warnings` | **PASS** (exit 0, no warnings) |

No source changes were required: no x64-only intrinsics/inline-asm in the
windows sandbox port, `windows-sys` 0.52 feature gates are arch-neutral, and
the `.sbpl` `include_str!` paths are macOS-gated.

Environment note: the x64 host has no ARM64-capable C toolchain (VS 2022
BuildTools carries only x64/x86 tools; no `clang`/`clang-cl`). `ring`
(via rustls→reqwest) hard-requires `clang` for `aarch64-pc-windows-msvc`
(`ring/build.rs` forces `clang` on PATH for Windows+AArch64). The gate was
run with a portable LLVM 18.1.8
(`clang+llvm-18.1.8-x86_64-pc-windows-msvc.tar.xz`, extracted to a scratch
dir, deleted after the run) wired in via:

```
export PATH="<scratch-llvm>/bin:$PATH"                       # provides clang
export AR_aarch64_pc_windows_msvc='<scratch-llvm>\bin\llvm-ar.exe'
```

(`llvm-lib.exe` does not work as `AR` here — cc-rs passes ar-style `cq`
flags; `llvm-ar.exe` accepts them and writes COFF archives.) UCRT/MSVC
headers were auto-detected by clang from the installed BuildTools +
Windows SDK 10.0.26100.0. Tests were not run for this target (cannot
execute ARM64 binaries on this host) — check+clippy only, as specified.
