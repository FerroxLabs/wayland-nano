# Provision Script Gate Card

Packet validation is portable. The live arm is an explicit non-elevated refusal and external-state equality oracle.

---
card: 1
gate_id: provision-script
domain: repo-deliverable
tier: 1
gate_script_hash: 6b8e7b1b003a6f10da3ad859e5f0f8ab7478a3694db7aee43c4860e66c23b18b
relational_target:
  artifact: marker-framed provisioning payload
  relation: dry-run bytes preserve versioned identity idempotence and no-mutation invariants
disclosure_default: opaque
checks:
  - { id: PV-01, category: structure, desc: exact payload keys and provision mode, measures: closed key set }
  - { id: PV-02, category: security, desc: Wayland Nano sandbox identities only, measures: exact namespace and donor leak scan }
  - { id: PV-03, category: relation, desc: derived operations are unique and idempotent, measures: duplicate-free operation keys }
  - { id: PV-04, category: security, desc: cancellation is confined or live refusal preserves state, measures: path confinement or before-after digest }
  - { id: PV-05, category: value, desc: setup protocol version floor holds, measures: integer version at least five }
  - { id: PV-06, category: relation, desc: created and uninstall sets remain exact, measures: owned identities and wildcard ban }
wrapped_tools:
  - { name: node, version: 20, license: MIT, role: packet gate runner }
validation:
  reference: sealed:dir-sha256:11ac199f0b16ca2c2b93c0cad496253fafe2112ae113d7ace9fdd3ec52d56508
  pool_min: 6
  pool_status: full
  mutants:
    - { id: pv-m1, class: fluent-but-wrong, why_fluent: blank identity deferred, expected_drop: 1, must_fail: [PV-02], fixture: sealed:dir-sha256:5a011be7eb50fef3d4dd2048aaf2f18e8095f79e0b4df0b7fd94a21dcae65b52 }
    - { id: pv-m2, class: fluent-but-wrong, why_fluent: legacy donor identities retained, expected_drop: 1, must_fail: [PV-02], fixture: sealed:dir-sha256:74c5dc313b9eacb84ac242593afe00b21080ff6dc34449b9bc52c09c63cfb531 }
    - { id: pv-m3, class: fluent-but-wrong, why_fluent: normalized duplicate root retained, expected_drop: 1, must_fail: [PV-03], fixture: sealed:dir-sha256:e5eaa6e4d41c14252d7427c36b6a9b0420de4ccb6d4edf1467bd82ed60a5e214 }
    - { id: pv-m4, class: fluent-but-wrong, why_fluent: old protocol accepted for compatibility, expected_drop: 1, must_fail: [PV-05], fixture: sealed:dir-sha256:b78f1629e1e99017659f47e036e3b6194f94f8daae45049446d72372421d57ee }
    - { id: pv-m5, class: fluent-but-wrong, why_fluent: uninstall cleanup broadens to wildcard, expected_drop: 1, must_fail: [PV-06], fixture: sealed:dir-sha256:8b05885ffa3bf89cb602bd586dfbd4b245a7de48f952bb30377511d523af4a38 }
    - { id: pv-m6, class: fluent-but-wrong, why_fluent: elevation hint added to wire payload, expected_drop: 1, must_fail: [PV-01], fixture: sealed:dir-sha256:c1f7ed1604cc96bfdc62aba6687527dd2479e0a192538bc469b4e9466a8af9cf }
  rotation_k: 2
  last_validated: 6b8e7b1b003a6f10da3ad859e5f0f8ab7478a3694db7aee43c4860e66c23b18b
gamed_modes:
  - { mode: fabricated packet, status: sealed, note: generator reproduces bytes from real marker-framed serializer output }
  - { mode: elevated validation, status: mitigated, note: live arm invokes setup without elevation and requires refusal plus state equality }
escape_hatch_bans:
  - { ban: no unknown wire keys, check: PV-01 }
  - { ban: no donor identities, check: PV-02 }
  - { ban: no duplicate operations, check: PV-03 }
  - { ban: no live success or mutation, check: PV-04 }
  - { ban: no legacy protocol, check: PV-05 }
  - { ban: no wildcard ownership, check: PV-06 }
---
