# Technology Stack

**Analysis Date:** 2026-08-27

## Languages

**Primary:**
- Rust 1.95.0, edition 2024 - all runtime, protocol, security, persistence, verification, and TUI crates under `crates/`; the toolchain is pinned in `rust-toolchain.toml`.

**Secondary:**
- PowerShell - Windows provisioning, proof harnesses, packaging, and release assembly in `scripts/` and `packaging/npm/scripts/pack.ps1`.
- JavaScript/CommonJS on Node.js 18+ - npm launcher/install code and deterministic gate validators in `packaging/npm/bin/` and `gates/`.
- Bash - Unix proof, soak, and CI support scripts in `scripts/`.
- Python - focused CUA, cron, and sandbox smoke/proof scripts such as `scripts/s6-proof/f6_cron_create_proof.py` and `vendor/codex-windows-sandbox-rs/sandbox_smoketests.py`.
- JSON, TOML, YAML, Markdown, NDJSON - contracts, provider catalog, policy, CI, fixtures, and protocol evidence throughout `contracts/`, `gates/`, `crates/nano-model/data/`, and `.github/workflows/`.
- C/C++ (dependency build only) - bundled SQLite and the sqlite-vec extension are compiled locally through the Rust dependency build chain declared in `crates/nano-memory/Cargo.toml`.

## Runtime

**Environment:**
- Native Rust executables for Windows, Linux, and macOS; Windows x64 is release-blocking and uses native MSVC (`AGENTS.md`, `.github/workflows/gate.yml`).
- Rust 1.95.0 is mandatory for development and CI (`rust-toolchain.toml`); workspace MSRV is declared as 1.85, while `nano-tui` declares 1.88 (`Cargo.toml`, `crates/nano-tui/Cargo.toml`).
- Node.js >=18 is required only for the zero-dependency npm installer/launcher (`packaging/npm/package.json`); release CI currently uses Node 24 (`.github/workflows/release.yml`).

**Package Manager:**
- Cargo 1.95.0 for the Rust workspace; lockfile: `Cargo.lock` present and committed.
- npm for distribution metadata and the native-binary launcher; package dependencies: none (`packaging/npm/package.json`).
- `just` drives repository gates and release tasks from `justfile`.

## Frameworks

**Core:**
- Tokio 1.x - async agent turns, model/MCP HTTP, process control, hooks, protocol hosts, and TUI event handling across `crates/nano-agent`, `crates/nano-cli`, `crates/nano-mcp`, `crates/nano-model`, and `crates/nano-tui`.
- Serde 1.x / serde_json 1.x - journal operations, provider/model wire formats, ACP/NDJSON, MCP JSON-RPC, receipts, config, and fixtures.
- Reqwest 0.12 with rustls and default features disabled - HTTP transport behind `nano-egress`, used by model, MCP, plugin, and web-tool layers (`crates/nano-egress/Cargo.toml`).
- Ratatui 0.30.2 + Crossterm 0.29.0 - terminal UI (`crates/nano-tui/Cargo.toml`).
- Rusqlite 0.37 with bundled SQLite + sqlite-vec 0.1.9 - local persistent memory with FTS5 and vector KNN (`crates/nano-memory/Cargo.toml`).
- Portable PTY 0.9 - Unix process/terminal integration and TUI PTY tests (`crates/nano-sandbox/Cargo.toml`, `crates/nano-tui/Cargo.toml`).

**Testing:**
- Rust built-in test harness - co-located unit tests and crate-level integration tests under `crates/*/tests/`.
- `tempfile`, `pretty_assertions`, `insta`, `vt100`, and `portable-pty` - filesystem isolation, readable diffs, snapshot testing, terminal emulation, and PTY E2E tests.
- Node's built-in test runner/assertions - adversarial gate contract tests in `gates/tests/`.
- Scripted proof harnesses - external filesystem/process/network oracles under `scripts/c12-proof/`, `scripts/cua-proof/`, `scripts/soak/`, and `scripts/human-harness/`.

**Build/Dev:**
- rustfmt and Clippy with `-D warnings` - enforced by `just gate-all` and `.github/workflows/gate.yml`.
- cargo-deny - license, advisory, source, and dependency policy from `deny.toml`.
- cargo-cyclonedx - SBOM generation in `.github/workflows/gate.yml`.
- GitHub Actions - seven-target/platform gate matrix plus tagged release workflow (`.github/workflows/gate.yml`, `.github/workflows/release.yml`).
- PowerShell packaging - stages prebuilt binaries and creates integrity metadata (`packaging/npm/scripts/pack.ps1`).

## Key Dependencies

**Critical:**
- `nano-egress` + Reqwest/rustls - the only sanctioned outbound HTTP path; preserve the deny-by-default policy in `crates/nano-egress/src/policy.rs`.
- `nano-sandbox` + platform APIs - fail-closed OS containment: Windows restricted tokens/DACL/Job/WFP, Linux Landlock/seccomp/bubblewrap, and macOS seatbelt (`crates/nano-sandbox/`).
- `nano-session` + SHA-256 - append-only operation journal, replay, torn-tail recovery, attachments, and compaction equivalence (`crates/nano-session/`).
- Rusqlite/sqlite-vec - project- and agent-partitioned, bi-temporal memory store with FTS5, 384-dimensional local hashed embeddings, and schema-only KG tables (`crates/nano-memory/src/schema.rs`, `crates/nano-memory/src/embed.rs`).
- `nano-model` + jsonschema 0.32.1 - validated, code-generated provider catalog and provider-neutral model boundary (`crates/nano-model/build.rs`, `crates/nano-model/data/providerCatalog.vendored.json`).
- `windows-sys` 0.52 - pinned Win32 API binding used throughout security and process code; do not introduce another version (`AGENTS.md`).
- `image` 0.25.10 with only png/jpeg/gif/webp - hardened image decoding; decoder features are locked at workspace level (`Cargo.toml`, `crates/nano-tools/src/image.rs`).

**Infrastructure:**
- Git CLI - review diffs, verifier ancestry, checkpoints, and repository tooling in `crates/nano-cli` and `crates/nano-verify`.
- Bubblewrap, Landlock, and seccomp - Linux sandbox enforcement (`crates/nano-sandbox/src/linux/`, `.github/workflows/gate.yml`).
- Windows Filtering Platform and Job Objects - Windows network/process containment (`crates/nano-sandbox/src/`).
- X11 XTest (`x11rb`), CoreGraphics, and Win32 UI/GDI - platform computer-use backends (`crates/nano-cua/`).

## Configuration

**Environment:**
- Runtime home and policy are rooted through `NANO_HOME`, `NANO_RULES_FILE`, and namespaced `NANO_*` settings; configuration entry points live in `crates/nano-cli/src/` and `crates/nano-core/src/`.
- Model routing references only provider IDs from `WAYLAND_NANO_PROVIDERS`; endpoints and credential variable names come exclusively from `crates/nano-model/data/providerCatalog.vendored.json`.
- Flux credentials resolve in order from `FLUX_API_KEY`, `FLUX_TEST_KEY`, then the file named by `FLUX_API_KEY_FILE` (`crates/nano-cli/src/flux_key.rs`). Other providers use injected bearer, catalog-named API-key env, then `<VAR>_FILE` (`crates/nano-cli/src/provider_key.rs`).
- MCP servers are supplied by ACP session input and/or `NANO_MCP_SERVERS`; web-fetch hosts use `NANO_WEB_FETCH_HOSTS`; search backend selection uses `NANO_SEARCH_BACKEND` (`crates/nano-cli/src/acp_mode.rs`, `fetch_specs.rs`, `search_specs.rs`).
- Never read or commit `.secrets/`; live-gated tests intentionally self-skip without credentials (`AGENTS.md`).

**Build:**
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `deny.toml`, and `justfile` define the local build and quality gates.
- `.github/workflows/gate.yml` defines the cross-platform validation matrix; `.github/workflows/release.yml` builds, signs, attests, packages, and publishes tagged releases.
- `packaging/npm/package.json` and `packaging/npm/scripts/pack.ps1` define native binary distribution.

## Platform Requirements

**Development:**
- Use the pinned Rust toolchain and Cargo lockfile; run `just gate-all` for fmt, Clippy, and workspace tests (`README.md`).
- Windows development uses native MSVC and may require elevated provisioning for complete sandbox/WFP proofs (`scripts/provision/README.md`).
- Linux runtime tests require bubblewrap and usable user namespaces; hosted CI installs/configures these explicitly (`.github/workflows/gate.yml`).
- Live Flux/MCP/model tests require a credential source and otherwise skip by design; never make ordinary workspace tests network-dependent.

**Production:**
- Prebuilt package targets are Windows x64, Linux x64/arm64, and macOS x64/arm64 (`.github/workflows/release.yml`, `packaging/npm/bin/install.js`).
- npm package `waylandnano` installs the matching native host; Unix packages also carry `wayland-nano-pty-guard` (`packaging/npm/`).
- Windows releases can use Azure Trusted Signing and are verified before hashing/packaging; GitHub release archives receive build-provenance attestations (`.github/workflows/release.yml`).

---

*Stack analysis: 2026-08-27*
