# Phase 3 continuity modes report

Evidence class: **receipt**; seeded repetitions: **2**; frozen budget SHA-256: `01c267c0d14cbcce7a97c2db9ca6d33bd149685fd4d18495b903d56f7a8b2fbe`; harness SHA-256: `1a9b877064ae235358a2817f554dd8a969222173840c6c732f0a1ab1068f8a46`.

This is a measurement report, not a merge gate. Desktop remains the authority that selects defaults.

## Measured results

| mode | median turn latency (ms) | median total tokens | median quality | budget verdict |
|---|---:|---:|---:|---|
| fresh | 46.685 / ≤250 | 8000 / ≤12000 | 1.000 / ≥0.9 | PASS |
| session_resume | 47.087 / ≤350 | 8000 / ≤12000 | 1.000 / ≥0.9 | PASS |
| memory_recall | 47.780 / ≤350 | 8000 / ≤12000 | 1.000 / ≥0.9 | PASS |

Typed resume-drift refusals: **8/8** (`resume_drift`, zero silent fallbacks).

Quality is the fixed fixture battery result: the relevant labeled row was durably seeded in the admitted `(project, agent_id)` partition, the real ACP `memory_recall` tool completed with a nonempty digest, and the deterministic fake model emitted the fixture-derived answer. ACP intentionally exposes only the tool-result digest, so this harness measures continuity plumbing and cost; it does not claim semantic model reasoning quality. The independent mem-sec and recall fixtures own content-level retrieval correctness.

## Budget verdicts

- **fresh: PASS** — latency pass, tokens pass, quality pass.
- **session_resume: PASS** — latency pass, tokens pass, quality pass.
- **memory_recall: PASS** — latency pass, tokens pass, quality pass.

## RECOMMENDATION

For interactive ACP, default to **session_resume when a valid bound session exists**, otherwise **fresh**. Session resume preserved its fork/load substrate and rejected 8/8 drift probes without fallback; its measured quality was 1.000 at 8000 tokens.

For one-shot exec, default to **fresh**. It achieved the same measured quality with no resume dependency. Keep **memory_recall opt-in** until a semantic model evaluation can distinguish it: memory_recall did NOT beat session_resume on measured quality per token (1.250e-4 vs 1.250e-4).

These are recommendations from the measured fake-model chassis. Desktop selects and owns the actual defaults.

## Run manifests

| seed | binary sha256 | budget sha256 | harness sha256 | manifest | NDJSON |
|---:|---|---|---|---|---|
| 1010 | `376644e63782422e9bf3f4143095efe6880e070c97a70537982e0827445905e9` | `01c267c0d14cbcce7a97c2db9ca6d33bd149685fd4d18495b903d56f7a8b2fbe` | `1a9b877064ae235358a2817f554dd8a969222173840c6c732f0a1ab1068f8a46` | `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034263Z-receipt-1010-40432/continuity-manifest.json` | `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034263Z-receipt-1010-40432/continuity.ndjson` |
| 2020 | `376644e63782422e9bf3f4143095efe6880e070c97a70537982e0827445905e9` | `01c267c0d14cbcce7a97c2db9ca6d33bd149685fd4d18495b903d56f7a8b2fbe` | `1a9b877064ae235358a2817f554dd8a969222173840c6c732f0a1ab1068f8a46` | `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034100Z-receipt-2020-41204/continuity-manifest.json` | `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034100Z-receipt-2020-41204/continuity.ndjson` |

## Desktop consumption

Desktop may consume this report as decision input. This plan does not modify Desktop configuration or establish a default-setting surface; if that surface is absent, its ownership remains an owner follow-up.
