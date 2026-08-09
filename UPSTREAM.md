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
| `crates/nano-core/src/abs/mod.rs` | `codex-rs/utils/absolute-path/src/lib.rs` | reduced to consumer-named surface; thread-local deserialization guard dropped (cwd-explicit resolution instead); custom `Deserialize` documented |
| `crates/nano-core/src/abs/absolutize.rs` | `codex-rs/utils/absolute-path/src/absolutize.rs` (itself MIT-adapted from path-absolutize 3.1.1) | pub(super)→pub(crate); donor tests retained |
| `crates/nano-core/src/permissions.rs` | `codex-rs/protocol/src/permissions.rs` + `models.rs` | EXTRACT-TYPES layer per B-VND-01: JsonSchema/TS/strum derives dropped (serde shape preserved for config compat); behavioral layer (root getters, narrowing, ReadDenyMatcher) deferred to consumer landing |
| `crates/nano-core/src/permissions.rs` (additions) | `codex-rs/protocol/src/models.rs` + `permissions.rs` | `PermissionProfile::{read_only,workspace_write*,to_runtime_permissions,Default}`, `ManagedFileSystemPermissions` conversions, project-roots glob helpers (prefix keeps donor `codex-` token: config-compat, not branding) |
| `crates/nano-core/src/policy_engine.rs` | `codex-rs/protocol/src/permissions.rs` (engine half) + `protocol.rs` (`WritableRoot`) | full cwd-resolution engine ported; `.codex`→`.nano` metadata protection (deliberate branding); legacy conversions/signature/equivalence NOT ported (greenfield); `error!`→`tracing::warn!` |
| `crates/nano-sandbox/src/resolved_permissions.rs` | `codex-rs/windows-sandbox-rs/src/resolved_permissions.rs` | imports rewired codex_protocol→nano_core, codex_utils_absolute_path→nano_core::abs; donor tests retained (8, incl. temp-env + materialization + full-disk rejection) |
| `crates/nano-sandbox/src/deny_read_resolver.rs` | `codex-rs/windows-sandbox-rs/src/deny_read_resolver.rs` | imports rewired (ReadDenyMatcher → nano_core::policy_engine); donor tests retained (glob scan plans, cycle-safe expansion, fail-before-expansion) |
| `crates/nano-sandbox/src/setup_types.rs` | `codex-rs/windows-sandbox-rs/src/setup.rs` (data layer) | SetupMarker/SandboxUsersFile/OfflineProxySettings/SandboxNetworkIdentity/env parsing; **branding**: usernames → `NanoK3Sandbox{Offline,Online}`, env keys → `NANOK3_*` (dual-track isolation per scorecard §6) |
| `crates/nano-sandbox/src/dpapi.rs` | `codex-rs/windows-sandbox-rs/src/dpapi.rs` | verbatim except module path (machine-scope DPAPI) |
| `crates/nano-sandbox/src/identity.rs` | `codex-rs/windows-sandbox-rs/src/identity.rs` | PURE readiness layer only (load/select/reconcile); execution entry points deferred to B-SBX-08c with gather_*/run_*; setup::*→setup_types::*, codex_home→nano_home |
| `crates/nano-core/src/permissions.rs` (addition) | `codex-rs/protocol/src/config_types.rs` | `WindowsSandboxProxySettingsMode` extracted (serde shape kept) |
| `crates/nano-sandbox/src/allow.rs` | `codex-rs/windows-sandbox-rs/src/allow.rs` | verbatim except test imports rewired + `.codex`→`.nano` in metadata-deny test (recorded branding) |
| `crates/nano-sandbox/src/cap.rs` | `codex-rs/windows-sandbox-rs/src/cap.rs` | verbatim except module path |
| `crates/nano-sandbox/src/setup_error.rs` | `codex-rs/windows-sandbox-rs/src/setup_error.rs` | sanitize_metric_tag_value → crate::winutil |
| `crates/nano-core/src/abs/mod.rs` (addition) | `codex-rs/utils/absolute-path/src/lib.rs` | `From<&Path>`/`From<PathBuf>` parity impls |
| `crates/nano-sandbox/src/gather.rs` | `codex-rs/windows-sandbox-rs/src/setup.rs` (gather layer) | profile/full-read/write/effective roots + user-profile and sensitive-dir filters; codex_home→nano_home; tests adapted from donor |
| `crates/nano-sandbox/src/helper_materialization.rs` | `codex-rs/windows-sandbox-rs/src/helper_materialization.rs` | RESOURCES_DIRNAME `codex-resources`→`nanok3-resources` (branding) |
| `crates/nano-sandbox/src/ssh_config_dependencies.rs` | `codex-rs/windows-sandbox-rs/src/ssh_config_dependencies.rs` | verbatim except module path |
| `crates/nano-sandbox/src/setup_exec.rs` | `codex-rs/windows-sandbox-rs/src/setup.rs` (execution half) | orchestration/payload/elevation/singleflight; SETUP_EXE_FILENAME→`nanok3-sandbox-setup.exe` (isolation); otel→`telemetry::TelemetrySettings` facade; codex_home→nano_home; RID consts donor-local |
| `crates/nano-sandbox/src/gather.rs` (visibility) | — | filter/expand helper fns + platform-defaults const made pub for setup_exec |
| `crates/nano-sandbox/src/deny_read_acl.rs` | `codex-rs/windows-sandbox-rs/src/deny_read_acl.rs` | verbatim except module path |
| `crates/nano-sandbox/src/deny_read_state.rs` | `codex-rs/windows-sandbox-rs/src/deny_read_state.rs` | `crate::setup::sandbox_dir`→`crate::sandbox_dir` |
| `crates/nano-sandbox/src/audit.rs` | `codex-rs/windows-sandbox-rs/src/audit.rs` | `crate::setup::effective_write_roots_for_permissions`→`crate::gather::...`; otherwise verbatim |
| `crates/nano-sandbox/src/spawn_types.rs` | `codex-rs/utils/pty/src/process.rs` (surface only) | Nano-owned minimal session abstraction (writer/close/terminate + channels); full ProcessHandle internals intentionally not ported (v1 is non-interactive) |
| `crates/nano-sandbox/src/stdio_bridge.rs` (+ tests) | `codex-rs/windows-sandbox-rs/src/stdio_bridge.rs` | SpawnedProcess → crate::spawn_types; EOF/drain/Ctrl+C forwarding semantics intact |
| `crates/nano-sandbox/src/workspace_acl.rs` | `codex-rs/windows-sandbox-rs/src/workspace_acl.rs` | `.codex`→`.nano` (protect_workspace_nano_dir) |
| `crates/nano-sandbox/src/sandbox_utils.rs` | `codex-rs/windows-sandbox-rs/src/sandbox_utils.rs` | ensure_codex_home_exists→ensure_nano_home_exists |
| `crates/nano-sandbox/src/identity.rs` (addition) | `codex-rs/windows-sandbox-rs/src/identity.rs` (entry points) | require_/refresh_logon_sandbox_creds landed against gather+setup_exec; NANO_HOME write-protection note carried |
| `crates/nano-sandbox/src/spawn_prep.rs` | `codex-rs/windows-sandbox-rs/src/spawn_prep.rs` | imports rewired (gather, nano_core); .nano dir protection; donor tests retained |
