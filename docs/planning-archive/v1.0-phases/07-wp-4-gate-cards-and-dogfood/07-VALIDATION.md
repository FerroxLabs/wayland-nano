---
phase: 7
slug: wp-4-gate-cards-and-dogfood
status: planned
nyquist_compliant: true
wave_0_complete: false
created: 2026-08-21
---

# Phase 7 — Validation Strategy

## Test Infrastructure

| Property | Value |
|----------|-------|
| Framework | Node `node:test` plus Rust workspace gates |
| Config | none; tests under `gates/tests/` |
| Quick | `node --test gates/tests/` |
| Full | WP-4 seeded/dogfood loops plus `just gate-all` |

## Sampling Rate

- After each task: focused card/reference/mutant tests named by the task.
- After each wave: complete `node --test gates/tests/`.
- Before verification: exhaustive mutants, three recorded seeded runs, good/bad WP-3 run-only loops, cleanup/canary/provenance, and `just gate-all`.
- Integrator: repeat integration gate/dogfood, dedicated `.github` promotion commit, push/fetch, exact-SHA six-job success.

## Required Named Battery

- `t-card-schema-valid`, `t-registry-closure-digests`
- `t-ip-reference-scores-mm`, `t-pv-reference-scores-mm`, `t-cf-reference-scores-mm`
- `t-ip-mutants-caught`, `t-pv-mutants-caught`, `t-cf-mutants-caught`
- `t-fixture-digest-fails-closed`, `t-dirhash-canonical`
- `t-meta-mutant-passing-is-gate-defect`, `t-summary-contract`
- `t-gate-hash-drift-voids-validation`

Every name must be discovered exactly once and pass; zero-test filters are failures.

## Per-Requirement Verification Map

| Requirement | Automated evidence |
|-------------|--------------------|
| CARD-01 | registry/closure digest tests and WP-3 run-only loop |
| CARD-02 | closed card schema and hash-drift tests |
| CARD-03 | ≥5 mutants per pack, exhaustive catch, three seeded runs |
| CARD-04 | M/M references and all fail-closed meta-tests |
| CARD-05 | install payload reference/mutants and good/bad verifier loop |
| CARD-06 | provision packet reference/mutants plus Windows live no-mutation oracle |
| CARD-07 | config reference/mutants and exact worktree cleanup proof |
| CARD-08 | good all-three plus ip-m1/pv-m2/cf-m3 bad trees through WP-3 only |
| PROV-03 | owned-file inventory against exact UPSTREAM rows |
| EVID-01..03 | promotion manifest, canary scan, external final evidence, terminal stop |

## Wave 0 Requirements

- [ ] `gates/lib/{card,contract,dirhash}.cjs`
- [ ] Three cards/gates/generators with sealed references and mutant pools
- [ ] Populated schema-1 registry and `docs/verify/gates.md`
- [ ] Four Node test files and deterministic seeded runner

The `.github` dogfood job is intentionally not a builder Wave 0 file; the integrator promotes it only after mutant evidence is green.

## Manual/Integrator Verifications

- Windows provision `--live` before/after oracle where the required host capability is available.
- Dedicated integrator `.github` promotion commit after all pack evidence.
- Final `F:/Development/waylandnano/shared/reviews/verified-change/WP4-FINAL-EVIDENCE.md` after exact-SHA CI.

## Validation Sign-Off

- [x] Every task has fail-fast automated verification specified.
- [x] Exact named battery and no-zero oracle are assigned.
- [x] Exhaustive and seeded mutation evidence are independently assigned.
- [x] Mutant worktree/target residue assertions are assigned.
- [x] Good/bad dogfood is constrained to the WP-3 CLI.
- [x] `just gate-all`, canary, ownership, and provenance gates are assigned.
- [x] `nyquist_compliant: true` reflects complete plan coverage; execution remains pending.

## Canonical Artifact Writer Coverage

- Wave 1 creates and failure-tests `gates/lib/artifact-writer.cjs` plus governed `atomic-replace-win32.ps1`: bounded `create_new` lock contention, same-directory tempfile, complete flush/fsync, exact `MoveFileExW(...MOVEFILE_REPLACE_EXISTING)` on Windows with no fallback, Unix rename and parent-directory sync, deterministic errors, and unconditional cleanup. Its read API retries exactly once after 100 ms then fails corruption/Unverifiable.
- Install, provision, and config generators route every fixture, seal, and card update through it.
- Registry, seeded validation, dogfood, audit/recheck, builder evidence, promotion request, and external final evidence route every persisted byte through the same importable API or stdin CLI.
- Contention, crash/stale-lock recovery, replacement failure, sync failure, preservation of prior bytes, and residue are mandatory Wave 1 tests.
- Existing-target continuous visibility and transient/second read-failure behavior are mandatory Wave 1 tests; CI/canary scanner outputs are classified as ephemeral scratch and deleted after validated digests/outcomes are retained.
- Integrator promotion modifies only the existing `.github/workflows/gate.yml` with top-level Windows `gate-cards`; the docs-owned consumer is never copied into `.github/`.

Approval: plan-complete; execution evidence pending
