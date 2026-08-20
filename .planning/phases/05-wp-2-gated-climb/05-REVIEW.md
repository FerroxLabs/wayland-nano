# WP-2 Bounded Critical/High Review

## Identity

- Phase base: `7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0`
- Reviewed implementation: `bb6b304c3f2a317d8a32316fc4863396ca815aa8`
- Reviewed tree: `69ae9d0ad38da38528b53b10780df48c61e46a6a`
- Canonical binary-diff SHA-256: `e2739235cff49cc4b9655e4d40c6051265b61335a9120399620bb15f62b60a2d`
- Single consolidated fix commit: `96b36e339dd0189052f7369c28ab6a984f8ce2af`
- Rechecked tree: `2c9ec4f05104da33053f2af5f30921a749533e18`
- Fix rounds: **1**

## Scope and method

Exactly one bounded audit covered all eight WP-2 research threat groups, test weakening, mutation survivors, WP-1 regression, portability, dependency drift, provenance, and ownership. The independent recheck examined the exact consolidated-fix bytes and ran focused regressions. No dependency, lockfile, forbidden-surface, provenance, or additional Critical/High issue remained.

## Findings and closure

| ID | Severity | Original defect | Closure evidence | Status |
|---|---|---|---|---|
| H-ART-01 | High | Gate-time artifact replacement could preserve a trusted result. | Pre/post exact readback now fails closed and clears exit/log evidence; mutation exact test passes. | Resolved |
| H-MAN-02 | High | Manifest reads were pathname-based and vulnerable to component/leaf substitution. | Unix uses retained `openat` descriptors; Windows uses `NtCreateFile` relative to retained parent handles; both rewalk the chain, reopen/recheck the leaf, repeat Add absence, reject links, and bind sorted facts. Platform-focused tests and clippy pass. | Resolved |
| H-CAN-03 | High | Cancellation was not observed while a gate subprocess ran. | Engine and gate poll cooperatively at bounded intervals and terminate/reap the process tree. | Resolved |
| H-BUD-04 | High | Early ensemble exit lost charges for already-started generation calls. | Every provider attempt is charged before awaiting; the cancellation regression preserves `rounds_used`. | Resolved |
| H-INV-05 | High | Duplicate or malformed inventories could be treated as trusted gate input. | Every raw, baseline, execution, and climb entry rejects empty, malformed, or duplicate inventories before effects/spawn. | Resolved |
| H-TST-06 | High | Nested Cargo contract checks could hang without bound. | Redundant nesting was removed; remaining downstream processes have a 180-second kill/wait bound and 1 MiB diagnostic cap. | Resolved |

## Independent recheck verdict

**APPROVE.** Zero Critical findings, zero unresolved High findings, and no new Critical/High findings at `96b36e339dd0189052f7369c28ab6a984f8ce2af`.

The builder has not merged, pushed, run detached integration, claimed CI, or self-promoted.
