# Wayland Nano (Track B) — gate entry points
# Local and CI use the same commands; scripts may add platform args but never
# replace gate semantics.

default:
    @just gate-all

# Format check (pinned rustfmt from rust-toolchain.toml)
gate-fmt:
    cargo fmt --all -- --check

# Clippy with the architecture bans (egress disallowed-methods)
gate-lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Full test suite (unit + integration, no live/networked tests)
gate-test:
    cargo test --workspace

# License/advisory gate
gate-deny:
    cargo deny check

# Live tests (Flux + MCP + slices) — requires the key in env, never in CI logs
gate-live:
    cargo test -p nano-model -- --ignored
    cargo test -p nano-mcp live_
    cargo test -p nano-agent --test c2_fixture
    cargo test -p nano-cli --test vertical_slice

# The full local gate
gate-all: gate-fmt gate-lint gate-test

# Release binaries for packaging/metrics
gate-release:
    cargo build --release --workspace --bins
