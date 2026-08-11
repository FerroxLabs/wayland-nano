# Audit: allow-only DACL scan in `path_mask_allows` / `dacl_mask_allows`

Date: 2026-08-10
Scope: `nano-k3/crates/nano-sandbox` (Windows ACL code), all callers workspace-wide,
donor comparison against `nano-k3/vendor/codex-windows-sandbox-rs` (pinned, byte-identical)
and the live upstream reference `nano/codex-rs/windows-sandbox-rs` (read-only).
Method: workspace-wide grep for both functions and their wrappers; manual read of every
production call site; no source modified.

## Verdict: CORRECT-BY-DESIGN today — with one LATENT-RISK caveat for future ports

No production code path in nano-k3 relies on `path_mask_allows` / `dacl_mask_allows`
returning FALSE after a deny ACE lands. Every production caller uses the result in exactly
one of two fail-safe directions:

1. **Grant-time idempotency** — "does this SID already have an allow ACE, so we can skip
   re-granting?" A stale allow masking a deny causes setup to *skip a grant*. The worst
   outcome is the sandbox losing access it should have (availability), never gaining access
   it should not have. And when the deny was placed intentionally (capability-SID deny-write
   carveouts), skipping the grant is precisely the desired behavior — a re-grant could not
   override the deny at OS level anyway.
2. **World-writable detection** — "does any allow ACE grant write to World, so we should
   harden this path with capability deny ACEs?" Here the allow-only scan is the
   *conservative* direction: a stale World allow (even one neutralized by a deny) triggers
   flagging and *additional* denies. Ignoring deny ACEs can only over-flag, never under-flag.

Actual access enforcement in nano-k3 is OS-level: restricted/impersonation tokens plus deny
ACEs, applied via the **deny-aware** helpers `dacl_has_write_deny_for_sid` /
`dacl_has_read_deny_for_sid` (through `DenyAceKind::already_present`,
`crates/nano-sandbox/src/acl.rs:632-637`). The allow-only scan is never consulted to
*permit* an operation at spawn/exec time. The Track-B exercise test
(`acl.rs:1029-1100`, `nano_tests`) already documents the stale-allow behavior explicitly and
uses `AccessCheck` as the effective oracle — the codebase treats the scan's semantics as
known, not accidental.

**No exploitable hole. No code change required.**

## Call-site table (production)

| # | Call site | Function used | Decision riding on result | Can a deny ACE be present? | Failure direction if stale allow masks a deny |
|---|-----------|---------------|---------------------------|----------------------------|-----------------------------------------------|
| 1 | `crates/nano-sandbox/src/acl.rs:386-393` (`dacl_allow_mask_needs_refresh`) | `dacl_mask_allows` + `dacl_mask_allows_with_scope(Explicit)` | Does SID need its allow ACE (re)granted? | Theoretically (cap-SID deny on the same path), but deny paths are carveouts disjoint from grant roots | Skip a grant → sandbox *loses* access (fail-closed); if the deny was intentional, skipping is correct |
| 2 | `crates/nano-sandbox/src/bin/setup_main/win.rs:1022` (`path_write_aces_need_refresh`) | via #1 | Skip write-ACE grant for sandbox group + write-root cap SID on each write root | Deny-write carveouts target cap SIDs on `deny_write_paths` (win.rs:1102-1153), which are not write roots; `audit.rs:291-297` explicitly skips flagged paths under workspace roots | Same as #1: fail-closed |
| 3 | `crates/nano-sandbox/src/acl.rs:420` (`ensure_allow_mask_aces_with_inheritance_impl`) | via #1 | Skip pushing a SET_ACCESS entry for a SID that already has the mask | Same as #1 | Fail-closed |
| 4 | `crates/nano-sandbox/src/bin/setup_main/win.rs:214,226` (`read_mask_allows_or_log` in `apply_read_acls`) | `path_mask_allows` | Do builtin RX SIDs / sandbox group already have read on a read root; skip grant if yes | A deny-read ACE for the sandbox group on a read root is not produced by any flow (`add_deny_read_ace` targets capability SIDs on deny-read paths, `deny_read_acl.rs:56-67`) | Skip a read grant → sandbox can't read a read root (availability, fail-closed) |
| 5 | `crates/nano-sandbox/src/bin/setup_main/win/setup_runtime_bin.rs:37` (`ensure_codex_app_runtime_paths_readable`) | `path_mask_allows` | Does sandbox group have read/execute on runtime bin dirs; skip grant if yes | No flow places deny ACEs for the sandbox group on runtime paths | Fail-closed (runtime not readable) |
| 6 | `crates/nano-sandbox/src/audit.rs:93` (`path_has_world_write_allow` in `audit_everyone_writable`) | `path_mask_allows` (World SID, write bits, any-bit) | Flag dir as world-writable → `apply_capability_denies_for_world_writable_for_permissions` adds **more** deny ACEs (`audit.rs:299`) | Yes — and that is fine: flagging a path that also has a deny only causes extra hardening | Over-flag (conservative). Under-flagging is impossible from ignoring denies |

Test-only callers (excluded from the verdict but noted): `acl.rs:1029,1061`
(exercise test — explicitly asserts the stale-allow behavior and cross-checks with
`AccessCheck`), `bin/setup_main/win.rs:1392` (refresh-logic unit test).

Paths checked with **no** use of either function: `capture.rs` / command-runner flow
(applies ACLs via `spawn_prep::apply_legacy_session_acl_rules` → `add_deny_write_ace` /
`add_allow_ace` / `sync_persistent_deny_read_acls`, all deny-aware or unconditional),
`deny_read_acl.rs`, `deny_read_resolver.rs`, `deny_read_state.rs`, `workspace_acl.rs`.

## Donor comparison: inherited design, not port drift

- The vendored donor (`nano-k3/vendor/codex-windows-sandbox-rs/src/acl.rs:102-216`) is
  byte-identical in these functions: same allow-only loop, same
  `if hdr.AceType != ACCESS_ALLOWED_ACE_TYPE { continue; }` skip. The allow-only semantics
  are donor semantics (recorded in `UPSTREAM.md` and `docs/STATUS.md` debt note).
- Live upstream (`nano/codex-rs/windows-sandbox-rs`) uses the functions in the same two
  fail-safe patterns only: grant-time idempotency (`bin/setup_main/win.rs:273`,
  `bin/setup_main/win/setup_runtime_bin.rs:33`, `acl.rs:501-502`) and world-write audit
  (`audit.rs:87`). All other upstream hits are tests.
- **One upstream call site does pair the scan with an effective check** — and it is the
  latent-risk caveat. Upstream `grant_legacy_user_delete_on_handle`
  (`nano/codex-rs/windows-sandbox-rs/src/acl.rs:1336-1351`) treats
  `dacl_mask_allows(..., DELETE, true) == true` as a fast path, but then verifies with
  `token_effectively_allows_delete` (an effective-token check) and *errors* on
  "legacy token has an effective DELETE denial". Upstream itself does not trust the
  allow-only scan where a wrong `true` would matter.
- nano-k3 does **not** port that function family (`grant_legacy_user_delete_on_handle`,
  `ensure_allow_write_aces_on_tree`, `token_effectively_allows_delete` — no matches in
  `crates/nano-sandbox`). The hazard therefore does not exist in k3 today, but any future
  port of the tree-grant machinery must bring the effective-check pairing with it.

## Recommendation

**No code change now.** The allow-only scan is used exclusively for pre-grant capability
checks and conservative detection, where allow-only semantics are the correct ones. Adding
a deny-aware variant today would be dead code.

Do instead:

1. **Document the contract** (low-cost, prevents the latent risk): add one line to the
   doc comments of `dacl_mask_allows` / `path_mask_allows` in
   `crates/nano-sandbox/src/acl.rs` stating that the scan ignores deny ACEs and must not be
   used to decide whether access is *permitted* — effective decisions require `AccessCheck`
   or a deny-aware helper. (Deferred here because this audit is docs-only.)
2. **Port-pairing rule**: if `ensure_allow_write_aces_on_tree` /
   `grant_legacy_user_delete_on_handle` are ever ported from the donor,
   `token_effectively_allows_delete` must be ported in the same change, and the pairing
   must be recorded in `UPSTREAM.md`. Add this as a checklist item when that port is
   scheduled; no standing code change is warranted until then.
3. Leave `docs/STATUS.md` debt note resolution to the owner (parent-managed file).

## Re-check after the Track A port (2026-08-11)

Scope: re-run of this audit's conclusion after the port-by-review of Track A's
post-baseline sandbox fixes (commits `1dae2d8ae`, `33f112c87`, `fa0ee4da3`, `9e0e88504`,
`1e3400144`, `f77057450`) into `crates/nano-sandbox/`.

**Verdict: conclusion holds; the latent-risk caveat is now closed.**

- The tree-grant family IS now ported (`ensure_allow_write_aces_on_tree*`,
  `grant_legacy_user_delete_on_handle`) — and `token_effectively_allows_delete` was ported
  in the same change, satisfying recommendation 2's pairing rule. The pairing is recorded
  in `UPSTREAM.md` (Track A post-baseline port rows). Every legacy-DELETE fast path
  (`dacl_mask_allows(..., DELETE, true) == true` in `grant_legacy_user_delete_on_handle`)
  is verified with the `AccessCheck` oracle and *errors* on an effective denial, exactly
  the discipline this audit flagged as required.
- Recommendation 1 is done: `dacl_mask_allows` / `path_mask_allows` doc comments now state
  the allow-only contract and forbid permit decisions.
- The donor's lenient deny scans (`dacl_has_write_deny_for_sid` /
  `dacl_has_read_deny_for_sid`, any-bit, silent on `GetAce` failure) are removed;
  `DenyAceKind::already_present` is replaced by the all-bits, generic-mapped, explicit-only
  `dacl_has_explicit_deny_mask`, and deny insertion is post-write verified and rolled back
  on failure. The stale-allow exercise test now asserts via `path_has_write_deny_for_sid`.
- Remaining allow-only call sites are unchanged in kind and still fail-safe: grant-time
  idempotency in `ensure_allow_mask_aces_with_inheritance_impl` / refresh checks (skip →
  lose access, never gain) and the world-write audit (over-flag only).
- New fail-closed note: the ported verification makes ACL mutation *sensitive to
  concurrent same-tree mutation by design* (quiescent precondition,
  `NANO_ACL_QUIESCENT_PRECONDITION`). Concurrent legacy spawns or concurrent setup runs
  against the same tree can now fail closed with a verification error where the donor
  silently proceeded. That is the intended honest behavior; the per-test TEMP/TMP fixtures
  keep the test suite quiescent, and production serialization of setup authority is
  in-process via the setup authority lane (machine-global serialization remains the P2
  broker's job per the Track A provisioning-boundary ADR).

**No exploitable hole. No further code change required.**
