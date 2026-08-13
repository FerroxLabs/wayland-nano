# Wayland Nano (Track B) — rules for agents working in this repo

These are standing rules. They apply to every agent and every change, and they
override convenience. If a rule blocks your task, stop and report — do not
route around it.

## Working discipline

Adapted for this repository from FerroxLabs `agents-md`. Repository security,
scope, and active-lane rules below take precedence whenever they are stricter.

- Start with the answer or action. No flattery, filler, or ceremonial status.
- Never fabricate paths, APIs, commits, behavior, or test results. Inspect the
  source or run the command; if evidence is unavailable, say so plainly.
- Disagree directly when a premise conflicts with repository evidence or these
  rules. Polite agreement never overrides correctness.
- Before editing, state verifiable success criteria, read the owned files and
  the interfaces they depend on, and match established repository patterns.
- Surface material assumptions. Ask only when two unresolved interpretations
  would materially change the result and repository evidence cannot decide.
- Implement the smallest complete solution to the locked requirement. Add no
  speculative features, future hooks, drive-by refactors, or unrelated cleanup.
- Every changed line must trace to the assignment. Remove orphaned code created
  by the change, but leave unrelated pre-existing code alone.
- Prefer executing focused tests over reasoning from a plausible-looking diff.
  Read complete failures, fix root causes, and never suppress a valid failure.
- Define completion with external evidence, run that evidence, and inspect its
  output. A plausible diff, compilation alone, or a narrow passing test is not
  completion.
- Communicate directly and concisely. Report facts, tradeoffs, blockers, and
  evidence; do not pad progress reports or restate the request.
- Commit messages are descriptive, have a subject under 72 characters, and
  explain why when the subject is insufficient. Do not add AI attribution.

## Scope and filesystem boundaries

- Write only inside `wayland-nano/` (this repo) and, when the task requires it,
  `../shared/`. Nothing else.
- `../nano/` (Track A) is **read-only**. `../resources/upstreams/` is
  **read-only** (immutable donor snapshots). Never write, delete, or "fix"
  anything there.
- Several agents may work in this repo concurrently. Stay inside your assigned
  files. If a workspace-wide cargo command fails transiently because of another
  agent's mid-edit code, retry after a minute, then scope to your own crates
  and note it.

## Secrets

- No secret values in files, test output, logs, fixtures, or commit messages.
  Ever.
- The Flux test key lives at `../.secrets/flux-test-key`
  (`waylandnano/.secrets/flux-test-key`). Reference the **path** only; never
  read, echo, copy, or embed the value.
- Credential resolution: Flux keeps `crates/nano-cli/src/flux_key.rs`
  (`FLUX_API_KEY`, then `FLUX_TEST_KEY`, then the file named by
  `FLUX_API_KEY_FILE`); every other catalog provider resolves via
  `crates/nano-cli/src/provider_key.rs` (injected bearer → canonical env var
  → `<VAR>_FILE`, per the vendored catalog in
  `crates/nano-model/data/providerCatalog.vendored.json` — the sole endpoint
  authority). Key files must be owner-only (0600) on unix. Use these
  patterns; do not invent new credential channels or hardcode keys.
- The vertical slice canary asserts no key appears in any frame. Keep it true.

## Fail-closed security

- Security invariants are: deny-by-default egress (`nano-egress`), OS
  containment (`nano-sandbox`), policy-enforced tools (`nano-tools`),
  append-only journal (`nano-session`). Fail closed everywhere —
  `SANDBOX_UNAVAILABLE`, never silent downgrade.
- **Never weaken sandbox/egress/policy/journal code — or a test — to make a
  run pass.** A failing test that exposes a real hole is a valuable result:
  report it prominently, do not patch it green.
- A scenario whose subject matter is missing must FAIL, never silently skip
  (precedent: `nano-protocol/src/corpus.rs`). Live-gated tests must keep
  self-skipping without `FLUX_TEST_KEY`.

## Naming and coexistence

- Track A coexists on dev boxes. Namespace everything Wayland Nano creates:
  `NanoSandbox*` identities (e.g. `NanoSandboxOffline`/`NanoSandboxOnline`),
  `wayland-nano-*` binaries/dirs, `NANO_*` env vars, `nano.*` metric names.
  Never reuse Track A's `Nano*`/`codex-*` names. (Renamed from the NanoK3
  codename; the authoritative map is `docs/REBRAND.md`.)

## Toolchain and code rules

- Pinned toolchain: **Rust 1.95.0** (`rust-toolchain.toml`), native MSVC
  (`x86_64-pc-windows-msvc`). Edition **2024** (workspace-wide).
- `windows-sys` is pinned to **0.52** — do not bump or add a second version.
- Gate before you claim done: `just gate-all` = fmt check +
  `cargo clippy --workspace --all-targets -- -D warnings` +
  `cargo test --workspace`. Clippy `-D warnings` is a hard gate, not a
  suggestion.
- Match the per-crate code style already present; ported files follow the
  transformation recorded in `UPSTREAM.md`, not your own preferences.

## Provenance

- Every file ported or adapted from a donor gets an entry in `UPSTREAM.md`:
  destination path, donor path, exact transformation. Verbatim copies say so;
  deviations (pins, renames, dropped surfaces) are recorded file-by-file.
- Vendored trees stay byte-identical to their pinned donor revisions.

## Evidence before claims

- No claim without externally verifiable evidence (SCORECARD §1.3):
  recorded fixtures before endpoint claims (`../shared/fixtures/flux/`),
  BUILD_PLAN_V3 §8 manifests before checkpoint claims.
- Capability flags stay false until end-to-end proof exists (the honesty
  rule — `mcp`/`skills` flipped only after live proof).
- Oracles are external state (fs / process inventory / network), never
  self-report.

## Checkpoints and promotion

- Checkpoints C1–C3 and the claim/verdict flow are defined in
  `../shared/SCORECARD.md`: a track posts a claim pointer in
  `../shared/reviews/<checkpoint>/<track>-claim.md`; the other track (or
  owner) records the verdict; **the owner promotes or rejects**. Agents never
  self-approve and never flip checkpoint status themselves.
- `docs/STATUS.md` and the gap register (§E) of
  `docs/compliance/SCENARIO_CATALOG.md` are owner/parent-managed — do not
  edit them unless explicitly assigned.
- No `git commit` / `git push` unless the owner explicitly asks.

## Reference map

- Architecture constitution: `ARCHITECTURE.md`
- Provenance ledger: `UPSTREAM.md`
- Third-party attribution: `NOTICES.md`
- Platform/Flux support levels: `docs/COMPATIBILITY.md`
- Release evidence bundle: `docs/release/EVIDENCE-BUNDLE.md` (+ `scripts/collect-evidence.ps1`)
- Sprint state: `docs/STATUS.md`
- Scenario catalog + gaps: `docs/compliance/SCENARIO_CATALOG.md`
- Scorecard / kill criteria: `../shared/SCORECARD.md`
- C1.2 proof harness: `scripts/c12-proof/`; provisioning: `scripts/provision/`

## Active lane contract: P4 persistent PTY

This worktree has exactly one active mission: implement
`../shared/reviews/panel-tui/CODEX-P4-PTY-ASSIGNMENT.md` against locked design
note `P4-comfort-deep-work-design.md` §4, including its audit correction
markers. Also read only the PTY-relevant traceability rows in §1, error kinds
in §8, test battery in §13, and §14 leg 3. The assignment and locked note are
the complete product contract. Do not expand, reinterpret, or improve adjacent
systems.

### Required deliverable

- Port and harden `spawn_conpty_process_as_user` into
  `nano-sandbox/src/conpty.rs`; never expose or use the donor `create_conpty`
  path. Replace donor `JobObject::create()` with
  `create_without_breakaway()` and preserve atomic job assignment through the
  process attribute list at `CreateProcessAsUserW` time.
- Promote the already-pinned `portable-pty` 0.9 dependency for Unix, with
  process-group teardown and typed fail-closed profile handling. Preserve
  `ConPtyDeferred` as the sandboxed-TTY deferred seam.
- Build exactly five session tools: `pty_spawn`, `pty_write`, `pty_read`,
  `pty_kill`, and `pty_list`. Yield-based polling is the only wait primitive.
- Implement the single contiguous 512 KiB rolling-tail byte buffer and exact
  cursor/resync schema required by §4, including UTF-8 boundary holdback and a
  dedicated per-session drain thread.
- Implement direct-descendant containment and session-scoped lifecycle cleanup,
  including `terminate_all` on session end and Drop. Keep broker-spawn escape
  documented and tested as the post-RC2 known limitation; never claim perfect
  process-tree containment.
- Keep `pty_spawn` always host-prompted for default/full-auto and denied for
  read-only/exec. Do not wire `acp_mode.rs`; report that integration seam.
- Register PTY tools on the session surface only, never in child definitions,
  and add the required child-exclusion regression test.
- Add the complete §13 PTY battery and §14 leg 3 external proofs on Windows
  ConPTY and one Unix profile. Record every donor adaptation in `UPSTREAM.md`.
- Commit the finished lane on `feat/p4-pty`; never push.
- Final evidence must name changed files, gate/test counts, exact five-tool
  signatures, deviations (expected: zero), coordination notes, and integrator
  wiring work.

### Owned write surface

- `crates/nano-sandbox/src/conpty.rs` (new)
- `crates/nano-sandbox/src/lib.rs` only to register `pub mod conpty;`
- `crates/nano-sandbox/Cargo.toml` only for dependencies required by the locked
  design
- `crates/nano-tools/src/pty*.rs` (new)
- The PTY manager module under `crates/nano-tools/src/`
- `crates/nano-tools/src/lib.rs` and `crates/nano-tools/Cargo.toml` only for PTY
  module/tool registration and required dependencies
- PTY-specific tests and fixtures under `crates/nano-sandbox/` and
  `crates/nano-tools/`
- `Cargo.lock` only for dependencies required by the locked design
- `UPSTREAM.md` only for this lane's provenance rows
- This `AGENTS.md` lane contract

Any other write requires an explicit owner amendment to this list. Reading
other repo files and immutable donors for interfaces/provenance is allowed.
The `nano-sandbox` `lib.rs` and Cargo manifest are merge coordination points:
preserve both this lane's changes and the P3 OAuth lane's changes, and flag
them in the final report.

### Forbidden work

- Do not touch `crates/nano-sandbox/src/process.rs`; the P3 OAuth lane owns it.
- Do not touch `acp_mode.rs`, `turn.rs`, `op.rs`, `replay.rs`, or `compact.rs`.
- Do not touch `crates/nano-mcp/**`, `crates/nano-egress/**`, repomap,
  execpolicy rules, review mode, or session-browser code.
- Do not implement the `pty_spawn` approval-gate arm. Expose the tools and
  report the exact `acp_mode.rs` wiring seam for the integrator.
- Do not add PTY tools to child definitions. The production child path remains
  `v1_tool_definitions`, and unknown TaskApproval names remain default-denied.
- Do not use `create_conpty`, assign a job after process start, use suspended
  spawn framing for ConPTY, hand-roll `posix_openpt`, add a stable-head buffer,
  add non-yield wait primitives, or silently downgrade containment/profile
  failures.
- Do not edit `docs/STATUS.md`, the scenario-catalog gap register, sibling
  worktrees, Track A, immutable upstream snapshots, or shared review notes.
- Do not weaken a security invariant, containment claim, test, or gate.

### Anti-loop execution rules

1. Work in this order only: binding requirements → owned implementation →
   focused tests → full PTY battery/provenance → required gates → scope audit →
   commit/report.
2. Every investigation must answer a named PTY requirement or a failing owned
   test/gate. If it does neither, stop it immediately as a side quest.
3. After two unsuccessful attempts at the same defect, write down the current
   hypothesis and gather new evidence before a third attempt. Never repeat the
   same command or edit expecting a different result.
4. A workspace failure outside owned files gets one retry after the repository
   settling interval required above, then crate-scoped checks. Record the
   external failure and continue owned work; do not repair another lane.
5. If the locked note conflicts with build reality, stop and report the exact
   citations and evidence. Never invent a deviation or broaden ownership.
6. Keep the active goal unchanged until every deliverable is proven. Partial
   compilation, a narrow test, or plausible code is progress, not completion.
7. Do not spend a turn producing another plan when an executable next step is
   available. Plans are navigation aids, not deliverables.
8. Do not read, print, copy, or test with the Flux key. This lane has no need
   for it.

### Project context for this lane

- Stack: Rust 1.95.0, edition 2024, Cargo workspace, native Windows MSVC;
  Windows ConPTY plus one Unix `portable-pty` profile.
- Iterate with focused `nano-sandbox` and `nano-tools` tests and clippy checks.
  Final commands are the completion-proof gates below; focused checks never
  substitute for them.
- Windows creation must fail closed before the child runs if job assignment or
  containment setup fails. Unix profile unavailability is likewise typed and
  fail-closed.
- The rolling buffer accounts in raw bytes and retains exactly
  `[oldest_retained, next_write)`. Below-range cursors resync with explicit gap
  accounting; above-range cursors return typed InvalidParams.
- Tests use external process inventory and filesystem/process state as oracles
  for direct-descendant reaping, assignment failure, breakaway rejection,
  flooding, and broker-escape limitation evidence.

### Project learnings

- Keep persistent-goal recovery autonomous: after two failed corrections,
  change the hypothesis and gather new evidence instead of requesting a reset.
- Treat the panel assignment and locked note as the only P4 product contract;
  adjacent improvements are out of scope even when useful.
- A missing platform security dependency activates the design's typed
  fail-closed posture; it never authorizes a local replacement or downgrade.
- Direct-descendant containment is the honest RC2 claim. Broker-spawned
  processes through scheduled tasks, WMI, or service RPC remain documented
  limitations, not grounds for a stronger claim.

### Completion proof

Completion requires all of the following from the current worktree:

- Requirement-by-requirement audit against the assignment, note §4, PTY rows
  in §1, §8 error kinds, §13 PTY battery, and §14 leg 3, with no missing named
  behavior.
- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `cargo test --workspace` passes.
- `cargo deny check` passes; `portable-pty` promotion is pre-approved, and any
  other dependency needs locked-note justification.
- `windows-sys` remains pinned to 0.52.
- `git diff --check` passes and the final diff contains only the owned write
  surface above.
- Provenance is complete and the branch contains clear local commits.

Never claim completion from intent, partial evidence, or absence of obvious
failures. If a security test exposes a real hole, preserve the failing evidence
and report it prominently rather than changing the expected result.
