---
phase: 6
name: WP-3 Verify CLI and CI Surface
source: external-authority
status: locked
---

# Phase 6 Context

## Source of Truth

The authoritative implementation contract is
`F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP3-verify-cli-ci.md`,
with imported interfaces governed by sibling `SPEC-WP-INTERFACES.md`. Where they differ,
the interface contract wins. Plans must verify the current landed WP-2 API rather than
redeclare or shim it.

## Decisions

- D-01: Implement only WP-3: CLI mint, run-only, offline receipt verification, JSONL v1,
  the empty schema-1 registry bootstrap, owned fixtures/docs/CI consumer, and provenance.
- D-02: Preserve the exact ownership fences in the WP-3 spec. In particular, WP-3 does not
  edit `crates/nano-verify/**`, populate production Gate Cards, or promote `.github/**`.
- D-03: Use the exact CLI modes, exit codes, trust boundaries, detached-worktree verification,
  materializer rules, model/deadline inputs, event vocabulary, and 13 named tests from the
  authoritative WP-3 spec.
- D-04: Start from exact green master `d7f4d3a2260f6d08e026fcb1263448355a7f175b` in the
  F-only worktree `.tmp-wt-vc-wp-3` on `feat/wp-3`; builder does not merge or push.
- D-05: Promotion remains one Critical/High audit, at most one consolidated fix round,
  full local gate, detached no-ff integration, exact-SHA six-leg CI, then WP-4. No WP-5,
  WP-6, DeepSeek, profile, memory, MCP, or external-agent expansion is authorized.

## Scope Fence

All functionality, fixtures, documentation, and provenance must remain within the precise
WP-3-owned paths listed in the authoritative spec and Phase 6 roadmap. Any missing imported
WP-2 surface is a blocking upstream mismatch, not permission to edit nano-verify.
