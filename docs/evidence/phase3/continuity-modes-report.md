# Phase 3 continuity modes report

Evidence class: **receipt**; seeded repetitions: **2**; budgets registered **2026-09-04T22:54:18Z** before receipt execution; frozen budget SHA-256: `a394961e053bce02b59f5d0a08adad854b1307817e4f7f931e99900e5332ba6d`; harness SHA-256: `df19ce9000ba0dcc8aad94113722123aa6201ff4518e2f5449d2147677ccb0f9`.

This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.

## Measured results

| mode | median turn latency (ms) | median total tokens | median quality | budget verdict |
|---|---:|---:|---:|---|
| fresh | 29.909 / ≤250 | 5104.50 / ≤8000 | 0.000 / ≥0 | PASS |
| session_resume | 23.352 / ≤350 | 10204 / ≤16000 | 1.000 / ≥0.9 | PASS |
| memory_recall | 31.263 / ≤350 | 11297 / ≤24000 | 0.950 / ≥0.9 | PASS |

Typed resume-drift refusals: **8/8** (`resume_drift`, zero silent fallbacks).

Quality is causal request evidence from the fixed fixture battery. Fresh creates a new admitted session with memory disabled and the soak model proves the fixture answer is absent. Session resume forks an activated parent, loads the returned child id, and emits success only when the actual model request contains the replayed answer. Memory recall exposes no explicit memory tool call: its model emits success only when automatic scoped retrieval placed the fixture answer in the actual request. Missing or irrelevant retrieval therefore becomes a typed model-protocol failure and a failed quality row. Token totals come only from emitted `_wayland/session/budget` notifications.

## Budget verdicts

- **fresh: PASS** — latency pass, tokens pass, quality pass.
- **session_resume: PASS** — latency pass, tokens pass, quality pass.
- **memory_recall: PASS** — latency pass, tokens pass, quality pass.

## RECOMMENDATION

For interactive ACP, default to **session_resume when a valid bound session exists**. It loaded the returned fork child, rejected 8/8 drift probes without fallback, and measured 1.000 quality at 10204 emitted tokens. With no resumable session, use **memory_recall only when project continuity is requested**; otherwise start fresh.

For one-shot exec, default to **fresh for stateless work** and require an explicit continuity choice for memory-backed work. Fresh correctly exposed no remembered answer; memory_recall measured 0.950 quality. Memory recall did NOT beat session_resume on measured quality per emitted token (8.409e-5 vs 9.800e-5), so it remains an explicit continuity mode rather than a universal default.

These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.

## Run manifests

| seed | binary sha256 | budget sha256 | harness sha256 | fixture sha256 | manifest sha256 | NDJSON sha256 |
|---:|---|---|---|---|---|---|
| 1010 | `148c138bacf121913f60551f2127f186ada28a5f2a7216e981e8da7340678b7d` | `a394961e053bce02b59f5d0a08adad854b1307817e4f7f931e99900e5332ba6d` | `df19ce9000ba0dcc8aad94113722123aa6201ff4518e2f5449d2147677ccb0f9` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `ab28e4c94bc5492dc7a17c93e52d782fe85e4e1034ad1997552d86f2bec78352` | `9e4fde7de5c7a0cf8ed30c4b50ace173467e3dcdcd63c0e19764aa677bdbee7e` |
| 2020 | `148c138bacf121913f60551f2127f186ada28a5f2a7216e981e8da7340678b7d` | `a394961e053bce02b59f5d0a08adad854b1307817e4f7f931e99900e5332ba6d` | `df19ce9000ba0dcc8aad94113722123aa6201ff4518e2f5449d2147677ccb0f9` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `e8a0ec9d367fe9cb41e29758cf8fd6fdf4b97533d0bf56c8df3e2fe637e8da50` | `03a990e4519a64d12d0daad9252c1303f7e15b2ffc2c32224596a71beb98a062` |

- Seed 1010 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225707234Z-receipt-1010-36100/continuity-manifest.json`
- Seed 1010 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225707234Z-receipt-1010-36100/continuity.ndjson`
- Seed 2020 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225706996Z-receipt-2020-40936/continuity-manifest.json`
- Seed 2020 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225706996Z-receipt-2020-40936/continuity.ndjson`

## Desktop consumption

Desktop may consume this report as decision input. This plan does not modify Desktop configuration or establish a default-setting surface; if that surface is absent, its ownership remains an owner follow-up.
