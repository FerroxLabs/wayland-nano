# WP-4 Upstream Owner Deviations

These are exact prerequisite repairs performed in separate owner lanes after locked WP-4
black-box tests exposed upstream mismatches. They are part of the Phase 7 diff but are not
WP-4 Gate Card builder ownership and do not grant general `crates/**` permission.

## RULE-LIMIT-01

- Integrated commit: `e199fcb` (`fix(rules): enforce evaluator limits`)
- Exact paths:
  - `crates/nano-core/src/execrules.rs`
  - `crates/nano-core/tests/execrules.rs`
  - `crates/nano-cli/tests/p4_rules.rs`
- Reason: persisted prefix rules accepted more than 64 positions and tokens larger than
  4096 bytes even though evaluation used those ceilings; locked CF-05 could not pass.
- Evidence: boundary-valid rules remain accepted; over-limit unit and real CLI black-box
  cases reject; full nano-core/nano-cli and strict Clippy passed before integration.

## REGISTRY-BOOTSTRAP-01

- Integrated commits:
  - `d5c7f10` (`test(verify): isolate empty bootstrap contract`)
  - `0762700` (`test(verify): decouple fixture registry`)
- Exact paths:
  - `crates/nano-cli/src/verify_cmd.rs` (test module only)
  - `crates/nano-cli/tests/verify_cmd.rs`
- Reason: WP-3 tests incorrectly required the live workspace registry to stay equal to the
  empty bootstrap after WP-4 legitimately populated it.
- Evidence: exact 41-byte bootstrap is fixture-tested, populated canonical loading is tested,
  WP-3 integration remains 14/14, full nano-cli and strict Clippy passed.

## Audit Rule

The independent WP-4 audit must inspect these five files and their tests for security and
regression risk, but ownership validation classifies them only as the two exact owner deviations
above. Any other Phase 7 `crates/**` path is forbidden. The bounded WP-4 fix round still may edit
only its declared Gate Card/docs/provenance files; any new upstream defect stops for a separate
owner decision.
