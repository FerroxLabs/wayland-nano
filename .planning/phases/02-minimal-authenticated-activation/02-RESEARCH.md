# Phase 2: Minimal Authenticated Activation - Research

**Researched:** 2026-08-29
**Domain:** Cross-repository authenticated activation admission, durable authorization state, ACP carrier integration, and legacy-persistence quarantine
**Confidence:** HIGH for repository seams and governing requirements; MEDIUM for the proposed new crate/package boundaries until the Phase 2 schema vectors and dependency checkpoints are ratified

## Summary

Phase 2 must add one security boundary, not a second agent product. Desktop remains the product control plane and signs a minimal assertion; Nano verifies it, resolves immutable subject/principal/project enrollment, checks replay and resume drift, narrows or refuses authority, durably records the decision, and only then permits session creation or resume. This ordering is load-bearing: Nano currently creates or opens session journals and runs session lifecycle work inside `session/new`/`session/load` before any authenticated admission, while both filesystem-memory injection and Nano's live cron ticker are enabled in the ACP host. [VERIFIED: `crates/nano-cli/src/acp_mode.rs:1131-1223,1320-1409,1661-1669,1759-1830,2047-2166,3062-3068,3901-3908,3998-4008,5200-5247`]

Both Desktop ACP implementations can carry the assertion without changing ACP itself. The legacy `AcpConnection` assembles `session/new` and `session/load` parameters directly, and the newer `AcpRuntime` path flows through `AgentConfig` → `AcpSession` → `SessionLifecycle` → `ProcessAcpClient`/`AcpProtocol`. ACP SDK 0.18.2 types already permit arbitrary `_meta` on both request types. [VERIFIED: `desktop/src/process/agent/acp/AcpConnection.ts:800-905`; `desktop/src/process/acp/runtime/AcpRuntime.ts:52-152`; `desktop/src/process/acp/session/SessionLifecycle.ts:95-142,384-411`; `desktop/src/process/acp/infra/AcpProtocol.ts:10-88`; `desktop/node_modules/@agentclientprotocol/sdk/dist/schema/types.gen.d.ts:1935-1972,2919-2952`] The shared Desktop implementation should therefore be a pure main-process assertion builder injected into both request paths, while the single authoritative admission gate lives in Nano immediately before any journal, hook, memory, tool, or scheduler work.

The most important implementation hazard is raw JSON loss. Nano's ACP reader currently parses each frame directly into `serde_json::Value`; duplicate keys are already collapsed by the time a later validator sees the carrier. RFC 8785 forbids duplicate properties, requires I-JSON, preserves Unicode without normalization, sorts object keys by UTF-16 code units, and refuses invalid Unicode/non-finite numbers. [VERIFIED: `crates/nano-cli/src/acp_mode.rs:5330-5365`] [CITED: https://www.rfc-editor.org/rfc/rfc8785.html] The admission path must validate the raw frame/carrier before ordinary `Value` parsing can erase ambiguity. Do not “canonicalize then accept” an ambiguous parse.

**Primary recommendation:** Land a small `nano-activation` kernel crate and contract vectors first; make `AdmissionGate::admit(raw_carrier, entrypoint)` the only persistent-activation constructor; then integrate Nano ACP/direct CLI/quarantine, and only after the exact Nano artifact is green wire one shared Desktop assertion service into both ACP stacks.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| REQ-ACT-01 | Route both Desktop ACP stacks through one shared Nano admission gate and accept the minimal signed descriptor. | Exact request seams, SDK `_meta` support, descriptor fields, admission ordering, resume fingerprint, and exact-artifact test architecture are mapped below. |
| REQ-POL-01 | Verify issuer/project grant, intersect Nano ceilings, freeze receipts, quarantine unauthenticated memory/cron, and admit direct CLI only through an enrolled local issuer. | Authority-store design, admin ceremony, replay ledger, capability/budget rules, receipt vocabulary, CLI seam, and quarantine inventory/test matrix are specified below. |
</phase_requirements>

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Product subject selection, project selection, requested continuity, persona/module references | Desktop main process | Desktop persistence | Desktop owns bot/product truth; Nano treats signed product fields as opaque assertions. [VERIFIED: signed amendment §§3-5] |
| Issuer private key custody and assertion signing | Desktop main process | OS credential facility | Desktop already has Electron `safeStorage`; the private issuer key must never be passed to Nano. [VERIFIED: `desktop/src/process/secrets/safeStorage.ts:1-95`; signed amendment §6] |
| ACP carrier transport | Desktop ACP adapters | ACP SDK | `_meta.waylandNanoActivation` is backend-specific and must be attached only for the Nano backend. [VERIFIED: signed amendment §§5,10] |
| Raw carrier validation, signature verification, immutable binding, replay, grant/policy resolution | Nano activation kernel | Nano CLI adapters | Nano is the trust boundary and cannot rely on Desktop's validation. [VERIFIED: signed amendment §§3-7] |
| Administrator root, issuer enrollment, grant/revoke/rotate state | Nano activation kernel | Local non-model CLI | Administration is local, interactive, journal-first, and separate from ACP/model tools. [VERIFIED: signed amendment §6] |
| Admission/control receipts and offline verification | Nano activation kernel | Nano CLI verifier | Nano signs canonical receipts with a separate local key. [VERIFIED: signed amendment §6] |
| Session creation/resume and tool execution | Existing Nano CLI/agent/session crates | `nano-activation` decision | Existing runtime executes only after a positive admission result; it must not reimplement trust checks. |
| Legacy filesystem memory and Nano cron quarantine | Nano CLI/agent integration | Regression gates | Phase 2 disables every unauthenticated persistence/firing path but preserves old journal decoding. [VERIFIED: signed amendment §9] |
| T2 scoped memory wiring | Phase 3, not Phase 2 | — | `nano-memory` is merged but not a runtime seam; Phase 2 must keep it inaccessible, not wire it. [VERIFIED: ROADMAP Phase 3; current source inventory] |

## Project Constraints (from AGENTS.md)

- Work only in explicit worktrees; Nano implementation belongs in `wayland-nano`, Desktop implementation in a separate Desktop worktree, and donor/shared sources remain read-only unless a governed fixture explicitly requires `shared`. Preserve concurrent and unrelated changes.
- Never read or print `.secrets`; private-key tests use generated ephemeral keys, and production keys are referenced only through owner-only/OS credential channels.
- Fail closed. Never weaken sandbox, egress, tool policy, journal, or a test to make Phase 2 green.
- Use the pinned Rust 1.95.0 MSVC toolchain and keep `windows-sys` at 0.52; no second Windows bindings version.
- Nano completion requires `just gate-all` (fmt, workspace clippy with `-D warnings`, workspace tests) and the governing CI matrix.
- Desktop main-process code stays under `src/process`; common wire-only types may live under `src/common`; no renderer/UI work is in scope. Strict TypeScript, `type` over `interface`, aliases, no `any`, and no directory over ten direct children.
- Desktop tests use Vitest 4, cover failure behavior, and require `bun run test`, typecheck, lint/format checks, and project CI. [VERIFIED: `desktop/AGENTS.md`; `desktop/.claude/skills/architecture/SKILL.md`; `desktop/.claude/skills/testing/SKILL.md`]
- No commits or pushes from this research task.

## Governing Decisions and Hard Boundary

The signed `WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md` is the Phase 2 authority. It supersedes Nano product registry/persona/roster/scheduler work; keeps physical `agent_id`; fixes `_meta.waylandNanoActivation`; mandates Ed25519 + RFC 8785 JCS; requires an administrator root, immutable enrollment, replay/idempotency, signed receipts, exact-artifact evidence, default-off persistence, and typed negative/crash cases. [VERIFIED: signed amendment §§2-12]

Phase 2 must not:

- wire T2 memory, build MEM-SEC cards, or choose continuity defaults (Phase 3);
- migrate/remove scheduler state beyond disabling and quarantining it (Phase 4 owns migration/removal and host-triggered execution);
- add a Nano agent/persona/team/module/product registry;
- add Desktop UI or approval surfaces;
- add browser/desktop providers, graph/KG, compaction extraction, procedures, or cross-project reads;
- reinterpret existing `Cron*`, `MemoryWrite*`, or other journal vocabulary as authenticated authority.

## Standard Stack

### Core

| Library/API | Version | Purpose | Why Standard |
|-------------|---------|---------|--------------|
| `serde` / `serde_json` | Existing workspace versions | Strict typed schema and raw-frame parsing seam | Already workspace-standard; use `deny_unknown_fields`, but add raw duplicate-key detection before `Value`. [VERIFIED: workspace manifests] |
| `serde_jcs` | 0.2.0 exact | RFC 8785 canonical bytes in Rust | Purpose-built RFC 8785 serializer; do not reuse Nano's simpler sorted-JSON helpers as JCS. [VERIFIED: crates.io + package legitimacy OK] [CITED: https://docs.rs/serde_jcs/0.2.0/serde_jcs/] |
| `ed25519-dalek` | 3.0.0 exact, default verification features only | Ed25519 signing/verification in Nano | Maintained RustCrypto implementation; Rust 1.85 minimum is below the pinned 1.95 toolchain. [VERIFIED: crates.io + package legitimacy OK] [CITED: https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/] |
| `base64` | Existing 0.22 | Strict unpadded URL-safe signature/key encoding | Already workspace-pinned; decode exactly 64-byte signatures and 32-byte public keys. |
| `sha2` | Existing 0.10 | Assertion/build/fingerprint hashes | Already workspace-standard. |
| `rusqlite` + existing SQLite profile | Existing locked version | Rebuildable authority/replay projection | Already proven cross-platform by `nano-memory`; use a distinct activation DB and journal. |
| Node `crypto` | Node 24 runtime | Desktop Ed25519 key generation/signing and SHA-256 | Built in; avoid a second JavaScript crypto implementation. [VERIFIED: environment probe and Node API availability] |
| Electron `safeStorage` wrapper | Existing Desktop code | Encrypt Desktop issuer private key at rest | Existing OS-backed credential rail; fail closed if encryption/custody is unavailable. [VERIFIED: `desktop/src/process/secrets/safeStorage.ts`] |
| `canonicalize` | Pin 2.1.0 pending human package checkpoint | RFC 8785 canonical bytes in Desktop | RFC 8785 Appendix G names this implementation. Latest 4.0.0 is too new for the package gate, so do not float `^`; inspect/pin the older 2.1.0 tarball or explicitly approve 4.0.0 after review. [CITED: https://www.rfc-editor.org/rfc/rfc8785.html#appendix-G] |
| ACP SDK | Existing 0.18.2 | `_meta` transport on `session/new`/`session/load` | Both request types already include `_meta?: Record<string, unknown>`. [VERIFIED: installed SDK declarations] |

### Supporting Existing Patterns

| Pattern/API | Purpose | Reuse rule |
|-------------|---------|------------|
| `JournalCoordinator`/fsync writer and `FileLock` | Journal-first, crash-safe append and cross-process exclusion | Reuse the durability/locking pattern, not the session-op vocabulary. Authority records need a separate closed schema/journal. |
| `nano-core::execrules` owner-only file/DACL audit | Key-reference and authority-store permission checks | Extract only if a clean shared helper already exists; otherwise duplicate the minimal audited platform call with provenance rather than refactor unrelated rules code. |
| `NanoErrorKind` + `NanoErrorExtras` | Typed ACP refusals | Add a bounded activation family and regenerate the existing error table; never return only free-form strings. |
| Desktop `safeStorage` | Issuer key custody | Add a small activation-key store beside existing secrets code; never place raw keys in `AgentConfig.env`, logs, IPC, SQLite, or fixtures. |

### Alternatives Considered

| Instead of | Could Use | Tradeoff / disposition |
|------------|-----------|------------------------|
| `nano-activation` crate | Put everything in `nano-cli`/`nano-protocol` | Rejected: authority state, crypto, receipts, and admission must be independently testable and shared by ACP + direct CLI; adapters should not own security state. |
| `serde_jcs` | Existing in-tree “canonical JSON” helpers | Rejected: existing helpers generally sort UTF-8/Rust keys or restrict integers and are not demonstrated RFC 8785/ECMAScript-number implementations. |
| Node built-in crypto | `tweetnacl` already in Desktop | Rejected for this phase: built-in Ed25519 avoids another private-key implementation and dependency surface. |
| `canonicalize` package | Hand-written recursive key sort | Rejected: RFC 8785 has Unicode/number edge cases; use the RFC-listed implementation plus cross-language vectors. |
| SQLite as source of truth | Append-only authority journal + rebuildable SQLite projection | Rejected: the signed contract requires journal-first crash/rebuild truth. |

**Installation (only after package checkpoints):**

```bash
# Nano workspace dependencies, pinned exactly by Cargo.lock
cargo add --package nano-activation ed25519-dalek@=3.0.0 serde_jcs@=0.2.0

# Desktop: do not float to the new 4.0.0 release without review
bun add --exact canonicalize@2.1.0
```

## Package Legitimacy Audit

| Package | Registry | Age / activity | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|----------------|-----------|-------------|---------|-------------|
| `ed25519-dalek` 3.0.0 | crates.io | Package first published 2016; current 3.0.0 | ~3.98M/week | `dalek-cryptography/curve25519-dalek` | OK | Approved, exact pin. |
| `serde_jcs` 0.2.0 | crates.io | Published 2020 | ~158k/week | `l1h3r/serde_jcs` | OK | Approved, exact pin; validate against RFC vectors. |
| `canonicalize` | npm | Package created 2018; latest 4.0.0 published 2026-08-12 | ~3.2M/week | `erdtman/canonicalize` | SUS (`too-new` latest) | Planner must add `checkpoint:human-verify`; inspect exact 2.1.0 tarball/repository and pin it, or approve an exact later version explicitly. No caret. |

**Packages removed due to SLOP verdict:** none.

**Packages flagged as suspicious:** `canonicalize` latest. The package itself is named by RFC 8785 Appendix G, but the current release is newly published; provenance does not remove the exact-version review requirement.

## Contract-First Wire Profile

### Identifier and scalar grammar to freeze in Wave 0

The signed amendment requires fixed ASCII grammars/bounds before Phase 2. Freeze these in a versioned JSON Schema plus cross-language vectors before implementation:

| Field family | Grammar / bound | Rationale |
|--------------|-----------------|-----------|
| `principal_id` | `[a-z][a-z0-9-]{0,63}` | Exactly matches existing physical `agent_id` validation. [VERIFIED: `crates/nano-memory/src/types.rs:420-439`] |
| `issuer_id`, `key_id`, `admin_id` | `[a-z][a-z0-9-]{0,63}` | Stable, non-display security identifiers. |
| `project_id`, `product_subject_id` | `[A-Za-z0-9][A-Za-z0-9._:-]{0,127}` | Accommodates Desktop UUID/prefixed IDs without paths, whitespace, normalization, or display names. Desktop projects are UUID-backed. [VERIFIED: `desktop/src/process/services/ProjectServiceImpl.ts:45-72`] |
| `activation_id`, `idempotency_key`, `nonce`, optional `session_id`, admin `operation_id` | `[A-Za-z0-9][A-Za-z0-9_-]{0,127}` | Filesystem/log safe, bounded correlation identifiers. Existing session IDs already accept alphanumeric/`-_` but lack a length cap; Phase 2 must add the cap at the shared validator. [VERIFIED: `crates/nano-agent/src/bootstrap.rs:269-275`] |
| SHA-256 fields | `[0-9a-f]{64}` | One lowercase canonical form. |
| times | RFC 3339 UTC `Z`, whole seconds | Signed amendment rule; parse without timezone aliases or fractional seconds. |
| numeric budgets | positive safe integers `<= 9_007_199_254_740_991` | Avoid cross-language JCS/IEEE-754 ambiguity. |
| carrier size | max 32 KiB UTF-8; depth max 8; arrays individually bounded | Prevent parse/signature DoS before crypto. The exact bounds must be fixture-pinned. |

Do not derive `project_id` from a workspace path and do not derive `product_subject_id` from display names. A Desktop conversation lacking an explicit stable project and enrolled product binding remains non-persistent in Phase 2.

### Activation payload

`_meta.waylandNanoActivation` should be a single object with `signature` beside the signed payload fields. Unknown fields are refused. Recommended v1 shape:

```json
{
  "schema": "wayland.nano.activation/v1",
  "issuer_id": "desktop",
  "key_id": "desktop-2026-01",
  "alg": "Ed25519",
  "issued_at": "2026-08-29T10:00:00Z",
  "not_before": "2026-08-29T09:59:55Z",
  "not_after": "2026-08-29T10:05:00Z",
  "nonce": "n_aabbcc",
  "product_subject_id": "builtin-wayland-nano",
  "principal_id": "main",
  "project_id": "018f-project-id",
  "activation_id": "act_aabbcc",
  "idempotency_key": "idem_aabbcc",
  "session_id": null,
  "continuity": {
    "strategy": "fresh",
    "fallback": "none",
    "resume_fingerprint": null
  },
  "capabilities": ["filesystem.read", "filesystem.write", "shell.execute"],
  "budgets": {
    "max_turns": 64,
    "max_tool_calls": 256,
    "max_input_tokens": 1000000,
    "max_output_tokens": 250000,
    "max_cost_microcents": 100000000,
    "wall_clock_ms": 3600000
  },
  "deadline": "2026-08-29T11:00:00Z",
  "controls": ["cancel", "pause"],
  "signature": "<unpadded-base64url-64-bytes>"
}
```

The signature input is exactly the amendment domain prefix plus RFC 8785 bytes with `signature` omitted. [VERIFIED: signed amendment §5] The raw parser must reject duplicate keys, invalid UTF-8/I-JSON, lone surrogates, non-finite/unsafe numbers, over-depth/over-size payloads, and noncanonical encodings before constructing trusted types. After parsing, re-canonicalization must be byte-identical to the signed canonical payload representation used for verification.

### Continuity behavior in Phase 2

- `fresh`: admitted with no prior session dependency.
- `session_resume`: requires a signed `session_id` and fingerprint that matches the prior admitted session's issuer, subject, principal, project, policy/grant/revocation epochs, Nano artifact, effective tool set, persona reference, and module reference set. Refuse drift before reading the session journal.
- `memory_recall`: schema-valid but typed `continuity_not_enabled` in Phase 2; Phase 3 owns T2 wiring. It must not silently degrade to `fresh`.
- Fallback is explicit only. `fresh` or `memory_recall` fallback requires the signed request to name it; revoked authority never falls back. Phase 2 may implement only `none`/`fresh` while returning a typed feature-disabled refusal for `memory_recall` fallback until Phase 3.

Fingerprint components should be an explicit object of lower-hex digests/epochs, not one opaque caller-provided digest. Nano computes/validates its own `policy_epoch`, `grant_epoch`, `revocation_epoch`, Nano build identity, and effective tool-set digest; Desktop supplies signed persona/module reference digests only because Nano consumes them for equality-on-resume, not as a registry.

### Capability and budget policy

Use a closed capability family enum, not a policy language: `filesystem.read`, `filesystem.write`, `shell.execute`, `network.egress`, `mcp.invoke`, `task.spawn`, `checkpoint.mutate`, and `computer.use`. `memory.*` and `schedule.*` are not admissible in Phase 2. Every advertised/model-visible tool must map to exactly one family; an unmapped tool is denied and fails the coverage test.

Resolution order:

1. Verify schema/canonical form/signature/issuer/key/time.
2. Resolve immutable subject→principal and project grant.
3. Check revocation and exact replay/idempotency.
4. Reject any capability outside the active grant as `authority_widening`.
5. Compute numeric effective budgets/deadline as `min(request, project grant, Nano local ceiling)` and record every narrowing.
6. Intersect requested permission mode/tool availability with the existing Nano policy; Nano's existing sandbox/egress/tool gates remain final runtime authority.

This is not authorization by Desktop: a valid signature authenticates the issuer's assertion, while the Nano-local grant authorizes the project/principal/capability tuple.

## Canonical Receipt and Error Vocabulary

Freeze `wayland.nano.activation-receipt/v1` and its exact JSON vectors before runtime integration. The receipt should contain:

- schema, receipt id, receipt key id, `alg`, Nano signature;
- decision `admitted | admitted_narrowed | refused | replayed`;
- closed `reason` enum;
- issuer/key/subject/principal/project/activation/session/idempotency correlation fields where safely parsed;
- SHA-256 of raw carrier bytes and canonical signed payload bytes;
- effective capabilities/budgets/deadline/controls;
- intent state and result state;
- authority journal position and projection generation;
- issuer/grant/revocation/admin epochs;
- Nano `(source_commit_sha, Cargo.lock_sha256, executable_sha256)`;
- issued/decided times and receipt signer rotation evidence reference.

Minimum refusal reasons: `carrier_missing`, `carrier_oversized`, `malformed_json`, `duplicate_key`, `noncanonical_payload`, `unknown_field`, `unsupported_schema`, `unsupported_algorithm`, `invalid_key_encoding`, `invalid_signature_encoding`, `invalid_signature`, `unknown_issuer`, `revoked_issuer`, `unknown_key`, `revoked_key`, `key_not_yet_valid`, `key_expired`, `assertion_not_yet_valid`, `assertion_expired`, `clock_out_of_bounds`, `nonce_replay`, `idempotency_conflict`, `unknown_product_subject`, `retired_product_subject`, `principal_mismatch`, `principal_remap`, `retired_identifier_reuse`, `unauthorized_project`, `authority_widening`, `artifact_mismatch`, `resume_fingerprint_missing`, `resume_drift`, `fallback_unauthorized`, `continuity_not_enabled`, `control_unauthorized`, `control_race_lost`, `authority_store_unavailable`, and `ambiguous_recovery`.

Parsing/authentication refusals may persist only a bounded audit receipt in the authority journal; they must create no session journal, memory row, tool/hook activity, cron state, or external effect. Exact replay of an admitted assertion returns the previously stored signed receipt without redispatch. Same idempotency tuple with changed immutable content returns `idempotency_conflict`.

## Authority Store and Administrator Ceremony

### Recommended Nano project structure

```text
crates/nano-activation/
├── Cargo.toml
├── src/
│   ├── lib.rs          # trusted public constructors only
│   ├── schema.rs       # strict payload/receipt/admin types and bounds
│   ├── canonical.rs    # raw duplicate/I-JSON checks + serde_jcs
│   ├── crypto.rs       # Ed25519/domain/base64url wrappers
│   ├── authority.rs    # immutable bindings, grants, epochs, revocations
│   ├── journal.rs      # append/fsync/rebuild record vocabulary
│   ├── store.rs        # locked journal + SQLite projection
│   ├── admission.rs    # ordered decision state machine
│   ├── receipt.rs      # canonical signed receipts/offline verify
│   └── tests/          # split before directory exceeds ten children
└── tests/
    ├── contract_vectors.rs
    ├── crash_rebuild.rs
    └── adversarial.rs
```

The exact file split may be tightened to obey the ten-child rule, but keep one crate as the sole trusted constructor. Do not expose public struct fields that let callers fabricate an `AdmittedActivation`; use constructors and private fields.

### Durable state

Use `<nano_home>/activation/authority.jsonl` as truth and `<nano_home>/activation/authority.db` as a rebuildable projection. Hold one cross-process writer lock from validation of replay state through journal flush and projection commit. Records are closed, versioned operations: admin bootstrap/epoch, issuer enroll/key add/key revoke/issuer revoke, immutable subject-principal bind/retire, project grant/revoke, receipt signer add/revoke, admission intent/decision, and control decision. Unknown future records remain reader-visible but cannot authorize.

Crash ordering:

1. validate request against a locked snapshot;
2. append and fsync immutable admin/admission intent;
3. commit projection/outbox transaction;
4. append and fsync decision/receipt;
5. acknowledge on wire.

Rebuild must reproduce bindings, epochs, replay/idempotency outcomes, and receipt verification. Crash tests cover every boundary named in amendment §7 and admin partial writes named in §12.

### Admin commands

Keep administration local and non-model-facing:

```text
wayland-nano admin bootstrap --root-key-file <owner-only path> --receipt-key-file <owner-only path>
wayland-nano admin issuer enroll --request <signed-admin-request.json>
wayland-nano admin issuer rotate --request <signed-admin-request.json>
wayland-nano admin issuer revoke --request <signed-admin-request.json>
wayland-nano admin grant apply|revoke --request <signed-admin-request.json>
wayland-nano admin recovery apply --request <signed-admin-request.json>
wayland-nano activation verify-receipt <receipt.json>
```

Bootstrap requires an attached TTY, current owning OS account, empty authority store, secure Nano-home permissions, and explicit confirmation. Test it through injected `AdminKeyProvider`, `ReceiptKeyProvider`, TTY, clock, and filesystem traits; production accepts only an owner-only key-reference path or OS credential provider. Environment-provided public keys, project files, ACP, hooks, and tools cannot bootstrap.

The Desktop issuer private key belongs in a dedicated main-process key store built on existing `safeStorage`; the encrypted blob may persist in Desktop config/storage but plaintext exists only in process memory for signing. Never forward it in `AgentConfig.env`. The receipt-signing private key belongs to Nano's owner-only/OS credential key-reference channel and is separate from the administrator root and issuer key.

## One Shared Nano Admission Gate

### Required ordering at ACP entry

For both `session/new` and `session/load`:

```text
raw JSON-RPC line
  → bounded raw-frame scan (duplicate keys / I-JSON / carrier extraction)
  → AdmissionGate::admit
      → signature + trust + time
      → immutable binding + project grant + revocation
      → replay/idempotency + resume drift
      → effective authority + durable intent/receipt
  → only if admitted:
      session ownership/journal open or creation
      lifecycle hooks
      MCP/tool registry
      prompt/context
  → signed receipt in response `_meta`
```

The current `reader_loop` must preserve raw request bytes (or perform the strict typed raw parse there) for activation-bearing methods. A later `serde_json::Value` validator is insufficient. The gate must run before `acquire_session_ownership`, `JournalCoordinator::open`, `SessionBegin`, replay frames, `SessionStart` hook, attachment/checkpoint store, MCP registration, memory reads, or cron setup.

Session state must retain the admitted activation identity/effective authority. `session/prompt`, `session/set_*`, cancel, pause/goal controls, forks, child tasks, hooks, and tool dispatch derive only downward authority from it. A session ID alone is never authority.

### Direct CLI

`wayland-nano exec` currently calls `bootstrap_session` before any admission. [VERIFIED: `crates/nano-cli/src/exec_run.rs:59-127`] Move admission ahead of seed resolution/bootstrap. Authenticated direct CLI uses a separately enrolled issuer and explicit `--principal main --project <id>` plus an issuer private-key reference. It constructs the same payload, calls the same `AdmissionGate`, and receives the same signed receipt.

Compatibility behavior must be explicit:

- old `exec` invocation without an enrolled assertion may run only the bounded non-persistent compatibility mode;
- it cannot resume/create persistent session journals, read/write filesystem memory, access T2, create/fire cron, or silently acquire `main` persistence;
- requesting `--resume`, persistence, or authenticated-only continuity without a carrier returns `carrier_missing`;
- no implicit `principal_id = main`; `main` is explicit and must be enrolled/granted.

### Both Desktop stacks

Create one Desktop main-process module, e.g. `src/process/agent/activation/`, that owns:

- issuer key custody/signing;
- strict activation request construction/JCS;
- stable per-logical-activation nonce/idempotency reuse across startup retries;
- product/project/principal binding input validation;
- receipt parsing and offline verification handoff;
- Nano-backend predicate (other ACP backends receive no field).

Then thread an already-built `waylandNanoActivation` object through both paths:

1. Legacy: `AcpAgentManager`/`AcpAgentV2` config → `AcpConnection.newSession`, `resumeSession`, and `loadSession` `_meta`.
2. New: `AgentConfig` → `SessionLifecycle` → `CreateSessionParams`/`LoadSessionParams` → `ProcessAcpClient` and `AcpProtocol` `_meta`.

Do not duplicate signing in `AcpConnection` and `AcpProtocol`. Both adapters should only attach a value produced by the shared service. Extend the local parameter types with the SDK's `_meta` type; do not cast through `any`.

The current new-stack `agentId` is the conversation id, explicitly marked incorrect. [VERIFIED: `desktop/src/process/acp/compat/typeBridge.ts:96-108`; `desktop/src/process/acp/runtime/AcpRuntime.ts:43-49`] Do not reuse it as `principal_id` or `product_subject_id`. Add explicit activation binding fields sourced from stable assistant/project configuration. Conversations without a stable project + enrolled subject/principal binding stay non-persistent. This is not the deferred ACP Discovery persistence work.

## Legacy Quarantine Inventory

| Existing route | Current persistence/execution | Phase 2 action | Regression oracle |
|----------------|-------------------------------|----------------|-------------------|
| `acp-host session/new` | Creates session journal before auth | Require positive admission before ownership/open/`SessionBegin`; otherwise no session file. | Before/after Nano-home inventory and typed refusal. |
| `acp-host session/load` | Opens/replays journal before auth | Require admission + exact bound resume fingerprint before any journal read/replay. | Mutant session id/carrier produces no replay frames and no file touch. |
| ACP filesystem memory | Fresh injection every prompt; read tools always, writes optional | Disable/register nothing unless a later Phase 3 admitted T2 seam enables it. Phase 2 admitted sessions still get no legacy filesystem memory. | Seed `<nano_home>/memory`; canary text never reaches model/frames; tools absent. |
| ACP cron tools | `cronjob` executor/definition included | Remove from admitted and compatibility tool surfaces in Phase 2; preserve code/data for Phase 4 migration. | Tool inventory lacks `cronjob`; direct call typed denied; jobs file unchanged. |
| ACP cron ticker | 30-second `tick_once` when `cron_home` is `Some` | Production Phase 2 passes `None`/removes ticker construction; existing jobs cannot fire. | Due job + external fire oracle remains untouched across host lifetime. |
| `protocol-host` filesystem memory | `MemoryStore` wrapper and prompt injection; capability advertises true | Force compatibility path non-persistent, remove memory wrapper/tools/injection, advertise `memory_enabled=false`. | Seeded canary absent; no writes; capability false. |
| `exec` | Persistent session bootstrap/resume; cron wrapper/definition exists though gate denies actions | Admission before bootstrap. Unauthenticated compatibility uses no durable session/resume and has no cron definition. | Nano-home unchanged; resume/carrier failure typed. |
| T2 `nano-memory` | Library present, not runtime wired | Keep inaccessible; add negative linkage/runtime test, no Phase 3 wiring. | Runtime binary tests show no T2 open/read/write on Phase 2 paths. |
| Legacy `Cron*`/`MemoryWrite*` journal ops | Replay-readable | Leave variants and folds readable/replay-neutral; they confer zero activation authority. | Old journal fixture still parses; cannot create admission/grant/receipt. |
| Lifecycle hooks | Can run at session start | Run only after admission; compatibility mode must not invoke persistent activation hooks. | Hook external canary untouched on refusal. |

Quarantine is default-off behavior, not deletion. Do not migrate jobs or filesystem memory in Phase 2.

## Runtime State Inventory

Phase 2 changes the authority of existing persistence paths, so source grep alone is not enough. The executor must inventory the actual test/operator Nano home before and after quarantine without reading secret contents.

| Category | Items Found | Action Required |
|----------|-------------|-----------------|
| Stored data | `<nano_home>/sessions/*.jsonl`; `<nano_home>/memory/**` legacy filesystem memory; `<nano_home>/memory/memory.db` and memory journals when P-MEM has been exercised; `<nano_home>/cron/jobs.json`; `CronCreated`/`CronDeleted`/`CronFired` and `MemoryWrite*` records in journals. [VERIFIED: current Nano source] | Phase 2 does **not** migrate/delete. Inventory metadata and hashes, disable access/fire, retain replay readability, and prove files/rows are unchanged by unauthenticated paths. Phase 3/4 own migration. |
| Live service config | `NANO_MEMORY_WRITE`, `NANO_MEMORY_BLOCK_CHARS`, `NANO_HOME`, `hooks.toml`, ACP process environment, Desktop saved `acpSessionId`, conversation `extra.projectId`, and future encrypted issuer-key record. [VERIFIED: Nano/Desktop source] | Memory env switches cannot re-enable quarantined persistence. Preserve provider credentials untouched. Add activation setup status without exposing key material. |
| OS-registered state | No Nano OS task/service scheduler registration was found in the repository; Nano cron firing is an in-process 30-second ACP-host ticker. [VERIFIED: `acp_mode.rs:1661-1669,5200-5247`] | Before execution, enumerate live `wayland-nano` processes and OS task/service registrations by name without reading credentials. Stop old hosts before asserting quarantine; record any unexpected registration as a blocker/follow-up, not an auto-delete. |
| Secrets/env vars | Provider keys and `.secrets` remain out of scope. New issuer/admin/receipt private keys require safeStorage or owner-only key-reference paths; values must never appear in commands, logs, fixtures, receipts, or journals. | Tests generate ephemeral keys in temp roots. Production evidence records only key ids/fingerprints and reference-path metadata, never bytes. |
| Build artifacts | Nano binaries are emitted under the external Cargo target root; Desktop may bundle/cache a Nano binary. Exact Phase 2 identity is source SHA + `Cargo.lock` SHA-256 + executable SHA-256. | Rebuild after final Nano merge, reject stale/mismatched binaries, and run Desktop both-stack tests against the recorded executable. Do not treat branch name or PATH resolution as identity. |

## Architecture Patterns

### Pattern 1: Trusted constructor and typestate

```rust
// Conceptual API; exact names are planner discretion.
pub struct AdmittedActivation {
    identity: BoundIdentity,
    effective: EffectiveAuthority,
    receipt: SignedReceipt,
    // private fields
}

impl AdmissionGate {
    pub fn admit_raw(
        &self,
        raw_carrier: &[u8],
        entrypoint: Entrypoint,
        now: SystemTime,
    ) -> Result<AdmittedActivation, RefusalReceipt>;
}
```

Only this constructor can mint `AdmittedActivation`. Runtime APIs that can persist or dispatch accept `&AdmittedActivation`, not strings/booleans.

### Pattern 2: Exact-replay idempotency

Store `assertion_hash` under `(issuer_id, principal_id, project_id, idempotency_key)`. Same hash returns the stored receipt; different hash refuses. The nonce ledger is checked under the same writer lock so two processes cannot both admit.

### Pattern 3: Independent cross-language contract vectors

Commit canonical payload bytes, signature, public key, assertion hash, receipt bytes/signature, and expected refusal vectors. Rust verifies JavaScript-produced vectors; Desktop verifies Rust-produced vectors. At least one set must be generated outside either implementation (RFC vectors + a tiny audited fixture generator) so the implementation does not grade itself.

### Pattern 4: Exact artifact handshake

Nano lands first. Evidence records source commit SHA and `Cargo.lock` SHA-256. Desktop builds that exact Nano checkout, hashes the executable, and runs both ACP stacks against that binary. Branch names and “latest” never count.

### Anti-Patterns to Avoid

- **Validation after `serde_json::Value`:** duplicate keys and some lexical ambiguity are already lost.
- **Desktop-only auth:** signatures checked only in Desktop do not protect Nano from alternate callers.
- **Two admission implementations:** ACP and CLI must call one gate; both Desktop stacks must use one assertion builder.
- **Session id as authority:** a valid/stolen session id never replaces a signed, enrolled resume assertion.
- **Environment public-key trust:** keys from env/project/workspace files cannot enroll or bootstrap.
- **Implicit `main`:** old clients do not silently get persistent identity.
- **Capability clamp that hides widening:** unauthorized capability families refuse typed; budget narrowing is explicit in the receipt.
- **Self-signed receipt with issuer key:** Nano receipt signer is separate.
- **Database-first authority:** projection loss must be rebuildable from journal.
- **Quarantine by renaming files:** prove no runtime read/write/fire; do not merely hide UI or remove tool descriptions.
- **Changing legacy op meaning:** additive authority ops belong in a separate activation journal.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Ed25519 | Curve arithmetic/signature code | `ed25519-dalek` in Rust, Node `crypto` in Desktop | Cryptographic correctness and strict encoding. |
| RFC 8785 | Ad hoc recursive sort/`serde_json::to_vec` | `serde_jcs` and RFC-listed `canonicalize`, plus raw duplicate checks | UTF-16 ordering, ECMAScript numbers, Unicode error cases. |
| Secret storage | Plaintext config/env secret | Desktop `safeStorage`; owner-only/OS key-reference provider in Nano | Prevent logs/config/fixtures from holding keys. |
| Durable concurrency | In-memory nonce map or lock file only | OS file lock + append/fsync journal + SQLite projection | Cross-process replay and crash correctness. |
| ACP extension protocol | New ACP methods for initial activation | Standard `_meta` on `session/new`/`session/load` | Existing SDK support and backend isolation. |
| Product registry | Nano assistant/persona records | Signed opaque Desktop subject + Nano security enrollment | Preserves ownership boundary. |

## Common Pitfalls

### Pitfall 1: Admission occurs after a side effect
**What goes wrong:** A rejected assertion still creates a session file, replays history, invokes a hook, opens memory, or ticks cron.
**How to avoid:** Gate on raw request before all existing bootstrap code; tests inventory Nano-home and external canaries.

### Pitfall 2: Startup retry mints a new assertion
**What goes wrong:** Desktop retry loops generate new nonce/idempotency values and can create duplicate activations.
**How to avoid:** Build once per logical activation and reuse exact bytes until terminal receipt; a changed request uses a new activation/idempotency identity.

### Pitfall 3: Resume fallback bypasses auth
**What goes wrong:** Both Desktop stacks currently fall back from failed load to new session. An auth/drift/revocation refusal could become a fresh unauthenticated session.
**How to avoid:** Classify Nano activation refusals as terminal for the persistent path; fallback only when the signed carrier explicitly permits it and Nano issues the fallback receipt.

### Pitfall 4: Current Desktop identity fields are reused
**What goes wrong:** New-stack `agentId` equals conversation id and legacy fields mix backend/custom assistant identity.
**How to avoid:** Add explicit activation binding inputs; never infer principal from conversation/backend/name/path.

### Pitfall 5: Clock skew extends authority
**What goes wrong:** A skew allowance is added to `not_after` or deadline.
**How to avoid:** Skew may tolerate observation around `issued_at/not_before`; it never extends the signed expiry/deadline. Use injected monotonic/wall clocks in tests.

### Pitfall 6: Receipt signing fails after admission
**What goes wrong:** Runtime side effects occur but no verifiable decision reaches the caller.
**How to avoid:** Ensure receipt signer availability before durable admission; record intent, sign/store decision, then acknowledge/create session. Signer unavailable is a pre-session refusal.

### Pitfall 7: Revocation caches survive
**What goes wrong:** Loaded session or in-memory grant ignores a newly durable revocation.
**How to avoid:** Compare revocation epoch before resume, prompt/effect dispatch, and control operations; stale epoch stops the session.

### Pitfall 8: Desktop `safeStorage` fallback weakens silently
**What goes wrong:** Headless/file fallback stores issuer key under weaker conditions without operator knowledge.
**How to avoid:** Activation issuer custody is stricter than generic credentials: if the approved OS/owner-only key store is unavailable, persistent activation remains disabled and reports a typed setup state.

## State of the Art

| Old approach in current tree | Phase 2 approach | Impact |
|------------------------------|------------------|--------|
| Trust the process that opened ACP and create/load session immediately | Authenticate a backend-specific `_meta` assertion at Nano before session work | Alternate callers and stale Desktop state cannot gain persistence merely by speaking ACP. |
| Global filesystem memory injected by host configuration | Persistent memory default-off pending admitted, scoped T2 integration | Removes the shared-memory bypass without prematurely implementing Phase 3. |
| Session id as sufficient resume locator | Signed identity/project/session binding plus fingerprint/epochs | Resume becomes authorization-sensitive and drift-aware. |
| In-process cron ticker owned by Nano | Quarantined now; Desktop-only trigger ownership later | Prevents dual schedulers while preserving state for Phase 4 migration. |
| Free-form/log-only decision evidence | JCS/Ed25519 offline-verifiable receipts | Admission/refusal/replay claims can be checked independently of the running process. |

**Deprecated for persistent activation:** unauthenticated `main`, filesystem memory injection, Nano cron firing, session-id-only resume, and source-SHA-only artifact identity. These paths may remain only as explicitly bounded non-persistent compatibility or replay-readable history.

## Validation Architecture

### Test Framework

| Property | Nano | Desktop |
|----------|------|---------|
| Framework | Rust built-in tests + integration/process tests | Vitest 4 + process integration harness |
| Quick command | `cargo test -p nano-activation` | `bun run test:vitest -- tests/unit/process/agent/activation` |
| Integration command | `cargo test -p nano-cli --test activation_admission` | `bun run test:vitest -- tests/integration/process/acp/waylandNanoExactArtifact.test.ts` |
| Full command | `just gate-all` | `bun run test && bun run typecheck && bun run lint && bun run format:check` plus repository PR checks |

### Requirement → Test Map

| Req | Behavior | Test type | Automated evidence | Exists? |
|-----|----------|-----------|--------------------|---------|
| ACT-01 | Strict schema/JCS/Ed25519 cross-language vectors | contract | Rust + Vitest consume the same protected vectors | Wave 0 |
| ACT-01 | ACP legacy new/load carries identical shared assertion | unit/integration | Legacy adapter spy + exact Nano binary | Wave 0 |
| ACT-01 | ACP new runtime new/load carries identical shared assertion | unit/integration | `SessionLifecycle`/`AcpProtocol` spy + exact Nano binary | Wave 0 |
| ACT-01 | Admission precedes every session side effect | process/adversarial | Nano-home/hook/tool/fire external canaries | Wave 0 |
| ACT-01 | Resume fingerprint match/drift/explicit fallback | integration | load session matrix | Wave 0 |
| POL-01 | Admin bootstrap/enroll/grant/rotate/revoke/recovery | unit/process/crash | injected key store/TTY + process-kill matrix | Wave 0 |
| POL-01 | Concurrent nonce/idempotency exact replay/conflict | concurrency | multi-process barrier fixture | Wave 0 |
| POL-01 | Capability widening and budget narrowing | table/unit | exhaustive capability mapping and limits | Wave 0 |
| POL-01 | Direct CLI explicit `main` and project uses same gate | process | exec positive/refusal/replay matrix | Wave 0 |
| POL-01 | Filesystem memory/T2/cron/hooks inaccessible without admission | negative process | canary stores + due job + hook sentinel | Wave 0 |
| POL-01 | Old journals remain readable but confer no authority | compatibility | pre-Phase-2 journal corpus | Existing corpus + new negative |
| ACT/POL | Offline receipt verify + key rotation/revocation | contract/process | mutate each field/signature/epoch | Wave 0 |
| ACT/POL | Exact Nano artifact through both Desktop stacks | cross-repo E2E | recorded source/lock/exe hashes and fixture run | Wave 0 |

### Negative Security Matrix

At minimum test: missing carrier; oversized/deep carrier; duplicate keys at every nesting level; unknown fields; invalid UTF-8/lone surrogate/non-finite/unsafe number; noncanonical key order/escaping/number representation; padded/wrong-length base64url; wrong alg/key/signature/domain; unknown/revoked/expired issuer/key; time before/after/skew; nonce replay and concurrent replay; idempotency exact replay/conflict; unknown/retired/reused subject; principal mismatch/remap/case/Unicode/confusable; unauthorized project; every capability widening; budget overflow/zero; artifact mismatch; resume subject/project/session/fingerprint/epoch drift; unauthorized fallback; unauthorized cancel/pause; cancel/complete race; receipt signer unavailable; journal/DB/lock failures; every admin partial-write boundary; legacy memory, T2, hook, and cron bypass.

### Crash Matrix

Exercise process kill or injected hard-stop at:

1. before/during/after authority intent append;
2. after journal flush before DB commit;
3. after DB commit before decision append;
4. during receipt signing/write;
5. after receipt journal before wire acknowledgment;
6. each enrollment/grant/rotation/revocation/recovery journal/projection boundary;
7. concurrent admission while recovery rebuilds.

After each, drop the projection, rebuild from journal, and compare bindings, epochs, nonce/idempotency decisions, and receipts. No test may infer success from self-report alone.

### Sampling Rate

- Per Nano task: affected crate tests + one relevant process negative.
- Per Desktop task: focused Vitest unit/integration + typecheck for touched process types.
- Per wave: Nano `just gate-all`; Desktop full test/type/lint/format checks.
- Phase gate: exact-artifact cross-repo fixture, both CI systems, security audit, then promotion remains default-off until evidence binds the final reviewed heads.

### Wave 0 Gaps

- [ ] Protected activation payload/receipt/admin request JSON Schema and independent vectors.
- [ ] `nano-activation` crate test harness with injected clock/key store/TTY/crash points.
- [ ] Nano raw JSON duplicate/I-JSON parser tests.
- [ ] Nano ACP/direct-CLI process harnesses with external state inventory.
- [ ] Desktop shared assertion builder tests and both-stack carrier spies.
- [ ] Exact-artifact runner recording Nano source, lockfile, and executable hashes.
- [ ] Legacy memory/T2/cron/hook quarantine fixture.
- [ ] Cross-platform receipt/key-reference permission tests.

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Control |
|---------------|---------|---------|
| V2 Authentication | yes | Ed25519 issuer/admin/receipt keys, immutable key ids, explicit lifecycle. |
| V3 Session Management | yes | Signed activation/session binding, freshness, replay/idempotency, resume fingerprint, cancellation ordering. |
| V4 Access Control | yes | Nano-local subject/principal/project grant and closed capability intersection before runtime. |
| V5 Validation | yes | Bounded raw I-JSON/JCS parse, strict schemas, ASCII identifiers, unknown-field refusal. |
| V6 Cryptography | yes | RFC 8032 Ed25519 via maintained libraries, separate key roles, domain separation, unpadded base64url. |
| V7 Error Handling/Logging | yes | Closed refusal vocabulary, signed receipts, no key/raw secret logging. |
| V8 Data Protection | yes | OS/owner-only key custody; public authority state only in Nano config/journal. |
| V9 Communications | limited | ACP stdio carrier integrity is cryptographic; no new network trust channel. |
| V10 Malicious Code | yes | Hooks/tools/scheduler cannot run before admission; exact artifact identity. |
| V11 Business Logic | yes | Never-remap, never-reuse, rotation overlap, revocation precedence, exact replay. |
| V12 Files/Resources | yes | Owner-only authority/key reference paths; no project/env bootstrap key. |

### STRIDE Threat Map

| Threat | STRIDE | Mitigation |
|--------|--------|------------|
| Forged Desktop/CLI assertion | Spoofing | Ed25519 + enrolled issuer/key + domain separation. |
| Duplicate/noncanonical JSON interpretation | Tampering | Raw duplicate/I-JSON validation + RFC 8785 cross-language vectors. |
| Replayed activation/effect | Repudiation/Tampering | Locked nonce/idempotency journal and stored signed receipt. |
| Subject remap or project substitution | Elevation | Immutable binding, retired tombstones, Nano-local project grant. |
| Session resume under changed persona/tools/policy | Elevation | Fingerprint and epoch match before journal read. |
| Legacy memory/cron bypass | Elevation | Default-off quarantine with external canaries. |
| Private key leak | Information disclosure | safeStorage/owner-only key references, redaction canaries, no env/log/fixture keys. |
| Oversized/pathological carrier | Denial of service | byte/depth/count bounds before signature work. |
| Receipt signer failure after effect | Repudiation | signer preflight, journal-first decision before acknowledgment/session. |
| Concurrent admissions | Tampering | cross-process lock around replay check and durable commit. |

## Worktree, Ownership, and Merge Sequence

1. Planning artifacts stay on `plan/persistent-agent-program`; do not mix implementation.
2. Nano implementation uses a new worktree from current `origin/master` on a dedicated Phase 2 branch. Nano owns schema/vectors, `nano-activation`, CLI integration, quarantine, receipts, and Nano CI.
3. Nano is reviewed/merged first. Record final source commit and `Cargo.lock` SHA-256.
4. Desktop implementation uses a separate Desktop worktree from the owner-selected integration base (the repository default is `main`, while current Nano integration work is on `feature/wayland-nano`; the planner must pin the actual base SHA rather than assume). Desktop owns signing/key custody and both ACP adapter paths.
5. Desktop builds/checks out the exact Nano source+lock pair, records executable SHA-256, and runs both stack fixtures.
6. No branch may mutate the other repository; cross-repo evidence is a separately owned fixture/evidence step.

## Risks and Tripwires

| Risk | Early proof | Tripwire / response |
|------|-------------|---------------------|
| Raw ACP parser cannot retain duplicate-key evidence without broad rewrite | Build a minimal raw-line carrier parser test first. | Three failed isolated attempts → stop with handoff; do not accept `Value` validation. |
| SDK or one adapter strips `_meta` | Wire-capture `session/new` and `session/load` from both stacks before business logic. | If exact field cannot traverse without forking ACP, stop/replan carrier; no new generic protocol. |
| Cross-language JCS bytes differ | RFC vectors + Rust↔Node vectors in Wave 0. | Three strikes → stop; never patch custom canonicalization until vectors happen to match. |
| Admin lifecycle balloons into product registry/UI | Keep CLI-only security enrollment and opaque IDs. | Any persona/team/assistant CRUD or renderer work is scope failure. |
| Quarantine breaks ordinary nonpersistent use | Define explicit ephemeral compatibility tests. | Preserve bounded no-persistence operation; never restore implicit `main` persistence. |
| Receipt/durable store cannot land independently | Implement and kill-test before adapters. | If not bounded, stop/replan; do not bury authority in session DB. |
| Existing hook/task/CUA paths bypass effective capability | Exhaustive tool mapping + pre-dispatch activation token. | Unmapped effect is denied; do not add permissive fallback. |
| Windows key/lock behavior diverges | Run Windows-specific permission/concurrency fixture early. | Follow three-strikes with isolated handle/ACL repro; no retries-by-variation. |

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|-------------|-----------|---------|----------|
| Rust | Nano build/tests | yes | 1.95.0 pinned | none |
| Cargo | crates/dependency lock | yes | 1.95.0 | none |
| Bun | Desktop tests/build | yes | 1.3.11 | repository-defined only |
| Node | Desktop crypto/tooling | yes | 24.16.0 | none |
| Git | worktrees/artifact identity | yes | 2.54.0.windows.1 | none |
| GitHub CLI | PR/CI evidence | yes | 2.86.0 | browser/manual only if auth fails |
| ACP SDK | Desktop carrier | yes | 0.18.2 installed | none; no upgrade needed |
| Electron safeStorage | Desktop runtime custody | code present | runtime-dependent | Persistent activation remains disabled; no plaintext fallback |

No missing build dependency blocks planning. Runtime OS credential availability must be tested and must fail closed.

## Assumptions Log

| # | Claim | Risk if wrong | Required resolution |
|---|-------|---------------|---------------------|
| A1 | A dedicated `nano-activation` crate is the smallest clean trusted boundary. | Existing dependency graph could create a cycle. | Planner runs `cargo metadata`/dependency mapping before locking files; keep crate depending only on external libs, not `nano-cli`/`nano-agent`. |
| A2 | `canonicalize@2.1.0` provides the needed TS/runtime shape and exact RFC behavior. | Types/export format or vectors may differ. | Human package checkpoint and vector spike before Desktop implementation; do not upgrade silently. |
| A3 | Non-project Desktop conversations should remain non-persistent in Phase 2. | Product may require a stable standalone scope. | This follows the signed active Nano-local project grant requirement; changing it requires an explicit owner amendment, not path-derived identity. |
| A4 | Phase 2 may refuse `memory_recall` as feature-disabled while accepting the schema. | Roadmap reader could expect recall now. | Phase 3 explicitly owns runtime memory wiring; plan acceptance must assert typed refusal/no silent fallback. |
| A5 | Owner-only key-reference files are acceptable for Nano admin/receipt keys on every platform. | Operator may require OS keychain-only custody. | Signed amendment permits OS credential facility **or** owner-only key-reference; pin exact production provider during planning. |

## Planning Resolutions (Closed)

1. **Desktop JCS package:** `canonicalize` remains conditional on Plan 02-02's deterministic exact tarball/integrity/source/export/vector approval. The approved exact version is installed with no range. Rejection selects the bounded in-repo strict JCS producer already covered by the same Nano/RFC vectors; this decision completes before any Desktop integration and never changes the wire bytes.
2. **Desktop product binding:** no existing assistant/conversation field qualifies. Desktop creates a dedicated owner-provisioned main-process store at `userData/wayland-nano/activation-bindings.json`, atomically written with owner-only permissions and immutable retirement tombstones. It explicitly records opaque product subject, principal, project and issuer/key reference. Desktop owns it; Nano independently authorizes its mirrored enrollment/grant. Missing binding means persistent activation disabled.
3. **Nano key providers:** production uses explicit owner-only key-reference files for admin root, receipt signer and local CLI issuer. Unix and Windows validation is fixed by D2-15, including no symlink/reparse traversal, owner/ACL checks, attached TTY and OS-owner proof. Test providers generate ephemeral keys only. Desktop activation keys require OS-backed Electron safeStorage and explicitly reject the file-cipher/legacy fallback.
4. **Compatibility journal:** `protocol-host` is an authenticated persistent entrypoint using the local CLI issuer and shared gate before journal open. Unauthenticated `exec` compatibility uses process-local in-memory state only, cannot resume and never writes Nano home. No scratch journal ambiguity remains.
5. **Controls and enablement:** control requests use a signed `wayland.nano.control/v1` carrier authenticated in the raw reader. Default-off enablement is a journaled local-admin operation bound to exact artifact, epochs and expiry; no environment/config switch exists.

All former implementation open questions are resolved. A failure of the package checkpoint selects its already-bounded in-repo alternative; other changes require an explicit owner amendment rather than executor choice.

## Sources

### Primary (HIGH confidence)

- `D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md` — signed Phase 2 authority.
- `.planning/ROADMAP.md`, `.planning/REQUIREMENTS.md`, `.planning/phases/01-ownership-contract-and-foundation/01-VERIFICATION.md` — phase scope and completed prerequisites.
- Nano source: `crates/nano-cli/src/acp_mode.rs`, `exec_run.rs`, `host_mode.rs`; `crates/nano-agent/src/bootstrap.rs`, `memory.rs`, `cron.rs`; `crates/nano-session/src/{op,replay,error_kind,writer,lock}.rs`; `crates/nano-memory/src/types.rs`.
- Desktop source: `src/process/agent/acp/AcpConnection.ts`; `src/process/acp/{runtime,session,infra,compat}`; `src/process/secrets/safeStorage.ts`; `src/process/task/AcpAgentManager.ts`.
- Installed ACP SDK 0.18.2 generated request types.

### Primary Standards / Official Documentation (MEDIUM confidence classification from web provider)

- https://www.rfc-editor.org/rfc/rfc8785.html — JCS requirements and Appendix G implementation reference.
- https://www.rfc-editor.org/rfc/rfc8032.html — Ed25519 encoding/signature requirements and vectors.
- https://agentclientprotocol.com/protocol/v1/extensibility — `_meta` extension semantics; installed SDK declarations independently confirm request support.
- https://docs.rs/serde_jcs/0.2.0/serde_jcs/ — Rust JCS API.
- https://docs.rs/ed25519-dalek/3.0.0/ed25519_dalek/ — Rust Ed25519 API.

## Metadata

**Confidence breakdown:**

- Repository seams: HIGH — read from current Nano planning worktree and Desktop source.
- Governing scope: HIGH — signed amendment and completed Phase 1 verification.
- Wire/crypto rules: HIGH content confidence, MEDIUM provider classification — primary RFCs plus installed SDK types.
- Proposed crate/file split: MEDIUM — dependency-cycle and directory-count checks remain a planning Wave 0 task.
- Package choices: HIGH for Rust crates after legitimacy checks; MEDIUM for Desktop `canonicalize` until the required exact-version checkpoint.
- Quarantine inventory: HIGH — every current runtime call site was located; regression oracles are specified.

**Research date:** 2026-08-29
**Valid until:** 2026-09-28 for repository seams; revalidate ACP SDK/Desktop integration base and package versions before execution.
