# Wayland Nano — Session Journal Semantics (v1)

**FROZEN v1.0 — 2026-08-11**
Change control: changes require owner sign-off plus an evidence update (the
cited tests must be updated in the same change). Descriptive-first: every
rule below is implemented in `wayland-nano/crates/nano-session/` and proven by the
kill-boundary suite (`src/tests.rs` — COMP-JRNL-001/002/003) and
the adversarial journal suite (`tests/adversarial_journal.rs`, 11 tests incl.
a 1000-case seeded fuzz). Anchors SCORECARD C1.3 / C2.2 / C3.4.

## 1. Format

- Append-only NDJSON: one `OpEnvelope` per line, `{v, id, ts, op}`.
  `v` = `SCHEMA_VERSION = 1`; additive Op variants ride the same version
  (`op.rs:15`). Existing content is never rewritten — the sole exception is
  torn-tail truncation at open (§3).
- Every append is followed by `sync_data()` (fsync) by default; tests may
  relax this to simulate crash windows (`writer.rs:18-20, 74-76`).

## 2. Op vocabulary

`op.rs:49-103` (`#[serde(tag = "type", rename_all = "snake_case")]`):

| Op | Payload notes |
|---|---|
| `session_begin` | `session_id`, `cwd` |
| `turn_begin` | `turn_id`, `input` (pending user input is journaled) |
| `tool_call` | `turn_id`, `call_id`, `name`, `args` |
| `tool_result` | `call_id`, `ok`, **`output_digest` (digest, never the output)**, `changed_files` |
| `assistant_text` | `turn_id`, `text` — assistant-produced content only; transcript, not execution state |
| `turn_end` | `turn_id`, `outcome`: `completed` / `cancelled` / `failed` / `interrupted` (`interrupted` is written only by replay/restore, never by a live writer — `op.rs:42-44`) |
| `compaction_begin` / `compaction_complete` / `compaction_cancel` | complete carries `summary`, `covers_op_ids`, and the `changed_files` durable-effect inventory |
| `unknown` | `#[serde(other)]` — see §4 |

**No secret payloads by default:** tool results journal a digest of the
output, not the output (`op.rs:64-71`). On restore the elided result is
surfaced with an explicit marker — `[tool output elided from journal:
ok=…, digest=…]` — never a fabricated payload
(`nano-cli/src/acp_mode.rs:766-817`; proven by
`nano-agent/tests/c2_kill_mid_edit.rs:215-264`).

## 3. Append-only guarantees and torn tail

- Reader rule (`reader.rs:1-8`): valid lines parse in order; a final line
  that fails to parse is a crash-torn tail — dropped, reported via
  `torn_tail_at`, not fatal; a parse failure on any **non-final** line is an
  **integrity error** and the whole restore fails loudly
  (`malformed_middle_line_is_an_integrity_error`).
- Writer open (`writer.rs:31-52`): scans existing ids, truncates a torn tail
  (removes exactly the newline-free final line, keeps every complete line
  byte-for-byte) so a retried append starts on a fresh line instead of
  gluing onto torn bytes. An integrity-broken middle is **never** truncated
  — restore stays fail-loud (`writer_never_truncates_integrity_broken_middle`,
  `writer_on_middle_corrupt_journal_stays_fail_loud`).
- Proven at every kill boundary: `torn_tail_is_dropped_and_middle_stays_authoritative`,
  `writer_open_truncates_torn_tail_so_retry_stays_recoverable`,
  `writer_open_truncation_removes_only_torn_bytes`,
  `writer_retry_of_committed_record_after_truncate_still_noops`;
  adversarially: `truncation_at_every_offset_drops_partial_tail_and_keeps_prefix_exact`,
  `corrupted_final_line_is_dropped_never_partially_recovered`,
  `four_mib_garbage_append_is_a_torn_tail_not_fatal`,
  `interleaved_valid_corrupt_valid_is_always_an_integrity_error`,
  `seeded_fuzz_1000_corruptions_never_panic_and_never_resurrect`.

## 4. Forward tolerance and idempotence

- **Unknown-op skip:** an `Op` variant this build does not know deserializes
  to `Unknown`; it is skipped on replay without failing the fold, and the
  raw line stays in the journal for future readers (`op.rs:100-103`;
  `unknown_ops_are_skipped_without_failing_replay`;
  `unknown_future_op_lines_survive_neighbor_tail_corruption`).
- **Idempotence:** the writer loads existing ids at open and `append`
  returns `Ok(false)` (no-op) for a duplicate id, so a retried write after
  a crash-uncertain append cannot double-append (`writer.rs:63-78`;
  `writer_is_idempotent_across_reopen`). The replayer independently dedupes
  ids, so duplicate ids on disk never double-apply (`replay.rs:67-69`;
  `duplicate_ids_never_double_apply`;
  `crafted_duplicate_ids_on_disk_first_wins_never_double_applies`;
  `writer_duplicate_id_with_different_payload_is_a_noop`).

## 5. Restore invariants and replay semantics

Reducer-fold replay (`replay.rs`), invariants asserted post-fold:

- A stranded `turn_begin` (no `turn_end`) marks the turn **interrupted**:
  pending user input preserved, tool calls that already returned keep their
  results, and **no tool call is re-executed on resume**
  (`crash_mid_turn_marks_interrupted_without_duplicate_effects` —
  COMP-JRNL-002; external-oracle proof in the C2.2/C3.4 legs).
- A stranded `compaction_begin` (no complete/cancel) resets to `Idle`
  (`stranded_compaction_running_resets_to_idle`).
- Open tool calls survive to the resume surface
  (`open_tool_call_survives_to_resume_surface`).
- **Interrupted tool call replays as failed, with nothing fabricated:** on
  `session/load` the interrupted call appears as a bare ToolUse with no
  synthesized result — no fabricated digest, no fabricated completion update
  (`nano-cli/tests/acp_live.rs::session_load_replays_interrupted_tool_call_as_failed`).
  Completed calls replay with the digest-elided result marker (§2).
- Out-of-order ops fold without panic into safe states
  (`out_of_order_ops_fold_without_panic_into_safe_states`).

## 6. Compaction equivalence

"Actionably equivalent" (`compact.rs:1-7`): replay after compaction
reconstructs the same pending user instructions, approval/execution state,
unresolved tool calls, changed-file inventory, and the next legal transition
(`AcceptUserInstruction` / `ResolveInterruptedTurn` / `ContinueTurn`).
Transcript wording may differ; executable decisions may not. Ops covered by
a completed compaction keep their durable effects (`changed_files` carries
them forward) but drop from the pending-execution surface
(`compaction_replay_is_actionably_equivalent` — COMP-JRNL-003).

## Machine-readable authority

`contracts/journal-semantics.json` is the canonical machine-readable sibling.
Generated by `gen_contracts`; `opVocabulary` comes from `nano_session::op::OP_VOCABULARY`, while the frozen format and invariant fields follow this evidence contract and its change-control rule.

