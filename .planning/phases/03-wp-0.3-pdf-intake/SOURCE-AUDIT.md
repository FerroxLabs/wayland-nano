# Phase 03 Multi-Source Coverage Audit

| SOURCE | ID | Feature / requirement | Plan | Status | Notes |
|---|---|---|---|---|---|
| GOAL | — | Close F-P2B-4 with Anthropic document-block PDF intake, live anti-blind proof, and typed incompatible-wire refusal | 03-01..03-11 | COVERED | Preflight, intake, audit/fix/recheck, live evidence, closure, and handoff chain. |
| REQ | PDF-01 | Inline/path validation, count, cap, sniff, MIME/extension, typed refusals | 03-04 | COVERED | Closed converter test table. |
| REQ | PDF-02 | Additive `DocumentRef` and document blocks; image contract unchanged | 03-01, 03-03 | COVERED | No rename or generic attachment ref. |
| REQ | PDF-03 | Exact Anthropic block; explicit D5 active catalog leaf; pre-network OpenAI refusal; no drop/reroute | 03-02, 03-03, 03-05 | COVERED | Normal runtime binding controls dispatch; no direct client proof. |
| REQ | PDF-04 | Digest-verified kill/resume plus GC reachability | 03-01, 03-05 | COVERED | Existing attachment store reused. |
| REQ | PDF-05 | Mandatory D6 exact quote, same-path delta >=1000, paired evidence, exact seven-file non-self-referential canary, metering note | 03-05, 03-07, 03-08 | COVERED | Any missing/failing proof blocks Phase 3 and leaves F-P2B-4 OPEN. |
| REQ | PDF-06 | Canonical typed error and mandatory generated Nano/shared mirrors | 03-02 | COVERED | Optional Desktop refresh is owner/integrator-only and non-DoD. |
| RESEARCH | — | Three-view TurnInput invariant and dependency order | 03-01, 03-03, 03-04, 03-05 | COVERED | Manifest, projection, and live blocks derive from one ordered input. |
| RESEARCH | D1 | Exact two-file session grant is a hard pre-edit gate | 03-01 | COVERED | `op.rs` and `attachment_store.rs` only for named document slices. |
| RESEARCH | D2 | Exact ModelLacksPdf vocabulary/presentation/count | 03-02 | COVERED | Full source mapping and generator parity. |
| RESEARCH | D3 | Exact DocumentRef fields and projection/replay strings | 03-01, 03-03, 03-05 | COVERED | No unresolved schema/placeholder assumption. |
| RESEARCH | D4 | Existing-kind validation policy and bounded messages | 03-04 | COVERED | Only incompatible wire adds a new kind. |
| RESEARCH | D5 | Canonical flux-router-anthropic catalog entry plus selector-only runtime payload | 03-03, 03-05, 03-07, 03-11, 03-08 | COVERED | Drift pin, generated golden, provenance, runtime path, bare negative control, and an exact detached provider-catalog endpoint receipt bound into final closure. |
| RESEARCH | D6 | Exact oracle/prompt and >=1000 same-path token delta | 03-05, 03-07 | COVERED | Raw counts and 94/1650 provenance are durable evidence. |
| RESEARCH | D7 | Concrete ignored ACP runtime live harness and command | 03-05, 03-07 | COVERED | Explicit invocation is fail-closed. |
| RESEARCH | D8 | Six manifest-described repo/shared payload pairs plus manifest-as-seventh non-self-referential receipt | 03-07, 03-08 | COVERED | Manifest absence is fatal; it has exactly six payload entries and no self entry; the receipt validates the six payloads plus current manifest as seven. |
| RESEARCH | D9 | Executable OWNS/reparse verifier and durable external SHA manifests | 03-01..03-08 | COVERED | Initialize precedes product mutation; every task Checks; final Closure exact-compares. |
| RESEARCH | — | Existing confinement/store/base64/hash stack; no new dependency | 03-01, 03-04 | COVERED | No package install task. |
| RESEARCH | — | Anthropic-only live path and OpenAI-bound negative control | 03-03, 03-05, 03-07 | COVERED | Canonical entry supplies endpoint; selector payload cannot inject it. |
| RESEARCH | D9 | Ownership preflight is the sole first predecessor before every product mutation | 03-01; all product plans depend on it | COVERED | Script/control initialization only in Wave 1. |
| RESEARCH | — | One Critical/High audit, one continuous bounded fix round with zero-to-two grouped commits and canonical fix.commits[] and final commit/tree metadata finalization, independent committed-byte recheck, full gate, builder-only handoff | 03-06, 03-10, 03-12, 03-13, 03-11, 03-08 | COVERED | Durable schema-closed audit JSON; every Critical/High command reference is bound to an exact command receipt created by the detached execution loop; no merge/push/CI authority. |
| CONTEXT | — | No CONTEXT.md decisions supplied | — | EXCLUDED | Binding decisions are in master/spec/GOALS and DEV-WP-0.3A. |

No deferred capability, PDF parsing/extraction, Files API, multi-document support, routing-policy change, attachment-store redesign, `ImageRef` rename, `ToolResult` change, Desktop-owned commit, `../nano/` write, or `../resources/upstreams/` write appears in the plan set.
# DEV-WP-0.3I source reconciliation — COVERED

The lifecycle model is covered by Plans 13, 11, 07, and 08: immutable product fix, known metadata, recheck artifacts, and live evidence occupy separate exact Git intervals. The six paths changed by `d731426` are explicitly lifecycle-allowed. `03-VALIDATION.md` and `docs/FOLLOWUPS.md` carry the resolved contract. No product scope or deferred feature was introduced.

Receipt closure is non-circular: summaries identify only already-committed inputs, while later phases discover summary commits from Git and verify exact one-file diffs. Both detached commands use normalized deterministic evidence instead of raw cargo output or temporary paths.

# DEV-WP-0.3J source reconciliation — COVERED

Plans 11 and 08 now use the fully qualified exact PDF dispatch test, require one executed/passed test, and reject Cargo's zero-test false green. The normalized receipt carries the exact fully qualified test ID. The lifecycle remains the exact ordered Git chain: immutable consecutive P13A/P13S are discovered from Git, while the post-P13S correction suffix through recheck HEAD is restricted to the five named planning/audit-document paths and excludes audit JSON, summaries, and product code. Validation and follow-up artifacts carry the same PS5.1-safe contract; no product scope was introduced.

# DEV-WP-0.3M source reconciliation — COVERED

Plans 11 and 08 cover the canonical second final fix, exact 18-commit audit history, two product-fix projections, all four findings, generic post-fix metadata, and the separate P11/live closure segments. Their identical four-command detached catalog binds endpoint, PDF, Windows verbatim, and clippy evidence to deterministic normalized receipts. The companion validation and follow-up records carry the same PowerShell 5.1-safe contract; no product or deferred scope was introduced.
