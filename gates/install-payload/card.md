---
card: 1
gate_id: install-payload
domain: repo-deliverable
tier: 1
gate_script_hash: 2e6cd1c3a026b4fd5611f143dc25534eb9a1cbb51254a746b6d91703f59d862d
relational_target:
  artifact: the staged npm package tree
  relation: every staged binary resolves against the integrity manifest and install refuses tampering
disclosure_default: opaque
checks:
  - { id: IP-01, category: execution, desc: postinstall completes on a clean prefix, measures: copied package install verifies the host platform }
  - { id: IP-02, category: structure, desc: manifest and directory sets match, measures: bidirectional primary and helper inventory }
  - { id: IP-03, category: value, desc: payload bytes match metadata, measures: independently recomputed size and sha256 for every binary }
  - { id: IP-04, category: security, desc: tampered payload is refused, measures: one-byte host tamper returns WAYLAND_NANO_INTEGRITY_MISMATCH }
  - { id: IP-05, category: execution, desc: wrapper and executable modes work, measures: wrapper emits semver and unix binaries are executable }
  - { id: IP-06, category: structure, desc: manifest is well formed, measures: exact schema 1 sha256 entry and helper fields }
wrapped_tools:
  - { name: node, version: 20, license: MIT, role: stdlib-only gate and package lifecycle runtime }
validation:
  reference: sealed:dir-sha256:da60507be5b50970dcb31ba0d8b98a908da680fb7c69164e06f86285f0525623
  pool_min: 6
  pool_status: full
  mutants:
    - { id: ip-m1, class: fluent-but-wrong, why_fluent: the win32-x64 executable is staged under linux-x64 with the expected filename, so the tree looks complete, expected_drop: 1, must_fail: [IP-03], fixture: sealed:dir-sha256:b15114e7c39cb211e130be6ea4299abf1c95f9bb95a95f30fff764bb14cd10f8 }
    - { id: ip-m2, class: fluent-but-wrong, why_fluent: the valid manifest quietly omits the least-used linux-arm64 lane while its directory remains, expected_drop: 1, must_fail: [IP-02], fixture: sealed:dir-sha256:f22406bbb69574b705d409437800a335987c1bb29f46ac50f6ee3e2f95ab866a }
    - { id: ip-m3, class: fluent-but-wrong, why_fluent: a well-formed 64-hex digest pins previous bytes and passes superficial schema review, expected_drop: 1, must_fail: [IP-03], fixture: sealed:dir-sha256:8b402e711fe3af27c45708f63156dacb17cd56b2ad92f388e6196e1bb91558e1 }
    - { id: ip-m4, class: fluent-but-wrong, why_fluent: the darwin-arm64 directory is absent while the complete-looking manifest still declares it, expected_drop: 1, must_fail: [IP-02], fixture: sealed:dir-sha256:4af00ab5384200683157ad1ab4f8d3198c9b212fbd78e9cc4da0ff4660dc0507 }
    - { id: ip-m5, class: fluent-but-wrong, why_fluent: the PTY guard is present and hashed but recorded non-executable while the primary smoke path remains green, expected_drop: 1, must_fail: [IP-05], fixture: sealed:dir-sha256:7f6522969f8068ccd6e9d953f3ca5baca670b004ad6edd4b417f43b0415f2f6d }
    - { id: ip-m6, class: fluent-but-wrong, why_fluent: postinstall is a successful no-op and the shipped wrapper and binaries still look runnable, expected_drop: 1, must_fail: [IP-04], fixture: sealed:dir-sha256:b3905718e20394924a8f6fdd92d630fc669142957f76d7eafa077650e4379b6e }
  rotation_k: 2
  last_validated: 2e6cd1c3a026b4fd5611f143dc25534eb9a1cbb51254a746b6d91703f59d862d
gamed_modes:
  - { mode: hardcoded hashes over swapped bytes, status: sealed, note: ip-m1 and ip-m3 require independent whole-pool rehashing }
  - { mode: host-only inspection, status: mitigated, note: IP-02 and IP-03 traverse every platform and helper }
escape_hatch_bans:
  - { ban: skipping postinstall because wrapper verification exists, check: IP-01 }
  - { ban: treating the tamper probe failure as ignorable, check: IP-04 }
---

## Intent

Verify the copied npm install payload without modifying or repairing packaging producers.
