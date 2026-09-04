# Phase 3 continuity modes report

Evidence class: **receipt**; seeded repetitions: **2**; budgets registered **2026-09-04T23:23:48Z** before receipt execution; frozen budget SHA-256: `00800d1a945985d296c3d1b60b33f221c2acd566146d456fbdb8292593aa11f3`; harness SHA-256: `3417673546e6ddafb81d24354385320b59e0cb8bdd0dee228c17cd652e6666f9`.

This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.

## Measured results

| mode | median turn latency (ms) | setup tokens | probe tokens | median quality | budget verdict |
|---|---:|---:|---:|---:|---|
| fresh | 25.876 / ≤250 | 0 | 5136 / ≤8000 | 0.000 / ≥0 | PASS |
| session_resume | 21.814 / ≤350 | 5152.50 | 5136 / ≤8000 | 1.000 / ≥0.9 | PASS |
| memory_recall | 31.434 / ≤350 | 0 | 11303 / ≤16000 | 0.950 / ≥0.9 | PASS |

Typed resume-drift refusals: **8/8** (`resume_drift`, zero silent fallbacks).

Quality is causal request evidence from the fixed fixture battery. Fresh creates a new admitted session with memory disabled and the soak model proves the fixture answer is absent. Session resume forks an activated parent, loads the returned child id, and emits success only when the actual model request contains the replayed answer. Memory recall exposes no explicit memory tool call: its model emits success only when automatic scoped retrieval placed the fixture answer in the actual request. Missing or irrelevant retrieval therefore becomes a typed model-protocol failure and a failed quality row. Token totals come only from emitted `_wayland/session/budget` notifications.

## Budget verdicts

- **fresh: PASS** — latency pass, tokens pass, quality pass.
- **session_resume: PASS** — latency pass, tokens pass, quality pass.
- **memory_recall: PASS** — latency pass, tokens pass, quality pass.

## RECOMMENDATION

For interactive ACP, default to **session_resume when a valid bound session exists**. It loaded the returned fork child, rejected 8/8 drift probes without fallback, and measured 1.000 quality at 5136 probe tokens after a separately reported 5152.50-token resumed-history baseline. With no resumable session, use **memory_recall only when project continuity is requested**; otherwise start fresh.

For one-shot exec, default to **fresh for stateless work** and require an explicit continuity choice for memory-backed work. Fresh correctly exposed no remembered answer; memory_recall measured 0.950 quality. Memory recall did NOT beat session_resume on measured quality per emitted token (8.405e-5 vs 1.947e-4), so it remains an explicit continuity mode rather than a universal default.

These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.

## Run manifests

| seed | binary sha256 | budget sha256 | harness sha256 | fixture sha256 | manifest sha256 | NDJSON sha256 |
|---:|---|---|---|---|---|---|
| 1010 | `fba0a81b552da7904e1a713e3bf9cbe6da5f88bd0debcc0ba984ef5c8b685933` | `00800d1a945985d296c3d1b60b33f221c2acd566146d456fbdb8292593aa11f3` | `3417673546e6ddafb81d24354385320b59e0cb8bdd0dee228c17cd652e6666f9` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `1fd7f201bc0d9972ee4296cb59f699b2dce1ca79d8466cb4e01ce13a0445eede` | `975d1d82bd564950a9c50686955bef594255c1cf91544cf5bf15431cd7f87bdb` |
| 2020 | `fba0a81b552da7904e1a713e3bf9cbe6da5f88bd0debcc0ba984ef5c8b685933` | `00800d1a945985d296c3d1b60b33f221c2acd566146d456fbdb8292593aa11f3` | `3417673546e6ddafb81d24354385320b59e0cb8bdd0dee228c17cd652e6666f9` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `46fe0c267c822656c50717b28d8f2c46632ed70082e90ec28d7def76b9639574` | `1bcdb8fd9272a95c46d71a9d3be604b640bb55fabc103ee84ee48a67e5d94bf7` |

- Seed 1010 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T232852661Z-receipt-1010-41300/continuity-manifest.json`
- Seed 1010 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T232852661Z-receipt-1010-41300/continuity.ndjson`
- Seed 2020 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T232852444Z-receipt-2020-39548/continuity-manifest.json`
- Seed 2020 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T232852444Z-receipt-2020-39548/continuity.ndjson`

## Desktop consumption

Desktop may consume this report as decision input. This plan does not modify Desktop configuration or establish a default-setting surface; if that surface is absent, its ownership remains an owner follow-up.
