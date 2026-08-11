# C2 metrics — Track B (nano-k3)

> **Naming note (rebrand, 2026-08-11):** these numbers were measured under the
> old NanoK3 codename; binary/env names below (`nanok3-acp-profile.exe`,
> `nanok3.exe`, `NANOK3_EXE`) are quoted as recorded. Current names per
> `docs/REBRAND.md`: `wayland-nano-acp-profile.exe`, `wayland-nano.exe`,
> `NANO_EXE`. The measurements themselves are unaffected by the rename.

Measured 2026-08-10 on the primary truth machine (Win11 Pro, i9-13900KF,
128 GB RAM, Rust 1.95.0 MSVC, release profile: lto=thin, codegen-units=1,
strip=symbols). Agent-path numbers. The fixture metrics (cold start,
handshake, throughput, wall time, idle RSS, in-process storm RSS) make no
live model calls; the active-AGENT RSS row is explicitly LIVE-measured
(opt-in, keyed) — each row is labeled (G-C2-4).

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
same profiler runs in CI (see below). Rows are labeled **CI-fixture-measured**
(no live model; this is what the CI step records) or **live-measured**
(opt-in keyed run; CI prints `skipped` for it and exits 0).

| Metric | Value | Method | Label |
|---|---|---|---|
| acp-host spawn→ready (cold process, "cold start") | **median 6.59 ms** (mean 6.93, min 6.50, max 9.55; n=10) | spawn → first `initialize` response | CI-fixture-measured |
| — first-ever run cold effect | max 257.5 ms on the first spawn of a run (AV/DLL warm-up), subsequent spawns ~6 ms | same | CI-fixture-measured |
| initialize handshake (warm process) | **median 0.02 ms** (mean 0.03, max 0.05; n=50) | `initialize` round trip on a live host | CI-fixture-measured |
| fixture-replayed turn frame throughput | **~2.64 M frames/sec** (5,101 frames / 588,745 bytes in 1.93 ms) | 50 turns × 100 scripted events through `run_host_loop` (encode + frame + flush) into a sink | CI-fixture-measured |
| task wall time (1 fixture turn, end-to-end) | **median 0.03 ms** (mean 0.04, max 0.05; n=50) | message frame in → terminal `stream_end` out through `run_host_loop` | CI-fixture-measured |
| idle RSS | **9.81 MiB** (10,285,056 bytes, median of 3 samples after 300 ms settle) | spawned `acp-host`, initialized, no turn running; read externally via `Get-Process` WorkingSet64 | CI-fixture-measured |
| active RSS (codec/host-loop path) | **5.13 MiB** (5,382,144 bytes peak working set) | profiler process peak across a 51,001-frame fixture turn storm through the real host loop | CI-fixture-measured |
| **active RSS (spawned agent, live turn)** | **13.59 MiB** (14,254,080 bytes child peak `WorkingSet64`) | the REAL spawned `acp-host` child, measured EXTERNALLY (250 ms `WorkingSet64` sampler + `PeakWorkingSet64`) while it processed a live multi-tool turn (fs_read ×3 + fs_write, permissions auto-approved; 11 frames, 15.9 s wall) | **live-measured** (keyed run 2026-08-10; CI prints `skipped`) |

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
- Active-RSS scope, stated exactly — TWO numbers, two subjects:
  - **CI-fixture-measured (5.13 MiB):** the load is generated IN the
    profiler process — a spawned `acp-host` cannot run turns without a live
    model (live turns never run in CI). This number is the peak working set
    of the real codec/host-loop/framing code path under a turn storm.
  - **Live-measured (13.59 MiB):** the scorecard's active-agent number —
    the spawned `acp-host` CHILD's own peak working set during a real
    live multi-tool Flux turn, read externally via `Get-Process`
    `(Peak)WorkingSet64`. Opt-in: the profiler runs this leg only when
    `FLUX_TEST_KEY`/`FLUX_API_KEY` is present; otherwise it prints
    `skipped` for the metric and still exits 0 (verified 2026-08-10:
    `active RSS (live acp-host)  skipped (no FLUX_TEST_KEY / FLUX_API_KEY)`,
    exit 0). The JSON report records it as `active_agent_rss_bytes` /
    `active_agent_rss_kind` (`live-measured spawned acp-host child,
    multi-tool turn` vs `skipped-no-key`).
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

Live complement (the scorecard C2.3 oracle): `crates/nano-cli/tests/
c2_watchdog.rs` is a Desktop-style watchdog harness over a live long turn
against the real spawned `acp-host` — per-frame arrival timestamps, active
inter-frame gaps asserted under Desktop's `acp.promptTimeout` default
(300 s, stricter than the scorecard's 600 s). Live-gated on `FLUX_TEST_KEY`
(self-skips without it). Measured 2026-08-10 (live): 23 update frames over
a 90.3 s multi-tool essay turn, max active inter-frame gap **11,195.2 ms**
(bound 300,000 ms), turn `end_turn`, stream well-ordered; manifest
preserved at `shared/reviews/C2/c2-watchdog-manifest.json`.

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
