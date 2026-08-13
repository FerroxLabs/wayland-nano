# RC2 integrator wiring pass — the owed registrations

Everything below is integrator-owed wiring, deferred by lane splits. All of it
needs `acp_mode.rs`, so it lands AFTER the P2b + ToolSearch merges, in one
coordinated pass, on a `feat/rc2-wiring` branch off the then-current master.
Each item names its source obligation.

## 1. PTY tool registration (from the p4-pty fix-round checklist, F-31 context)

In the SAME merge that registers the five `pty_*` tools:

1. **Approval arm** — `acp_mode.rs` `AcpApproval::approve` (~:2966), an
   explicit `"pty_spawn"` arm BEFORE the mode match: `ReadOnly ⇒ Deny`;
   exec ⇒ Deny; `Default | FullAuto ⇒ prompt_host` ALWAYS — no
   sandbox-available fast path, no rule-DSL interaction (design §4.3).
2. **Session-surface registration** — append
   `nano_tools::pty::pty_tool_definitions()` at the host per-turn
   tool-definition build (`acp_mode.rs` ~:1867, where `v1_tool_definitions`
   is consumed). NEVER in `v1_tool_definitions` itself — children consume
   that set and `TaskApproval` default-denies unknown names. The
   child-exclusion regression test exists in pty.rs.
3. **Dispatch** — route the five names to the session-owned
   `PtySessionManager` in the session wrapper (the SESSION_TOOL_NAMES
   layer; see the miswiring guard at wiring.rs:846). Base
   `RealToolExecutor::dispatch` must never see them.
4. **Error kind** — `PtySessionGone` (retryable: false) in
   `error_kind.rs` per design §8: spec() arm + ALL_KINDS entry + pinned
   count bump + `gen_error_table` regen (Desktop mirror to a scratch dir —
   it regenerates only on a Desktop feature branch). Map
   `PtyError::PtySessionGone` at the tool-result boundary; Capacity/
   SandboxUnavailable/InvalidParams ride existing kinds.
5. **Gate-matrix tests** (design §13): the spawn-gating matrix that could
   not exist pre-wiring.
6. **Distribution**: `wayland-nano-pty-guard` is a new runtime-shipped unix
   binary — release/packaging must place it next to
   `wayland-nano-linux-sandbox` (or document NANO_PTY_GUARD_EXE). Update
   `packaging/npm/scripts/pack.ps1` staging + the release workflow.

## 2. repo_map registration (F-28 MEDIUM-2 — not optional)

§5.5 read-only registration at ALL THREE gates (`acp_mode.rs`, `tasks.rs`,
`exec_mode.rs`) + the three-predicate agreement regression test in the same
merge. The tool ships merged-but-unwired (`nano-tools/src/repomap.rs`).

## 3. Attachment GC sweep wiring (F-34, production finding from the P2a proof)

- `AttachmentStore::sweep()` at host startup (lease+grace discipline is
  in-crate) + `/doctor` store report (size, blob count).
- FIRST: F-32 LOW-7 — extend the sweep's reference scan to cover
  `ToolResult.image_refs` (P2b merged by then), or blobs referenced only
  by tool results get reaped live.
- Proof: re-run the P2a leg-4 GC concurrent leg (sweep racing a live
  attach — blob survives) and append to the consolidated manifest.

## 4. Session browser entry points (from the session-browser brief)

`session_browser.rs` is lane-owned; integrator wires: module registration,
`_wayland/session/list` dispatch, capability advertisement, `/doctor` call
site. Picker-load ships gated on the session-ownership slice's own review.

## 5. Desktop error-table mirror

After ALL kind-adding lanes land (P3's seven + PtySessionGone + any P4 §8
kinds): regenerate with `NANO_ERROR_TABLE_DESKTOP_DIR` pointing at a Desktop
FEATURE BRANCH worktree — never the user's main Desktop checkout, never from
a lane branch.

## 6. Post-wave

- Dispatch the review-mode lane (CODEX-P4-REVIEW-MODE-ASSIGNMENT.md) —
  acp_mode.rs is free only after items 1–4 merge.
- Per-pack adversarial proofs: P3 (dispatcher full-duplex live, ToolSearch
  hydration kill-resume, resources, elicitation round-trip, OAuth PKCE
  through egress) and P4 (rules narrow-only + per-shell parsing, PTY
  spawn-prompts + orphan teardown incl. the unix guard, repomap Windows
  path legs, session browser, review mode) — evidence appended to
  shared/reviews/RC/evidence/CONSOLIDATED-VERIFICATION.md.
- THEN the §13 leg-7 Desktop drive on Mac (owner).
