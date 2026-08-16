# Phase 3: WP-0.3 PDF Intake - Research

**Researched:** 2026-08-17
**Domain:** Fail-closed ACP PDF intake, journaled attachment rehydration, and Anthropic Messages wire encoding
**Confidence:** HIGH
**Baseline:** `feat/wp-03` and `origin/master` both resolve to `d8702f22f76aac7dc2d7fcc77b34e4482557ee12`. [VERIFIED: git rev-parse]

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|---|---|---|
| PDF-01 | Inline and confined-path PDF intake validates `%PDF-` magic, MIME/extension agreement, one-document-per-message, and the 20 MiB cap with typed refusals. | Extend the existing ACP block converter and its handle-verified confined-read pattern; add PDF-specific validation before storage or dispatch. [VERIFIED: `.planning/REQUIREMENTS.md`; codebase grep] |
| PDF-02 | Use additive `DocumentRef` and document block types without changing the image contract. | Add parallel variants through the model, turn, and journal projections; do not rename `ImageRef`. [VERIFIED: `SPEC-WP0-hardening.md` WP-0.3; codebase grep] |
| PDF-03 | Emit the exact Anthropic document source block and refuse OpenAI-completions leaves before network I/O. | Gate after binding resolution, using `binding.wire`, but before driver dispatch; add the exact `message_to_wire` arm. [VERIFIED: codebase grep; recorded Flux media contract] |
| PDF-04 | Kill/resume rehydrates a digest-verified PDF through the attachment store. | Extend `InputBlock`, `ContextFold`, store-reference discovery, and missing-attachment degradation for documents. [VERIFIED: codebase grep] |
| PDF-05 | Live Flux proof records quoted content, token jump, canary-clean evidence, and metering limits. | Follow the existing probe/canary scripts while keeping credentials path-only and fixtures header-free. [VERIFIED: `AGENTS.md`; `SPEC-WP0-hardening.md` WP-0.3] |
| PDF-06 | Add the typed error through the canonical table and regenerate mirrors only under an exact ownership grant. | Update the enum, exhaustive table, pinned counts, then run the generator; never edit JSON mirrors. [VERIFIED: `.planning/REQUIREMENTS.md`; codebase grep] |
</phase_requirements>

## Summary

WP-0.3 is an additive extension of the already-shipped image pipeline, not a new upload subsystem. The correct end-to-end shape is ACP `document`/`document_path` → validated bytes → digest-keyed blob store plus a journal-only `DocumentRef` → request-time `ContentBlock::Document` → Anthropic `document.source.base64`. The three-view `TurnInput` invariant (projection, manifest, live content) and `ContextFold` replay path must remain aligned. [VERIFIED: `SPEC-WP0-hardening.md` WP-0.3; `crates/nano-agent/src/turn_input.rs`; codebase grep]

The highest-risk issue is authorization, not implementation complexity. `DocumentRef` necessarily changes `crates/nano-session/src/op.rs`, and safe GC necessarily changes `crates/nano-session/src/attachment_store.rs`, yet neither file is granted by the WP-0.3 OWNS list. Without both, PDF manifests either cannot exist or their blobs can be collected while still referenced. Planning must begin with an owner-approved card correction/deviation that names these exact files and limits edits to document manifest/reference handling. [VERIFIED: ownership text in `SPEC-WP0-hardening.md`; `InputBlock` and reference scan in current code]

**Primary recommendation:** obtain the two-file ownership correction first, then implement one vertical document path in dependency order: durable type/store reachability → turn projections → ACP validation → replay → wire/refusal → generated errors → deterministic tests → live proof. [VERIFIED: dependency analysis of current code]

## Project Constraints (from AGENTS.md)

- Write only in `wayland-nano/` and task-required `../shared/`; `../nano/` and `../resources/upstreams/` are read-only. [VERIFIED: `AGENTS.md`]
- Stay within assigned files; stop and request a deviation when the boundary is insufficient. No product edits belong in research. [VERIFIED: `AGENTS.md`; master plan]
- Never read, print, copy, or embed the Flux key; reference only `../.secrets/flux-test-key`/its configured path. No secrets or auth headers may enter fixtures, logs, frames, or commits. [VERIFIED: `AGENTS.md`]
- Fail closed; never weaken sandbox, egress, policy, journal code, or tests. Missing subject matter fails; live-gated tests alone may self-skip when the key is absent. [VERIFIED: `AGENTS.md`]
- Rust 1.95.0, edition 2024, MSVC; `windows-sys` remains 0.52. Completion requires `just gate-all`, whose current recipes include fmt, clippy `-D warnings`, workspace tests, and both generators in `gate-gen-check`. [VERIFIED: `AGENTS.md`; `justfile`; local version probes]
- Generated error artifacts are regenerated with `cargo run -p nano-cli --bin gen_error_table`; they are never hand-edited. Pinned counts must move when the new kind lands. [VERIFIED: master plan; error-table source/tests]
- Do not commit or push unless explicitly authorized; this research task authorizes only this planning artifact. [VERIFIED: `AGENTS.md`; task assignment]

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|---|---|---|---|
| ACP inline/path validation | API / Backend (`nano-protocol`) | Storage | The host boundary validates untrusted JSON, confinement, magic, MIME, count, and size before persistence. [VERIFIED: codebase grep] |
| Durable PDF manifest | Database / Storage (`nano-session`) | API / Backend (`nano-agent`) | Journal contains digest metadata only; live base64 never enters it. [VERIFIED: current image contract] |
| Resume rehydration | API / Backend (`nano-cli` ContextFold) | Storage | The reducer resolves a journal reference using digest-verified store reads. [VERIFIED: codebase grep] |
| Provider encoding | API / Backend (`nano-model`) | External Flux service | Only the Anthropic driver translates the universal document block to provider JSON. [VERIFIED: `anthropic_messages.rs`] |
| Incompatible-wire refusal | API / Backend (`nano-cli`) | Error contract | Binding resolution is authoritative; the rejection must precede outbound dispatch. [VERIFIED: `acp_mode.rs`; spec] |
| Live evidence | External service boundary | Static evidence store | A real response and input-token discontinuity establish ingestion rather than HTTP success alone. [VERIFIED: recorded Flux media contract] |

## Exact Ownership and Required Correction

### Granted surfaces

`crates/nano-cli/src/acp_mode.rs` (document intake/replay/routing only), `crates/nano-agent/src/turn_input.rs` (document intake only), `crates/nano-model/**`, `crates/nano-protocol/src/acp.rs` (block converter only), `shared/fixtures/flux/pdf/**`, and `docs/FOLLOWUPS.md`; typed-refusal-only access to `crates/nano-session/src/error_kind.rs`, `crates/nano-session/src/error_codes.rs`, `crates/nano-protocol/src/error_codes.rs`, `crates/nano-cli/src/bin/gen_error_table.rs`, and the two generated JSON mirrors. [VERIFIED: `SPEC-WP0-hardening.md` WP-0.3]

### Blocking ownership gap

| Required file | Why unavoidable | Requested narrow grant |
|---|---|---|
| `crates/nano-session/src/op.rs` | `InputBlock` currently has only `Text` and `ImageRef`; a durable `DocumentRef` manifest cannot be represented elsewhere without violating the locked one-manifest contract. [VERIFIED: codebase grep] | Add `DocumentRef` and `InputBlock::DocumentRef`; tests/validation only. |
| `crates/nano-session/src/attachment_store.rs` | GC reference discovery currently preserves only `InputBlock::ImageRef` and tool-result image refs. A PDF blob journaled under a new variant would appear unreferenced and be eligible for deletion. [VERIFIED: codebase grep] | Teach reference discovery and tests to preserve `DocumentRef`; reuse `put`/`read_verified`, no store redesign. |

The GOALS card also inaccurately says `crates/nano-model/**` includes canonical error-table sources/mirrors; those sources actually live in `nano-session`, `nano-protocol`, and `nano-cli`, while one mirror is under `shared/contracts`. The explicit later file list is the usable authority. [VERIFIED: `GOALS.md` WP-0.3; repository paths]

## Standard Stack

No external package installation is required. Existing workspace dependencies already provide base64 (`base64 0.22`), serialization (`serde`/`serde_json 1`), hashing (`sha2 0.10`), and temporary test storage (`tempfile 3`). [VERIFIED: crate Cargo.toml files]

| Component | Existing surface | Purpose |
|---|---|---|
| ACP conversion | `nano-protocol::acp_blocks_to_content_blocks` | Parse untrusted blocks and construct `TurnInput`. [VERIFIED: codebase grep] |
| Durable blobs | `AttachmentStore::{put,read_verified}` | Digest-addressed publication and verified replay. [VERIFIED: codebase grep] |
| Universal request type | `nano_model::types::ContentBlock` | Add provider-neutral `Document { media_type, data }`. [VERIFIED: spec/current type] |
| Anthropic codec | `anthropic_messages::message_to_wire` | Emit the exact base64 document source object. [VERIFIED: spec/current driver] |
| Error generation | `nano_session::error_codes` + `gen_error_table` | Exhaustive typed surface and byte-drift mirrors. [VERIFIED: codebase grep] |

## Architecture Patterns

### System Architecture Diagram

```text
ACP prompt JSON (untrusted)
  ├─ document: base64 + claimed MIME
  └─ document_path: confined local path
          ↓ validate count/cap + %PDF- + MIME/extension agreement
TurnInput::Document { DocumentRef, live base64 }
          ├─ projection → TurnBegin.input placeholder
          ├─ manifest → InputBlock::DocumentRef (digest only)
          └─ live request → ContentBlock::Document
                    ↓ resolve leaf binding
            AnthropicMessages? ── no → typed ModelLacksPdf, zero dispatch
                    │ yes
                    ↓
Anthropic message_to_wire → {type:document,source:{type:base64,media_type:application/pdf,data}}

Resume: journal DocumentRef → AttachmentStore::read_verified → ContentBlock::Document
GC: journal DocumentRef → referenced digest set → blob retained
```

### Recommended change order

1. After ownership correction, define `DocumentRef` and `InputBlock::DocumentRef`, including serde round-trip and digest-only tests. [VERIFIED: dependency analysis]
2. Extend attachment GC/reference scanning before any producer can journal documents. [VERIFIED: current GC scan]
3. Extend `TurnBlock` and every exhaustive projection/manifest/content match; add `has_documents()` rather than overloading image-specific session provenance. [VERIFIED: current `TurnInput` API]
4. Add a PDF helper in the owned ACP converter that reuses the exact confined-open algorithm but applies `.pdf`, a 20 MiB bounded read, `%PDF-` at byte zero, and MIME equality. Do not send PDF bytes through the image decoder. [VERIFIED: spec; current image converter]
5. Publish PDFs under the same attachment-store write lease and extend `ContextFold` manifest detection/rehydration. Missing or corrupt documents degrade explicitly and never reconstruct from the textual placeholder. [VERIFIED: current image flow]
6. Add the Anthropic codec arm and its body-shape unit test. [VERIFIED: current driver testability]
7. Resolve the binding, then reject document-bearing turns unless `WireKind::AnthropicMessages`, before the driver is called. Auto/client-side routing requires the same per-leaf rule; checking only the configured alias before leaf selection is insufficient. [VERIFIED: current routing branches]
8. Add the new error kind/spec/ALL_KINDS entries and update both pinned counts from 70 to 71, then run the generator. `nano-protocol/src/error_codes.rs` is primarily a re-export/count test and may need no production mapping change. [VERIFIED: codebase grep]

### Key implementation invariant

The journal manifest stores `DocumentRef` metadata and digest only; base64 exists only in the live turn and is rebuilt from verified blob bytes on replay. Projection, manifest, and live content must all derive from the same `TurnInput`, preserving order and duplicates. [VERIFIED: established image pattern]

## Don't Hand-Roll

| Problem | Don't build | Use instead | Why |
|---|---|---|---|
| Blob persistence/integrity | A PDF directory or path-id cache | Existing `AttachmentStore` | It already supplies digest keys, write leases, integrity reads, and permission auditing. [VERIFIED: codebase grep] |
| Path security | `canonicalize` followed by ordinary `read` | Existing handle-verified confined-open pattern | The existing code closes authorize/open swap and reparse/symlink escape races. [VERIFIED: `acp.rs`] |
| PDF parsing | A PDF parser or page extractor | `%PDF-` signature plus opaque bytes | This phase transports a provider document; it does not interpret PDFs locally. [VERIFIED: locked spec] |
| Wire selection | PDF-aware rerouting | Existing resolved `WireKind` plus typed refusal | Silent rerouting is explicitly rejected as a policy change. [VERIFIED: locked spec] |
| Error JSON | Manual mirror edits | `gen_error_table` | The generator and parity test are the canonical drift controls. [VERIFIED: generator source] |

## Common Pitfalls

### 1. Implementing only the request codec
The request may work once yet fail after resume or lose its blob to GC. Require tests spanning manifest serialization, GC reachability, verified read, and `ContextFold` reconstruction. [VERIFIED: current architecture]

### 2. Reusing image limits or errors blindly
Images currently allow more items and a 50 MiB aggregate. PDF rules are exactly one and 20 MiB. Add document-specific count/cap handling; do not report `ImageTooMany` for a PDF unless the owner explicitly locks that compatibility choice. [VERIFIED: current constants; WP-0.3 spec]

### 3. Trusting extension or claimed MIME
Inline requires claimed `application/pdf`; path derives `.pdf`; both must agree with `%PDF-` at offset zero. Base64 validity alone is not content validation. [VERIFIED: locked spec]

### 4. Gating too early or too late
The model name is not the wire. Refuse after the actual leaf binding is known but before any provider call. Auto routing must not select an OpenAI leaf for a document and then silently discard it. [VERIFIED: routing code/spec]

### 5. Exhaustive-match blast radius
Adding variants to `ContentBlock`, `TurnBlock`, and `InputBlock` breaks or semantically affects matches across crates. Use compiler failures plus `rg` inventories; inspect wildcard arms because they compile while possibly dropping documents. [VERIFIED: codebase grep]

### 6. Incorrect error-table regeneration
Adding `ModelLacksPdf` requires the enum, exhaustive spec match, `ALL_KINDS`, pinned tests in two crates, and generator output. Run generation, never edit either JSON. The optional shared mirror must be present/fresh in this monorepo task. [VERIFIED: generator and tests]

### 7. Treating the recorded contract as vendor documentation
`flux-media-contract-2026-08-14.md` is owner-supplied empirical evidence measured on 2026-08-14, not official Anthropic or FluxRouter API documentation. It is authoritative for this project decision, but citations must call it a recorded probe contract, not an official API guarantee. [VERIFIED: contract header/content]

### 8. Weak live oracle
HTTP 200 or a fluent answer does not prove PDF ingestion. The acceptance oracle is a known exact quote plus the substantial input-token jump; captures must exclude authorization headers and pass canary scan. [VERIFIED: recorded contract/spec]

## Validation Architecture

### Test Framework

| Property | Value |
|---|---|
| Framework | Rust 1.95 built-in test harness; shell/Node evidence scripts [VERIFIED: local probes/repo] |
| Config | Workspace Cargo manifests and `justfile` [VERIFIED: codebase] |
| Quick runs | `cargo test -p nano-protocol`, `cargo test -p nano-agent`, `cargo test -p nano-model`, `cargo test -p nano-session`, targeted `cargo test -p nano-cli <filter>` [VERIFIED: workspace structure] |
| Full gate | `just gate-all` [VERIFIED: `justfile`] |

### Requirements to tests

| Req | Required automated evidence | File status |
|---|---|---|
| PDF-01 | Converter table: inline/path happy paths; malformed base64; missing/incorrect MIME; bad magic; `.pdf` mismatch; zero/20MiB boundary/over cap; two documents; symlink/reparse and authorize-open swap. | ❌ Wave 0 additions in `acp.rs` tests [VERIFIED: current tests] |
| PDF-02 | Serde `DocumentRef` manifest round-trip; no base64 in journal; ordered text/image/document projection; existing image tests unchanged. | ❌ Wave 0 [VERIFIED: current types/tests] |
| PDF-03 | Exact Anthropic JSON; fake driver proves OpenAI binding returns typed refusal with zero calls; compatible binding sends once; auto leaf coverage. | ❌ Wave 0 [VERIFIED: current driver/routing seams] |
| PDF-04 | Journal → kill/reopen → digest-verified rehydrate; corrupt/missing blob fail-closed/degrade explicitly; GC retains referenced PDF and removes orphan. | ❌ Wave 0 [VERIFIED: current store/replay tests] |
| PDF-05 | Live probe script/fixture schema plus canary receipt; normal CI replays fixture and does not require network. | ❌ Wave 0 [VERIFIED: project testing convention] |
| PDF-06 | Exhaustive mapping compiles, both count tests assert 71, parity/generated checks pass. | Existing framework; assertions require update. [VERIFIED: error-code tests] |

### Sampling and gates

- Per task: run the owning crate's focused tests and `cargo fmt --all -- --check`. [VERIFIED: project convention]
- After cross-crate plumbing: run the five affected crate suites, then `cargo clippy --workspace --all-targets -- -D warnings`. [VERIFIED: `justfile`]
- Generator task: `cargo run -p nano-cli --bin gen_error_table`, then `cargo run -p nano-cli --bin gen_error_table -- --check`. [VERIFIED: generator contract]
- Phase gate: `just gate-all`; then the explicitly live Flux probe and `node scripts/canary/scan.mjs <receipt-out.json>` against the new fixture set. [VERIFIED: spec]
- Promotion includes one Critical/High audit, at most one fix round, fix verification, integration `just gate-all`, and CI green; builder does not merge/push. [VERIFIED: roadmap/master plan]

## Security Domain

### Applicable ASVS Categories

| Category | Applies | Control |
|---|---|---|
| V2 Authentication | yes (live proof only) | Existing credential resolver/path-only key; never fixture the bearer. [VERIFIED: `AGENTS.md`] |
| V3 Session Management | yes | Journaled digest manifest and verified replay; no raw document in journal. [VERIFIED: architecture] |
| V4 Access Control | yes | Workspace/pictures-root confinement, sensitive subtree rejection, handle identity verification. [VERIFIED: `acp.rs`] |
| V5 Input Validation | yes | Closed block tags, strict base64, exact MIME, `%PDF-`, count and byte caps. [VERIFIED: spec] |
| V6 Cryptography | yes | Existing SHA-256 digest store; no custom crypto. [VERIFIED: Cargo/store code] |

### Threat and boundary traps

| Threat | STRIDE | Required mitigation |
|---|---|---|
| Path traversal/symlink/reparse swap | Elevation/Tampering | Reuse handle-verified confined open; test swap race on supported OS paths. [VERIFIED: current threat model] |
| MIME/content confusion | Tampering | Claim/derived MIME must equal `application/pdf` and bytes start `%PDF-`. [VERIFIED: spec] |
| Oversize allocation/base64 amplification | DoS | Reject decoded bytes over 20 MiB using saturating accounting and bounded path read before encoding/storage. [VERIFIED: spec/pattern] |
| Silent provider drop | Spoofing | Wire-kind gate and zero-call fake assertion; never rely on HTTP success. [VERIFIED: recorded probe contract] |
| Resume blob deletion | Tampering/DoS | Add document refs to GC reachability before producer rollout. [VERIFIED: current GC logic] |
| Credential leakage | Information disclosure | Path-only key, no header capture, canary scan all fixtures/receipts. [VERIFIED: `AGENTS.md`] |
| Hostile PDF internals | DoS | Treat bytes as opaque; no local PDF parsing in this phase; provider execution remains outside the local trust boundary. [VERIFIED: scoped design] |

## Environment Availability

| Dependency | Available | Version | Note |
|---|---:|---|---|
| Rust/rustc | yes | 1.95.0 | Matches pin. [VERIFIED: local probe] |
| Cargo | yes | 1.95.0 | Matches toolchain. [VERIFIED: local probe] |
| just | yes | 1.51.0 | Gate runner available. [VERIFIED: local probe] |
| Node | yes | 24.16.0 | Canary script runtime available. [VERIFIED: local probe] |
| Flux credential | not inspected | — | Deliberately do not read it during planning; live step checks presence via approved resolver/path without exposing value. [VERIFIED: security rule] |

## Source Citation Reality Corrections

- Current code baseline is `d8702f22…`, not the specs' historical `466f030`; symbol anchors were re-derived on the current baseline. [VERIFIED: git/codebase grep]
- `provider_router.rs` is at `crates/nano-cli/src/provider_router.rs`, not under `nano-model`. [VERIFIED: repository paths]
- Flux currently resolves through the catalog row whose test pins `WireKind::OpenAiCompletions`; therefore the shipped Flux alias path will refuse PDFs until a genuinely Anthropic-bound leaf exists. That refusal is expected behavior, not evidence that the codec arm is unused. [VERIFIED: provider router/catalog tests]
- The shared media contract is empirical owner evidence, not official documentation. Its exact document shape and anti-blind token observation are project-locked, while multi-page/large-PDF behavior remains explicitly unverified. [VERIFIED: recorded contract]
- `SPEC-WP-INTERFACES.md` is authoritative for verification artifacts but provides no additional PDF schema beyond general hardening context; WP-0.3's operative PDF contract is the hardening spec plus recorded media contract. [VERIFIED: canonical interface spec review]

## Assumptions Log

| # | Claim | Risk if wrong |
|---|---|---|
| A1 | The new typed kind should be named `ModelLacksPdf` / `model_lacks_pdf`; the spec says “e.g.” rather than locking the spelling. [ASSUMED] | Public error vocabulary changes; owner should lock the exact symbol/wire name in the ownership correction. |
| A2 | Document placeholders should use `[Document #n: …]` and missing-document text should be document-specific. [ASSUMED] | Replay/UI text contract differs; lock wording before snapshot tests. |
| A3 | PDF `DocumentRef` needs digest, MIME, bytes, and placeholder but no image dimensions/normalization fields. [ASSUMED] | Journal schema changes; owner should approve the exact struct fields. |

## Open Questions

1. **Will the owner grant `op.rs` and `attachment_store.rs`?** Required before product work; there is no safe in-scope workaround. [VERIFIED: ownership/code dependency]
2. **What exact error vocabulary is locked?** Recommend `ModelLacksPdf` serialized as `model_lacks_pdf`, non-retryable `ErrorResponse`, with guidance to select an Anthropic Messages binding. [ASSUMED]
3. **Which configured model provides the live Anthropic leaf?** Current Flux catalog is OpenAI-completions. The live plan must identify/provision an existing proven Anthropic binding without changing routing policy; otherwise implementation can be complete but PDF live proof is blocked. [VERIFIED: current catalog/router]
4. **Should invalid PDF content reuse generic/image kinds or add document-specific validation kinds?** The spec requires typed rejection but grants only one new typed refusal explicitly. Recommend use `InvalidParams` for malformed base64/MIME/magic, a bounded existing storage error for store failure, and reserve the new kind solely for incompatible wire unless the owner expands the error grant. [ASSUMED]

## Sources

### Primary (HIGH confidence)

- `shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md` — execution order, ownership discipline, generator rule. [VERIFIED: local canonical document]
- `shared/reviews/research-0.2/specs/SPEC-WP0-hardening.md` WP-0.3 — locked intake/type/routing/evidence design. [VERIFIED: local canonical document]
- `shared/reviews/research-0.2/GOALS.md` WP-0.3 and `.planning/REQUIREMENTS.md` PDF-01..06 — scope and acceptance requirements. [VERIFIED: local canonical documents]
- `shared/reviews/stable-wave/flux-media-contract-2026-08-14.md` — owner-recorded live probe evidence; empirical, not official vendor docs. [VERIFIED: local evidence contract]
- Current source at baseline `d8702f22…` — type, converter, storage, routing, codec, generator, and test seams. [VERIFIED: codebase grep]

### Secondary / external

None used. No external package or vendor-document decision was needed; this phase is constrained by project-locked source and recorded live evidence. [VERIFIED: research scope]

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new dependencies; manifests inspected. [VERIFIED: Cargo manifests]
- Architecture: HIGH — traced current producer, journal, store, reducer, binding, and codec paths. [VERIFIED: codebase grep]
- Pitfalls: HIGH — derived from concrete ownership and exhaustive-match/store reachability seams. [VERIFIED: codebase grep]
- Live model availability: MEDIUM — current incompatible Flux binding is verified, but an eligible live Anthropic binding/key was not probed during research. [VERIFIED: catalog; credential intentionally uninspected]

**Research date:** 2026-08-17  
**Valid until:** 2026-09-16, or immediately stale if `origin/master`, the WP-0.3 ownership card, provider catalog, or Flux media contract changes. [ASSUMED]
