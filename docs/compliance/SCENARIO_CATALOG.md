# NanoK3 Compliance Scenario Catalog

Backbone for the compliance matrix / CI evidence mapping. Every verification
that exists in Track B is keyed by a **stable scenario ID** and mapped to the
contract requirement it proves. Do not renumber existing IDs; append new ones.

**Snapshot:** 2026-08-10 · workspace tests 235 (all green) · clippy `-D warnings` clean.

## Contract basis

`shared/contracts/` is **empty** — the P1 frozen-contract artifacts from Track A
have not landed there yet (SCORECARD §1: Track A produces, Track B adopts
verbatim). Until they do, the operative frozen references are:

| Ref | Artifact |
|---|---|
| `SCORECARD` | `shared/SCORECARD.md` §2 checkpoint criteria (C1.1–C1.6, C2.1–C2.5, C3.1–C3.4) |
| `CORPUS` | `shared/fixtures/desktop-core-v1/POINTER.md` → `desktop/contracts/wayland-desktop-core/v1/` (pinned producer commit `d0aa0abc…`), replayed from the immutable copy at `resources/upstreams/wayland-desktop/contracts/wayland-desktop-core/v1` |
| `FLUX` | `shared/fixtures/flux/FINDINGS.md` (recorded live wire truth, batches 1–2) |
| `ARCH` | `nano-k3/ARCHITECTURE.md` constitution (egress chokepoint, fail-closed, append-only journal, honest capabilities) |

## Status vocabulary

| Status | Meaning |
|---|---|
| `green` | Runs and passes in `cargo test --workspace` with no external input |
| `live-gated` | Real test, self-skips unless `FLUX_TEST_KEY` is set (key file: `waylandnano/.secrets/flux-test-key` — path reference only, value never in repo) |
| `provisioning-gated` | Harness runs but probe self-skips (`SKIP: not provisioned`) until owner runs elevated provisioning (`scripts/provision/`) |
| `recorded` | Proven against live Flux with fixtures/evidence on disk; not re-executed in CI |
| `pending-unix-port` | Blocked on the in-flight macOS/Linux sandbox port (seatbelt/landlock) |
| `pending` | No proving scenario exists yet |

---

## A. Workspace tests (`cargo test --workspace` — 235 tests)

Per-crate totals: nano-agent 15, nano-cli 5, nano-core 9, nano-egress 7,
nano-mcp 8, nano-model 17, nano-platform 0, nano-protocol 17, nano-sandbox 121
(105 lib + 16 `nanok3-sandbox-setup` bin), nano-session 10, nano-skills 11,
nano-tools 15.

### A.1 Wire protocol & Desktop profile (nano-protocol, 17)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-PROTO-001 | CORPUS: full 110-fixture conformance — supported subset accepted, unsupported fail typed, adversarial zero-panic; corpus counts pinned (11 cmd / 39 evt / 23 compat / 37 adversarial) | `nano-protocol corpus::tests::full_corpus_conformance` | green |
| COMP-PROTO-002 | CORPUS: command/event fixture parse + emitted frames match corpus shapes + honest v1 profile | `messages::tests::{command_fixtures_parse, corpus_fixtures_parse_for_supported_subset, emitted_frames_match_corpus_shapes}`, `profile::tests::v1_profile_is_honest_in_corpus_shape` | green |
| COMP-PROTO-003 | ARCH: malformed-tolerant NDJSON codec (CRLF, mixed valid/malformed stream, single-line encode, partial tail held) | `codec::tests::*` (4) | green |
| COMP-PROTO-004 | SCORECARD C3.1 engine leg: ready-first host loop, malformed input → error frame + continue, typed shutdown, turn-state labels cover the machine | `host::tests::*` (4) | green |
| COMP-PROTO-005 | ACP adapter wire shapes (initialize v1, typed method-not-found, prompt stop reasons, session update shapes) | `acp::tests::*` (4) | green |

### A.2 Egress chokepoint (nano-egress, 7)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-EGRESS-001 | ARCH fail-closed: deny-by-default policy; `flux_only` preset allows Flux hosts and denies the rest; host parsing handles ports/userinfo | `policy::tests::{deny_by_default, flux_only_allows_flux_and_denies_rest, host_parsing_handles_ports_and_userinfo}` | green |
| COMP-EGRESS-002 | Policy identity: digest stable and short (audit/logging safe) | `policy::tests::digest_is_stable_and_short` | green |
| COMP-EGRESS-003 | SCORECARD C3.3: denied request never builds; allowed request carries no secret echo; error `Display` is redaction-proof (observability fields only) | `client::tests::*` (3) | green |

### A.3 Model wire & Flux client (nano-model, 17)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-MODEL-001 | FLUX C1.1: recorded batch-1 completion fixture parses (reasoning content + `usage.cost_usd`); message roles wire correctly; request body uses completions shape | `fixture_tests::{parses_batch1_completion_with_reasoning_and_cost, message_roles_wire_correctly, request_body_uses_completions_shape}` | green (offline replay of `shared/fixtures/flux/`) |
| COMP-MODEL-002 | FLUX batch-2: streaming SSE fixture parses into deltas | `fixture_tests::parses_streaming_sse_fixture_into_deltas` | green |
| COMP-MODEL-003 | FLUX batch-2: tool-call fixture parses into complete tool call | `fixture_tests::parses_tool_call_fixture_into_complete_tool_call` | green |
| COMP-MODEL-004 | FLUX: Flux error shapes classify typed | `fixture_tests::error_classification_maps_flux_shapes` | green |
| COMP-MODEL-005 | SSE parser robustness: data-only frames, event+multiline data, CRLF/partial tail, DONE sentinel | `sse::tests::*` (4) | green |
| COMP-MODEL-006 | Retry policy: backoff grows/caps, budget exhaustion, non-retryable immediate, `Retry-After` always wins | `retry::tests::*` (4) | green |
| COMP-MODEL-007 | FLUX C1.1 live: non-streaming + streaming completion against real Flux | `live_smoke::{live_complete_non_streaming, live_complete_streaming}` | live-gated |
| COMP-MODEL-008 | ARCH fail-closed live: egress denies non-Flux host end-to-end | `live_smoke::live_egress_denies_non_flux_host` | live-gated |

### A.4 MCP (nano-mcp, 8)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-MCP-001 | MCP stdio handshake + tools/list; initialize params advertise `nanok3` | `client::tests::{handshake_and_tools_list_over_stdio, initialize_params_advertise_nanok3}` | green |
| COMP-MCP-002 | MCP protocol shapes (request shape, result-vs-error) | `protocol::tests::*` (2) | green |
| COMP-MCP-003 | FLUX quirk #4: SSE-framed and plain-JSON HTTP responses parsed; neither-JSON-nor-SSE rejected | `http::tests::*` (3) | green |
| COMP-MCP-004 | FLUX C1.1 `/mcp/` live: handshake + tools/list against real Flux MCP (trailing-slash derivation, `Mcp-Session-Id`) | `http::live_tests::live_flux_mcp_handshake_and_tools_list` | live-gated |

### A.5 Session journal (nano-session, 10)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-JRNL-001 | ARCH append-only journal: append/replay round trip; writer idempotent across reopen | `tests::{append_and_replay_round_trip, writer_is_idempotent_across_reopen}` | green |
| COMP-JRNL-002 | SCORECARD C1.3: kill at write boundaries — crash mid-turn marks interrupted without duplicate effects; torn tail dropped, middle authoritative; malformed middle line = integrity error; stranded compaction resets to idle | `tests::{crash_mid_turn_marks_interrupted_without_duplicate_effects, torn_tail_is_dropped_and_middle_stays_authoritative, malformed_middle_line_is_an_integrity_error, stranded_compaction_running_resets_to_idle}` | green |
| COMP-JRNL-003 | SCORECARD C1.3/C2.2: duplicate ids never double-apply; unknown ops skipped (forward-additive); compaction replay actionably equivalent; open tool call survives to resume surface | `tests::{duplicate_ids_never_double_apply, unknown_ops_are_skipped_without_failing_replay, compaction_replay_is_actionably_equivalent, open_tool_call_survives_to_resume_surface}` | green |

### A.6 Sandbox & containment (nano-sandbox, 121 = 105 lib + 16 setup bin)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-SBX-001 | SCORECARD C1.2 write containment: workspace-write allows inside root, denies outside root, end-to-end through the full ported stack (unified_exec legacy backend + capture backend); cancellation terminates fast | `unified_exec::backends::legacy::nano_tests::*` (3), `capture::nano_tests::*` (3) | green |
| COMP-SBX-002 | C1.2 allow-root computation: additional roots, tmp env vars in/out, `.git`/`.nano`/`AGENTS` denied inside writable roots, unix `/tmp` ignored on Windows, runtime workspace roots | `allow::tests::*` (9) | green |
| COMP-SBX-003 | C1.2 DACL enforcement: deny-write ACE blocks write and check reports denied (live dir exercise) | `acl::nano_tests::deny_write_ace_blocks_write_and_check_reports_denied` | green |
| COMP-SBX-004 | C1.2 deny-read planning: glob expansion bounded/cycle-safe, missing paths preserved, fail-before-expansion; deny-read ACL plan canonical targets | `deny_read_resolver::tests::*` (7), `deny_read_acl::tests::*` (2) | green |
| COMP-SBX-005 | C1.2 network containment: capture applies WFP network block for read-only / disabled access, skips when allowed, legacy preflight skip; WFP filter keys/names unique; elevated token carries network-proxy restricting SID; spawn-env offline rewrite rules | `capture::tests::*` (4), `wfp::tests::*` (2), `token::tests::*` (1), `spawn_prep::tests::{common_spawn_env_keeps_network_env_unchanged, legacy_spawn_env_applies_offline_network_rewrite, no_network_env_rewrite_*}` (4) | green |
| COMP-SBX-006 | SCORECARD C1.2/C2.5: Job-object contained spawn + tree kill (no survivors) | `job::nano_tests::contained_spawn_succeeds_and_tree_kill_works` | green |
| COMP-SBX-007 | C1.2 capability SIDs: path-scoped write-root SIDs, cwd-spelling equivalence, only active roots | `cap::tests::*` (2), `spawn_prep::tests::root_capability_sids_only_include_active_roots`, `spawn_prep::tests::{legacy_deny_path_includes_nested_active_root_sid, legacy_capability_roots_use_effective_write_roots, legacy_session_capability_roots_use_runtime_workspace_roots_for_workspace_root}` | green |
| COMP-SBX-008 | C1.2 permission-profile validation: rejects disabled profiles, unrestricted managed fs, full-disk write entries; token mode per profile; temp env roots scoped (D9) | `resolved_permissions::tests::*` (8) | green |
| COMP-SBX-009 | C1.2 root gathering + sensitive filters: profile read roots, top-level exclusions, sensitive filter strips nano-home/sandbox dirs | `gather::tests::*` (5), `audit::tests::*` (1) | green |
| COMP-SBX-010 | C1.2 setup/identity state: marker + users-file round trips, offline drift detection, loopback-only proxy ports, identity readiness, guardian preserve mode, singleflight, NanoK3-branded payload | `setup_types::tests::*` (5), `identity::tests::*` (4), `setup_exec::tests::*` (3) | green |
| COMP-SBX-011 | C1.2 helper materialization: versioned helper exe copy/reuse/freshness, NanoK3 resource dirs | `helper_materialization::tests::*` (11) | green |
| COMP-SBX-012 | C1.2 elevated-runner IPC: framed round trip, spawn-request/error serialization, credential-error recognition | `elevated::ipc_framed::tests::*` (3), `elevated::runner_client::tests::*` (1) | green |
| COMP-SBX-013 | Supporting plumbing: stdio bridge EOF/chunks, session handle lifecycle, wrapper argv round trip, git safe-directory injection, ssh config includes, path key normalization, argv quoting, log rolling/redaction, telemetry sink, metric-tag sanitization | `stdio_bridge` (2), `spawn_types` (1), `wrapper` (1), `sandbox_utils` (2), `ssh_config_dependencies` (2), `path_normalization` (1), `winutil` (3), `logging` (4), `telemetry` (2), `setup_error` (3) | green |
| COMP-SBX-014 | C1.2 setup binary: firewall COM accepts production rule scopes and rejects ineffective/partial policy; runtime bin paths; deny-path SID mapping; setup payload (OTel, provision-only); write-root refresh ACL semantics | `nanok3-sandbox-setup` bin: `win::firewall::tests::*` (5), `win::setup_runtime_bin::tests::*` (2), `win::tests::*` (9) | green |

### A.7 Agent loop (nano-agent, 15)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-AGENT-001 | Loop protection: budget stops typed, canonical key ignores arg order, force-stop at 12, no-progress replan→stop, streak reminders at 3/5/8 | `loop_protection::tests::*` (5) | green |
| COMP-AGENT-002 | MCP integration: executor routes namespaced calls; namespacing shapes; unknown namespaces rejected | `mcp::integration_tests::*` (1), `mcp::tests::*` (2) | green |
| COMP-AGENT-003 | Skills reach the model context only when valid skills exist | `skills::tests::*` (2) | green |
| COMP-AGENT-004 | SCORECARD C2.1 turn engine: full act→observe→verify→complete; no-progress stops; repeat-breaker force-stops identical calls | `turn_tests::tests::*` (3) | green |
| COMP-AGENT-005 | SCORECARD C2.1 live: fixture task on bare-metal Windows — read project → patch → run tests → verify, driven by real Flux; external-oracle assertions | `tests/c2_fixture.rs::c2_fixture_agent_fixes_broken_project_live` | live-gated |
| COMP-AGENT-006 | SCORECARD C2.2: cancellation stops the turn at a step boundary, typed, journal records `TurnOutcome::Cancelled` | `tests/c2_fixture.rs::c2_cancellation_stops_turn_at_boundary` | live-gated (self-skips without key; flag check is pre-call) |

### A.8 Tools (nano-tools, 15)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-TOOLS-001 | C1.2/C2.1 fs policy: read/write/edit round trip in workspace; write outside workspace denied; sensitive files denied without override; sensitive detection covers key material; read-deny matcher blocks; typed zero/ambiguous edit; bounded reads | `fs::tests::*` (7) | green |
| COMP-TOOLS-002 | Search: glob finds/bounds/excludes sensitive; denied dir invisible; content search with line numbers | `search::tests::*` (5) | green |
| COMP-TOOLS-003 | Shell through sandbox path: echo exit 0, write lands inside workspace, nonzero exit surfaces | `shell::tests::*` (3) | green |

### A.9 Skills (nano-skills, 11)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-SKILLS-001 | Loader: bounded scoped activation, directory-name default, malformed skill surfaces as error (never silent drop), repaired frontmatter loads | `loader::tests::*` (4) | green |
| COMP-SKILLS-002 | Parser repair/sanitize: prose colons, block scalars, overlong/short descriptions, unrecognized fields, default name + required description | `parser::tests::*` (7) | green |

### A.10 Core types (nano-core, 9) & CLI (nano-cli, 5)

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-CORE-001 | Absolute-path handling: Windows drive-relative and root-relative forms; checked reject of relative; join/absolutize against cwd | `abs::*::tests::*` (5) | green |
| COMP-CORE-002 | Permission profile types: access-mode precedence, deny entries mark restrictions, read-only default, serde shape matches donor (config compat) | `permissions::tests::*` (4) | green |
| COMP-CLI-001 | Credential hygiene: Flux key resolution order (env → file path), file fallback; secret never in config blob | `nano-cli flux_key::tests::resolution_order_and_file_fallback` | green |
| COMP-CLI-002 | SCORECARD C3.1: ACP slice — initialize v1 → session/new → prompt with streamed updates → end_turn through Desktop protocol | `tests/acp_slice.rs::acp_slice_live_prompt_through_desktop_protocol` | live-gated |
| COMP-CLI-003 | SCORECARD C3.1/C3.3/C3.4: vertical slice through real binary — ready-first, live framed turn, MCP tool call through registry, skill context reaches model; canary asserts no key in any frame; zero orphan processes on exit | `tests/vertical_slice.rs::{vertical_slice_live_turn_through_protocol, vertical_slice_mcp_tool_call_through_protocol, vertical_slice_skill_context_reaches_model}` | live-gated |

---

## B. Wire conformance corpus (110 fixtures)

Corpus: `wayland-desktop-core/v1` (immutable audit copy
`resources/upstreams/wayland-desktop/contracts/wayland-desktop-core/v1`;
canonical writable copy `desktop/contracts/wayland-desktop-core/v1/` per
`shared/fixtures/desktop-core-v1/POINTER.md`). Replay harness:
`nano-protocol/src/corpus.rs` — **fails (never skips) if the corpus is
missing**. All rows are proven by `cargo test -p nano-protocol corpus` plus the
fixture-parse tests in `messages::tests`.

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-CORPUS-001 | CORPUS commands (11): 6 supported (`message`, `stop`, `ping`, `tool_approve`, `tool_deny`, `approval_resume`) parse; 5 unsupported tolerated as typed error, never executed | `corpus::tests::full_corpus_conformance` + `messages::tests::command_fixtures_parse` | green |
| COMP-CORPUS-002 | CORPUS events (39): 15 supported parse (unknown extra fields tolerated, forward-additive); 24 unsupported fail closed | same harness + `messages::tests::corpus_fixtures_parse_for_supported_subset` | green |
| COMP-CORPUS-003 | CORPUS compat (23 files): every compat fixture handled (accepted shape or typed error), zero panics | `corpus::tests::full_corpus_conformance` (compat count pinned to 23) | green |
| COMP-CORPUS-004 | CORPUS adversarial (37 `.jsonl` streams: anvil/commands/events/policy/workflow): every line handled — accepted shape, event shape, or typed error — zero panics; fail-closed on unknown-critical-extension, stale-replay, sequence-gap, version-mismatch et al. | `corpus::tests::full_corpus_conformance` (adversarial count pinned to 37; ≥90 handled lines) | green |
| COMP-CORPUS-005 | CORPUS profile honesty: emitted frames match corpus shapes; v1 capability profile is honest in corpus shape (`mcp`/`skills` flipped true only after live proof) | `messages::tests::emitted_frames_match_corpus_shapes`, `profile::tests::v1_profile_is_honest_in_corpus_shape` | green |

Last reported replay (C3 claim, commit `7e4b3bd`): 21 accepted, 5 tolerated,
24 rejected-closed, 90 adversarial/compat handled, 0 violations, 0 panics.

---

## C. Flux live fixtures (`shared/fixtures/flux/`, FINDINGS.md)

Fixture capture is one-time and shared (SCORECARD §1.2); replay-side adapter
coverage is in §A.3/A.4. Credential: owner test key at
`waylandnano/.secrets/flux-test-key` (never committed).

| Scenario | Proves (contract) | Test / command | Status |
|---|---|---|---|
| COMP-FLX-001 | SCORECARD C1.1: `GET /v1/models` exists live (200; tier aliases + pinned models) | fixtures `models/`; probe `scripts/flux-probe/` | recorded |
| COMP-FLX-002 | C1.1: `POST /v1/chat/completions` 200, `usage.cost_usd` present — **v1 production wire** (wire-2 gate verdict) | fixtures `chat-completions/`; replay COMP-MODEL-001/002/003 | recorded + replayed green |
| COMP-FLX-003 | C1.1: `POST /anthropic/v1/messages` 200 (translation layer — `call_*` tool ids, not native) | fixtures `anthropic-messages/` | recorded |
| COMP-FLX-004 | C1.1: `POST /anthropic/v1/messages/count_tokens` 200, matches messages input count | fixtures `anthropic-count-tokens/` | recorded |
| COMP-FLX-005 | C1.1: `POST /v1/responses` 200, valid Responses object with reasoning items | fixtures `responses/` | recorded |
| COMP-FLX-006 | C1.1: `POST /mcp/` 200 — initialize OK (`litellm-mcp-server` v1.0.0); trailing slash required (`/mcp` → misleading 401) | fixtures `mcp/`; live COMP-MCP-004 | recorded + live-gated |
| COMP-FLX-007 | Batch-2: streaming canonical on all three wires (completions chunks w/ `reasoning_content` deltas; anthropic lifecycle; responses lifecycle) | fixtures `streaming/`; completions-side replay COMP-MODEL-002 | recorded + replayed green (completions wire only) |
| COMP-FLX-008 | Batch-2: tool calls on both inference wires (`finish_reason:"tool_calls"`; `tool_use` block + `stop_reason:"tool_use"`) | fixtures `tool-calls/`; completions-side replay COMP-MODEL-003 | recorded + replayed green (completions wire only) |
| COMP-FLX-009 | Batch-2 WIRE-2 GATE: thinking/cache pass-through **FAILS** on live Flux (silently dropped, incl. pinned claude-sonnet) → contract amended: Completions is the v1 wire; Messages adapter is compatibility-only | fixtures `thinking/`, `cache/`; verdict in FINDINGS.md | recorded (gate outcome: FAIL → amendment adopted) |
| COMP-FLX-010 | Batch-2: omit-`max_tokens` on completions confirmed (stop, no 400, `service_tier` present) — #456/#462 contract holds | fixtures `omit-max-tokens/` | recorded |
| COMP-FLX-011 | Batch-2: `/mcp/` tools/list works, catalog **empty** — DoD "Flux MCP invoke" has no invocable tool yet | fixtures `mcp/`; live COMP-MCP-004 | recorded (blocked upstream, not by engineering) |
| COMP-FLX-012 | Client smoke: egress → client → API → parsed events end-to-end against real Flux | fixtures `client-smoke/`; `nano-model live_smoke::*` (COMP-MODEL-007/008) | live-gated |

FINDINGS.md "not yet covered" items are gaps — see §E (G-FLX-*).

---

## D. C1.2 proof harness (`nano-k3/scripts/c12-proof/`)

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\c12-proof\Test-C12Proof.ps1`
(standard user, no WSL). External-state oracles only (fs / CIM process scan /
socket), never self-report. Emits BUILD_PLAN_V3 §8 manifests to
`scripts/c12-proof/evidence/`. Latest evidence
(`c12-manifest-20260809T233109Z.json`, commit `b05e26a`, dirty tree):
**8 pass / 0 fail / 2 skip**, `provisioned: false`.

| Scenario | Proves (contract) | Probe / oracle | Status |
|---|---|---|---|
| COMP-C12-001 | C1.2: workspace write allowed | `write-inside-root` — file exists | green |
| COMP-C12-002 | C1.2: denied read fails | `sensitive-read-deny` — read throws | green |
| COMP-C12-003 | C1.2: junction cannot escape deny | `junction-escape` — file absent | green |
| COMP-C12-004 | C1.2: descendants dead ≤5s | `tree-kill-5s` — CIM scan, killed in 4ms | green |
| COMP-C12-005 | C1.2/C2.5: no orphan helpers | `process-cleanup` — CIM scan | green |
| COMP-C12-006 | C1.2: offline-identity network deny (WFP filters present) | `network-deny-offline` — netsh WFP | provisioning-gated (SKIP in latest manifest) |
| COMP-C12-007 | C1.2: broker reaches Flux (TLS+HTTP respond; auth failure still proves reachability) | `broker-network-ok` — HTTPS status | green |
| COMP-C12-008 | C1.2: path edge cases — long path, Unicode, reserved names | `path-edgecases` — create/read | green **with open observation**: latest run logged `reserved-name:WROTE(unexpected)` (`aux.txt` write succeeded); probe still reports PASS — needs owner decision whether reserved-name rejection is a C1.2 requirement |
| COMP-C12-009 | C1.2: setup idempotent (rerun makes no changes) | `setup-idempotent` — marker hash equal | provisioning-gated (SKIP in latest manifest) |
| COMP-C12-000 | Harness self-check (host/user/provisioned recorded) | `harness-env` | green |

**README/script drift:** `scripts/c12-proof/README.md` lists two probes the
script does not implement — `write-outside-root` and `uninstall-scope`. They
are carried as gaps G-C12-1/G-C12-2 until either the script gains them or the
README is corrected.

---

## E. Gaps — contract requirements with no proving scenario yet

| Gap ID | Requirement (contract ref) | Why open | Unblocked by |
|---|---|---|---|
| G-UNIX-1 | C1.2-equivalent containment on macOS (seatbelt) | **landed** (b68e7a9): `macos_seatbelt` + 3 `.sbpl` profiles ported, 22 builder tests green on host, `aarch64-apple-darwin` check+clippy clean | hosted macOS runner leg (matrix authored) |
| G-UNIX-2 | C1.2-equivalent containment on Linux (landlock/seccomp, `nanok3-linux-sandbox` helper) | **landed** (b68e7a9 + bwrap pipeline): legacy landlock/seccomp + modern bwrap orchestration ported; `x86_64-unknown-linux-gnu` check+clippy clean; runtime proof pending hosted leg | hosted Linux runner leg (matrix authored) |
| G-UNIX-3 | 6-target CI matrix (incl. ARM64 Windows — compile-gate only, not claimed without hardware) | **authored** (0ecf88a): win x64/arm64, macos-13/14, ubuntu x64/arm64 in `gate.yml`; not yet pushed to a remote | remote + first hosted run |
| G-ADV-1 | Adversarial formalization beyond the corpus: Track-B-owned adversarial cases (egress bypass attempts, journal fuzzing, policy confusion) | **closed**: 31 tests in `tests/adversarial_*.rs` (egress/fs/shell/sse); found+fixed 6 real holes (6e44921); corpus adversarial replay (COMP-CORPUS-004) covers Desktop's set | — |
| G-PKG-1 | NPM packaging acceptance (pack/install/run from packed artifact; signing via NPM per owner) | **closed**: offline clean-prefix install + doctor + acp initialize + negative-platform checks all PASS — `packaging/npm/ACCEPTANCE.md` | — |
| G-C12-1 | C1.2: write-outside-root probe in the live harness (README promises it) | **implemented** (gated): probe added to `Test-C12Proof.ps1`, external-oracle via sandboxed child; SKIP until provisioned | owner provisioning (runs for real) |
| G-C12-2 | C1.2: uninstall-scope probe (uninstall removes only Nano state) | **implemented** (gated): residue-scan probe added; NOTE no uninstall mode exists in the setup helper — probe audits provisioning residue, extend when uninstall ships | owner provisioning (runs for real) |
| G-C12-3 | C1.2 full criterion sign-off | **provisioning-gated**: owner elevated provisioning (`scripts/provision/`) must run, then COMP-C12-006/009 flip from SKIP | owner action |
| G-FLX-1 | Streaming on anthropic + responses wires consumed by adapters (fixtures exist; only completions wire is replayed in tests) | Completions chosen as v1 wire (COMP-FLX-009); other adapters compat-only | if Messages/Responses adapters are promoted |
| G-FLX-2 | FINDINGS "not yet covered": 402/409 typed errors, `Retry-After` header live, cancellation mid-stream, `x-wl-*`/`x-flux-*` header behavior | **closed**: batch 3 recorded (FINDINGS.md Batch-3 section + fixtures); live-wire corrections landed in nano-model (500+auth_error→Auth non-retryable, 413→ContextOverflow, 503-HTML→retryable Server; fixture-replay tests). 402 shape unverifiable with test key — documented substitution | — |
| G-FLX-3 | Flux MCP `tools/call` | upstream catalog empty (COMP-FLX-011) — mercy-rule deferral, not engineering | Flux adds an invocable tool |
| G-C2-1 | C2.3: Desktop-facing frame stream + <600s frame-cadence conformance test | **closed**: `streamed_turn_cadence_orders_frames_and_flushes_between_them` in nano-protocol host tests (order + per-frame flush asserted) | — |
| G-C2-2 | C2.4: cold-start / idle-RSS / active-RSS for the agent path | **closed**: `docs/metrics/C2-metrics.md` via `nanok3-acp-profile` — spawn→ready median 5.41ms, initialize 0.02ms, ~3.16M frames/s codec throughput (measured, repro commands included) | — |
| G-C3-1 | C3.1 full leg: **real** Desktop launches real runtime (negotiate→…→resume) | Desktop-side engine-kind work in the Desktop repo; engine-side proven (COMP-CLI-002/003); Desktop registration done, awaiting owner live conversation | owner + Desktop lane |
| G-CTR-1 | Frozen contract artifacts in `shared/contracts/` (capability profile, journal semantics, Flux endpoint contract, event types) | Track A has not produced them; Track B must adopt verbatim — catalog currently keys off SCORECARD/POINTER/FINDINGS | Track A P1 freeze |

## Maintenance rules

- Scenario IDs are append-only; never reuse a retired ID (mark it `retired`
  with a pointer to its replacement).
- A scenario that cannot find its subject matter (corpus, fixtures, binary)
  must FAIL, never silently skip — existing precedent: `corpus.rs`.
- Live-gated scenarios must stay self-skipping without `FLUX_TEST_KEY` so a
  credential-less CI lane stays green; the live lane asserts they ran.
- Update the status column in the same PR that flips it; stale `green` is a
  compliance bug.
