# Compatibility matrix — Wayland Nano (Track B), alpha

Support levels, highest to lowest:

- **Provisioned** — full provisioning proofs executed on a real host
  (`scripts/provision/`, `scripts/c12-proof/`); the sandbox/egress/policy
  stack is proven end-to-end on that target.
- **CI-gated** — the 6-leg gate matrix (`.github/workflows/gate.yml`) builds,
  lints, and tests the target on a hosted runner on every push/PR.
- **Runtime-gated** — the OS containment backend is exercised at runtime in CI
  (real seatbelt / bwrap invocations, not just argv-builder string tests).
- **Not claimed** — no evidence; do not assume it works.

## Platform targets

| Target | Level | Containment backend | Evidence | Not claimed |
|---|---|---|---|---|
| Windows 11 x64 (`x86_64-pc-windows-msvc`) | Provisioned + CI-gated | Restricted-token job-object sandbox (`NanoSandbox*` accounts) | `scripts/provision/`, `scripts/c12-proof/`, `windows-latest` gate leg | Resistance to concurrent same-user NTFS hard-link races during provisioning (see `scripts/provision/PROVISIONING-BOUNDARY.md`) |
| Windows 11 ARM64 (`aarch64-pc-windows-msvc`) | CI-gated | Same backend, compiled for ARM64 | `windows-11-arm` gate leg | Provisioning proofs (x64-only today); on-device validation |
| macOS 14 arm64 (`aarch64-apple-darwin`) | CI-gated + runtime-gated | Seatbelt (`sandbox-exec`, ported `.sbpl` policies) | `macos-14` gate leg; spawn tests `cfg(target_os = "macos")` | Provisioning/host hardening beyond the seatbelt profile; notarized distribution |
| macOS 15 intel (`x86_64-apple-darwin`) | CI-gated + runtime-gated | Seatbelt (same policies, x86_64) | `macos-15-intel` gate leg | Same as macOS arm64 |
| Linux x64 (`x86_64-unknown-linux-gnu`) | CI-gated + runtime-gated | bubblewrap (required) + seccomp; legacy Landlock fallback kept tested | `ubuntu-22.04` gate leg, `wayland-nano-linux-sandbox` helper | Distributions other than Ubuntu; systems without unprivileged user namespaces |
| Linux ARM64 (`aarch64-unknown-linux-gnu`) | CI-gated + runtime-gated | bubblewrap (required) + seccomp; legacy Landlock fallback | `ubuntu-24.04-arm` gate leg | Same as Linux x64 |

### Linux notes

- **bubblewrap is required** on the modern path. The helper fails closed
  (`SANDBOX_UNAVAILABLE`) when no usable bwrap is found; there is no silent
  unsandboxed fallback. Legacy Landlock-only enforcement remains covered by
  builder tests but is not the runtime default.
- **Ubuntu 24.04+** restricts unprivileged user namespaces via AppArmor
  (`kernel.apparmor_restrict_unprivileged_userns=1`), which breaks bwrap
  (`loopback: Failed RTM_NEWADDR: Operation not permitted`). The sandbox
  requires userns+netns, so the host must lift that restriction
  (`sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0`) or run a
  setuid bwrap. CI does this on the throwaway runner; on a real host it is an
  explicit administrator decision.

## Flux API surface (nano-model)

| Surface | Status | Evidence | Not claimed |
|---|---|---|---|
| Completions (`flux_completions`) | **Primary** — the exercised path | Recorded fixtures (`../shared/fixtures/flux/`) replayed in CI; live tests owner-gated (`FLUX_TEST_KEY`) | — |
| Responses (`flux_responses`) | Implemented, fixture-verified | Fixture tests; live smoke owner-gated | Feature parity with Completions on every model |
| Anthropic-compatible Messages (`anthropic_messages`) | Implemented, fixture-verified | Fixture tests | Full Anthropic API surface (tool use, vision, etc.) |
| `count_tokens` (`/anthropic/v1/messages/count_tokens`) | Implemented, fixture-verified | Fixture tests | Exact parity with server-side billing tokenization |
| Flux MCP (`/mcp/`) | **Upstream-blocked** — `tools/list` returns an empty catalog | COMP-FLX-011 (`docs/compliance/SCENARIO_CATALOG.md`) | Any invocable Flux MCP tool; blocked upstream, not by engineering |
| Model catalog (`GET /v1/models`) | Implemented (live + fixture fallback) | `flux_models.rs` | Stable model IDs across Flux provider rotations |

## Explicitly not claimed (alpha)

- Track A (`nano/`) interop or migration.
- MCP/skills end-user capability flags beyond what `docs/STATUS.md` records as
  proven (honesty rule: flags stay false until end-to-end proof exists).
- Publish/release automation — the release scaffold is owner-gated; CI
  produces evidence artifacts only.
- Windows ARM64 provisioning, non-Ubuntu Linux, macOS hardened-runtime
  distribution, WSL1 (detected and warned, unsupported).
