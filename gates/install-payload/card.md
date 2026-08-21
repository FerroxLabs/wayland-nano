---
card: 1
gate_id: install-payload
domain: repo-deliverable
tier: 1
gate_script_hash: 2a696abe30930ba2419fb8cc48bca7aacef00f3b9ae016e3b6101f3797ae387d
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
  reference: sealed:dir-sha256:f52747028a8ac7620b9b0e23f9dde8fc38c2baec6a716ed13d37f9034a6a5d7f
  pool_min: 6
  pool_status: full
  mutants:
    - { id: ip-m1, class: fluent-but-wrong, why_fluent: the win32-x64 executable is staged under linux-x64 with the expected filename, so the tree looks complete, expected_drop: 1, must_fail: [IP-03], fixture: sealed:dir-sha256:0c51e5a8ae134d6a8523a52df676fef3e8bc90431c332cc844cf62a3203a3333 }
    - { id: ip-m2, class: fluent-but-wrong, why_fluent: the valid manifest quietly omits the least-used linux-arm64 lane while its directory remains, expected_drop: 1, must_fail: [IP-02], fixture: sealed:dir-sha256:66834ebe874a1719b6759d06703b43f56e917f738447ddd48913a2f7d11c15a6 }
    - { id: ip-m3, class: fluent-but-wrong, why_fluent: a well-formed 64-hex digest pins previous bytes and passes superficial schema review, expected_drop: 1, must_fail: [IP-03], fixture: sealed:dir-sha256:59d1910d2347ba15351f5126b8af2d3ddac41f4d02560171384274811d55c3e4 }
    - { id: ip-m4, class: fluent-but-wrong, why_fluent: the darwin-arm64 directory is absent while the complete-looking manifest still declares it, expected_drop: 1, must_fail: [IP-02], fixture: sealed:dir-sha256:7bd786ae849f04209def58278794fb7042d1c3ae9f4c635cb0a837829c017023 }
    - { id: ip-m5, class: fluent-but-wrong, why_fluent: the PTY guard is present and hashed but recorded non-executable while the primary smoke path remains green, expected_drop: 1, must_fail: [IP-05], fixture: sealed:dir-sha256:f25d42124bd09b7c8e8b24898b28286be92d3807aa2b80f70016629a6bd51dca }
    - { id: ip-m6, class: fluent-but-wrong, why_fluent: postinstall is a successful no-op and the shipped wrapper and binaries still look runnable, expected_drop: 1, must_fail: [IP-04], fixture: sealed:dir-sha256:fa791862a0da41b992b85158785d79939326e4a45437df898140d741638a0612 }
  rotation_k: 2
  last_validated: 2a696abe30930ba2419fb8cc48bca7aacef00f3b9ae016e3b6101f3797ae387d
gamed_modes:
  - { mode: hardcoded hashes over swapped bytes, status: sealed, note: ip-m1 and ip-m3 require independent whole-pool rehashing }
  - { mode: host-only inspection, status: mitigated, note: IP-02 and IP-03 traverse every platform and helper }
escape_hatch_bans:
  - { ban: skipping postinstall because wrapper verification exists, check: IP-01 }
  - { ban: treating the tamper probe failure as ignorable, check: IP-04 }
---

## Intent

Verify the copied npm install payload without modifying or repairing packaging producers.
