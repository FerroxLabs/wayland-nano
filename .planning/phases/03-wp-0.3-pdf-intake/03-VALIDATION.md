# Phase 03 Validation — WP-0.3 PDF Intake

## Validation objective

Prove that an untrusted inline or confined-path PDF becomes one digest-backed journal document, survives resume and GC, reaches only an Anthropic Messages leaf in the exact base64 document shape, and is refused before network I/O on every OpenAI Completions leaf. Validation is additive: existing image and attachment behavior remains unchanged.

## Nyquist requirement map

| Requirement | Observable proof | Automated gate | Evidence |
|---|---|---|---|
| PDF-01 | Inline/path acceptance plus malformed base64, MIME/extension disagreement, bad `%PDF-` magic, zero/20 MiB/over-cap, second-document, traversal, link/reparse, and authorize/open-swap rejection | `cargo test -p nano-protocol acp::tests` | Typed converter assertions |
| PDF-02 | `DocumentRef` round-trip contains digest metadata and placeholder but no base64; ordered text/image/document projections preserve the existing image contract | `cargo test -p nano-session`; `cargo test -p nano-agent turn_input` | Serde and projection tests |
| PDF-03 | Exact Anthropic JSON; compatible leaf dispatches once; OpenAI and auto-selected OpenAI leaves return `model_lacks_pdf` with zero driver/network calls | `cargo test -p nano-model`; targeted `cargo test -p nano-cli pdf` | Wire-body and zero-call fakes |
| PDF-04 | Reopen/replay uses `read_verified`; referenced document survives GC, orphan is removed, malformed/missing/corrupt reference degrades explicitly | `cargo test -p nano-session attachment_store`; targeted `cargo test -p nano-cli document` | Store and ContextFold tests |
| PDF-05 | Active catalog leaf `flux-router-anthropic:flux-auto` returns the exact D6 quote and same-path token delta >=1000 through the complete ACP runtime; paired captures contain no headers/key and exact-list canary receipt has zero hits | `cargo test -p nano-cli pdf_live_active_leaf_runtime_path -- --ignored --nocapture` plus exact-list scanner | in-repo `crates/nano-model/fixtures-flux/pdf/**` paired to canonical sibling `shared/fixtures/flux/pdf/**` |
| PDF-06 | Enum/spec/ALL_KINDS/count agree at 71 and both tracked JSON mirrors are generator-fresh | `cargo run -p nano-cli --bin gen_error_table -- --check` | Source tests and generated mirrors |

## Wave 0 tests required before implementation

1. Journal/store: serde round-trip, no base64, referenced retention, orphan removal, malformed digest rejection, missing/corrupt verified-read behavior.
2. Turn/model: ordered mixed projection/manifest/live content and exact Anthropic document-source JSON.
3. ACP: inline/path success and the complete count, size, sniff, MIME, extension, confinement, and handle-identity refusal table.
4. CLI: kill/reopen replay, compatible Anthropic dispatch once, explicit OpenAI leaf zero calls, auto-selected OpenAI leaf zero calls, and no silent reroute.
5. Error generation: exhaustive `ModelLacksPdf` mapping, serialized `model_lacks_pdf`, count 71, and byte-fresh tracked mirrors.

Each behavior-adding task writes the relevant failing test first, observes RED for the intended missing behavior, then implements and reruns GREEN. Existing image tests are mandatory regressions, not substitutes for document tests.

## Live evidence contract (D5-D8)

- Resolve `D:\Development\waylandnano\.secrets\flux-test-key` to an absolute path and set only `FLUX_API_KEY_FILE`; never read or print the governed value in orchestration.
- Set `WAYLAND_NANO_PROVIDERS` exactly to `[{"provider":"flux-router-anthropic","models":["flux-auto"],"hasKey":true}]` and exercise the ignored `acp_mode.rs::pdf_live_active_leaf_runtime_path` harness through `serve` → ACP → ProviderRouter → binding → driver. Direct client/curl proofs are invalid.
- The committed one-page PDF oracle is exactly `WAYLAND NANO PDF ORACLE 7F3A: copper owls navigate by moonlit checksum.` and the prompt is exactly `Return the complete oracle sentence from the attached PDF, preserving capitalization and punctuation.` The response must contain the full sentence byte-for-byte.
- Run a text-only control immediately before the document turn through the same leaf, prompt, process configuration, and max tokens; require control > 0 and `pdf_input_tokens - control_input_tokens >= 1000`. Record raw counts, delta, and the historical 94/1650 provenance without substituting historical values.
- Capture sanitized request shape, response facts, usage, exact quote result, and a manifest; never capture Authorization headers or raw secret-bearing diagnostics.
- Persist exactly seven byte-identical inputs (PDF, control request, document request, document response, usage summary, transcript, evidence manifest) in the in-repo and canonical sibling roots, verifying full SHA-256 and bytes. `evidence-manifest.json` is mandatory and absence is fatal.
- Generate the exact seven-path include list in a unique OS-temp file; exclude the final receipt. Require scanner exit 0, lowercase 64-hex fingerprint, hits=0/PASS, files_scanned=7, exact result-set equality with no duplicates/extras, and every full SHA-256/bytes against current files. Persist receipt hash/bytes in 03-07-SUMMARY; neither summary nor receipt hashes itself.
- If the credential, runtime route, manifest, network, quote, usage, delta, pair equality, or canary proof is missing/fails, Phase 3 fails and F-P2B-4 stays OPEN.

## Audit and closure

After deterministic implementation, serialized Plans 03-06, 03-10, 03-12, 03-13, and 03-11 write mandatory `03-AUDIT.json` schema `wp03_audit_v2`. Plan 06 performs exactly one audit and validates distinct builder/auditor identities, method/version/timestamp, exact baseline and audited commit/tree/path set, and a required findings array with unique IDs, closed severity/disposition, evidence, and command-backed `verified-no-fix`. Plans 10 and 12 form one continuous bounded fix round across all authorized product/golden/UPSTREAM surfaces; Plan 13 finalizes its zero-to-two ordered commits, exact trees/paths/findings/commands and pins the exact pre-fix and fix commit/tree/path set. Plan 11 uses an independent reviewer/method against those committed bytes, builds receipts only from commands actually executed in its detached final-product worktree, requires recorded commands to equal those receipts exactly, and gives every finding one verdict with every Critical/High command reference resolving to exactly one such receipt. The endpoint binding has its own exact focused provider-catalog test receipt proving `flux-router-anthropic:flux-auto` uses `anthropic-messages` at `/anthropic/v1/messages`. Absence, arbitrary minimal JSON, fabricated commands, dirty product bytes, malformed fields, identity reuse, path mismatch, or incomplete coverage is fatal.

Before product mutation, Plan 01 creates executable `03-OWNERSHIP-PREFLIGHT.ps1` and tracked `03-CONTROL.json`, with durable OS-temp sorted `{path,type,sha256,bytes}` manifests for external nano/resources/shared. Every task invokes Check; final Plan 08 invokes Closure. Status parsing is NUL-delimited and covers tracked/untracked; exact OWNS, every ancestor reparse/link check, literal resolution, external Compare-Object, allowed shared deltas, and pair hashes fail closed with propagated errors.

For every `gen_error_table` generation/check, resolve the canonical sibling shared mirror absolutely and set `NANO_ERROR_TABLE_DESKTOP_DIR` to a unique absent OS-temp target; require it remains absent and restore the prior environment value. The final builder gate is complete `just gate-all`. The handoff records baseline, branch, commits, audit/fix/recheck, strict ownership/external hashes, generator isolation, live/canary evidence, and I/R/L distinctions. The builder does not merge, push, create integration state, run/claim CI, or treat an optional Desktop mirror as completion evidence.
# DEV-WP-0.3I lifecycle validation addendum (resolved)

- Product identity is exactly commit `18d57a6724637f597883685749583253613a0884`, tree `c2dfe7aac460dd7cfe30084859d26eb2a4145403`.
- Plan 13 requires a clean pre-output worktree and captures its actual HEAD/tree as `input_tip`. Its lifecycle is the exact ordered projection of every non-product commit through that dynamic tip; `3fde7c5`, `f34da2f`, `d731426`, and `85a8b1d` remain immutable ordered anchors, while later commits are restricted to the closed planning allowlist.
- P13A and P11A are audit-only commits; P13S and P11S are summary-only commits. JSON artifacts never predict or self-hash their output commits.
- Audit history ends at `recheck_point` (actual P13S). P11 receipts and Plan 07 live evidence are independently checked phase-history segments.
- Plan 08 captures `closure_input_tip` before mutation and does not equate audited-to-closure HEAD with audit history.
- Summaries never record their own commit/tree: P13 summary records only P13A; Plan 11 discovers P13S from HEAD; P11 summary records only P11A; Plan 07 discovers P11S from HEAD; Plan 07 summary records only pre-summary live evidence; Plan 08 discovers later summary commits directly from Git.
- Detached command evidence is normalized to command ID, exit zero, product commit/tree, expected test name, and pass marker with `execution_mode=detached-worktree`; raw cargo timing and temporary paths are not persisted or compared.

# DEV-WP-0.3J exact-test recheck addendum (resolved)

- Canonical command 2 is `cargo test -p nano-cli acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded --lib -- --exact --nocapture`; its expected and normalized receipt test ID is exactly `acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded`.
- The transient proof must contain `running 1 test` and a successful result with `1 passed; 0 failed`; `running 0 tests` is fatal even when Cargo exits zero.
- Plan 11 discovers immutable P13S as the commit touching `03-13-SUMMARY.md` and P13A as its parent, and proves their consecutive one-file diffs. P13A/P13S are interior lifecycle anchors rather than terminal commits.
- Only post-P13S correction commits through the actual recheck HEAD are allowed, and every path must be one of `03-11-PLAN.md`, `03-08-PLAN.md`, `03-VALIDATION.md`, `SOURCE-AUDIT.md`, or `docs/FOLLOWUPS.md`; audit, summary, and product paths are forbidden in that suffix.
