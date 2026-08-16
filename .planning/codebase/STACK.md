# Technology Stack

**Analysis Date:** 2026-08-16

## Languages

**Primary:**
- Rust 2024 edition - All runtime, protocol, sandbox, agent, model, tool, and CLI implementation under `wayland-nano/crates/`; workspace policy is defined in `wayland-nano/Cargo.toml`.

**Secondary:**
- PowerShell - Provisioning, evidence collection, proof harnesses, and npm package assembly under `wayland-nano/scripts/` and `wayland-nano/packaging/npm/scripts/`.
- JavaScript (Node.js >=18) - Zero-dependency npm installer and command shim in `wayland-nano/packaging/npm/bin/install.js` and `wayland-nano/packaging/npm/bin/wayland-nano.js`.
- YAML - GitHub Actions gate and release automation in `wayland-nano/.github/workflows/gate.yml` and `wayland-nano/.github/workflows/release.yml`.
- JSON/NDJSON - Provider catalogs and durable/wire formats in `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`, `wayland-nano/crates/nano-session/`, and `wayland-nano/crates/nano-protocol/`.

## Runtime

**Environment:**
- Rust 1.95.0, pinned in `wayland-nano/rust-toolchain.toml`.
- Native Windows target `x86_64-pc-windows-msvc` is the pinned local target; CI/release also cover Windows ARM64, macOS x64/ARM64, and Linux x64/ARM64 in `wayland-nano/.github/workflows/gate.yml` and `wayland-nano/.github/workflows/release.yml`.
- Node.js 18+ is required only for the npm distribution wrapper; the release publisher uses Node.js 24 in `wayland-nano/.github/workflows/release.yml`.

**Package Manager:**
- Cargo from Rust 1.95.0 - Builds the 19-member Rust workspace declared in `wayland-nano/Cargo.toml`.
- Lockfile: present at `wayland-nano/Cargo.lock`; use locked dependency resolution for reproducible gates and releases.
- npm - Publishes the zero-runtime-dependency `waylandnano` wrapper defined by `wayland-nano/packaging/npm/package.json`.

## Frameworks

**Core:**
- Tokio 1 - Async runtime for model HTTP, MCP transports, tools, plugins, CLI hosting, and process coordination; feature sets are declared per crate, including `wayland-nano/crates/nano-model/Cargo.toml` and `wayland-nano/crates/nano-mcp/Cargo.toml`.
- Serde 1 / serde_json 1 - Typed configuration, JSON APIs, JSON-RPC, NDJSON protocol frames, and append-only journal envelopes throughout `wayland-nano/crates/`.
- Reqwest 0.12 with `rustls-tls` and default features disabled - Outbound HTTP transport used behind the `nano-egress` boundary by `wayland-nano/crates/nano-model/`, `wayland-nano/crates/nano-mcp/`, `wayland-nano/crates/nano-tools/`, and `wayland-nano/crates/nano-plugins/`.
- thiserror 2 - Typed error definitions across the workspace, with the closed user-facing error vocabulary in `wayland-nano/crates/nano-session/src/error_kind.rs`.

**Testing:**
- Rust built-in test harness (`cargo test`) - Unit and integration suites co-located under each crate's `src/` and `tests/` directories.
- pretty_assertions 1 - Structured assertion diffs in crates including `wayland-nano/crates/nano-model/Cargo.toml` and `wayland-nano/crates/nano-mcp/Cargo.toml`.
- Recorded integration fixtures - Network-independent Flux evidence under `shared/fixtures/flux/`; live suites are explicitly credential-gated by `wayland-nano/justfile`.

**Build/Dev:**
- Just - Canonical local and CI task runner in `wayland-nano/justfile`; use `just gate-all` for formatting, Clippy, workspace tests, and generated-artifact freshness.
- rustfmt + Clippy - Formatting and lint gates; Clippy runs workspace-wide with `-D warnings` in `wayland-nano/justfile`.
- cargo-deny - License, advisory, and dependency-policy gate configured by `wayland-nano/deny.toml`.
- cargo-cyclonedx - Generates the release SBOM in `wayland-nano/.github/workflows/gate.yml`.
- GitHub Actions - Six-target gate matrix and five-target tagged release pipeline in `wayland-nano/.github/workflows/`.

## Key Dependencies

**Critical:**
- `windows-sys` 0.52 - Pinned Windows API surface for sandboxing, process containment, Credential Manager, ConPTY/CUA, and setup; do not add a second version. Evidence: `wayland-nano/AGENTS.md`, `wayland-nano/crates/nano-mcp/Cargo.toml`, and `wayland-nano/crates/nano-cua/Cargo.toml`.
- `image` =0.25.10 - Hardened image decoding with default features disabled and only PNG/JPEG/GIF/WebP enabled at the workspace link boundary in `wayland-nano/Cargo.toml`.
- `jsonschema` =0.32.1 - Model/tool schema validation in `wayland-nano/crates/nano-model/Cargo.toml`.
- `sha2` 0.10 - Integrity digests for provider/catalog evidence, plugins, sessions, attachments, and packaging-related flows across `wayland-nano/crates/`.
- `base64` 0.22 - Model payloads, attachments, OAuth PKCE, and bounded image/tool handling; workspace-pinned in `wayland-nano/Cargo.toml`.
- `tar` =0.4.46 and `flate2` 1 - Bounded plugin/package archive handling in `wayland-nano/crates/nano-plugins/Cargo.toml`.

**Infrastructure:**
- `x11rb` 0.13 with XTest - Default Linux computer-use backend in `wayland-nano/crates/nano-cua/Cargo.toml`; the Wayland feature exists but carries no external crate dependency there.
- `core-foundation` 0.10 and `core-graphics` 0.23 - macOS computer-use backend in `wayland-nano/crates/nano-cua/Cargo.toml`.
- bubblewrap - Required external Linux sandbox executable on the modern runtime path; installation and user-namespace setup are explicit in `wayland-nano/.github/workflows/gate.yml`.
- Native OS containment APIs - Windows restricted tokens/DACLs/Job Objects/WFP, macOS Seatbelt, and Linux bwrap/seccomp/Landlock are implemented under `wayland-nano/crates/nano-sandbox/`.

## Configuration

**Environment:**
- Provider selection and endpoints come from the embedded authority `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`; do not invent provider endpoints in callers.
- Provider credentials resolve as injected bearer, canonical `<PROVIDER>_API_KEY`, then `<PROVIDER>_API_KEY_FILE`; Flux retains `FLUX_API_KEY`, `FLUX_TEST_KEY`, then `FLUX_API_KEY_FILE`. Implementations are `wayland-nano/crates/nano-cli/src/provider_key.rs` and `wayland-nano/crates/nano-cli/src/flux_key.rs`.
- Unix credential files must be owner-only (0600 or stricter); resolved values are registered immediately with the egress redactor in `wayland-nano/crates/nano-cli/src/provider_key.rs`.
- No `.env` contract is used as an authority. Never read or commit secret files; the owner-held Flux test credential path is documented in `wayland-nano/AGENTS.md`.

**Build:**
- Workspace/build profiles: `wayland-nano/Cargo.toml`.
- Toolchain/target pin: `wayland-nano/rust-toolchain.toml`.
- Lint policy: `wayland-nano/clippy.toml` and `wayland-nano/deny.toml`.
- Gate recipes: `wayland-nano/justfile`.
- npm platform staging and integrity manifest: `wayland-nano/packaging/npm/scripts/pack.ps1` and `wayland-nano/packaging/npm/package.json`.

## Platform Requirements

**Development:**
- Install Rust 1.95.0 with rustfmt and Clippy, Cargo, and Just; use native MSVC for the release-blocking Windows x64 path (`wayland-nano/rust-toolchain.toml`).
- Install `cargo-deny` for the dependency-policy gate; use PowerShell for packaging/provisioning scripts in `wayland-nano/scripts/` and `wayland-nano/packaging/npm/scripts/`.
- Linux runtime tests require bubblewrap plus usable unprivileged user and network namespaces; see `wayland-nano/docs/COMPATIBILITY.md`.
- Run `just gate-all` from `wayland-nano/` before claiming a verified change; run `just gate-deny` when dependency or supply-chain surfaces change.

**Production:**
- Shipped as prebuilt native `wayland-nano` binaries wrapped by the public npm package defined in `wayland-nano/packaging/npm/package.json`.
- Supported release artifacts target Windows x64, macOS x64/ARM64, and Linux x64/ARM64; Windows ARM64 is CI-gated but not included in the five-platform npm release matrix (`wayland-nano/docs/COMPATIBILITY.md`, `wayland-nano/.github/workflows/release.yml`).
- Tagged `v*` releases publish npm provenance and GitHub release archives with SLSA attestations; binaries are not Authenticode-signed or Apple-notarized (`wayland-nano/.github/workflows/release.yml`, `wayland-nano/docs/COMPATIBILITY.md`).

---

*Stack analysis: 2026-08-16*
