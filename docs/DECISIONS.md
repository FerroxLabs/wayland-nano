# NanoK3 decisions

- **D1 — Greenfield + vendored, not fork.** Per `resources/WAYLAND_NANO_PLAN_V3.md`
  §3, with the concession recorded in `resources/WAYLAND_NANO_BUILD_PLAN_V3_K3_REVIEW.md`:
  Track A's G2 may prove the fork; Track B is the extraction path running at full
  fidelity. The scorecard adjudicates.
- **D2 — 12-crate map is a soft budget.** Spike B-VND-01 closure results may
  merge or split crates; the map is an output, not an input.
- **D3 — Vendored crates are reference-only until closure analysis.** Not wired
  into the workspace build; byte-identical to donor @ `646f7c0a`; any future
  modification enters the `UPSTREAM.md` ledger file-by-file.
- **D4 — Fixtures before claims.** No endpoint, surface, or quirk is asserted
  without a scrubbed recorded body in `shared/fixtures/` (rule from
  `BUILD_PLAN_V3` §21 applied to ourselves; codified after the `/mcp`
  trailing-slash discovery).
- **D5 — Git identity `K3 (Wayland Nano Track B)`** so dual-track history is
  distinguishable at review time.
- **D6 — Secrets never persist.** Flux key lives only in `waylandnano/.secrets/`,
  read into env at call time, never in fixtures/evidence/memory. Owner advised
  to rotate (key transited chat).
- **D7 — Operating model.** K3 native goal mode as the durable driver +
  this STATUS.md as engineering truth + ijfw memory handoff for cross-session
  resume. Functionally mirrors Track A's `/goal` model; evidence formats follow
  `BUILD_PLAN_V3` §8 so gate receipts compare 1:1.
- **D8 — ConPTY/tty and elevated backend land deliberately late.** v1 spawns
  are non-interactive (`tty: false`). The ConPTY path (`portable-pty`/
  `shared_library`/`WinChild` web) is NOT ported; `tty=true` requests fail
  closed with a typed `SandboxUnavailable::ConPtyDeferred` error. The elevated
  backend (runner IPC) routes fail closed with
  `SandboxUnavailable::ElevatedBackendPending` until B-SBX-10 lands
  `elevated/`. Per BUILD_PLAN_V3 P8: unsupported controls are advertised
  explicitly and fail closed — never an unsandboxed fallback.
