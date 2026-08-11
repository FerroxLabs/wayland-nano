# Wayland Nano (Track B) — rules for agents working in this repo

These are standing rules. They apply to every agent and every change, and they
override convenience. If a rule blocks your task, stop and report — do not
route around it.

## Scope and filesystem boundaries

- Write only inside `nano-k3/` (this repo) and, when the task requires it,
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
- Credential resolution (`crates/nano-cli/src/flux_key.rs`): `FLUX_API_KEY`,
  then `FLUX_TEST_KEY`, then the file named by `FLUX_API_KEY_FILE`. Use this
  pattern; do not invent new credential channels or hardcode keys.
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
