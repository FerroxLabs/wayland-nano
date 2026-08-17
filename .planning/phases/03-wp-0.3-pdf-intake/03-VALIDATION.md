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

After deterministic implementation, the mandatory `03-AUDIT.json` schema `wp03_audit_v2` records the exact audit and correction history. Plan 06 performs exactly one audit and validates distinct builder/auditor identities, method/version/timestamp, exact baseline and audited commit/tree/path set, and a required findings array with unique IDs, closed severity/disposition, evidence, and command-backed `verified-no-fix`. The final canonical audit history is the exact 25-commit projection through product commit `f1372da6336f7bacad95b2c460c7f9ff1d4fcaf5` and tree `5ff1ea037d604c273095b5303062a68e936d83df`; it contains exactly three product fixes and their exact six-finding union. Plan 11 uses an independent reviewer/method against those committed bytes, builds receipts only from five commands actually executed in its detached final-product worktree, requires recorded commands to equal those receipts exactly, and gives every finding one verdict. Endpoint binding proves the canonical `/v1` base with the exact provider test, and the harness-schema receipt proves `pdf_evidence_manifest_schema_has_exact_six_payload_pairs` ran exactly once. Absence, fabricated commands, dirty product bytes, malformed fields, identity reuse, path mismatch, or incomplete coverage is fatal.

Before product mutation, Plan 01 creates executable `03-OWNERSHIP-PREFLIGHT.ps1` and tracked `03-CONTROL.json`, with durable OS-temp sorted `{path,type,sha256,bytes}` manifests for external nano/resources/shared. Every task invokes Check; final Plan 08 invokes Closure. Status parsing is NUL-delimited and covers tracked/untracked; exact OWNS, every ancestor reparse/link check, literal resolution, external Compare-Object, allowed shared deltas, and pair hashes fail closed with propagated errors.

For every `gen_error_table` generation/check, resolve the canonical sibling shared mirror absolutely and set `NANO_ERROR_TABLE_DESKTOP_DIR` to a unique absent OS-temp target; require it remains absent and restore the prior environment value. The final builder gate is complete `just gate-all`. The handoff records baseline, branch, commits, audit/fix/recheck, strict ownership/external hashes, generator isolation, live/canary evidence, and I/R/L distinctions. The builder does not merge, push, create integration state, run/claim CI, or treat an optional Desktop mirror as completion evidence.
# DEV-WP-0.3I lifecycle validation addendum (SUPERSEDED historical)

Historical record only; DEV-WP-0.3O is the sole active authority and the statements below must not drive execution.

- Product identity is exactly commit `18d57a6724637f597883685749583253613a0884`, tree `c2dfe7aac460dd7cfe30084859d26eb2a4145403`.
- Plan 13 requires a clean pre-output worktree and captures its actual HEAD/tree as `input_tip`. Its lifecycle is the exact ordered projection of every non-product commit through that dynamic tip; `3fde7c5`, `f34da2f`, `d731426`, and `85a8b1d` remain immutable ordered anchors, while later commits are restricted to the closed planning allowlist.
- P13A and P11A are audit-only commits; P13S and P11S are summary-only commits. JSON artifacts never predict or self-hash their output commits.
- Audit history ends at `recheck_point` (actual P13S). P11 receipts and Plan 07 live evidence are independently checked phase-history segments.
- Plan 08 captures `closure_input_tip` before mutation and does not equate audited-to-closure HEAD with audit history.
- Summaries never record their own commit/tree: P13 summary records only P13A; Plan 11 discovers P13S from HEAD; P11 summary records only P11A; Plan 07 discovers P11S from HEAD; Plan 07 summary records only pre-summary live evidence; Plan 08 discovers later summary commits directly from Git.
- Detached command evidence is normalized to command ID, exit zero, product commit/tree, expected test name, and pass marker with `execution_mode=detached-worktree`; raw cargo timing and temporary paths are not persisted or compared.

# DEV-WP-0.3J exact-test recheck addendum (SUPERSEDED historical)

Historical record only; DEV-WP-0.3O is the sole active authority and the statements below must not drive execution.

- Canonical command 2 is `cargo test -p nano-cli acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded --lib -- --exact --nocapture`; its expected and normalized receipt test ID is exactly `acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded`.
- The transient proof must contain `running 1 test` and a successful result with `1 passed; 0 failed`; `running 0 tests` is fatal even when Cargo exits zero.
- Plan 11 discovers immutable P13S as the commit touching `03-13-SUMMARY.md` and P13A as its parent, and proves their consecutive one-file diffs. P13A/P13S are interior lifecycle anchors rather than terminal commits.
- Only post-P13S correction commits through the actual recheck HEAD are allowed, and every path must be one of `03-11-PLAN.md`, `03-08-PLAN.md`, `03-VALIDATION.md`, `SOURCE-AUDIT.md`, or `docs/FOLLOWUPS.md`; audit, summary, and product paths are forbidden in that suffix.

# DEV-WP-0.3M second final-fix recheck addendum (SUPERSEDED historical)

Historical record only; DEV-WP-0.3O is the sole active authority and the statements below must not drive execution.

- Canonical final product identity is `4fd669bfb921769456f1603221bbe2326487d67c`, tree `84af3ddd0d0773bc72db7684c516a622bd4453c4`; audit history is the exact ordered 18-commit range from audited commit through that fix and contains two product-fix projections whose finding union is all four findings.
- Post-fix metadata is derived generically through ActualHead with actual parent/tree/diffs and a closed planning, audit, control, preflight, and summary allowlist. Product and evidence paths are forbidden; no Plan 13 terminal assumption remains.
- Plans 11 and 08 execute and exactly compare four normalized receipts at the detached final tree: endpoint, fully-qualified PDF refusal, fully-qualified nano-protocol Windows verbatim regression, and nano-model all-target clippy with warnings denied. Tests prove one run/one pass/no zero; clippy proves a stable clean completion without persisting raw output.
- Verdict binding is High 001 to PDF, High 002 to Windows, and High 003/004 to clippy. The audit output commit remains a non-self-referential child of the captured recheck point.

# DEV-WP-0.3O canonical final-tree recheck addendum (SUPERSEDED HISTORICAL)

- Canonical product identity is `f1372da6336f7bacad95b2c460c7f9ff1d4fcaf5`, tree `5ff1ea037d604c273095b5303062a68e936d83df`. Audit history is exactly 25 commits from the audited anchor and contains exactly three product fixes: `18d57a6` for finding 001, `4fd669b` for 002-004, and `f1372da` for 005-006. Their exact path projections and union of six unique findings are mandatory.
- The generic post-fix chain is derived from `f1372da..ActualHead`. `phase_history.post_fix` is exactly docs commit `2a55eae` followed by seven-file live-evidence commit `0eb5098`; later audit-only metadata, including `c0d6f69`, and the named plan-correction documents remain separate allowed shapes. Current evidence hashes and external receipt metadata are validated independently because evidence is not part of the f137 tree.
- Plans 11 and 08 execute the same five commands at detached f137: exact provider endpoint (`/v1` authority), fully-qualified PDF refusal, fully-qualified Windows verbatim regression, strict nano-model clippy, and exact harness-schema test `pdf_evidence_manifest_schema_has_exact_six_payload_pairs`. Every test proves exactly one execution and pass. Findings bind 001→PDF, 002→Windows, 003/004→clippy, 005→endpoint, and 006→harness schema.

# DEV-WP-0.3P final journal-tree recheck (ACTIVE AUTHORITY; live closure pending)

- Canonical product identity is `5040293cf4de8467555f4c74b46b34a91d6939d7`, tree `be34bb63f58cacd64bdab3a073f17fa5d4088719`. Audit history is the exact 37-commit projection from the audited anchor, with four product fixes `18d57a6`, `4fd669b`, `f1372da`, and `5040293`; their exact path projections cover all eight unique findings.
- Post-fix history is derived generically from `5040293..ActualHead`. The earlier documentation, live-evidence, audit, recheck, summary, control, and journal-finding commits remain exact ordered history rows; current seven-file evidence and its receipt are verified independently and are unchanged.
- Plans 11 and 08 execute the same seven detached commands at `5040293`. The two added exact one-test receipts are `RECHECK-JOURNAL-FORWARD-FIELDS` for `cargo test -p nano-session tests::p2a_op_never_denies_unknown_fields --lib -- --exact --nocapture` and `RECHECK-DOCUMENTREF-CLOSED` for `cargo test -p nano-session op::document_ref_tests::document_ref_rejects_duplicate_known_fields_from_raw_json --lib -- --exact --nocapture`. Finding 007 maps to the forward-fields receipt and finding 008 maps to the duplicate-known-field closure receipt. All test receipts require exactly one selected and passed test, reject zero-test output, and persist only normalized deterministic evidence.
- Closure keeps the post-recheck Plan 11 artifact/summary boundary separate from the Plan 07 summary closure. No obsolete product-fix count cap or special lifecycle-plan terminal assumption applies.
