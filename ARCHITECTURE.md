# Wayland Nano — Track B Architecture Constitution

Greenfield + vendored-crate implementation of the shared Wayland Nano contracts.
Track A (reduction fork) lives in `../nano/` — never write there.
Shared contracts, fixtures, and the scorecard live in `../shared/`.

## Constitution (every feature must pass)

1. Does it materially improve execution of a single agent task? If no, reject.
2. Can Wayland Desktop provide it? If yes, keep it out of Nano.
3. Is it orchestration rather than execution? If yes, it belongs in Desktop/Core.
4. Does it increase platform complexity disproportionately? If yes, architecture review.
5. Can it live behind an extension or protocol boundary? If yes, keep it outside the core.

## Boundaries

- The agent loop never sees OS details — `nano-platform` owns the OS boundary.
- All outbound HTTP flows through `nano-egress` — enforced by workspace clippy.
- Provider-specific behavior lives behind `nano-model`; universal types carry
  extensible metadata, never Flux-specific fields.
- Security is two layers: in-process egress chokepoint + OS containment.
  Fail closed everywhere. `SANDBOX_UNAVAILABLE`, never silent downgrade.
- Sessions are an append-only Op journal; no remote compaction.
- Subagents are temporary bounded helpers (fan-out 4, spawn depth 1 — no
  nested subagents; crates/nano-agent/src/tasks.rs), never organizations.

## Windows

Windows x64 is release-blocking from commit one. Native MSVC only
(`x86_64-pc-windows-msvc`). WSL is an opt-in execution target, never the
Windows implementation and never counted as Windows compliance.
