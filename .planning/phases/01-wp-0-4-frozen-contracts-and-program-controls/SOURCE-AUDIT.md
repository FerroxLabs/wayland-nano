# Phase 1 Multi-Source Coverage Audit

| SOURCE | ID | Feature / requirement | Plan | Status | Notes |
|---|---|---|---|---|---|
| GOAL | — | Four drift-protected contracts and evidence-backed serial promotion discipline; WP-0.1 owner-host-run | 01-01, 01-03 | COVERED | Generation, ownership, handoff, and non-self-approval are explicit. |
| REQ | CTRL-01 | Mandatory authority reads | 01-01 | COVERED | Context lists every implementation authority. |
| REQ | CTRL-02 | Exact OWNS boundary and deviation behavior | 01-01, 01-02, 01-03 | COVERED | Every task names allowed files and forbidden surfaces. |
| REQ | CTRL-03 | Current origin/master SHA and isolated canonical branch | 01-01 | COVERED | Existing worktree source fact is recorded; execution verifies it. |
| REQ | CTRL-04 | Audit, bounded fix, and full builder gate | 01-03 | COVERED | Dedicated serial quality task. |
| REQ | CTRL-05 | Detached no-ff integration, gate, push, CI green | 01-03 | COVERED | Handoff only; integrator executes after builder. |
| REQ | CTRL-06 | Canary-clean, path-only secret discipline | 01-01, 01-02, 01-03 | COVERED | No secret read; captures scanned. |
| REQ | CTRL-07 | Generator-only generated changes | 01-01 | COVERED | Generated JSON materialized by gen_contracts. |
| REQ | CTRL-08 | I/R/L and one-line evidence | 01-01, 01-03 | COVERED | Required in final handoff. |
| REQ | HOST-01 | WP-0.1 remains host-run | 01-01, 01-03 | COVERED | Explicit unexecuted owner handoff. |
| REQ | CTR-01 | Four canonical metadata-bearing JSON artifacts | 01-01, 01-02 | COVERED | Creation and independent validation. |
| REQ | CTR-02 | Deterministic source/corpus derivation | 01-01 | COVERED | Generator and exhaustive Op guard. |
| REQ | CTR-03 | Six endpoints and fixture validation | 01-01, 01-02 | COVERED | Frozen evidence plus reachability checks. |
| REQ | CTR-04 | Both stale/tamper tripwires | 01-02, 01-03 | COVERED | Generator check and protocol test. |
| REQ | CTR-05 | Explicit additive crate ownership | 01-01, 01-02 | COVERED | op.rs and contracts.rs scopes are narrow. |
| REQ | CTR-06 | G-CTR-1 ready, never self-approved | 01-01, 01-03 | COVERED | Catalog stays untouched. |
| RESEARCH | — | Research disabled; approved spec is implementation authority | — | EXCLUDED | No RESEARCH.md requirements exist. |
| CONTEXT | — | No CONTEXT.md decisions supplied | — | EXCLUDED | User/spec decisions are represented through GOAL/REQ authority. |

No deferred item, WP-0.1 implementation, later WP feature, catalog edit, error-table change, `nano/` write, or `resources/upstreams/` write appears in the plan set.
