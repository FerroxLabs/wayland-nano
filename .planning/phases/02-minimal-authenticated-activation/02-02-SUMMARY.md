---
phase: 02-minimal-authenticated-activation
plan: 02
subsystem: supply-chain
tags: [npm, canonicalize, rfc-8785, provenance, bun]
requires:
  - phase: 02-minimal-authenticated-activation
    provides: exact activation vectors and authorized implementation worktrees from Plans 02-01 and 02-11
provides:
  - exact canonicalize 2.1.0 artifact approval bound to registry integrity and tarball SHA-256
  - independent Windows PowerShell 5 provenance and Node/Bun RFC 8785 vector verifier
  - exact Desktop dependency and lock entry ready for activation producer implementation
affects: [02-08]
tech-stack:
  added: [canonicalize 2.1.0]
  patterns: [exact-artifact-receipt, independent-redownload, ascii-only-cross-shell-vectors]
key-files:
  created:
    - .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.json
    - .planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.schema.json
  modified:
    - scripts/phase2/Test-CanonicalizePackageReview.ps1
    - D:/Development/waylandnano/desktop/.tmp-wt-phase2/package.json
    - D:/Development/waylandnano/desktop/.tmp-wt-phase2/bun.lock
key-decisions:
  - "Approved only canonicalize 2.1.0 with the receipt-bound npm integrity and tarball SHA-256; no alternate or floating version is permitted."
  - "Recorded owner-directed agent operation honestly as same-human-controller and not independent human review."
patterns-established:
  - "BOM-less PowerShell 5 verifiers embed non-ASCII JavaScript fixtures only through ASCII Unicode escapes."
  - "A package entering the signing boundary is independently redownloaded and hash/source/vector checked before lockfile consumption."
requirements-completed: [REQ-ACT-01]
coverage:
  - id: D1
    description: The exact canonicalize 2.1.0 artifact is provenance-bound and independently reproducible.
    requirement: REQ-ACT-01
    verification:
      - kind: integration
        ref: "powershell -NoProfile -File scripts/phase2/Test-CanonicalizePackageReview.ps1 ... -RequireOwnerDecision"
        status: pass
    human_judgment: false
  - id: D2
    description: Desktop resolves only canonicalize 2.1.0 and both Node 24 and Bun reproduce RFC 8785 ordering.
    requirement: REQ-ACT-01
    verification:
      - kind: integration
        ref: "bun install --frozen-lockfile --ignore-scripts plus stdin Node/Bun RFC vector runners"
        status: pass
      - kind: other
        ref: "bun run typecheck"
        status: pass
    human_judgment: false
duration: 31min
completed: 2026-08-30
status: complete
---

# Phase 2 Plan 02: Exact Desktop JCS Artifact Summary

**Exact `canonicalize@2.1.0` provenance, source correspondence, runtime vectors, and Desktop lock consumption are frozen without any activation runtime integration.**

## Performance

- **Duration:** 31 min continuation
- **Started:** 2026-08-30T00:00:00+07:00
- **Completed:** 2026-08-30
- **Tasks:** 2
- **Files modified:** 7 across planning and Desktop worktrees

## Accomplishments

- Closed the owner-approved artifact receipt against exact npm integrity `sha512-F705...rXJHQ==` and tarball SHA-256 `65b2af82...5d015e`.
- Re-downloaded the tarball independently and confirmed the annotated `v2.1.0` tag peels to commit `7fed74ed8addd9f2fe4b2ea4c1c7caf7b793ead2`.
- Made the verifier byte-ASCII so Windows PowerShell 5 preserves all RFC ordering inputs, including the emoji surrogate pair.
- Added only the exact `canonicalize` dependency and corresponding Bun lock entry to the authorized Desktop worktree; no runtime source was changed.

## Task Commits

1. **Task 1: Inspect exact package provenance** - `ef431f3`, `fd2809a` (receipt, schema, verifier, handoff corrections)
2. **Task 2: Approve and consume exact artifact** - owner-directed approval is frozen in the receipt; Desktop dependency changes await parent commit because the executor guard prohibits committing from the authorized `feat/nano-activation-boundary` branch.

Planning metadata is intentionally left for the parent orchestrator to commit on the existing planning branch.

## Files Created/Modified

- `.planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.json` - closed artifact and owner-decision receipt.
- `.planning/phases/02-minimal-authenticated-activation/evidence/canonicalize-package-review.schema.json` - closed receipt schema.
- `scripts/phase2/Test-CanonicalizePackageReview.ps1` - independent redownload/source/runtime verifier compatible with Windows PowerShell 5.
- `desktop/.tmp-wt-phase2/package.json` - exact `canonicalize: 2.1.0` dependency.
- `desktop/.tmp-wt-phase2/bun.lock` - exact version and approved integrity lock entry.

## Decisions Made

- Accepted only the exact reviewed package. The package has no runtime dependencies or lifecycle hooks, exposes CommonJS `lib/canonicalize.js`, and passes the shared RFC/activation vector corpus under Node and Bun.
- Kept the governance disclosure explicit: this was owner-directed agent operation under one human controller, not an independent human review.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed host-encoding dependence from the RFC vector**
- **Found during:** Task 1 verifier continuation
- **Issue:** Windows PowerShell 5 decoded literal non-ASCII characters in a BOM-less script through the legacy code page.
- **Fix:** Expressed every non-ASCII JavaScript key through `\u` escapes, including `\ud83d\ude00`.
- **Files modified:** `scripts/phase2/Test-CanonicalizePackageReview.ps1`
- **Verification:** Exact mandated Windows PowerShell 5 verifier passed on fresh strike 1 with zero non-ASCII script bytes.

**2. [Owner-directed scope sequencing] Consumed the approved dependency immediately**
- **Found during:** continuation assignment
- **Issue:** The original Plan 02-02 stopped before install and reserved consumption for Plan 02-08 Task 1.
- **Fix:** Parent explicitly authorized completing that mechanical Plan 02-08 input step now. Only `package.json` and `bun.lock` changed; no runtime integration began.
- **Verification:** frozen lock, exact lock integrity, Node/Bun vector, package formatting, and TypeScript gates passed.

**Total deviations:** 2 (one correctness fix, one owner-directed sequencing change)
**Impact on plan:** No feature or runtime scope was added; Plan 02-08 now starts with its immutable dependency input already present.

## Issues Encountered

- The first focused command-line vector invocation was malformed by PowerShell quoting. Piping the same ASCII-only script through stdin isolated the shell variable and passed under both Node and Bun.
- Repository-wide `bun run test` is not green at this Desktop base: 566 suites fail, predominantly because platform services are not registered in the existing test harness. This package-only diff cannot cause that global setup failure; typecheck, frozen-lock, format, and focused dependency/vector gates pass. No unrelated harness code was changed.

## User Setup Required

None.

## Known Stubs

None.

## Next Phase Readiness

- Plan 02-08 may implement the sole Desktop assertion producer using the exact locked JCS dependency.
- No Desktop runtime integration, ACP seam, UI, scheduler, memory, provider, or Phase 3 work has started here.

## Self-Check: PASSED

- Receipt, schema, verifier, and summary exist.
- Exact Windows PowerShell 5 verifier passed with `-RequireOwnerDecision`.
- Independent tarball SHA-256, npm integrity, annotated tag object, and peeled commit all match the receipt.
- Desktop diff contains only `package.json` and `bun.lock`; the pin and lock integrity are exact.
- Node 24, Bun 1.3.11, frozen lock, package formatting, and Desktop typecheck gates passed.

---
*Phase: 02-minimal-authenticated-activation*
*Completed: 2026-08-30*
