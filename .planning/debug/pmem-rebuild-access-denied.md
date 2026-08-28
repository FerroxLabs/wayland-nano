---
status: resolved
trigger: "Windows P-MEM atomic rebuild returns AccessDenied in three successful-rebuild corrective regressions"
created: 2026-08-28
updated: 2026-08-28
---

# Debug Session: P-MEM Rebuild Access Denied

## Symptoms

- Expected: journal rebuild creates a sibling DB, syncs it, and atomically replaces the destination while holding the rebuild lock.
- Actual: three successful-rebuild tests fail with `Journal(Os { code: 5, kind: PermissionDenied, message: "Access is denied." })`; four other corrective regressions pass.
- Reproduction: `cargo test -p nano-memory --test corrective_regressions recovery_does_not_apply_receipt_from_a_different_agent -- --exact --nocapture` with `CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano`.
- Timeline: introduced by the uncommitted H4/H5/H6/H8/H9 corrective lane on branch `feat/p-mem-1-core`, committed base `d7df52c671e335b84e430bc9f5dd98f04e6c79b4`.
- Eliminated: test parallelism; stale nano-session shared-target artifacts; explicit synced sibling DB `File` handle lifetime before `MoveFileExW`.

## Current Focus

hypothesis: Proven: `File::open` created a read-only handle and Windows `sync_all()` uses `FlushFileBuffers`, which rejects that handle with `ERROR_ACCESS_DENIED` before atomic replacement.
test: Stage-specific I/O context isolated the error to sibling DB sync; reopening with `OpenOptions` read/write made the exact regression and full corrective suite pass.
expecting: Resolved.
next_action: Parent may continue the broader P-MEM corrective verification lane; do not revisit atomic replacement for this error.
reasoning_checkpoint: The smallest lifecycle fix preserves atomic replacement and fail-closed cleanup without weakening tests.

## Evidence

- timestamp: 2026-08-28
  result: focused suite compiled; 4 passed, 3 AccessDenied failures.
- timestamp: 2026-08-28
  result: exact single test reproduced, ruling out parallel interference.
- timestamp: 2026-08-28
  result: explicit sibling sync File drop before MoveFileExW did not change failure.
- timestamp: 2026-08-28T23:08:58+07:00
  result: stage-specific context proved `sync_all` returned Windows code 5 on the read-only `File::open` handle; atomic replacement was never reached.
- timestamp: 2026-08-28T23:08:58+07:00
  result: exact regression passed after opening the sibling DB read/write for sync (1 passed, 0 failed).
- timestamp: 2026-08-28T23:08:58+07:00
  result: full corrective regression suite passed (7 passed, 0 failed); cargo fmt check and git diff check passed.

## Eliminated

- hypothesis: Shared-target stale nano-session metadata is the AccessDenied cause.
  evidence: cleaning nano-session fixed compilation but runtime AccessDenied remained.
- hypothesis: Corrective tests race one another.
  evidence: exact single test reproduces.
- hypothesis: The explicit sync File handle remains open across MoveFileExW.
  evidence: explicit drop added; failure unchanged.

## Resolution

root_cause:
  Windows `File::sync_all` delegates to `FlushFileBuffers`, but rebuild reopened the sibling database with read-only `File::open`; Windows therefore returned `ERROR_ACCESS_DENIED` before `MoveFileExW`.
fix:
  Reopen the rebuilt sibling database through `OpenOptions` with read and write access before `sync_all`, retaining stage-specific I/O context.
verification:
  Exact regression 1/1 passed; `corrective_regressions` 7/7 passed; `cargo fmt --all -- --check` and `git diff --check` passed.
files_changed:
  crates/nano-memory/src/store.rs
