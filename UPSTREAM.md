# Upstream and provenance — Track B (NanoK3)

## Vendored components (copied source, pinned)

| Component | Source | Revision | License | Status |
|---|---|---|---|---|
| `vendor/codex-windows-sandbox-rs` | github.com/openai/codex `codex-rs/windows-sandbox-rs` | `646f7c0a91b8e327d263335da68ae8ef212895ce` | Apache-2.0 + NOTICE | vendored reference; dependency-closure analysis pending before build wiring |
| `vendor/codex-rollout` | github.com/openai/codex `codex-rs/rollout` | same | Apache-2.0 | same |
| `vendor/codex-skills` | github.com/openai/codex `codex-rs/skills` | same | Apache-2.0 | same |

Vendored trees are byte-identical copies (excluding `.git`). Modifications, if
any ever occur, are recorded file-by-file here with rationale. Donor `.git`
metadata is not copied; the immutable donor snapshot lives at
`../resources/upstreams/codex/`.

## Reference-only donors (no code copied)

| Component | Revision | License | Use |
|---|---|---|---|
| Grok Build | `8a14c91d88875a831a38b3a066b1683116bcb31c` | Apache-2.0 + THIRD-PARTY-NOTICES | wire-semantics reference (3-backend sampler); 9 MPL-2.0 packages require review if any code is adapted |
| Kimi Code | `01c74e9372fcbbbe99614e859b53b505ed1664a8` | MIT | behavioral invariants: toolDedupe mechanics, step-retry policy, wire.jsonl journal model |
| Wayland Core 0.12.26 | `98ad1c2836a543385a7a4298f4b3e54a55867ac5` | Apache-2.0 + NOTICE | egress-gate pattern, credential-protection invariants, Flux contract logic to hoist |
| Wayland Desktop beta | `b3cd0511a4406d5e837db9d7e42e395c08387baf` | AGPL-3.0-or-later | protocol fixtures only; no code copying |

## Adapted-file ledger

| Destination | Donor path | Transformation |
|---|---|---|
| `crates/nano-sandbox/src/lib.rs` | `codex-rs/windows-sandbox-rs/src/lib.rs` | module map reduced to ported subset; public API re-exports deferred until their modules land |
| `crates/nano-sandbox/src/telemetry.rs` | (seam only — `codex-rs/otel` NOT ported) | original Nano code replacing the `Option<&StatsigMetricsSettings>` hook with a `MetricsSink` trait |
| `crates/nano-sandbox/src/path_normalization.rs` | `codex-rs/windows-sandbox-rs/src/path_normalization.rs` | verbatim except module path |
| `crates/nano-sandbox/src/winutil.rs` | `codex-rs/windows-sandbox-rs/src/winutil.rs` | verbatim except module path + windows-sys 0.59 pin; donor tests retained; localization-safe SID note carried forward |
| `crates/nano-sandbox/src/token.rs` | `codex-rs/windows-sandbox-rs/src/token.rs` | verbatim except module path, windows-sys 0.52 pin, 2 added `# Safety` doc sections (clippy `missing_safety_doc`) |
| `crates/nano-sandbox/src/token_tests.rs` | `codex-rs/windows-sandbox-rs/src/token_tests.rs` | verbatim except module path |
| `crates/nano-sandbox/src/acl.rs` | `codex-rs/windows-sandbox-rs/src/acl.rs` | verbatim except module path + 5 added `# Safety` doc sections; **plus** Track-B exercise test `nano_tests` (original code) proving deny-write enforcement on a live dir. Donor behavior discovered: after a deny-write ACE on the harness's own SID, `fetch_dacl_handle` fails at open (self-lockout) — by design, deny ACEs must target the sandboxed identity, not the broker |
| `crates/nano-sandbox/src/env.rs` | `codex-rs/windows-sandbox-rs/src/env.rs` | verbatim except module path; architecture warning added (env proxies are discouragement, not containment) |
| `crates/nano-sandbox/src/wfp.rs` | `codex-rs/windows-sandbox-rs/src/wfp.rs` | `crate::to_wide` → `crate::winutil::to_wide`; otherwise verbatim; donor tests retained |
| `crates/nano-sandbox/src/wfp/filter_specs.rs` | `codex-rs/windows-sandbox-rs/src/wfp/filter_specs.rs` | verbatim except module path |
| `crates/nano-sandbox/src/logging.rs` | `codex-rs/windows-sandbox-rs/src/logging.rs` | module path; string util sourced from winutil; `*_for_codex_home` → `*_for_nano_home` (branding); donor tests retained |
| `crates/nano-sandbox/src/proc_thread_attr.rs` | `codex-rs/windows-sandbox-rs/src/proc_thread_attr.rs` | verbatim except module path |
| `crates/nano-sandbox/src/desktop.rs` | `codex-rs/windows-sandbox-rs/src/desktop.rs` | verbatim except module path |
| `crates/nano-sandbox/src/winutil.rs` (addition) | `codex-rs/utils/string/src/lib.rs` | `take_bytes_at_char_boundary` relocated into winutil (donor: separate codex-utils-string crate) |
| `crates/nano-sandbox/src/lib.rs` (`sandbox_dir`) | `codex-rs/windows-sandbox-rs/src/setup.rs` | interim helper at crate root until setup module lands |
| `crates/nano-sandbox/src/job.rs` | `codex-rs/utils/pty/src/win/job.rs` | extracted per B-VND-01 (not the whole PTY crate); winapi→windows-sys 0.52, filedescriptor→std OwnedHandle, log::warn→tracing::warn; Track-B live tree-kill test added |
| `crates/nano-sandbox/src/process.rs` | `codex-rs/windows-sandbox-rs/src/process.rs` | `codex_utils_pty::JobObject` → `crate::job::JobObject`; otherwise verbatim |
