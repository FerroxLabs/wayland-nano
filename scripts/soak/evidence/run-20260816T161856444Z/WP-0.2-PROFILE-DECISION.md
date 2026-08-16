---
profile_state: aborted_unclassified
selected_arm: none
---

# WP-0.2 900-second profile decision

## Pre-registered rule

A suspect is eligible only when it explains at least 60% of positive accounted retained growth and leads the next suspect by at least 10 percentage points. Fold eligibility is limited to `fold_seen`, `fold_covered`, and `fold_call_names`; tool eligibility requires measured retained tool accounting. Failed, ambiguous, spread, or uninstrumented growth selects `neither`.

## Run identity and disposition

- Run path: `scripts/soak/evidence/run-20260816T161856444Z`
- Requested command: `node scripts/soak/soak.mjs --mode ci --duration-seconds 900 --binary <resolved target/release/wayland-nano.exe> --evidence-dir <resolved scripts/soak/evidence>`
- Result: failed before the 900-second measurement completed. The foreground execution wrapper terminated after approximately five seconds (`exit 124`) despite the requested long-run setup.
- Discovery: the worktree was clean before execution. The named run directory and the uniquely named temporary reporter were the only new evidence paths.
- Reporter disposition: the temporary reporter was still held by the orphaned exact release host. The host PID was terminated, then the known temporary file was hashed and removed as required for a failed handoff. It contained 0 bytes; SHA-256 `E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855`.
- Partial evidence retained: `soak-journal.ndjson` and `soak-samples.ndjson`.
- Missing by construction: completed soak manifest and `mem-stats.ndjson`.
- Exact-value canary: blocked because the scanner's required repo-local path `.secrets/flux-test-key` is absent in this isolated worktree. The scanner failed before writing a receipt and did not expose a value. Evidence is not promotable.
- Defense-in-depth shape scan: `wp02-credential-shapes-v1`, 3 exact evidence files, 0 hits, PASS. This does not substitute for the blocked exact-value scan.

## Measurement table

No valid reporter rows were produced, so no baseline/final delta, fitted slope, PID-segment correlation, dominance percentage, or restart boundary can be calculated for any schema field. The single partial oracle sample is insufficient for a slope.

| Field | Samples | Baseline | Final | Delta | Slope per turn | Disposition |
|---|---:|---:|---:|---:|---:|---|
| fold_messages | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_assistant | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_call_names | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_seen | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_covered | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_uncompacted_image_manifests | 0 | n/a | n/a | n/a | n/a | unmeasured |
| fold_todos | 0 | n/a | n/a | n/a | n/a | unmeasured |
| prefix_cache | 0 | n/a | n/a | n/a | n/a | unmeasured |
| context_override | 0 | n/a | n/a | n/a | n/a | unmeasured |
| sessions_map | 0 | n/a | n/a | n/a | n/a | unmeasured |
| mcp_registry | 0 | n/a | n/a | n/a | n/a | unmeasured |
| pws_bytes | 0 | n/a | n/a | n/a | n/a | unmeasured |

External oracle: 1 partial `privateWorkingSetBytes` sample, one PID segment, no fitted delta or slope. Restart boundaries: none observed before termination.

## Classification

`profile_state: aborted_unclassified`

This wrapper-aborted attempt selects no arm. It remains exact failed-attempt evidence and cannot authorize fold, tool, or measured-neither. No correction or one-hour receipt may start from it.
