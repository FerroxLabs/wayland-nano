# Phase 4: WP-1 Gate and Receipt Foundation - Context

**Gathered:** 2026-08-17
**Status:** Ready for planning
**Source:** PRD Express Path (`shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md`)

<domain>
## Phase Boundary

Deliver only the WP-1 foundation of `crates/nano-verify`: canonical registry and gate execution, fail-closed output parsing, standalone schema-1 red-green receipts, atomic receipt storage, and read-only receipt preflight. WP-2 climb/engine behavior, WP-3 CLI/final receipt rerun, WP-4 Gate Cards, WP-5, and WP-6 are outside this phase.

</domain>

<decisions>
## Implementation Decisions

### Gate execution and parsing
- Implement `nano-verify` gate invocation as argv-only process execution; never construct a shell string.
- Spawn with `env_clear`, the exact platform baseline allowlist from IFACE §3, and declared closure environment taking precedence.
- Bound stdout at 16 MiB, enforce the invocation timeout, and terminate the complete process tree on timeout.
- Treat exit status as non-authoritative; parse the last valid gate summary and reconstruct every verdict from the Gate Card inventory.
- Fail closed on empty/missing output, empty inventory, timeout, spawn failure, unknown check IDs, and inconsistent summary totals.
- Expose canonical scores and opaque `<ID> <category>` failures without exposing gate source, commands, expectations, fixtures, or ambient secrets.

### Registry and gate identity
- Copy shared interface types verbatim where the spec says `IMPORT IFACE`; the interface contract wins over secondary prose.
- Load only schema-1 registries with unknown fields denied, repo-confined paths, valid direct-or-pinned-interpreter script shape, complete requirement mappings, and recomputed canonical closure digests.
- Use canonical JSON exactly as specified: UTF-8, lexicographically ordered object keys, no whitespace/newline, integers only, NFC strings, lowercase SHA-256.

### Receipts and preflight
- Persist one standalone schema-1 canonical JSON receipt per requirement, including red evidence, observed commit, fix commit, gate identity/pin, mint time, and producer.
- Require genuinely red evidence: nonzero exit, nonempty log digest, and existing observed commit; malformed or green-only evidence never advances.
- Implement bounded `create_new` writer locking, same-directory tempfile + fsync, `MoveFileExW(...REPLACE_EXISTING)` on Windows, atomic rename plus directory fsync on Unix, and no non-atomic fallback.
- Readers retry a mid-parse failure once after 100 ms and then fail closed as corruption/unverifiable.
- WP-1 preflight verifies schema/red evidence, commit existence, red-before-green ancestry, test existence, registry mapping, and gate pin, but returns only `ReceiptPreflight::Ready`; it must never claim final `VerifyVerdict::Valid`.

### Tests, ownership, and promotion
- Materialize git fixture repositories during tests; do not commit nested `.git` directories.
- Cover the complete named parser, subprocess, registry, receipt-preflight, ancestry/test-path, contention/retry, and corruption battery.
- Own only `crates/nano-verify/**`, required root workspace wiring, and exact donor transformation entries in `UPSTREAM.md`; preserve unrelated surfaces.
- Run one Critical/High audit, at most one fix round, named WP-1 tests, dependency review, `cargo deny check`, and full `just gate-all` before detached integration.

### the agent's Discretion
- Internal module-private helpers, error wording that does not alter canonical external outcomes, and test organization within the locked file ownership boundary.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Program and phase contract
- `shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md` — binding work-package order, ownership, audit, integration, and promotion rules.
- `.planning/ROADMAP.md` — Phase 4 goal, success criteria, dependencies, and promotion gate.
- `.planning/REQUIREMENTS.md` — GATE-01..03, RCPT-01..04, and PROV-01 traceability.

### Authoritative implementation contracts
- `shared/reviews/research-0.2/specs/SPEC-WP12-nano-verify-engine.md` — WP-1 module order, gate/receipt algorithms, fixtures, and named test battery.
- `shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` — authoritative shared shapes, canonical JSON, registry, gate protocol, receipt/preflight, and atomic-store decisions.
- `AGENTS.md` — repository security, filesystem, testing, provenance, and promotion constraints.
- `UPSTREAM.md` — required donor transformation ledger format.

</canonical_refs>

<specifics>
## Specific Ideas

- Build in the spec order: error/registry, pure parser, subprocess runner and gate fixtures, receipts and git fixtures; do not begin WP-2 climb/engine modules.
- Starting branch/worktree is `feat/wp-1` at `db0b678dc13e9486f9328808854598a0c5ba8725` in `.tmp-wt-vc-wp-1`.
- All temporary/build paths must remain on `F:`: `TEMP`/`TMP=F:\Temp\Codex`, `CARGO_TARGET_DIR=F:\CargoTarget\wayland-nano`.

</specifics>

<deferred>
## Deferred Ideas

- WP-2 gated climb and injected Effects driver.
- WP-3 CLI, detached fix-commit worktree, bounded gate rerun, final `Valid` verdict, and CI consumer.
- WP-4 Gate Card authoring and dogfood packs.
- WP-5/WP-6 are explicitly forbidden in this program run.

</deferred>

---

*Phase: 04-wp-1-gate-and-receipt-foundation*
*Context gathered: 2026-08-17 via PRD Express Path*
