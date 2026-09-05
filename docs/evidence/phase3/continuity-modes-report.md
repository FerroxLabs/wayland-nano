# Phase 3 continuity modes report

Evidence class: **receipt**; seeded repetitions: **2**; budgets registered **2026-09-05T00:05:52Z** before receipt execution; frozen budget SHA-256: `59e7924bebd93fd2ef3e9a65a4c0cb8177c382bc3484d0e6f8ad5fdabf8ff320`; harness SHA-256: `40fc1531154586fd0d2fdafe9791d9998bfe0cc0804a6a8508fce8f118610ad0`.

This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.

## Measured results

| mode | median turn latency (ms) | setup tokens | probe tokens | total tokens | median quality | budget verdict |
|---|---:|---:|---:|---:|---:|---|
| fresh | 63.012 / ≤250 | 0 | 5136 / ≤8000 | 5136 / ≤8000 | 0.000 / ≥0 | PASS |
| session_resume | 60.009 / ≤350 | 5152.50 | 5136 / ≤8000 | 10288.50 / ≤16000 | 1.000 / ≥0.9 | PASS |
| memory_recall | 31.614 / ≤350 | 16351 | 11303 / ≤16000 | 27654 / ≤40000 | 0.950 / ≥0.9 | PASS |

Typed resume-drift refusals: **8/8** (`resume_drift`, zero silent fallbacks).
Fresh isolation assertions: **40/40**; any leakage or protocol refusal invalidates the entire run before a manifest is selectable.

Quality is causal request evidence from the fixed fixture battery. Fresh creates a new admitted session with memory disabled and separately proves the fixture answer is absent; that isolation oracle must pass even though fresh continuity quality is zero. Session resume forks an activated parent, loads the returned child id, and emits success only when the actual model request contains the replayed answer. Memory recall exposes no explicit memory tool call: its model emits success only when automatic scoped retrieval placed the fixture answer in the actual request. Missing or irrelevant retrieval therefore becomes a typed model-protocol failure and a failed quality row. Token totals come only from emitted `_wayland/session/budget` notifications. Setup is attributed once on the first probe row of each `(mode, project, agent_id)` partition, including all four memory-seed sessions; every row and manifest conserves `total = setup + probe`.

## Budget verdicts

- **fresh: PASS** — latency pass, probe tokens pass, total tokens pass, quality pass.
- **session_resume: PASS** — latency pass, probe tokens pass, total tokens pass, quality pass.
- **memory_recall: PASS** — latency pass, probe tokens pass, total tokens pass, quality pass.

## RECOMMENDATION

For interactive ACP, default to **session_resume when a valid bound session exists**. It loaded the returned fork child, rejected 8/8 drift probes without fallback, and measured 1.000 quality at 5136 probe tokens plus 5152.50 setup tokens (10288.50 total). With no resumable session, use **memory_recall only when project continuity is requested**; otherwise start fresh.

For one-shot exec, default to **fresh for stateless work** and require an explicit continuity choice for memory-backed work. Fresh correctly exposed no remembered answer; memory_recall measured 0.950 quality. Memory recall did NOT beat session_resume on measured quality per emitted token (8.405e-5 vs 1.947e-4), so it remains an explicit continuity mode rather than a universal default.

These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.

## Run manifests

| seed | binary sha256 | budget sha256 | harness sha256 | fixture sha256 | manifest sha256 | NDJSON sha256 |
|---:|---|---|---|---|---|---|
| 1010 | `fba0a81b552da7904e1a713e3bf9cbe6da5f88bd0debcc0ba984ef5c8b685933` | `59e7924bebd93fd2ef3e9a65a4c0cb8177c382bc3484d0e6f8ad5fdabf8ff320` | `40fc1531154586fd0d2fdafe9791d9998bfe0cc0804a6a8508fce8f118610ad0` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `0c507e4bdd47108dcdd0c51eecce44a6142b0cf7c413967e10db6ca874c4b6b7` | `e94d95df8f19d660b2bb284cda902c67c1397645e6edf9db778dbc3a020f24f7` |
| 2020 | `fba0a81b552da7904e1a713e3bf9cbe6da5f88bd0debcc0ba984ef5c8b685933` | `59e7924bebd93fd2ef3e9a65a4c0cb8177c382bc3484d0e6f8ad5fdabf8ff320` | `40fc1531154586fd0d2fdafe9791d9998bfe0cc0804a6a8508fce8f118610ad0` | `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214` | `d51940957310c4d502f0e9236fa8f0659394445ff3348b156410acd08da5fbcd` | `1eb7c12a31d5c9f0891d9241e1ae79d6f91061825adbeb3df2be25fc319b9daa` |

- Seed 1010 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000637916Z-receipt-1010-8916/continuity-manifest.json`
- Seed 1010 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000637916Z-receipt-1010-8916/continuity.ndjson`
- Seed 2020 manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000638217Z-receipt-2020-16516/continuity-manifest.json`
- Seed 2020 NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000638217Z-receipt-2020-16516/continuity.ndjson`

## Desktop consumption

Desktop may consume this report as decision input. This plan does not modify Desktop configuration or establish a default-setting surface; if that surface is absent, its ownership remains an owner follow-up.
