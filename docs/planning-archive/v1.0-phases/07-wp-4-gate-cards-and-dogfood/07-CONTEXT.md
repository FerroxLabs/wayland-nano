---
phase: 7
name: WP-4 Gate Cards and Dogfood
source: external-authority
status: locked
---

# Phase 7 Context

## Source of Truth

The authoritative implementation contract is
`F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP4-gatecards-dogfood.md`,
with imported contracts governed by sibling `SPEC-WP-INTERFACES.md` and the landed WP-3
verifier surface. Plans must use the promoted verifier rather than duplicate it.

## Decisions

- D-01: Implement only the three WP-4 Gate Card packs, their sealed fixtures/mutants,
  registry population, dogfood proof, owned documentation/provenance, and final program evidence.
- D-02: Preserve exact WP-4 ownership. Producer sources being verified remain read-only;
  `.github/**` promotion is integrator-owned after all WP-4 mutant evidence is green.
- D-03: Every pack must have closed inventory/categories/pins, canonical closure and fixture
  digests, at least five fluent-but-wrong mutants, seeded repeatability, and fail-closed meta-tests.
- D-04: Start at exact green master `05637086c81e88550edb002a916a80aff4b278dc`
  in F-only `.tmp-wt-vc-wp-4` on `feat/wp-4`; builder does not merge or push.
- D-05: Promotion remains one Critical/High audit, at most one fix round, local full gate,
  detached no-ff integration, push/fetch proof, exact-SHA six-leg CI, final canary-clean evidence,
  then autonomous work stops. WP-0.1, WP-5, WP-6 and all expansion programs remain unexecuted.

## Scope Fence

Do not modify packaging, provisioning, config/catalog producer sources. Do not implement WP-5/6,
DeepSeek, memory, profiles, MCP, or external-agent capabilities. Any verifier defect is routed to
its owner rather than shimmed into Gate Cards.
