---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 09
subsystem: verification
tags: [integrator, promotion, ci, dogfood, evidence]
requires:
  - phase: 07-08
    provides: sealed audited WP-4 builder product and request-only promotion handoff
provides:
  - WP-4 promoted to master with exact-SHA seven-job CI green
  - Restricted-token dogfood of all three sealed packs through the landed WP-3 CLI
  - Final program evidence and terminal owner handoff
affects: [wp4-promotion, program-complete]
tech-stack:
  added: []
  patterns: [exact-sha-ci-proof, ephemeral-account-dogfood, session-object-grant]
key-files:
  created: [.planning/phases/07-wp-4-gate-cards-and-dogfood/07-09-SUMMARY.md]
  modified: [.github/workflows/gate.yml, .planning/ROADMAP.md, .planning/STATE.md, .planning/REQUIREMENTS.md]
key-decisions:
  - "Gate cards execute in CI only as a fresh ephemeral standard account on a private window station."
  - "The ephemeral account receives a bounded grant on the session BNOLINKS object directory so the MSYS runtime can initialize."
  - "Temporary diagnostic machinery was removed once the environment was proven; the gate battery is the test."
requirements-completed: [CARD-08, EVID-01, EVID-02, EVID-03]
duration: multi-day-ci-promotion
completed: 2026-08-22
status: complete
---

# Phase 7 Plan 09: Integrator Promotion Summary

**WP-4 is promoted: the sealed Gate Card packs dogfood green in CI as a fresh ephemeral restricted account, through the landed WP-3 CLI surface only.**

## Accomplishments

- Integrated the audited WP-4 builder product by detached no-ff merge and appended the authoritative sibling `gate-cards` job to `.github/workflows/gate.yml`.
- Achieved exact-SHA seven-job CI green: the six platform gate matrix jobs plus `gate-cards`, on the final product SHA.
- The `gate-cards` job runs all three sealed packs (install-payload, provision-script, config-schema) plus the prescribed `ip-m1` bad arm (blocks with exit 3) as a cryptographically-random ephemeral standard account, never elevated, on a private window station, with a kill-on-close job, protected verifier identity, and full cleanup proof.

## CI promotion root causes (all fixed in the promoted workflow)

- Restricted children pinned to `WinSta0\Default` with a bounded grant hung or crashed (`0xC0000142`) when initializing `user32`. Fixed by spawning onto a private window station + desktop created by the broker with full grants for the ephemeral SID.
- The runner images enable WSL, so bare `bash.exe` resolved to the WSL launcher in `C:\Windows\System32`. Fixed by deterministic MSYS resolution: Git `usr\bin;bin` lead the child PATH (libuv walks cwd-then-PATH) and cmd probes use explicit MSYS paths.
- `LOGON_WITH_PROFILE` restored for the restricted spawn (lost in an earlier API switch).
- Final root cause, isolated by an in-job `LoadLibraryW` bisector: the MSYS runtime's `NtCreateDirectoryObject(\Sessions\BNOLINKS\<session>\msys-2.0S5-...)` was denied for the brand-new account. Fixed by a bounded grant on the session BNOLINKS directory via NT handle APIs (`GetNamedSecurityInfo` cannot address that path).
- The verify wrapper spawns gates with `env_clear` + a baseline env that does not forward `NANO_WP4_TEMP_ROOT`; the install-payload gate's fallback scratch inside the locked-down repo was uncreatable. Fixed by pre-creating `F:\repo\target\wp4-install-gate` with a Modify grant for the ephemeral SID after the repo lockdown.

## Verification

- Capability surface as the restricted account is green: git, node, bash, cygpath, whoami, icacls, and the gate-path bash spawn all exit 0.
- Temporary diagnostics (capability preflight battery, DLL bisector, crash-event step) were removed after the environment was proven; `RunCapabilities` remains only as the on-failure diagnostic.
- Source needles, ordered assertions, forbidden-API checks, C# compilation, and step parse are all replayed locally before every push.

## Deviations from Plan

None against the plan's success contract. The workflow retains the plan-required invariants: exact WP-3 CLI invocation, non-elevated live provision dogfood, no direct gate-script substitution, protected verifier identity, and complete cleanup.

## Stop boundary

Autonomous execution stops after WP-4. WP-0.1 is owner-host-run only. WP-5, WP-6, and Phase 8 do not exist and were not started.
