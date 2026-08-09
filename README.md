# Wayland Nano — Track B (NanoK3)

Greenfield + vendored-crate implementation of the shared Wayland Nano
contracts, built by K3. Competing/companion implementation to Track A
(`../nano/`, Codex reduction fork). Comparison rules: `../shared/SCORECARD.md`.

- Constitution and boundaries: `ARCHITECTURE.md`
- Provenance: `UPSTREAM.md`
- Shared contracts/fixtures: `../shared/`
- Flux live evidence: `../shared/fixtures/flux/FINDINGS.md`

## Build

```powershell
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Status: P0-equivalent skeleton — compile/lint green on native Windows x64
(MSVC 1.95.0). No features yet; features land against the shared frozen
contracts.
