---
card: 1
gate_id: config-schema
domain: repo-deliverable
tier: 1
gate_script_hash: ff07e3668fe1d0e63b49eb0f17cb2d46d75e598ae7f5ed8ed9828453952cdcd1
relational_target:
  artifact: the committed strict rules.toml probe corpus and vendored provider catalog pin
  relation: the shipped rules CLI rejects malformed or over-budget configuration and prints deny rows without downgrade
disclosure_default: opaque
checks:
  - { id: CF-01, category: execution, desc: valid baseline accepted, measures: public wayland-nano rules exits zero and echoes every rule row }
  - { id: CF-02, category: security, desc: unknown fields rejected, measures: extra top-level and per-rule keys both exit nonzero }
  - { id: CF-03, category: security, desc: type coercion rejected, measures: string boolean integer decision and scalar pattern all exit nonzero }
  - { id: CF-04, category: relation, desc: deny fidelity preserved, measures: every deny source row is printed as deny }
  - { id: CF-05, category: value, desc: command and token budgets enforced, measures: overlong command and over-token rules exit nonzero }
  - { id: CF-06, category: structure, desc: provider catalog pin exact, measures: exactly one named RECORDED_SHA256 equals normalized catalog bytes }
wrapped_tools:
  - { name: bash, version: 5, license: GPL-3.0-or-later, role: black-box orchestration only }
  - { name: wayland-nano, version: workspace, license: Apache-2.0, role: shipped rules parser authority }
validation:
  reference: sealed:dir-sha256:486ffb91d391b48e195a891b11927a3b2c9545f9ba172ec50c11ff875166a723
  pool_min: 5
  pool_status: full
  mutants:
    - id: cf-m1
      class: fluent-but-wrong
      why_fluent: removing a deny-unknown annotation looks like harmless serde cleanup while valid files remain green
      expected_drop: 1
      must_fail: [CF-02]
      fixture: sealed:dir-sha256:2ce9d8001016d9e7ee54fd5966f235eeb08d5ee070bba2b65313558fed6e4faa
    - id: cf-m2
      class: fluent-but-wrong
      why_fluent: accepting yes as a boolean reads like operator-friendly TOML ergonomics
      expected_drop: 1
      must_fail: [CF-03]
      fixture: sealed:dir-sha256:0bde366ad7497a11ffa3f39ed547a35c1fcfb1ab0f13ad73123eceb164894d99
    - id: cf-m3
      class: fluent-but-wrong
      why_fluent: suppressing deny rows reads like quieter output while rule loading stays successful
      expected_drop: 1
      must_fail: [CF-04]
      fixture: sealed:dir-sha256:25f7e6342f5b49b3bbaa4b20ed1fe25e16526db9d6e89f5e7a41e3c706cc6167
    - id: cf-m4
      class: fluent-but-wrong
      why_fluent: a larger command budget looks generous but silently changes the security boundary
      expected_drop: 1
      must_fail: [CF-05]
      fixture: sealed:dir-sha256:4240152c0203bcdb1fb66767343089dd49edb07b47f460d83f018456fb15323d
    - id: cf-m5
      class: fluent-but-wrong
      why_fluent: a plausible endpoint refresh can bypass review when its named catalog pin is stale
      expected_drop: 1
      must_fail: [CF-06]
      fixture: sealed:dir-sha256:b61bb72131d9e916830ed0d4e3f0b9692eef8965efe217e275b304ed623e09cf
    - id: cf-m6
      class: fluent-but-wrong
      why_fluent: degrading unknown decisions to prompt appears resilient while invalid policy is accepted
      expected_drop: 1
      must_fail: [CF-03]
      fixture: sealed:dir-sha256:5f4e792f1cd07f6afc9b43d0968b50773bb33d4b631d27dfe5a5eb6b223e0f51
  rotation_k: 2
  last_validated: ff07e3668fe1d0e63b49eb0f17cb2d46d75e598ae7f5ed8ed9828453952cdcd1
gamed_modes:
  - { mode: loosen parser while preserving valid happy paths, status: sealed, note: cf-m1 cf-m2 cf-m4 and cf-m6 bind exact committed patches to the parser anchors }
  - { mode: hide policy or catalog drift in plausible refactors, status: sealed, note: cf-m3 and cf-m5 exercise printed deny fidelity and the named catalog pin }
escape_hatch_bans:
  - { ban: duplicate TOML parsing inside the gate, check: CF-03 }
  - { ban: select an unnamed hex string as the catalog authority, check: CF-06 }
---

# Config schema gate

Runs the shipped `wayland-nano rules` command against a sealed probe corpus. Source
mutants are applied only to detached exact-base worktrees; the builder checkout and
producer sources are never modified.
