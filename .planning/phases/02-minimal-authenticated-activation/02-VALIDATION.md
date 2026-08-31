# Phase 2 Nyquist Validation Strategy

## Principle

Every trust claim has a failing-before-implementation gate, an external oracle, and a final exact-artifact repetition. Source grep, mocks, runtime self-report and a green compiler are never sufficient. Each behavior below names its first owning plan and final closure in Plan 02-10.

## Waves 0–2 Preflight, Contracts and Harnesses

| Contract / harness | First owner | Required failing evidence before production code | Final oracle |
|---|---:|---|---|
| Worktree/base/branch isolation | 11 | Receipt verifier rejects primary path, dirty base, wrong branch/repo/SHA or pre-PR9 base | Git common-dir/worktree list + remote ancestry |
| Whole JSON-RPC raw scanner | 01 | Outer duplicate method/id/_meta, carrier/nested duplicate, escaped alias/decoy, trailing bytes, invalid UTF-8/I-JSON, depth/count/size/preallocation vectors fail before Value parse | Nano-home/hook/effect inventory unchanged |
| Activation/control/admin/receipt schemas | 01 | Missing/unknown/noncanonical/unsafe fields and every frozen enum/bound fail | Manifest counts/hashes plus Rust/Desktop consumers |
| JCS/Ed25519 cross-language bytes | 01/02 | RFC and independent vectors disagree before implementations exist | Exact raw/canonical/signature/public-key/hash bytes |
| Authority/key-provider harness | 03 | TTY/owner/ACL/symlink/reparse/file-type/back-end negatives fail | Real child processes + OS metadata, no private output |
| Replay/effect/control/enablement harness | 04 | Concurrent/crash/retention and state-machine boundaries fail | Dropped DB rebuild + external dispatch oracle |
| ACP/CLI/protocol-host pre-session harness | 05 | Refusal currently creates/reads journal or flips control | Before/after files, hook/fire/tool canaries |
| Desktop legacy raw-frame wire spy | 09 | Legacy send path lacks resolved binding and exact activation/control carrier | Captured final legacy JSON-RPC bytes |
| Desktop new-stack SDK raw-frame wire spy | 15 | Typed/SDK hops can strip or mutate create/load/control metadata | Captured final new-stack JSON-RPC bytes |
| Artifact launcher harness | 08/09 | PATH/symlink/reparse/replacement/stale hash currently reaches spawn | Spawn oracle remains zero; file identity receipt |

## Requirement Test Matrix

| Requirement behavior | Focused command | Cross-repo closure |
|---|---|---|
| Raw schemas/JCS/crypto/scanner | `cargo test -p nano-activation --test contract_vectors` | Dedicated Desktop CI fresh exact-SHA build + `waylandNanoExactArtifact.test.ts` |
| Admin/bootstrap/key custody/ACL/rebuild | `cargo test -p nano-activation --test admin_lifecycle --test admin_crash_rebuild -- --test-threads=1` | Public-schema ephemeral fixture and separate real interactive production bootstrap process matrix on governing CI |
| Admission/grants/nonce/replay/effect/control/enablement | `cargo test -p nano-activation --test admission_matrix --test replay_crash --test enablement -- --test-threads=1` | Dual-stack/CLI/control/crash receipt manifest |
| ACP raw ordering and CLI/protocol-host parity | `cargo test -p nano-cli --test activation_admission --test activation_cli -- --test-threads=1` | Final raw wire, terminal refusal/no fallback and zero-side-effect canaries |
| Real tool/effect dispatcher state | `cargo test -p nano-agent activation_effect -- --test-threads=1` | External effect count and unknown-outcome reconciliation |
| Legacy FS/T2/cron/hook quarantine | `cargo test -p nano-cli --test activation_quarantine -- --test-threads=1` | Seeded state hashes unchanged |
| Desktop signer/binding/key store/launcher | `bun run test:vitest -- tests/unit/process/agent/activation` | Fresh exact-SHA Nano build hash/identity before both spawn paths |
| Legacy/new ACP create/load/control/fallback | `bun run test:vitest -- tests/unit/process/agent/acp/waylandNanoActivationCarrier.test.ts tests/integration/process/acp/session/waylandNanoActivationLifecycle.test.ts` | Both final raw frames byte-match vectors; every Nano refusal is terminal and neither stack falls back |

## Security Mutation Matrix

- Raw frame: duplicates at outer `jsonrpc`, `id`, `method`, `params`, `_meta`; activation/control carrier and every nested authority object; escaped spellings and lookalike/decoy carrier names; concatenated/trailing JSON; overlong/deep/wide arrays/objects; hostile declared sizes and allocation counters.
- Crypto/trust: algorithm/key/domain/signature substitution; padding/length; unknown/revoked/not-yet-valid/expired key/issuer; root/receipt/local-issuer role confusion; signer unavailable.
- Identity/policy: unknown/retired/reused/remapped/case/Unicode subject or principal; principal/agent mismatch; unauthorized project; every capability widening; unmapped tool; zero/overflow budgets; expiry/deadline skew.
- Replay: same nonce across different activation/idempotency fields; same idempotency with changed immutable bytes; concurrent processes; kill at every journal/projection boundary; retention tombstones survive configured maximum authority window and reject reuse.
- Controls: missing/changed signed control carrier, wrong activation/session/issuer/principal/project, replay, revoked epoch, both Desktop stacks, cancel/pause vs dispatch/complete ordering.
- Enablement: absent/expired/revoked/artifact or epoch drift; crash before/after enable intent/decision; rollback binary; stale manifest; no env/config backdoor.
- Artifact: PATH substitution, relative path, symlink/reparse, mutate between hash/spawn, replace/rename, stale executable, source/lock mismatch, dirty build, receipt self-report mismatch.
- Custody: Nano project/env key, symlink/reparse/hardlink where detectable, wrong owner/mode/DACL/parent, nonregular/network path, detached TTY/wrong OS owner; Desktop FILE_CIPHER/legacy/basic_text/unavailable backend.
- Fixture/ceremony: malformed or privileged private fixture state rejected; public-schema ephemeral authority setup succeeds only via supported commands; production interactive bootstrap requires the corrected artifact, real TTY, owner and custody checks.
- Refusal/fallback: every typed Nano rejection across both Desktop stacks and direct CLI has zero child/session/effect side effects; no legacy, alternate binary, unsigned or receipt-driven Desktop retry path exists. Desktop need not verify Nano receipts.

## Wave Gates

1. Wave 0 ends after exact worktree receipts; Wave 1 freezes raw/schema/vector gates; Wave 2 closes the package decision while Nano authority work may proceed independently from the frozen vectors.
2. Each Nano plan runs focused tests plus one process negative; Plan 07 runs `just gate-all`, exact seven CI and compensated governance. The Nano input to Desktop must include the merged corrective interactive-bootstrap change and refreshed immutable source/Cargo.lock identity.
3. Desktop plans begin only from the merged Nano source/lock receipt. Each uses `WL_LANE=desktop`, claimed issue, `bun run test:vitest -- <paths>`, typecheck/lint/format. Before PR: full repository quality sequence, coverage and `prek run --from-ref origin/main --to-ref HEAD`.
4. Plan 10 depends on both Plan 09 and Plan 15. Its authorized Desktop workflow performs a fresh exact-SHA Nano checkout/build/hash because no prebuilt artifact exists, runs both stacks/CLI/protocol-host and the complete matrix, and prepares a non-self-referential committed premerge manifest. Desktop repository-native squash governance is verified by reviewed-head tree/diff/patch identity to the squash merge, not ancestry; the final postmerge receipt lives in external planning evidence.
5. A separately spawned `ferrox-verifier` alone authors `02-VERIFICATION.md` from fresh merged Nano/Desktop checkouts and the external final receipt; only PASS updates ROADMAP/STATE.

## Desktop File Ownership Conflict Audit

| Plan | Exclusive write surface | Overlap result |
|---:|---|---|
| 08 | `src/process/agent/activation/**`, activation unit tests, conditional package/lock | No overlap with adapter plans. Atomic warning accepted: binding, key custody, canonical producer and binary verifier share one private module contract and are split into two bounded tasks. |
| 09 | `src/common/config/storage.ts`, `src/process/task/{workerTaskManagerSingleton,AcpAgentManager}.ts`, legacy `src/process/agent/acp/{index,AcpConnection,acpConnectors}.ts`, legacy test | Seven files; no Plan15 file appears. |
| 15 | new `src/process/acp/{types,compat,runtime,session,infra}` chain and new-stack test | Nine files split into disjoint 4-file and 5-file tasks; no Plan09 file appears. |
| 10 | exact-artifact integration, authorized workflow, strict verifier and committed Desktop evidence | No adapter source overlap; consumes both summaries. Final postmerge receipt is external planning evidence. |

Plan 01 atomic warning is accepted because four closed schemas, three vector inventories and the single crate/parser test shell share one manifest/hash authority; splitting would create competing Wave-1 contract owners. Plan 13 atomic warning is accepted because compiled identity, journaled enablement, core effect wrapper and its operator contract form the single default-off runtime transition and are already separated into three bounded tasks; MCP/task extensions moved to Plan 14.

Sampling: every Plan09 legacy edit runs the legacy raw-frame suite; every Plan15 task runs the new-stack lifecycle suite filtered to its binding/launcher or serialization/fallback rows, followed by the full lifecycle suite before summary. Plan10 reruns both unfiltered against the freshly built exact executable.

## Three-Strikes and Handoff

Attempts are counted per identical failing test, CI leg, platform/provider, vector or crash boundary. Before attempt 2 record one root-cause sentence. Before attempt 3 produce an isolating reproduction varying one factor. After strike 3 stop; `.continue-here.md` records exact worktree path/branch/base/head, dirty diff, command, test/seed/vector/crash point, CI/PR IDs, hypotheses/proof, next command and prohibited retries. No extra diagnostic machinery is added after strike 3.

## Completion Bar

All focused/full local gates, Nano seven-leg CI including the corrective interactive-bootstrap artifact, Desktop governing CI including the dedicated fresh Nano checkout/build/hash job, public fixture plus production ceremony, terminal no-fallback refusals, exact compensated-control evidence, immutable Nano-first/Desktop-second squash merge identity, exact artifact triple, committed non-self-referential manifests/operator runbook, external final receipt and independent Phase 2 verification PASS are required. Default-off remains authoritative after completion; no Phase 3 memory wiring is enabled.
