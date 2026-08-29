# Phase 2 Multi-Source Coverage Audit

Requirement IDs are mapped exactly once at the ROADMAP phase level (Phase 2). PLAN frontmatter repeats applicable IDs because Ferrox requires every executable plan to declare nonempty requirement coverage. Plan 02-01 is the primary closure owner for REQ-ACT-01; Plan 02-03 is the primary closure owner for REQ-POL-01. Other occurrences are cross-cutting regression/wiring coverage, not new roadmap mappings.

| Source | ID | Feature / constraint | Plan(s) | Status | Notes |
|---|---|---|---|---|---|
| GOAL | — | Nano accepts only trusted-issuer assertion and independently narrows before activation | 01,03-10 | COVERED | Contract→authority→gate→adapters→exact E2E. |
| REQ | REQ-ACT-01 | Descriptor, shared Nano gate, both Desktop stacks, direct CLI, exact artifact | 01 primary; 05,08,09,15,10 regressions | COVERED | Roadmap maps only Phase 2. |
| REQ | REQ-POL-01 | Trust/grants/intersection/receipts/quarantine/CLI issuer | 03 primary; 04-08,10 regressions | COVERED | Roadmap maps only Phase 2. |
| CONTEXT | D2-01 | Raw bytes before lossy parse/effects | 01,05,10 | COVERED | Raw vector and process ordering oracles. |
| CONTEXT | D2-02 | Fixed carrier/schema; both stacks one gate | 01,05,09,15,10 | COVERED | No ACP fork/new method. |
| CONTEXT | D2-03 | CLI same gate, enrolled explicit main | 05,10 | COVERED | Compatibility remains nonpersistent. |
| CONTEXT | D2-04 | Sole trusted constructor; conforming JCS/Ed25519 | 01,03,04 | COVERED | Existing NFC helpers forbidden. |
| CONTEXT | D2-05 | principal=agent bytes; opaque subject; local project grant | 01,03,04,08 | COVERED | Immutable bind/tombstone tests. |
| CONTEXT | D2-06 | Own/project scope; no Phase3 T2 recall | 04,06,10 | COVERED | memory_recall typed-disabled. |
| CONTEXT | D2-07 | Default-off legacy memory/T2/cron quarantine | 06,10 | COVERED | No migration/deletion/op reinterpretation. |
| CONTEXT | D2-08 | Journal-first replay/effect/crash/reconciliation | 03,04,10 | COVERED | Full §7/admin crash matrix. |
| CONTEXT | D2-09 | Canonical offline receipts | 01,04,05,10 | COVERED | Separate signer and rotation evidence. |
| CONTEXT | D2-10 | Nano-first, separate Desktop worktree, exact triple | 07-10 | COVERED | Merge/order checkpoints explicit. |
| CONTEXT | D2-11 | Product IDs are not authority; explicit binding | 08,09 | COVERED | Negative source-field tests. |
| CONTEXT | D2-12 | No product registry/scheduler/UI/provider/Phase3/graph | all | COVERED | Scope scan in final verifier. |
| CONTEXT | D2-13 | Exact post-PR9 Nano and owner-authorized Desktop worktrees/bases | 11; all guarded | COVERED | Path/branch/base/repo/clean receipts precede writes. |
| CONTEXT | D2-14 | Dedicated Desktop owner-provisioned binding store; no inferred product fields | 08,09,15 | COVERED | Atomic owner-only file and tombstones. |
| CONTEXT | D2-15 | Exact Nano key-reference OS checks and strict Desktop OS safeStorage | 03,08,10 | COVERED | Real process ACL/reparse/backend negatives. |
| CONTEXT | D2-16 | Protocol-host same gate, nonce uniqueness, signed controls | 01,03-05,09,10 | COVERED | Raw reader authenticates controls before flags. |
| CONTEXT | D2-17 | Journaled exact-artifact bounded enablement; no env/config toggle | 04,13,05,10,12 | COVERED | Tests explicitly enable temp homes. |
| CONTEXT | D2-18 | Build-derived Nano identity and Desktop pre-spawn hash/file identity | 13,08-10,15 | COVERED | PATH/TOCTOU/symlink/reparse negatives. |
| CONTEXT | D2-19 | Phase1 compensated governance and Desktop lane/quality workflow | 11,07,08,10,12 | COVERED | Exact disclosure/review/merge/ancestry receipts. |
| RESEARCH | — | Fixed identifier/scalar/carrier/vector bounds | 01 | COVERED | Schemas and raw fixtures. |
| RESEARCH | — | Package legitimacy checkpoint for canonicalize | 02,08 | COVERED | Blocking human approval before exact install. |
| RESEARCH | — | Admin root/enroll/grant/rotation/revocation/recovery | 03,05,10 | COVERED | Local non-model, journal-first. |
| RESEARCH | — | Closed capabilities/budget minima/resume fingerprint | 01,04 | COVERED | No generalized policy language. |
| RESEARCH | — | Exact replay/control/unknown-outcome/offline receipt | 04,10 | COVERED | Crash and race matrices. |
| RESEARCH | — | Actual RealToolExecutor/MCP/task effect state transitions | 13,14,10 | COVERED | External dispatch/spawn oracles and unknown-outcome stop. |
| RESEARCH | — | Pre-session ACP/CLI ordering and typed errors | 05 | COVERED | External no-side-effect canaries. |
| RESEARCH | — | Full legacy quarantine inventory/replay neutrality | 06,10 | COVERED | ACP/protocol-host/exec/memory/T2/cron/hooks. |
| RESEARCH | — | One Desktop signer/custody/binding | 08 | COVERED | main process only. |
| RESEARCH | — | Legacy/new Desktop projections and fallback | 09,15 | COVERED | Disjoint ownership; each captures final raw frames. |
| RESEARCH | — | Exact artifact, complete negative/crash matrix, both CI | 07,10 | COVERED | Fresh reviewed heads and promotion default-off. |
| RESEARCH | resolved | Exact binding source/storage/ownership | 08 | COVERED | `userData/wayland-nano/activation-bindings.json`; missing disables. |
| RESEARCH | resolved | Nano admin/receipt key providers and platform behavior | 03 | COVERED | Owner-only files, TTY/owner/ACL/no-follow rules. |
| RESEARCH | resolved | Compatibility journal behavior | 05 | COVERED | Protocol-host admitted before journal; old exec in-memory only. |
| VALIDATION | — | Nyquist preflight harnesses, mutation/crash/security/full gates | 01-15 | COVERED | `02-VALIDATION.md` is executable acceptance map. |
| RESEARCH | out of scope | T2 runtime wiring/MEM-SEC/continuity default measurement | — | EXCLUDED | Phase 3. |
| RESEARCH | out of scope | Scheduler state migration/Desktop triggers | — | EXCLUDED | Phase 4. |
| RESEARCH | out of scope | Product registry/UI/providers/graph/extraction/x-project | — | EXCLUDED | Later/never Nano scope. |

## Reachability and Dependency Audit

- Worktree receipts from Plan 02-11 gate every source/dependency task; contract artifacts are consumed by the Nano crate, Desktop producer and final exact-artifact runner.
- Authority records are consumed only by the sole admission constructor; adapters cannot fabricate its trusted token.
- Nano admission precedes Plan 02-13's exact-artifact enablement and actual effect wrapper; ACP/CLI integration then precedes legacy quarantine.
- Nano must merge and yield source+lock identity before the Desktop dependency, producer or adapters execute.
- Desktop producer precedes disjoint legacy Plan02-09 and new-stack Plan02-15 ownership; both precede exact binary E2E.
- Final completion is unreachable until Nano and Desktop exact-head compensated review/CI/merge, post-merge ancestry/hash checks and fresh exact-artifact run pass, then a distinct ferrox-verifier writes PASS in Plan 02-12.

No source item is missing. No Phase 3 or product-control-plane work is planned.
