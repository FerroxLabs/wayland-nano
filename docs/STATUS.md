# NanoK3 (Track B) status

## Current position

### RC GOAL STATE (2026-08-11, late wave — Phase D + acceptance landed)
- **Phase D COMPLETE + UI-proven**: model picker lists + switches the Flux
  catalog (modelId wire fix, live-verified), MCP tool call through the
  permission gate, skills marker. Packaged-build smoke PASS (win-unpacked
  artifact, scratch profile, CDP via WAYLAND_CDP_PORT).
- **Clean-machine acceptance PASS** (`shared/reviews/C3/evidence/acceptance/`):
  offline npm install of the alpha tgz → registered in a fresh packaged
  Desktop profile → write-with-permission + read-back conversation →
  on-disk verification. Third sighting of the Desktop spawn-key quirk
  (fresh-profile migration strips the assistants record) — Desktop lane.
- CI caught + fixed a modelId assertion miss (de1cae4); pack.ps1 PS5.1
  compat (5b961ff). Wave HEAD: de1cae4.
- PENDING OWNER: (1) elevated migration (docs/REBRAND.md, kit in .tmp/);
  (2) npm waylandnano scope + NPM_TOKEN repo secret (enables publish).
- Remaining mine: soak run, panel re-cert, Phase A/F close-out.
- **Phase A COMPLETE except owner migration**: Track A verdict posted;
  rebrand committed + CI-green; 4 frozen contracts; sandbox fix port landed
  (transactional ACL rollback, root/descendant refresh, bounded helper
  lifecycle + taint sentinels, AccessCheck-verified token; 30+ new tests;
  deny-ACE latent caveat closed); hardlink race fixed. OWNER: elevated
  migration per docs/REBRAND.md (kit staged in ../.tmp/).
- **Phase C COMPLETE**: Responses + Anthropic-compat + count_tokens live-
  verified (6/6 smokes), shared classification, committed.
- **Phase D in motion**: model catalog + set_model landed (picker shows the
  Flux catalog; note Desktop's tier-id short-circuit for its lane); skills
  via UI PASS; MCP wired into acp-host (per-session registries, permission-
  gated) — UI re-proof pending; packaged smoke pending.
- **Phase E**: scaffold + release.yml (tag-driven, provenance publish);
  owner actions: create npm waylandnano scope + NPM_TOKEN repo secret.
- Wave committed as 70f0771 (MCP + sandbox port + models). CI validating.
- **Phase A**: Track A verdict posted (`shared/reviews/tracka-comparison.md`,
  kill/archive recommended; salvage: hardlink spike + provisioning ADR +
  sandbox fixes). Rebrand committed (1efa0fe): wayland-nano binaries,
  NanoSandbox* machine state + fresh WFP GUIDs, @waylandnano/nano, Desktop
  profiles re-registered. 4 frozen contracts in shared/contracts/. Sandbox
  fix port-by-review + hardlink-race FIX landed (real hole: multi-link
  writes now denied; docs/audits/hardlink-race.md). PENDING OWNER:
  elevated migration run (uninstall NanoK3 → provision Wayland Nano).
- **Phase C (done)**: flux_responses + anthropic_messages (COMPAT) +
  count_tokens implemented; shared flux_common classification (500-auth /
  413-overflow can't diverge); 16 fixture tests + 6/6 live smokes green vs
  real Flux. /mcp documented upstream-blocked.
- **Phase D (in motion)**: model discovery + session/set_model landed
  (picker catalog, routed turns, fail-closed unknown ids; Desktop tier-id
  short-circuit noted for Desktop lane). Skills via UI PASS. MCP via UI
  exposed a real gap (acp-host dropped mcpServers) — wiring in progress.
  Packaged-app smoke pending.
- **Phase E (scaffold done)**: gate-deny is a HARD gate now (wildcard deps
  pinned, CDLA-Permissive allowed for webpki-roots), CycloneDX SBOM step on
  windows-latest leg, NOTICES.md, docs/COMPATIBILITY.md, evidence-bundle
  collector (scripts/collect-evidence.ps1, sha256 manifest, fail-closed).
  Publish is owner-gated (npm waylandnano scope + token in CI secrets).
- Consolidated commit of the agent wave lands after the two running agents
  settle (their files interleave); tree currently compiles clean.

### Previous state (2026-08-11 — GOAL COMPLETE: C1/C2/C3 promoted by owner)
- **OWNER SIGNED ALL THREE VERDICTS** (Sean, 2026-08-11, VERIFIED—promote):
  `shared/reviews/C1,C2,C3/trackb-verdict.md`. This follows the unanimous
  panel PROMOTE and the final cross-audit: Codex (GPT 5.6 Sol) + Claude
  (Fable) both MEETS-THE-BAR on every checkpoint
  (`shared/reviews/panel/codex-sol-final-audit.md`,
  `claude-fable-final-audit.md`).
- Final audit hygiene closed the same day: firewall DisplayName scanner fix,
  SHA-bound kill manifest, C3 timestamp note, RSS reconciliation, and the
  published full-corpus canary scanner (`scripts/canary/scan.mjs`, 33
  artifacts / 219KB / zero hits).
- Track B (nano-k3) is COMPLETE against the scorecard: C1 substrate proven,
  C2 minimum native task proven, C3 Desktop vertical slice proven — all
  externally verified, all owner-promoted.
- Follow-on work (new goals, not this one): Track A comparison
  (`shared/reviews/tracka-comparison.md` pending), model discovery in
  Desktop (ACP models/set_mode), MCP+skills via Desktop UI, packaged-app
  smoke, soak, NPM publish, Desktop-lane bugs (4 filed in C3 evidence),
  wl-cdp renderer regression is Desktop's.
- Debt ledger clean: deny-ACE audit correct-by-design
  (`docs/audits/deny-ace-scan.md`).
- **Debt (from Track A salvage)**: same-user NTFS hard-link containment race
  is UNANALYZED in our DACL model — Track A's spike
  (`shared/contracts/windows-hardlink-containment.md`, adopted) shows a
  hard link created inside a writable root can point at a file outside it,
  and DACL containment alone may not cover the link target's semantics.
  Needs analysis + adversarial test. Track A's sandbox fixes (transactional
  ACL rollback, AccessCheck-verified token, bounded helper lifecycle) are
  being ported by review; re-audit deny-ace conclusions after.

### Previous state (2026-08-11, PANEL AUDIT COMPLETE — corrective rounds closed)
- External panel (codex + claude lenses; gemini disqualified — Google retired
  the tier) audited the C1/C2/C3 claims adversarially over 4 rounds. Verdicts
  archived in `shared/reviews/panel/` (codex-verdict.md, claude-verdict.md).
- Round 1 dispute produced REAL corrective work: C1.2 probes re-aimed at the
  offline identity (real blocked connect, junction/read denials as
  NanoK3SandboxOffline), PID-scoped tree-kill, full uninstall lifecycle
  (caught a stale-binary residue failure, fixed, zero-residue pass),
  kill-mid-edit artifact, C3 restart→resume oracle via real session/load.
- Round 3-4: REAL process-kill oracle (TerminateProcess mid-turn, clean-SHA
  manifest at edb8e62), Desktop-style cadence watchdog (300s semantics, max
  gap 11.2s), live active-agent RSS 13.6 MiB, C3.4 crash-boundary inventory
  + hash-pinned non-empty state diff, interrupted-call replay branch pinned,
  unified C3 evidence pack (canary 0/7, process inventory, raw protocol/DB
  artifacts), narrative reconciliation, gate logs attached.
- Final panel state: **UNANIMOUS — PROMOTE on C1, C2, C3 from both judges**
  (codex's final C2.4 hold closed by `shared/reviews/panel/c2-perf-live.json`,
  the machine-readable live-agent RSS artifact). Panel orientation:
  `shared/reviews/panel/README.md`.
- AWAITING OWNER ONLY: the 3 verdict signatures (C1/C2/C3) per SCORECARD §4.
- Desktop lane bugs reported (their repo): orphaned custom-agent UI,
  assistants/customAgents spawn-key mismatch, broken `where` cli_check,
  detector drops avatar; wl-cdp worktree @ cfc318ab has a transcript-
  rendering regression (turns persist, nothing renders).
- Debt: path_mask_allows/dacl_mask_allows ignore deny ACEs (donor semantics;
  audit whether production deny flow can rely on a false return).

### Previous state (2026-08-10, C1 CLAIM POSTED — all three checkpoints claimed)
- **C1.2 FULL CRITERION: 12/12 PASS, elevated** (manifest
  `scripts/c12-proof/evidence/c12-manifest-20260810T211025Z.json`). Owner
  provisioned the box (accounts/group/firewall/WFP/marker). Final C1 claim
  posted: `shared/reviews/C1/trackb-claim.md`.
- **ALL THREE CLAIMS POSTED**: C1 (substrate), C2 (`shared/reviews/C2/`),
  C3 (`shared/reviews/C3/` incl. live-Desktop evidence). Awaiting ONLY owner
  verdict signatures (3 × trackb-verdict.md).
- Harness nits recorded in the C1 claim (providers-dump skip detail,
  firewall Name-vs-DisplayName enumeration) — cosmetic, security probes green.
- Audit follow-up queued: `path_mask_allows`/`dacl_mask_allows` ignore deny
  ACEs (donor semantics; evaluate whether production deny flow can rely on
  a false return — flagged by the env-test fix agent).

### Previous state (2026-08-10, CI MATRIX 6/6 GREEN — unix parity runtime-proven)
- **ALL 6 LEGS GREEN** (run 31379610255, commit b2f7ef1): windows-latest x64,
  windows-11-arm, macos-14 arm64, macos-15-intel x64, ubuntu-22.04 x64,
  ubuntu-24.04-arm. Full gate: fmt, clippy -D warnings, complete test suite —
  including seatbelt/bwrap/landlock RUNTIME enforcement tests and the
  adversarial containment suite on real macOS/Linux runners. G-UNIX-1/2/3 closed.
- Shakeout record (12 fixes): retired runner label, missing rustfmt/clippy
  components, include! path resolution, corpus + flux fixtures vendored
  (standalone-repo self-containment), doctor/shell cross-platform, 3
  env-robust test fixes, 2 MCP sh twins, bin-build ordering, ubuntu-24.04
  userns sysctl, seatbelt carveout determinism.
- **CONTAINMENT HOLE FOUND + FIXED BY CI** (b2f7ef1): the unix shell tool used
  donor-default workspace_write() which grants /tmp writes — adversarial
  escape tests caught it on first real execution. Unix shell profile now
  excludes TMPDIR//tmp writes. Verified in WSL2 against real bwrap 0.9.0 and
  legacy landlock. NOTE: unix sandboxed shell can no longer write system temp.
- CI watcher cron retired on green.

### Previous state (2026-08-10, ACP live-I/O rework + full Desktop re-verification)
- **ACP ADAPTER REWORKED** (9145348): frames now stream DURING the turn
  (TurnEngine event sink, not batch replay); session/cancel works (reader
  thread + per-session AtomicBool, stopReason cancelled); ApproveAll replaced
  by a real ACP session/request_permission bridge (fail-closed deny on
  reject/malformed/disconnect; read-only tools auto-approve per Desktop
  Default mode). Proven by tests/acp_live.rs (interleave/cancel/permission).
- **UNINSTALL COMPLETE** (460f304): secrets file (content-verified), UserList
  registry values, .sandbox log dir — all fail-closed NanoK3-scoped.
- **DESKTOP RE-VERIFIED LIVE** (post-rework): read regression PASS; permission
  round-trip PASS (fs_write card "Allow once / Deny" → Enter → file written,
  byte-verified); cancel PASS (stop button → no write, session survives).
  Evidence: shared/reviews/C3/evidence/07-10 + manifest final section.
  Two Desktop UI nits noted (no cancelled marker; one presentation race).
- **C1 CLAIM DRAFTED**: shared/reviews/C1/trackb-claim.draft.md — finalizes
  mechanically after owner provisioning.
- Ops note: stale target/release/nanok3.exe was held by a stray agent child
  (file lock); killed + rebuilt. If the release exe ever looks stale, check
  for stray nanok3.exe processes first.

### Previous state (2026-08-10, C3 OWNER LEG DONE — live Desktop conversation)
- **C3 LIVE LEG PASS**: real Desktop UI (wl-cdp dev build @ 9f009f81, CDP-driven)
  ran Nano K3 end-to-end, two independent conversations: picker select → fs_read
  tool card → streamed correct answer → completed. Evidence:
  `shared/reviews/C3/trackb-desktop-live-evidence.md` + 5 screenshots + ACP
  transcripts in `shared/reviews/C3/evidence/`. Driver scripts: `../../.tmp/cdp-drive/`.
- **3 Desktop bugs found + reported** (in the manifest): (1) custom-agent
  registration UI is orphaned (mount chain dead-ends; both entry links redirect
  away); (2) spawn path reads `assistants` but detection reads `acp.customAgents`
  — every custom agent spawns without its args/env (worked around via dual-key
  registration; one-line Desktop fix suggested); (3) Test Connection's `where`
  cli_check always fails on Windows absolute paths.
- CDP recipe for future UI tests: `WAYLAND_CDP_PORT=9243` + scratch
  `WAYLAND_DEV_PROFILE=WIN-CDP` on the wl-cdp worktree; Chromium ≥136 ignores
  --remote-debugging-port on default profiles; packaged build fuses block it.
- Remaining owner actions: elevated provisioning (C1.2), C2/C3 verdict
  signatures, git remote for the 6-target CI matrix.

### Previous state (2026-08-10, swarm-2 landing: bwrap + probes + live-wire fixes)
- **BWRAP LANDED**: full modern Linux path (bwrap FS isolation → self re-exec
  `--apply-seccomp-then-exec`) in nanok3-linux-sandbox; bundled/system bwrap
  resolution, WSL1 detection, NANOK3_BWRAP_SHA256 pin. Linux parity code-complete;
  runtime proof awaits hosted Linux leg.
- **C1.2 HARNESS COMPLETE**: write-outside-root + uninstall-scope probes added
  (gated, external-oracle); setup-idempotent vacuous-pass FIXED (refresh_marker_only
  payload + tamper-detection oracle). aux.txt anomaly resolved: NOT a violation —
  Win11 build 26200 no longer rejects reserved names at create; token+DACL
  enforcement attaches to the resolved NT object regardless of spelling.
- **LIVE-WIRE CORRECTIONS** (Flux batch-3 fixtures): auth fail = HTTP 500 +
  auth_error (was: retried as Server); context overflow = HTTP 413; 429/Retry-After
  never occurs (burst = edge 503 HTML); no x-wl-* headers (x-flux-*/x-litellm-*
  instead). nano-model classifier + 4 fixture-replay tests landed. 402 shape
  unverifiable with test key (documented substitution).
- **C2 DEBT CLOSED**: frame-cadence test (order + per-frame flush) + C2-metrics.md
  (spawn→ready 5.41ms median, initialize 0.02ms, ~3.16M frames/s codec throughput).
- **NPM ACCEPTANCE PASS**: offline clean-prefix install, doctor exit 0, acp
  initialize handshake, unsupported-platform refusal — packaging/npm/ACCEPTANCE.md.
- **KNOWN FLAKE (debt)**: one transient red in adversarial_egress under full-
  workspace parallelism (hostile-listener race); green in 2 consecutive reruns +
  isolated runs. Harden listeners before trusting CI signal blindly.
- Workspace: 2 consecutive full-suite runs green, clippy clean.

### Previous state (2026-08-10, swarm landing: unix parity + adversarial + CI + packaging DONE)
- **UNIX PARITY LANDED** (b68e7a9): macOS seatbelt + Linux landlock/seccomp ported
  into nano-sandbox; cargo check + clippy clean on linux-gnu + apple-darwin targets;
  22 builder tests run on Windows host. bwrap deferred (UPSTREAM.md TODO).
- **ADVERSARIAL SUITE** (6e44921): 31 tests; found + fixed 6 real holes (egress
  redirect bypass, 3 credential-leak displays, junction/symlink write escape).
  nano-model transport leak closed via shared sanitizer. No test weakened.
- **CI MATRIX AUTHORED** (0ecf88a): 6 targets (win x64/arm64, macos-13/14,
  ubuntu x64/arm64); unix legs go green on first hosted run. Not yet pushed to a remote.
- **NPM SCAFFOLD** (f387c03): zero-dep installer + launcher, win32-x64 staged,
  win32-arm64 runtime-rejected per compile-gate-only rule.
- **COMPLIANCE CATALOG** (973f5b7): docs/compliance/SCENARIO_CATALOG.md — 290-test
  inventory keyed to COMP-* IDs + 16-gap register. NOTE: shared/contracts/ is EMPTY
  (Track A freeze never landed, gap G-CTR-1) — catalog keys off SCORECARD §2 instead.
- Workspace: 290+ tests green, clippy -D warnings clean, HEAD = 973f5b7.

### Previous state (2026-08-10, post-compaction: Desktop registration DONE)
- **ACP ADAPTER LIVE** (commit 2eddab7): `nanok3 acp-host` speaks ACP; slice proves
  initialize v1 → session/new → prompt with streamed updates → end_turn → canary
  clean → zero orphans. Release binary: target/release/nanok3.exe (6.4MB).
- **DESKTOP REGISTRATION DONE**: `acp.customAgents` entry `nanok3` written to
  `%APPDATA%/Wayland/config/wayland-config.txt` (backup: `../../.tmp/wayland-config-backup-20260810-071442.txt`).
  Launch: `target/release/nanok3.exe acp-host`, env `FLUX_API_KEY_FILE` →
  `../../.secrets/flux-test-key` (new `flux_key.rs` resolver: env first, then
  key-file path — secret stays out of the config blob). Exact launch config
  smoke-verified: initialize handshake clean with file-only credential.
  **Awaiting owner: fully quit + restart Desktop, pick "Nano K3", run one
  real prompt.** (Desktop was running during the edit; if the entry vanished,
  re-apply after quit.)
- **NEXT (in order)**: (a) OWNER live conversation in Desktop UI;
  (b) OWNER provisioning → C1.2 → C1 claim; (c) Unix containment port
  (seatbelt/landlock from Codex); (d) 6-target CI matrix; (e) compliance matrix;
  (f) adversarial formalization; (g) NPM packaging (signing via NPM per owner).
- ARM64 Windows: compile-gate only, not claimed without hardware.
- User directive: NOTHING cut — parity/signing/compliance/adversarial all required.

### Original position

- Goal: active (K3 native goal mode; objective = C1→C2→C3 per `../../shared/SCORECARD.md`)
- Phase: **C1 code-complete; C2 + C3 CLAIMED (shared/reviews/C2,C3)**; awaiting owner: C2/C3 promotion decisions + elevated provisioning for C1.2 + C1 claim
- Checkpoint: C1 ready-to-prove (no claim posted); C2 in progress
- Last green: workspace-wide tests + clippy `-D warnings` clean, commit HEAD
- Track A (observed read-only): G0-era sandbox corrections ongoing

## C1 scoreboard

| Leg | Status | Evidence |
|---|---|---|
| C1.1 Flux fixtures | ✅ | all six endpoints live-verified, 2 batches + client smoke; wire-2 gate failed → Completions = v1 wire (`shared/fixtures/flux/FINDINGS.md`) |
| C1.2 containment mechanisms | ✅ ported + unit/integration proven | 40+ files ported; live proofs: outside-root write DENIED (unified_exec + capture), DACL deny blocks write, Job tree-kill 3–4ms no survivors |
| C1.2 full criterion | ⏳ **awaiting owner elevated provisioning** | `scripts/provision/README.md`; proof harness green: `scripts/c12-proof/` (8 pass / 0 fail / 2 provisioning-gated skips) |
| C1.3 session journal | ✅ | 10 kill-boundary tests; torn-tail, unknown-skip, idempotence, compaction equivalence |
| C1.4/5/6 metrics | ✅ | `docs/metrics/C1-metrics.md`: bins ~1MB, 26s release build, 71-crate production closure |

## Capabilities flipped after slice proof (2026-08-10)

- `mcp: true` — proven: model called `mcp__fake__probe` through the registry in the live slice; marker returned and reported.
- `skills: true` (extensions) — proven: skill instruction (SKILLCONFIRMED) visible in model reply in the live slice.
- The honesty rule held: flags flipped only AFTER end-to-end proof.

## C3 (vertical slice passing)

- `nanok3` binary: `doctor` (real self-diagnostics, exit 0) + `protocol-host`.
- `nano-protocol`: NDJSON wire, honest capabilities, malformed-tolerant codec, ready-first host loop.
- Vertical slice (`nano-cli/tests/vertical_slice.rs`): simulated Desktop host spawns real binary — ready-first, ping→pong, live framed turn (model read fixture file), clean exit, **zero orphans**.

## C2 foundations (landed after C1 code-complete)

- `nano-egress`: deny-by-default policy chokepoint, redaction-proof errors, flux_only preset.
- `nano-model`: neutral types, SSE parser, **Flux Completions client (v1 primary wire)**, Kimi retry policy, fixture-replay tests + **live smoke verified against real Flux** (egress → client → API → parsed events).
- `nano-tools`: fs (policy-enforced read/write/edit, sensitive defaults, bounded reads) + search (glob/content, deny-invisible, cycle-safe, bounded).

## Next actions (in order)

1. **OWNER: C2 promotion decision** (claim posted) + **elevated provisioning** (dry-run review first: `cargo run -p nano-sandbox --bin nanok3-provision-dry-run`).
2. C1.2 full proof → C1 claim to `shared/reviews/C1/`.
3. C2: shell tool (via sandbox SpawnSpec), agent loop (turn state machine + loop protection), journal integration.
