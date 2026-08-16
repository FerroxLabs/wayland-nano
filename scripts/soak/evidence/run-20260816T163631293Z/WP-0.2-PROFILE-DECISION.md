---
profile_state: classified
selected_arm: neither
owner_confirmation: pending
---

# WP-0.2 900-second profile decision

## Pre-registered rule

A suspect is eligible only if it explains at least 60% of positive accounted retained growth and leads the next suspect by at least 10 percentage points. Fold eligibility is limited to `fold_seen`, `fold_covered`, and `fold_call_names`; tool eligibility requires measured retained tool accounting. Sufficiently measured but non-dominant growth selects `neither`.

## Run validity

- Run: `scripts/soak/evidence/run-20260816T163631293Z`
- Manifest: completed CI profile, 901,636 ms, 1,441 turns, 3 PID segments and 2 intentional kill/resume boundaries.
- Harness exit: 1 because B1 and scaled B5 failed; the completed manifest and evidence were still written. This is a measured profile result, not a wrapper abort.
- Reporter: 57 exact-schema rows, monotonic 25-turn cadence from turn 25 through 1,425.
- Oracle: 15 PWS samples, 5 in each PID segment; all 15 align to reporter samples within 65 seconds.
- Wrapper audit: pre PIDs `[]`, post PIDs `[]`, node PID 86144, no timeout/error, cleanup `clean`.
- Reporter move: 17,229 bytes, SHA-256 `E78A60002A5896F37FB7B11857F95053FEDB0C13FC7EC782C14A79AE22AF7801`; source/destination size and hash matched.
- Exact-value canary: PASS over 12 exact files, 269,655 bytes, zero hits. The receipt contains only the key fingerprint, never its value.

## Retained-structure measurements

| Field | Samples | Baseline | Final | Delta | Fitted bytes/turn |
|---|---:|---:|---:|---:|---:|
| fold_messages | 57 | 38,362 | 2,220,595 | 2,182,233 | 1,579.729 |
| fold_assistant | 57 | 0 | 0 | 0 | 0 |
| fold_call_names | 57 | 1,689 | 47,206 | 45,517 | 27.183 |
| fold_seen | 57 | 13,497 | 820,597 | 807,100 | 563.780 |
| fold_covered | 57 | 0 | 0 | 0 | 0 |
| fold_uncompacted_image_manifests | 57 | 0 | 0 | 0 | 0 |
| fold_todos | 57 | 0 | 0 | 0 | 0 |
| prefix_cache | 57 | 0 | 0 | 0 | 0 |
| context_override | 57 | 0 | 0 | 0 | 0 |
| sessions_map | 57 | 1 | 1 | 0 | 0 |
| mcp_registry | 57 | 112 | 112 | 0 | 0 |
| pws_bytes | 57 | 5,767,168 | 45,826,048 | 40,058,880 | 20,318.072 |

Positive accounted retained growth excluding PWS/cardinality was 3,034,850 bytes. Eligible fold auxiliaries contributed 852,617 bytes (28.094%); measured tool registry growth contributed 0 bytes (0%). `fold_messages`, which is not an eligible correction arm, contributed the remaining dominant growth.

## PID-segment correlation

| PID | Reporter rows | Oracle rows/aligned | Turn range | Reporter/external PWS correlation | External PWS slope bytes/ms |
|---:|---:|---:|---:|---:|---:|
| 11832 | 18 | 5/5 | 25-450 | 0.982573 | 107.273 |
| 51352 | 20 | 5/5 | 475-950 | 0.999744 | 133.217 |
| 65140 | 19 | 5/5 | 975-1425 | 0.979863 | 155.612 |

The independent oracle and reporter PWS series agree directionally in every PID segment. No retained tool growth was measured, and eligible fold auxiliaries are below the signed 60% threshold.

## Proposed arm

`selected_arm: neither`

This is measured-neither, pending the blocking owner confirmation checkpoint. No correction or one-hour receipt is authorized before confirmation.
