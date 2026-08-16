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

After deterministic implementation, serialized Plans 03-06, 03-10, 03-12, 03-13, and 03-11 write mandatory `03-AUDIT.json` schema `wp03_audit_v2`. Plan 06 performs exactly one audit and validates distinct builder/auditor identities, method/version/timestamp, exact baseline and audited commit/tree/path set, and a required findings array with unique IDs, closed severity/disposition, evidence, and command-backed `verified-no-fix`. Plans 10 and 12 form one continuous bounded fix round across all authorized product/golden/UPSTREAM surfaces; Plan 13 finalizes its zero-to-two ordered commits, exact trees/paths/findings/commands and pins the exact pre-fix and fix commit/tree/path set. Plan 11 uses an independent reviewer/method against those committed bytes and records successful commands/exits plus exactly one verdict for every finding and PASS evidence for every Critical/High finding. Absence, arbitrary minimal JSON, dirty product bytes, malformed fields, identity reuse, path mismatch, or incomplete coverage is fatal.

Before product mutation, Plan 01 creates executable `03-OWNERSHIP-PREFLIGHT.ps1` and tracked `03-CONTROL.json`, with durable OS-temp sorted `{path,type,sha256,bytes}` manifests for external nano/resources/shared. Every task invokes Check; final Plan 08 invokes Closure. Status parsing is NUL-delimited and covers tracked/untracked; exact OWNS, every ancestor reparse/link check, literal resolution, external Compare-Object, allowed shared deltas, and pair hashes fail closed with propagated errors.

For every `gen_error_table` generation/check, resolve the canonical sibling shared mirror absolutely and set `NANO_ERROR_TABLE_DESKTOP_DIR` to a unique absent OS-temp target; require it remains absent and restore the prior environment value. The final builder gate is complete `just gate-all`. The handoff records baseline, branch, commits, audit/fix/recheck, strict ownership/external hashes, generator isolation, live/canary evidence, and I/R/L distinctions. The builder does not merge, push, create integration state, run/claim CI, or treat an optional Desktop mirror as completion evidence.
