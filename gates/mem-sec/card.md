---
card: 1
gate_id: mem-sec
domain: repo-deliverable
tier: 1
gate_script_hash: ab689f69e6ae240fa32e767a391dc556cf7d8f2bfaf12b5955ffd63cc2437a4a
relational_target:
  artifact: the six sealed MEMORY-CONTRACT section 6.5 fixtures and nano-memory card harness
  relation: trust tiers project and agent partitions attribution and removed scopes remain fail closed through retrieval replay and rebuild
disclosure_default: opaque
checks:
  - { id: MS-01, category: security, desc: poisoned supersession rejected, measures: lower-tier F2 never supersedes User F1; F1 remains current and ranks above stored non-superseding F2 }
  - { id: MS-02, category: relation, desc: same-tier supersession boundary preserved, measures: exact adjusted-confidence tie keeps G1 and the next representable just-over-boundary ToolOutput row supersedes }
  - { id: MS-03, category: security, desc: cross-project retrieval partition enforced, measures: project B query returns zero project A rows at FTS KNN and assembled output }
  - { id: MS-04, category: security, desc: extraction tier and bot attribution preserved, measures: User H1 plus force-capped ModelInference extraction retain distinct tiers and bot-a identity through independent replay and rebuild; unreceipted model rows stay absent }
  - { id: MS-05, category: security, desc: removed scope and unconfigured agent refused, measures: Global unknown trust unknown agent scope unconfigured open and unconfigured write all fail typed }
  - { id: MS-06, category: security, desc: cross-bot retrieval partition enforced, measures: bot-b Own query returns zero bot-a rows at FTS KNN and assembled output }
wrapped_tools:
  - { name: node, version: 20, license: MIT, role: gate orchestration and sealed fixture verification }
  - { name: cargo, version: workspace, license: Apache-2.0 OR MIT, role: build and execute the Rust card harness }
  - { name: wayland-nano, version: workspace, license: Apache-2.0, role: WP-3 verify CLI entry authority }
validation:
  reference: sealed:dir-sha256:bf8e163151246640136e1e2d06436f5615d8dc5c37f086e1ac91a4211aa7e5ac
  pool_min: 30
  pool_status: full
  mutants:
    - { id: ms1-m1-tier-rank-inversion, class: fluent-but-wrong, why_fluent: letting a newer tool row replace a user row resembles ordinary last-write-wins conflict handling, expected_drop: 1, must_fail: [MS-01], fixture: sealed:dir-sha256:ef7270cce06a9d68afdf0bd46c3a74b1a5961645ca57717925eb6fbef907a056 }
    - { id: ms1-m2-timestamp-precedence, class: fluent-but-wrong, why_fluent: using valid_from recency as the primary resolver key looks deterministic, expected_drop: 1, must_fail: [MS-01], fixture: sealed:dir-sha256:ef7270cce06a9d68afdf0bd46c3a74b1a5961645ca57717925eb6fbef907a056 }
    - { id: ms1-m3-drop-losing-row, class: fluent-but-wrong, why_fluent: discarding the losing row appears like harmless compaction, expected_drop: 1, must_fail: [MS-01], fixture: sealed:dir-sha256:ef7270cce06a9d68afdf0bd46c3a74b1a5961645ca57717925eb6fbef907a056 }
    - { id: ms1-m4-equalize-tier-weight, class: fluent-but-wrong, why_fluent: assigning all trust tiers weight one looks like score simplification, expected_drop: 1, must_fail: [MS-01], fixture: sealed:dir-sha256:ef7270cce06a9d68afdf0bd46c3a74b1a5961645ca57717925eb6fbef907a056 }
    - { id: ms1-m5-cross-agent-conflict, class: fluent-but-wrong, why_fluent: omitting agent_id from the conflict key looks like index simplification, expected_drop: 1, must_fail: [MS-01], fixture: sealed:dir-sha256:ef7270cce06a9d68afdf0bd46c3a74b1a5961645ca57717925eb6fbef907a056 }
    - { id: ms2-m1-disable-same-tier, class: fluent-but-wrong, why_fluent: always keeping the first same-tier row appears maximally conservative, expected_drop: 1, must_fail: [MS-02], fixture: sealed:dir-sha256:3380243f29a738ebb6d3f952a723979476dc35c20200e720ce83d41a7a298071 }
    - { id: ms2-m2-invert-confidence-test, class: fluent-but-wrong, why_fluent: comparing existing confidence against adjusted new confidence looks algebraically plausible, expected_drop: 1, must_fail: [MS-02], fixture: sealed:dir-sha256:3380243f29a738ebb6d3f952a723979476dc35c20200e720ce83d41a7a298071 }
    - { id: ms2-m3-timestamp-only, class: fluent-but-wrong, why_fluent: using timestamp instead of confidence seems aligned with the newer-row seed, expected_drop: 1, must_fail: [MS-02], fixture: sealed:dir-sha256:3380243f29a738ebb6d3f952a723979476dc35c20200e720ce83d41a7a298071 }
    - { id: ms2-m4-tie-coexist, class: fluent-but-wrong, why_fluent: allowing every same-tier conflict to coexist preserves history, expected_drop: 1, must_fail: [MS-02], fixture: sealed:dir-sha256:3380243f29a738ebb6d3f952a723979476dc35c20200e720ce83d41a7a298071 }
    - { id: ms2-m5-wrong-conflict-domain, class: fluent-but-wrong, why_fluent: dropping predicate from the conflict key appears to reduce index complexity, expected_drop: 1, must_fail: [MS-02], fixture: sealed:dir-sha256:3380243f29a738ebb6d3f952a723979476dc35c20200e720ce83d41a7a298071 }
    - { id: ms3-m1-fts-post-filter, class: fluent-but-wrong, why_fluent: filtering project rows after BM25 looks equivalent at final output, expected_drop: 1, must_fail: [MS-03], fixture: sealed:dir-sha256:35d65abc24b3b67cc00634998570ac919524d895ab973ed8aec682f50dab8072 }
    - { id: ms3-m2-knn-post-filter, class: fluent-but-wrong, why_fluent: filtering vector rows after KNN looks equivalent at final output, expected_drop: 1, must_fail: [MS-03], fixture: sealed:dir-sha256:35d65abc24b3b67cc00634998570ac919524d895ab973ed8aec682f50dab8072 }
    - { id: ms3-m3-omit-project-fts, class: fluent-but-wrong, why_fluent: removing a redundant-looking FTS partition predicate simplifies SQL, expected_drop: 1, must_fail: [MS-03], fixture: sealed:dir-sha256:35d65abc24b3b67cc00634998570ac919524d895ab973ed8aec682f50dab8072 }
    - { id: ms3-m4-omit-project-knn, class: fluent-but-wrong, why_fluent: removing a redundant-looking vec partition predicate simplifies SQL, expected_drop: 1, must_fail: [MS-03], fixture: sealed:dir-sha256:35d65abc24b3b67cc00634998570ac919524d895ab973ed8aec682f50dab8072 }
    - { id: ms3-m5-final-only-assert, class: fluent-but-wrong, why_fluent: checking only assembled output appears sufficient, expected_drop: 1, must_fail: [MS-03], fixture: sealed:dir-sha256:35d65abc24b3b67cc00634998570ac919524d895ab973ed8aec682f50dab8072 }
    - { id: ms4-m1-inherit-tool-tier, class: fluent-but-wrong, why_fluent: preserving the source episode tier looks provenance-friendly, expected_drop: 1, must_fail: [MS-04], fixture: sealed:dir-sha256:10b595dfa60e93af08652820364dd5e69db0b1cb93bc254bb6f62b6836593797 }
    - { id: ms4-m2-rederive-tier-replay, class: fluent-but-wrong, why_fluent: recomputing trust during replay looks like normalization, expected_drop: 1, must_fail: [MS-04], fixture: sealed:dir-sha256:10b595dfa60e93af08652820364dd5e69db0b1cb93bc254bb6f62b6836593797 }
    - { id: ms4-m3-default-agent-rebuild, class: fluent-but-wrong, why_fluent: defaulting missing rebuild identity to main looks backward compatible, expected_drop: 1, must_fail: [MS-04], fixture: sealed:dir-sha256:10b595dfa60e93af08652820364dd5e69db0b1cb93bc254bb6f62b6836593797 }
    - { id: ms4-m4-receipt-agent-unbound, class: fluent-but-wrong, why_fluent: accepting a receipt from any agent looks resilient, expected_drop: 1, must_fail: [MS-04], fixture: sealed:dir-sha256:10b595dfa60e93af08652820364dd5e69db0b1cb93bc254bb6f62b6836593797 }
    - { id: ms4-m5-direct-model-write, class: fluent-but-wrong, why_fluent: letting direct ModelInference writes land avoids mediation overhead, expected_drop: 1, must_fail: [MS-04], fixture: sealed:dir-sha256:10b595dfa60e93af08652820364dd5e69db0b1cb93bc254bb6f62b6836593797 }
    - { id: ms5-m1-accept-global, class: fluent-but-wrong, why_fluent: retaining a legacy Global spelling looks migration-friendly, expected_drop: 1, must_fail: [MS-05], fixture: sealed:dir-sha256:98ae2f795ac76f8e5f5f59a2dbeef1eea6ef23c00db934052e13588ab507ccfd }
    - { id: ms5-m2-unknown-tier-default, class: fluent-but-wrong, why_fluent: defaulting unknown trust to ModelInference looks fail-safe, expected_drop: 1, must_fail: [MS-05], fixture: sealed:dir-sha256:98ae2f795ac76f8e5f5f59a2dbeef1eea6ef23c00db934052e13588ab507ccfd }
    - { id: ms5-m3-unknown-scope-own, class: fluent-but-wrong, why_fluent: defaulting unknown agent scope to Own looks narrow, expected_drop: 1, must_fail: [MS-05], fixture: sealed:dir-sha256:98ae2f795ac76f8e5f5f59a2dbeef1eea6ef23c00db934052e13588ab507ccfd }
    - { id: ms5-m4-open-unconfigured, class: fluent-but-wrong, why_fluent: allowing store open before registry validation looks like clean layering, expected_drop: 1, must_fail: [MS-05], fixture: sealed:dir-sha256:98ae2f795ac76f8e5f5f59a2dbeef1eea6ef23c00db934052e13588ab507ccfd }
    - { id: ms5-m5-write-unconfigured, class: fluent-but-wrong, why_fluent: validating grammar but not configuration looks sufficient, expected_drop: 1, must_fail: [MS-05], fixture: sealed:dir-sha256:98ae2f795ac76f8e5f5f59a2dbeef1eea6ef23c00db934052e13588ab507ccfd }
    - { id: ms6-m1-fts-agent-post-filter, class: fluent-but-wrong, why_fluent: filtering agent rows after BM25 looks equivalent at final output, expected_drop: 1, must_fail: [MS-06], fixture: sealed:dir-sha256:391fb955bf5e16487722c3a8bbffb429e092d2379ea345af7c5868a3ae22b848 }
    - { id: ms6-m2-knn-agent-post-filter, class: fluent-but-wrong, why_fluent: filtering agent rows after KNN looks equivalent at final output, expected_drop: 1, must_fail: [MS-06], fixture: sealed:dir-sha256:391fb955bf5e16487722c3a8bbffb429e092d2379ea345af7c5868a3ae22b848 }
    - { id: ms6-m3-own-means-project, class: fluent-but-wrong, why_fluent: interpreting Own as all project agents sounds collaborative, expected_drop: 1, must_fail: [MS-06], fixture: sealed:dir-sha256:391fb955bf5e16487722c3a8bbffb429e092d2379ea345af7c5868a3ae22b848 }
    - { id: ms6-m4-omit-agent-fts, class: fluent-but-wrong, why_fluent: project partitioning alone looks sufficient, expected_drop: 1, must_fail: [MS-06], fixture: sealed:dir-sha256:391fb955bf5e16487722c3a8bbffb429e092d2379ea345af7c5868a3ae22b848 }
    - { id: ms6-m5-omit-agent-knn, class: fluent-but-wrong, why_fluent: project partitioning alone looks sufficient, expected_drop: 1, must_fail: [MS-06], fixture: sealed:dir-sha256:391fb955bf5e16487722c3a8bbffb429e092d2379ea345af7c5868a3ae22b848 }
  rotation_k: 2
  last_validated: ab689f69e6ae240fa32e767a391dc556cf7d8f2bfaf12b5955ffd63cc2437a4a
gamed_modes:
  - { mode: post-filter project or agent rows after retrieval instead of partitioning inside SQL, status: sealed, note: MS-03 and MS-06 expose leaks at FTS KNN and final checkpoints }
  - { mode: rederive trust tier or identity during replay and rebuild, status: sealed, note: MS-04 binds User plus ModelInference tiers and bot-a bit-for-bit across independent stores }
  - { mode: accept a prebuilt harness using only a same-party hash sidecar, status: sealed, note: the compiled harness embeds store resolver and type source bytes and compares them to the runtime checkout before scoring }
  - { mode: pass ambient gate environment through to nested tools, status: sealed, note: gate.cjs forwards only the closed platform key allowlist }
  - { mode: silently coerce removed or unknown enum values and unconfigured agents, status: sealed, note: MS-05 requires typed parse open and write refusals }
escape_hatch_bans:
  - { ban: never replace pass-local partition assertions with final-output-only filtering, check: MS-03 }
  - { ban: never infer source trust or agent identity during replay or rebuild, check: MS-04 }
  - { ban: never default unknown scope trust or configured-agent inputs, check: MS-05 }
  - { ban: never activate explicit or cross-agent reads for this pack, check: MS-06 }
---

# Memory security gate

Implements the six MEMORY-CONTRACT v1.2 section 6.5 adversarial checks. Fixtures and
human-readable labels are sealed in the gates-first commit and are never tuned from
retrieval output.
