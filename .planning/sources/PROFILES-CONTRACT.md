# PROFILES-CONTRACT.md — the P-PROF interface freeze (2026-08-20)

**Authority:** the lane-split authority for Profiles (P-PROF, Stage 2). Where a lane's
spec disagrees, this file wins. **P-PROF does not start until P-MEM lands AND this
contract is owner-signed.** Version 1.0 (draft — unsigned).

**Research base:** `research-0.2/MEMO-profiles.md` (Core config overlays + trust gate +
narrow-only personas; Hermes home-per-profile; dsh named compositions; codex closed
struct; Kimi exhaustive mode map). Owner-ratified direction: named declarative profiles
— "same engine, many shapes."

---

## 1. What a profile IS (the closed struct)

TOML `[profiles.<name>]` in `$NANO_HOME/config.toml`. `deny_unknown_fields` — unknown
keys are a hard error, never ignored (the codex lesson: silent-ignore config is a
bug farm). Fields:

| Field | Type | v1 |
|---|---|---|
| `model` | string (Flux alias or pinned leaf) | ✓ |
| `model_reasoning_effort` | string enum (existing ladder) | ✓ |
| `permission_mode` | `read_only` \| `default` \| `full_auto` | ✓ |
| `plan` | bool (plan posture) | ✓ |
| `tools.allow` / `tools.deny` | arrays of tool names | ✓ |
| `hooks` | string (named hooks profile under nano home) | ✓ |
| `rules` | string (named execpolicy rule set) | ✓ |
| `system_prompt_file` | path (relative to nano home) | ✓ |
| `extends` | string (single inheritance) | ✓ |

**v2 (NOT now):** `memory` (MemoryPolicy knobs — MEMORY-CONTRACT §5), `egress`
(allowlist class), `mcp_servers` (allowlist).

Profile names: Core's strict grammar — ASCII `[a-z0-9._-]`, case-folded, Windows device
names + control-plane names rejected.

## 2. The merge math (field-by-field, no hand-waving)

Effective posture = `min(launch_ceiling, profile, session_overrides)` with these rules:

- **Scalar modes** (`permission_mode`, `plan`): the permission lattice minimum —
  `read_only < default < full_auto`; plan is orthogonal and AND-combines.
- **Sets** (`tools.allow`): INTERSECT across layers. `tools.deny`: UNION.
- **Refs** (`hooks`, `rules`, `system_prompt_file`): narrow-only substitution — a
  profile may substitute only a ref whose posture is a subset of the baseline's
  (a stricter rule set may replace a looser one; never the reverse).
- **`extends`:** single inheritance, cycle-checked; the CHILD may only narrow the parent
  (a child that widens a parent field is a hard config error).
- **`model`/`effort`:** free choice (narrowing is meaningless for model choice — cost
  is handled by the budget authority, not the profile).

**Fail-closed resolution:** an explicitly requested profile (`--profile`) that can't
resolve (unknown name, untrusted project profile, invalid grammar) is a HARD ERROR —
exit 2 with the typed usage message. Only the sticky default falls through with a
warning (Hermes' rule: `--help` must always run).

## 3. Selection & layering

Resolution ONCE at process entry: `--profile` > config `profile = "…"` > sticky
`active` pointer under nano home > built-in defaults. Project-level
`.nano/config.toml` may DECLARE profiles but they load ONLY under workspace trust
(Core's exact gate — profile tables expand authority).

Session switching: `/profile <name>` applies ONLY narrow-safe fields (`model`,
`effort`) mid-session. Any permission-affecting field requires session restart —
typed refusal, never silent posture change.

## 4. Journal & resume (the security-sensitive part)

- The **fully resolved profile** (name + post-inheritance field hash + the resolved
  field values) is journaled at session start — `Op::ProfileSet`, additive,
  serde-defaulted, replay context-neutral.
- **Resume narrows, never re-widens:** on load, the journaled resolved posture is
  intersected with the CURRENT launch ceiling AND the current on-disk profile (a
  revoked tool stays revoked). Only if nothing narrowed does the journaled posture
  stand whole. No interactive "offer to re-resolve" — revocation always wins,
  non-interactively.
- Mid-session `/profile` switches are journaled events so replay reconstructs the
  posture timeline.

## 5. Trust posture (non-negotiable)

A profile may TIGHTEN or restate — NEVER widen beyond the launch-time ceiling.
Widening requires an explicit per-session grant (CLI flag `--full-auto` or interactive
approval), journaled as a grant event. Rationale: a profile file is a lower-trust write
surface than an interactive grant; if `nano --profile ci` could silently yield
full_auto, any process able to drop a TOML file escalates.

## 6. The three shipped profiles

- **`safe-review`** — `read_only` + plan posture; tools: read/grep/glob/repomap only;
  hooks: none; rules: deny-all exec; model: auto.
- **`builder`** — `default`; full tool set; standard hooks; default execpolicy;
  model: auto.
- **`ci`** — `default` + exec-only tool allowlist (no web/media); `rules = "ci-exec"`;
  hooks `ci`; model pinned. Combines with an explicit `--full-auto` GRANT at launch —
  never carries it in-file.

## 7. Lane ownership

| Lane | Owns | Never touches |
|---|---|---|
| Profiles lane | `crates/nano-core/src/profiles.rs` (new), `crates/nano-cli/src/profile_cmds.rs` (new) + main.rs arm, `crates/nano-session` Op::ProfileSet (additive) | nano-agent turn engine, acp_mode (integrator seam), execrules.rs |
| Integrator | the acp/exec/session-start consumption seams, `Op::ProfileSet` replay arm | the lane's files |

**Acceptance:** the merge-math battery (lattice min, set intersect/union, narrow-only
refs, extends-cycle hard error), fail-closed resolution tests, journal roundtrip +
resume-narrows tests (a revoked tool stays revoked across kill-resume), the three
shipped profiles load and behave, `just gate-all` green.

## 8. Signature

**Owner:** ______________ (date) — with MEMORY-CONTRACT signed, this unlocks P-PROF.

