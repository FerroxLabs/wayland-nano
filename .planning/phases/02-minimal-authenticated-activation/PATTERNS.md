# Phase 2: Minimal Authenticated Activation - Pattern Map

**Mapped:** 2026-08-29  
**Scope:** REQ-ACT-01 and REQ-POL-01 only  
**Primary authority:** signed `WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md` §§3-13  
**Context note:** no Phase 2 `CONTEXT.md` or `RESEARCH.md` existed when mapping began. File candidates below are derived from the active roadmap, requirements, signed amendment, and inspected Nano/Desktop code. The planner must reconcile them with later research rather than treating every candidate as mandatory.

## Architectural conclusion

Do not put admission logic independently in each transport. The common Nano gate must be a small library boundary invoked before `session/new`, `session/load`, direct CLI bootstrap, journal creation, memory, tools, or effects. Desktop owns assertion minting and product metadata. Nano owns JCS/Ed25519 verification, immutable enrollment/grants, replay/idempotency state, authority intersection, durable intent, and signed receipts.

The legacy Desktop `AcpConnection` and newer `AcpRuntime`/`AcpSession` stacks do **not** currently share a session-request builder. Both must use one Desktop assertion-builder/signer, and both must place its result at `_meta.waylandNanoActivation`. Nano must then converge both requests at one admission function below the ACP dispatcher. Other ACP backends remain unchanged.

## File Classification

Paths marked **candidate new** are planner-level assignments, not pre-authorized abstractions.

| New/Modified File | Repository | Role | Data Flow | Closest Analog | Match |
|---|---|---|---|---|---|
| `crates/nano-activation/src/types.rs` **candidate new** | Nano | model | request-response | `crates/nano-protocol/src/acp.rs` | role match |
| `crates/nano-activation/src/canonical.rs` **candidate new** | Nano | utility | transform | no conforming analog | none |
| `crates/nano-activation/src/crypto.rs` **candidate new** | Nano | service | transform | no Ed25519 application analog | none |
| `crates/nano-activation/src/store.rs` **candidate new** | Nano | store | file I/O / CRUD | `nano-memory/src/store.rs`, `nano-session/src/writer.rs` | partial |
| `crates/nano-activation/src/admission.rs` **candidate new** | Nano | service/middleware | request-response | `nano-core/src/policy_engine.rs`, `nano-agent/src/bootstrap.rs` | role match |
| `crates/nano-activation/src/receipt.rs` **candidate new** | Nano | model/service | event-driven | `nano-verify/src/receipt.rs`, `nano-memory/src/mediation.rs` | role match |
| `crates/nano-activation/src/admin.rs` **candidate new** | Nano | service | request-response / journal-first CRUD | `nano-session/src/writer.rs`, `nano-agent/src/bootstrap.rs` | partial |
| `crates/nano-activation/tests/*` **candidate new** | Nano | test | batch | `nano-verify/tests/receipt_git.rs`, `nano-session/tests/adversarial_journal.rs` | role match |
| `crates/nano-session/src/op.rs` | Nano | model | event-driven | same file additive vocabulary | exact |
| `crates/nano-session/src/replay.rs` | Nano | reducer | event-driven | same file context-neutral arms | exact |
| `crates/nano-cli/src/acp_mode.rs` | Nano | controller | request-response | existing `session/new`/`session/load` handlers | exact |
| `crates/nano-cli/src/exec_run.rs` | Nano | controller | request-response | existing one-bootstrap path | exact |
| `crates/nano-cli/src/main.rs` plus narrowly scoped admin command module | Nano | route/controller | request-response | existing command modules such as `auth_cmds.rs` | role match |
| `crates/nano-agent/src/memory.rs` | Nano | legacy store/provider | file I/O | its current single memory chokepoint | exact quarantine target |
| `crates/nano-agent/src/cron.rs` | Nano | scheduler/provider | event-driven | its current tool/runner boundaries | exact quarantine target |
| `crates/nano-cli/src/cron_fire.rs` | Nano | controller | event-driven | current host fire executor | exact quarantine target |
| `src/process/agent/acp/waylandNanoActivation.ts` **candidate new** | Desktop | signer/builder | transform | `src/process/agent/wcore/desktopContractV1.ts` | role match |
| `src/process/agent/acp/AcpConnection.ts` | Desktop | controller | request-response | its existing session request construction | exact |
| `src/process/acp/types.ts` | Desktop | model | request-response | existing `AgentConfig` | exact extension point |
| `src/process/acp/session/SessionLifecycle.ts` | Desktop | controller | request-response | its existing create/load convergence | exact |
| `src/process/acp/infra/ProcessAcpClient.ts` | Desktop | adapter | request-response | its existing SDK projection | exact |
| `src/process/acp/runtime/AcpRuntime.ts` | Desktop | service | event-driven | its existing config cloning/publication binding | exact |
| Desktop activation tests under `tests/unit` and `tests/integration/process/acp` | Desktop | test | batch | `acpSessionCapabilities`, `AcpSession.lifecycle`, `hub-install-flow` | exact/role match |
| cross-repo exact-artifact fixture/gate, path chosen by planner | Shared/Nano gate | fixture/test | batch | Nano protocol corpus + Phase 1 exact-artifact evidence | role match |

## Nano Pattern Assignments

### Activation carrier types and strict parsing

**Analog:** `crates/nano-protocol/src/acp.rs:1026-1103`

Use the established serde model + explicit wire rename pattern and keep response construction separate from transport handling:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModel {
    #[serde(rename = "modelId")]
    pub id: String,
    pub name: String,
}

pub fn session_new_result(...) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "modes": session_modes_value(...),
        "models": session_models_value(...)
    })
}
```

Phase 2 types should be closed (`#[serde(deny_unknown_fields)]`) and bounded. Do not parse the signed payload directly from permissive `serde_json::Value` at the transport call site. Preserve encoded string bytes exactly; identity comparisons must not normalize, trim, case-fold, or alias.

### Canonical JSON and hashing

**Partial analog:** `crates/nano-verify/src/registry.rs:53-89`

```rust
pub fn closure_digest(closure: &GateClosure) -> Result<String, VerifyError> {
    let bytes = canonical_json(&serde_json::to_value(closure)?)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
```

The useful convention is deterministic bytes before hashing and rejection of non-integer JSON numbers where the contract requires it. **Do not copy its normalizer:** lines 71 and 78 NFC-normalize strings and keys. The amendment explicitly requires RFC 8785 JCS/I-JSON with no Unicode normalization and rejects duplicate keys/lone surrogates/noncanonical forms. `nano-verify/src/receipt.rs:679-696` has the same incompatibility. A conforming JCS implementation and duplicate-key-aware input path are required; bespoke recursive sorting based on these helpers is not sufficient.

### Cryptography

**No conforming application analog exists.** The workspace lock contains transitive `ring`, but Nano has no inspected Ed25519 assertion/signature API to copy. Phase 2 must choose one reviewed, pinned Ed25519 implementation deliberately and expose only:

- strict public-key/signature decoding;
- fixed `alg == "Ed25519"`;
- exact domain tags from the amendment;
- unpadded base64url and exact byte lengths;
- verification over JCS payload with `signature` omitted;
- separate activation issuer, administrator root, and receipt-signer key roles.

Never accept an embedded public key or algorithm selection from untrusted payload as authority.

### Shared admission boundary

**Analogs:** `crates/nano-agent/src/bootstrap.rs:53-125`, `crates/nano-core/src/policy_engine.rs:117-145`

```rust
pub fn session_guard_registry() -> &'static SessionGuardRegistry {
    REGISTRY.get_or_init(SessionGuardRegistry::default)
}

pub fn resolve_access_with_cwd(&self, path: &Path, cwd: &Path) -> FileSystemAccessMode {
    ...
    .unwrap_or(FileSystemAccessMode::Deny)
}
```

Copy the single process-wide chokepoint and deny-by-default result shape. Admission returns a trusted type that cannot be constructed by transport callers. Effective authority is intersection-only: asserted request ∩ enrolled project grant ∩ Nano ceilings. Refusal must occur before session bootstrap/journal genesis, memory, tool construction, or any external effect.

### ACP insertion point

**Analog:** `crates/nano-cli/src/acp_mode.rs:1759-1838` and `2047-2107`

Current `session/new` reads `cwd`, creates a session id, acquires ownership, opens the journal, and appends `SessionBegin`. Current `session/load` validates the id and opens existing state. Insert the shared gate after strict request extraction but **before** line 1778 (`new_session_id`) and before any existing-session access at line 2092. Both paths must receive the same admitted trusted activation.

```rust
let params = params.unwrap_or_default();
// Phase 2: strict carrier extraction + shared admission belongs here.
let session_id = new_session_id();
let journal = config.sessions_dir.join(format!("{session_id}.jsonl"));
```

The `session/cancel` pattern at `acp_mode.rs:1723-1733` is useful for immediate control propagation, but Phase 2 controls must also authenticate/bind the caller and journal typed outcomes; a bare session id/cancel flag is not sufficient.

### Direct CLI insertion point

**Analog:** `crates/nano-cli/src/exec_run.rs:59-127`

The exec path already declares one honest bootstrap and establishes session ownership before later work. Resolve/mint the separately enrolled local CLI assertion and call the same admission service before `resolve_seed`/`bootstrap_session`. Do not add an environment bypass or an implicit trusted `main`; `main` is an explicit enrolled subject/principal under the same verifier and replay rules.

`protocol-host` is also persistent today (`host_mode.rs:88-104`, fixed `protocol-host.jsonl`) and exposes filesystem memory (`host_mode.rs:201-242`). It must be covered by the negative quarantine gate; it must not become an accidental third compatibility bypass.

### Journal vocabulary, idempotency, and replay

**Analog:** `crates/nano-session/src/op.rs:1-34`, `writer.rs:12-77`, `replay.rs:287-290`

```rust
pub struct OpEnvelope { pub v: u32, pub id: String, pub ts: String, pub op: Op }

if !self.seen_ids.insert(envelope.id.clone()) {
    return Ok(false);
}
...
self.file.write_all(&line)?;
self.file.sync_data()?;
```

Use additive op variants with schema version 1, bounded numbers/ids/digests, and context-neutral replay where appropriate. Envelope-id dedupe is an analog, not the complete Phase 2 rule: admission idempotency is keyed by `(issuer_id, principal_id, project_id, idempotency_key)` and changed immutable content must conflict. Intent must be flushed before projection, dispatch, or acknowledgment; exact retries return the stored receipt without redispatch.

**Receipt analog:** existing `MemoryWriteReceipt` at `op.rs:1764-1769` and context-neutral replay at `replay.rs:639-644`. Follow the additive journal family, but activation receipts require the full frozen signed schema, not the three-field memory receipt.

### Receipt construction and validation

**Analogs:** `crates/nano-memory/src/mediation.rs:13-79`, `crates/nano-verify/src/receipt.rs:60-90`

```rust
pub struct MemoryReceipt {
    pub write_id: String,
    pub agent_id: String,
    pub message: String,
}

pub fn canonical_receipt(receipt: &Receipt) -> Result<Vec<u8>, VerifyError> {
    validate_receipt(receipt)?;
    ...
}
```

Keep proposal/decision separate from receipt minting, validate before serialization, and provide offline verification. Unlike `nano-verify`, the activation receipt is cryptographically signed by a separately enrolled local key and must bind assertion hash, effective authority, lifecycle state, journal positions, epochs, build identity, and all correlation identifiers. Never journal secrets or private-key material.

### Legacy memory quarantine

**Exact target:** `crates/nano-agent/src/memory.rs`

Current authority is unscoped filesystem storage at `<nano_home>/memory`, with model-visible `memory_list/read/save/delete` (`memory.rs:63-79`, `359-416`) and opt-in writes controlled by `NANO_MEMORY_WRITE` (`host_mode.rs:201-242`). Phase 2 does not migrate it to T2 (Phase 3); it must make every unauthenticated activation unable to read or write it. Test forced invocation as well as advertised-tool absence, copying the double-gate test style at `nano-agent/src/wiring.rs:2252-2335`.

### Nano cron/routine quarantine

**Exact targets:** `crates/nano-agent/src/cron.rs`, `crates/nano-cli/src/cron_fire.rs`, `crates/nano-cli/src/exec_run.rs`

Current model-visible scheduler surface is `cronjob_tool_definition` and `CronjobExecutor` (`cron.rs:880-1100`); current autonomous firing is `CronRunner::tick` (`cron.rs:543-878`) via `HostCronFire` (`cron_fire.rs:1-123`). Exec currently wraps tools with `CronjobExecutor` and advertises `cronjob` (`exec_run.rs:266-357`) even though its exec approval gate denies actions. Phase 2 quarantine must prove:

- no tool advertisement and forced invocation denial on unauthenticated paths;
- no tick/fire entrypoint can run durable work;
- existing cached/journaled jobs do not auto-fire or auto-migrate;
- legacy `CronCreated`/`CronDeleted`/`CronFired` remain replay-readable.

Do not delete the op vocabulary or build the Desktop-triggered replacement (Phase 4).

## Desktop Pattern Assignments

### One assertion builder/signer

**Candidate file:** `src/process/agent/acp/waylandNanoActivation.ts`

Desktop should have one backend-specific builder called by both stacks. It consumes authoritative Desktop product/config state and OS-credential key references, freezes the exact payload, JCS-canonicalizes it, signs the domain-separated bytes, and returns only `_meta.waylandNanoActivation`. It must not implement Nano policy decisions or derive `principal_id` from display names/paths.

Use `AcpRuntime.ts:72-147` as the immutability/publication analog: shallow-clone caller config, bind a fresh correlation identity (`randomUUID` at lines 125-135), then pass the derived config onward without mutating the source.

### Legacy `AcpConnection` stack

**Exact insertion:** `src/process/agent/acp/AcpConnection.ts:800-831` and `892-903`

Both `newSession` and `loadSession` construct request bodies directly. Add the same Nano-only `_meta.waylandNanoActivation` result to both. Preserve Claude `_meta` by merging namespaces rather than replacing it.

```typescript
const response = await this.sendRequest('session/new', {
  cwd: normalizedCwd,
  mcpServers: options?.mcpServers ?? [],
  ...(meta && { _meta: meta }),
});
```

The builder must run for the exact activation attempt, not once at process initialization, because nonce/times/activation/idempotency and resume fingerprint are attempt-bound.

### New `AcpRuntime` / `AcpSession` stack

**Authoritative flow:**

1. `src/process/acp/runtime/AcpRuntime.ts:72-162` clones `AgentConfig`, enriches it, constructs `AcpSession`, then starts it.
2. `src/process/acp/session/AcpSession.ts:97-139` owns `SessionLifecycle`.
3. `src/process/acp/session/SessionLifecycle.ts:133-143` calls `createSession`; lines 384-410 call `loadSession` or create fallback.
4. `src/process/acp/infra/ProcessAcpClient.ts:166-184` is the final SDK projection.

Extend `AgentConfig` (`src/process/acp/types.ts:21-71`) with a Nano-only authority input/reference or a callback capable of minting a fresh attempt. Do not duplicate cryptographic assembly in `SessionLifecycle` and `ProcessAcpClient`. The shared builder output should be injected into both create/load requests at the narrowest point that still has attempt/resume context; tests must cover create, successful load, and load-failure→fresh-create with distinct replay-safe assertions.

`AcpRuntime.ts:149-160` currently disables ACP persistence because `agent_id` semantics are wrong. Phase 2 must not re-enable that repository path; runtime memory wiring belongs to Phase 3.

### Desktop authority narrowing

**Analog:** `src/process/team/sandbox/capabilityCheck.ts:45-73`

```typescript
export function isCapGranted(team: TTeam, cap: TeamCapability): boolean {
  if (!team.importedFrom) return true;
  const grants = team.importCapabilityGrants ?? {};
  return grants[cap]?.by_user === true;
}
```

Use the single-source-of-truth/explicit-throw convention for Desktop-side preparation, but do not mistake it for Nano authorization. Desktop's requested capabilities are assertions; Nano intersects them independently with enrolled local grants and ceilings.

### Product identity source

Desktop product metadata is assembled before runtime in `src/common/utils/buildAgentConversationParams.ts:79-115` and converted to new ACP `AgentConfig` in `src/process/acp/compat/typeBridge.ts:20-90`. These are identity-input analogs, not trusted Nano mappings. The planner must locate the immutable stored opaque product subject (or add one under Desktop ownership) and prohibit using mutable assistant name, cwd, persona text, backend, or conversation id as `product_subject_id`/`principal_id`.

## Test and Fixture Assignments

### Nano tests

| Concern | Closest test pattern | Required extension |
|---|---|---|
| Strict carrier/schema/canonical vectors | `nano-protocol/tests/contracts.rs`; protocol corpus adversarial fixtures | JCS official vectors plus duplicate keys, invalid Unicode/numbers, unknown fields, padding, algorithm/key substitution |
| Signature and offline receipt verification | `nano-verify/tests/receipt_git.rs` | Ed25519 known-answer vectors, domain separation, receipt/key rotation/revocation |
| Journal/idempotency/crash recovery | `nano-session/tests/adversarial_journal.rs`; `nano-memory/tests/durability.rs`; `corrective_regressions.rs` | every amendment §7 boundary, exact replay/no redispatch, immutable-content conflict, `unknown_outcome` reconciliation |
| ACP exact artifact | `nano-cli/tests/acp_slice.rs`, `acp_record.rs`, `acp_live.rs` | positive and negative `_meta.waylandNanoActivation` through real `session/new/load` |
| CLI parity | `nano-cli/tests/c11_exec_process.rs` and in-process `exec_run` generics | explicit enrolled `main`, same gate/refusals/receipts, no environment bypass |
| Memory quarantine | `nano-cli/tests/c5_memory.rs`; `wiring.rs` forced-unadvertised tests | no advertise, forced call denied, no filesystem/T2 read/write before admission |
| Cron quarantine | `nano-agent/src/cron_tests.rs`, `nano-cli/tests/c11_exec_process.rs` | no model surface, no tick/fire, existing job inert, old ops replay |

### Desktop tests

| Stack/concern | Closest test | Required extension |
|---|---|---|
| Legacy request shape | `tests/unit/acpSessionCapabilities.test.ts`, `acpMcpInjection.test.ts` | exact Nano-only `_meta` on new/load; preserve other `_meta`; other backends unchanged |
| New lifecycle create/load/fallback | `tests/integration/process/acp/session/AcpSession.lifecycle.test.ts` | fresh assertion per attempt and fallback; identical builder used |
| SDK projection | `tests/unit/process/acp/infra/ProcessAcpClientSetModel.test.ts` plus new focused test | `_meta` survives create/load projection byte-for-byte |
| Spawn/handshake exact artifact | `tests/integration/hub-install-flow.test.ts`, `acp-smoke.test.ts` | build pinned Nano commit/lock, record executable digest, exercise both stacks |
| Capability non-widening | `tests/unit/process/team/sandbox/capabilityCheck.test.ts` | requested capability cannot widen Nano effective grant |

### Cross-repo exact-artifact fixture

Follow the Phase 1 immutable-evidence discipline and Nano protocol corpus layout: committed positive/negative request/receipt vectors, manifest with counts and SHA-256, and a gate that fails when subject matter is absent. It must record the Nano triple `(source commit SHA, Cargo.lock SHA-256, executable SHA-256)` and run both Desktop stacks plus direct CLI against that executable. A mock-only Desktop unit test cannot prove this criterion.

## Shared Patterns

### Fail closed before durable or external effects

- Validate structure → canonical bytes → signature/key/issuer → time/replay → immutable binding/project grant → revocation → local ceiling intersection.
- Only then append and flush intent.
- Only then project/dispatch/acknowledge.
- Typed refusal creates no activation or memory content.

This ordering must be frozen in tests because moving session bootstrap, journal genesis, memory context, tool construction, or effect dispatch before admission creates a bypass.

### Error handling

Use typed Nano errors at the ACP boundary as in `acp_mode.rs:1760-1817`: map a bounded error kind and safe message into `JsonRpcResponse::err_typed`; do not leak assertion bytes, keys, signatures, enrollment data, or secret paths. Preserve machine-readable reason vocabulary in signed receipts.

### Atomic writes and durable authority

Use journal-first, sync-bounded semantics from `nano-session::JournalWriter`, then atomic projection. Enrollment/grant/rotation/revocation/recovery operations must bind before/after digests and operation ids. Never make a mutable config file the authority or acknowledge before flush.

### Secret handling

Private administrator, issuer, and receipt-signing keys stay in OS credential custody or owner-only key-reference channels. Do not read `.secrets`, introduce raw-key environment variables, log signing inputs containing authority metadata unnecessarily, or commit live keys to fixtures.

## No Analog Found / Explicit Non-Patterns

| Need | Finding | Planner consequence |
|---|---|---|
| RFC 8785 JCS without Unicode normalization and duplicate-key rejection | Existing Nano canonicalizers NFC-normalize | Select/constrain a conforming implementation; do not reuse `nano-verify` normalizers |
| Ed25519 activation/admin/receipt signing | No Nano application analog | Pin reviewed implementation and known-answer tests; no ad-hoc crypto |
| Administrator-root bootstrap/recovery ceremony | No conforming analog | Plan as a bounded non-model attached-TTY admin subdeliverable, not ACP/tool/config |
| Shared Desktop assertion signer across both ACP stacks | No current shared builder | Create exactly one backend-specific builder; both stacks consume it |
| Durable activation intent/outbox/reconciliation ledger | Session journal has pieces but no complete effect state machine | Add the minimal Phase 2 ledger; do not claim envelope dedupe alone solves dispatch ambiguity |
| `pause` control in current ACP surface | Current direct surface has cancel and goal-specific pause/resume | Freeze Phase 2 authenticated control vocabulary and ownership; do not conflate goal control with activation control |

## Cross-Repository Ownership and Merge Ordering

### Pinned worktrees and runtime ownership

- Nano writes resolve only under `D:/Development/waylandnano/wayland-nano/.tmp-wt-phase2`; Desktop writes resolve only under `D:/Development/waylandnano/desktop/.tmp-wt-phase2`. Plan 02-11 freezes exact bases before implementation.
- Desktop legacy producer injection includes `src/process/agent/acp/index.ts`; new-stack producer inputs include `src/process/acp/compat/typeBridge.ts`. Final process creation is owned in `src/process/agent/acp/acpConnectors.ts` and `src/process/acp/compat/AcpAgentV2.ts`, where the exact Nano executable manifest is checked.
- Nano effect state transitions wrap the actual `RealToolExecutor::dispatch` seam in `crates/nano-agent/src/wiring.rs` and session/MCP/task executor compositions; intent/dispatch/result/unknown-outcome cannot stop at an adapter-only ledger.
- Desktop activation ciphertext is accepted only when the existing global wrapper returns OS-backed `enc:v1:` and reports an approved backend. The activation module rejects the wrapper's FILE_CIPHER/legacy formats locally; it does not weaken or refactor global credential behavior.
- Desktop contribution workflow is part of the execution pattern: `WL_LANE=desktop`, `wl queue`, claim an open `area:desktop-ui` issue, then architecture/testing/oss-pr rules and `prek run --from-ref origin/main --to-ref HEAD` before PR.

### Ownership

- **Desktop:** opaque product subject, bot/persona/team/backend/model/schedule/approval UI, issuer private-key custody, assertion minting, requested authority, both ACP carrier integrations.
- **Nano:** administrator root/public trust, issuer enrollment/binding/grants/revocations, strict carrier verification, admission and intersection, durable intent/replay/idempotency, controls, signed receipts, quarantine enforcement.
- **Shared fixture:** immutable cross-repo request/receipt vectors and exact-artifact evidence; it is not runtime authority.

### Required merge sequence

1. Nano lands first through the protected workflow: contract/types, JCS/Ed25519 verification, admin enrollment, admission, ledger/receipts, CLI parity, ACP consumer, negative/quarantine tests. Persistent activation remains default-off.
2. Record immutable Nano source commit and `Cargo.lock` SHA-256.
3. Desktop separately builds exactly that pair, records executable SHA-256, and integrates the single signer/builder into both ACP stacks.
4. Desktop exact-artifact tests run both stacks against the triple; Nano/CLI offline verification consumes the same fixtures.
5. Cross-repo negative/crash/enrollment/quarantine matrix and both CI systems pass at exact reviewed heads.
6. Enablement remains default-off until promotion evidence is accepted. Rollback returns to quarantine, never unauthenticated persistence.

Do not merge Desktop first against a branch name or `latest`, do not develop incompatible producer/consumer schemas in parallel without frozen vectors, and do not enable persistence merely because unit tests are green.

## Planner Warnings

1. The signed amendment is larger than a transport-field change. It mandates admin bootstrap, issuer lifecycle, project grants, receipt signer, replay/idempotency ledger, crash reconciliation, quarantine, and exact-artifact evidence. Plans that omit these cannot satisfy Phase 2.
2. Keep subdeliverables bounded: (a) canonical schema/crypto vectors, (b) admin/enrollment store, (c) admission+ledger+receipts, (d) Nano ACP/CLI/quarantine, (e) Desktop dual-stack producer, (f) exact-artifact promotion.
3. Do not move Phase 3 work forward: no T2 runtime memory wiring, recall strategy evaluation, filesystem-memory migration, or ACP session repository enablement.
4. Do not build a product registry, scheduler, generalized policy language, persona/module system, or backend selector.
5. If attached-TTY admin ceremony, durable effect reconciliation, or dual-stack carrier cannot fit as bounded verified subdeliverables, trigger the roadmap tripwire and replan instead of weakening them.

## Metadata

**Nano search scope:** `crates/nano-{protocol,cli,agent,session,memory,verify,core}` plus contracts/tests  
**Desktop search scope:** `src/process/{agent/acp,acp,team,services/cron}`, `src/common`, and related unit/integration tests  
**Strong analogs inspected:** 22 files  
**Exact/role-match assignments:** 24 / 29 candidate touched/new surfaces  
**No-conforming-analog areas:** JCS, Ed25519 application API, admin ceremony, shared Desktop signer, full effect reconciliation ledger
