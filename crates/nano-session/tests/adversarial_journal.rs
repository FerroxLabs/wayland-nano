//! Adversarial journal tests (round 2): fuzzed corruption against the
//! append-only Op journal's torn-tail recovery.
//!
//! Invariants under test (the documented semantics in `src/lib.rs` /
//! `src/reader.rs`):
//! - recovery NEVER panics, whatever the bytes on disk;
//! - a malformed final line is dropped as a crash-torn tail (reported via
//!   `torn_tail_at`, never fatal);
//! - a malformed non-final line is an integrity error — restore fails loudly
//!   and NEVER silently skips the record;
//! - corruption never alters, invents, or resurrects records: every envelope
//!   parsed from lines that end before the first corrupted byte must equal the
//!   original exactly, and a destroyed record must not come back as VALID;
//! - duplicate envelope ids never double-apply, even when crafted on disk.

use nano_session::op::Op;
use nano_session::op::OpEnvelope;
use nano_session::op::TurnOutcome;
use nano_session::reader::parse_journal_bytes;
use nano_session::reader::read_journal;
use nano_session::replay::SessionState;
use nano_session::writer::JournalWriter;
use std::path::PathBuf;

fn env(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "2026-08-09T00:00:00Z", op)
}

/// Six valid ASCII envelopes forming a complete turn (plus a compaction pair).
fn base_ops() -> Vec<OpEnvelope> {
    vec![
        env(
            "1",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "C:\\repo".into(),
            },
        ),
        env(
            "2",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "fix the build".into(),
            },
        ),
        env(
            "3",
            Op::ToolCall {
                turn_id: "t1".into(),
                call_id: "c1".into(),
                name: "fs_edit".into(),
                args: serde_json::json!({"path": "main.rs"}),
            },
        ),
        env(
            "4",
            Op::ToolResult {
                call_id: "c1".into(),
                ok: true,
                output_digest: "d1".into(),
                changed_files: vec!["main.rs".into()],
            },
        ),
        env(
            "5",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: TurnOutcome::Completed,
            },
        ),
        env(
            "6",
            Op::CompactionBegin {
                compaction_id: "k1".into(),
            },
        ),
    ]
}

/// Serializes one envelope per line. Returns (bytes, line start offsets,
/// line JSON end offsets). Every line ends with '\n'; the JSON end excludes it.
fn serialize_lines(ops: &[OpEnvelope]) -> (Vec<u8>, Vec<usize>, Vec<usize>) {
    let mut bytes = Vec::new();
    let mut starts = Vec::new();
    let mut json_ends = Vec::new();
    for op in ops {
        starts.push(bytes.len());
        bytes.extend(serde_json::to_vec(op).unwrap());
        json_ends.push(bytes.len());
        bytes.push(b'\n');
    }
    (bytes, starts, json_ends)
}

/// Index of the line containing byte `pos` (a position exactly at a line
/// start belongs to that line).
fn line_index(starts: &[usize], pos: usize) -> usize {
    starts.partition_point(|&start| start <= pos) - 1
}

/// The core no-resurrection property: envelopes parsed from lines that end
/// before the first corrupted byte must equal the originals exactly, and the
/// recovery must never surface MORE records than the journal held.
fn assert_no_resurrection(
    report: &nano_session::JournalReport,
    originals: &[OpEnvelope],
    first_affected_line: usize,
    context: &str,
) {
    assert!(
        report.envelopes.len() <= originals.len(),
        "{context}: corruption invented records ({} parsed from {} originals)",
        report.envelopes.len(),
        originals.len()
    );
    let intact = first_affected_line.min(report.envelopes.len());
    for (index, envelope) in report.envelopes.iter().take(intact).enumerate() {
        assert_eq!(
            envelope, &originals[index],
            "{context}: record {index} before the first corrupted byte was altered"
        );
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nanok3-adv-journal-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// --- Truncation ---------------------------------------------------------------

#[test]
fn truncation_at_every_offset_drops_partial_tail_and_keeps_prefix_exact() {
    let originals = base_ops();
    let (bytes, starts, json_ends) = serialize_lines(&originals);

    for cut in 0..=bytes.len() {
        // Truncation must NEVER be an integrity error: the cut leaves at most
        // one partial line, and a partial final line is a torn tail by rule.
        let report = parse_journal_bytes(&bytes[..cut]).unwrap_or_else(|err| {
            panic!("truncation at byte {cut} must recover, got integrity error: {err}")
        });

        // A record is recoverable iff its complete JSON bytes survived; a
        // partial record must NEVER be resurrected as a valid envelope.
        let complete = json_ends.partition_point(|&end| end <= cut);
        assert_eq!(
            report.envelopes.as_slice(),
            &originals[..complete],
            "truncation at byte {cut} resurrected or dropped the wrong records"
        );

        let leftover_start = if complete < starts.len() {
            starts[complete].min(cut)
        } else {
            cut
        };
        let leftover = &bytes[leftover_start..cut];
        let expect_torn =
            !leftover.is_empty() && !leftover.iter().all(|byte| matches!(byte, b'\r' | b'\n'));
        assert_eq!(
            report.torn_tail_at.is_some(),
            expect_torn,
            "truncation at byte {cut}: torn-tail flag wrong"
        );
    }
}

// --- Torn tail shapes -----------------------------------------------------------

#[test]
fn corrupted_final_line_is_dropped_never_partially_recovered() {
    let originals = base_ops();
    let (bytes, starts, _) = serialize_lines(&originals);
    let tail_start = starts[starts.len() - 1];

    let mut corruptions: Vec<(&str, Vec<u8>)> = Vec::new();
    // Tail truncated mid-line.
    corruptions.push(("mid-line cut", bytes[..tail_start + 9].to_vec()));
    // Tail line overwritten with NULs.
    let mut nul_tail = bytes.clone();
    for byte in &mut nul_tail[tail_start..] {
        *byte = 0;
    }
    corruptions.push(("nul tail", nul_tail));
    // Tail line overwritten with deterministic garbage (no newline).
    let mut garbage_tail = bytes.clone();
    for (i, byte) in garbage_tail[tail_start..].iter_mut().enumerate() {
        *byte = b'A' + (i % 26) as u8;
    }
    corruptions.push(("garbage tail", garbage_tail));
    // Garbage appended WITHOUT a separator glues onto the last line.
    let mut glued = bytes[..bytes.len() - 1].to_vec(); // drop final '\n'
    glued.extend(b"garbage-instead-of-newline");
    corruptions.push(("glued tail", glued));
    // Half-written UTF-8 at the tail (truncated 4-byte emoji sequence).
    let mut half_utf8 = bytes.clone();
    half_utf8.extend([0xF0, 0x9F, 0x92]);
    corruptions.push(("half-written utf-8", half_utf8));

    for (label, mutated) in corruptions {
        let report = parse_journal_bytes(&mutated)
            .unwrap_or_else(|err| panic!("{label}: torn tail must recover: {err}"));
        assert!(
            report.torn_tail_at.is_some(),
            "{label}: torn tail must be reported"
        );
        assert_no_resurrection(&report, &originals, originals.len() - 1, label);
    }
}

#[test]
fn four_mib_garbage_append_is_a_torn_tail_not_fatal() {
    let originals = base_ops();
    let (bytes, _, _) = serialize_lines(&originals);

    // 4 MiB of newline-free garbage after a clean final newline: one huge
    // malformed last line.
    let mut appended = bytes.clone();
    for i in 0..(4 * 1024 * 1024) {
        appended.push(b'a' + (i % 26) as u8);
    }
    let report = parse_journal_bytes(&appended).expect("4MiB torn tail must recover");
    assert_eq!(
        report.envelopes.as_slice(),
        originals.as_slice(),
        "valid prefix must survive 4MiB tail"
    );
    assert_eq!(
        report.torn_tail_at,
        Some(bytes.len() as u64),
        "torn tail must start exactly where the garbage began"
    );

    // Same bytes through the file-backed reader.
    let dir = temp_dir("4mib");
    let path = dir.join("wire.jsonl");
    std::fs::write(&path, &appended).unwrap();
    let file_report = read_journal(&path).expect("file-backed recovery must not fail");
    assert_eq!(file_report.envelopes, originals);
    let _ = std::fs::remove_dir_all(&dir);

    // 4 MiB of garbage WITH embedded newlines: malformed middle lines are an
    // integrity error, never a silent skip.
    let mut interleaved = bytes.clone();
    for i in 0..(4 * 1024 * 1024) {
        interleaved.push(if i % 80 == 79 {
            b'\n'
        } else {
            b'a' + (i % 26) as u8
        });
    }
    assert!(
        parse_journal_bytes(&interleaved).is_err(),
        "malformed middle lines must fail loudly, not be skipped"
    );
}

// --- Strict middle ----------------------------------------------------------------

#[test]
fn interleaved_valid_corrupt_valid_is_always_an_integrity_error() {
    let originals = base_ops();
    let (bytes, starts, json_ends) = serialize_lines(&originals);

    // Corrupt every non-final line in turn, three different ways. Every shape
    // must fail loudly — a silent skip would rewrite session history.
    for line in 0..originals.len() - 1 {
        let start = starts[line];
        let end = json_ends[line];

        let mut prefixed = bytes.clone();
        prefixed.splice(start..start, *b"!!!");
        assert!(
            parse_journal_bytes(&prefixed).is_err(),
            "line {line}: `!!!` prefix on a middle line must be an integrity error"
        );

        let mut nulled = bytes.clone();
        nulled[start] = 0;
        assert!(
            parse_journal_bytes(&nulled).is_err(),
            "line {line}: NUL in a middle line must be an integrity error"
        );

        // Half-written UTF-8 (truncated 4-byte sequence) inside the line.
        let mut half_utf8 = bytes.clone();
        half_utf8.splice(end - 1..end - 1, [0xF0, 0x9F]);
        assert!(
            parse_journal_bytes(&half_utf8).is_err(),
            "line {line}: half-written UTF-8 in a middle line must be an integrity error"
        );

        // Newline injected mid-line: the orphaned head is a malformed middle
        // line, so this must error rather than drop half a record.
        let mut split = bytes.clone();
        split.insert(start + (end - start) / 2, b'\n');
        assert!(
            parse_journal_bytes(&split).is_err(),
            "line {line}: split middle line must be an integrity error"
        );
    }
}

#[test]
fn unknown_future_op_lines_survive_neighbor_tail_corruption() {
    let originals = base_ops();
    let (bytes, _, _) = serialize_lines(&originals[..2]);
    let mut journal = bytes;
    journal.extend(
        b"{\"v\":1,\"id\":\"x1\",\"ts\":\"t\",\"op\":{\"type\":\"future_feature\",\"payload\":42}}\n",
    );
    journal.extend(serde_json::to_vec(&originals[2]).unwrap());
    journal.extend(b"\n{\"v\":1,\"id\":\"4\",\"ts\":\""); // torn tail

    let report = parse_journal_bytes(&journal).expect("unknown ops + torn tail must recover");
    assert_eq!(
        report.envelopes.len(),
        4,
        "two valid + one unknown-op line survive"
    );
    assert!(report.torn_tail_at.is_some());
    // Replay skips the unknown op without failing the fold.
    let state = SessionState::fold(&report.envelopes);
    assert_eq!(state.open_tool_calls.len(), 1);
    assert_eq!(state.open_tool_calls[0].call_id, "c1");
}

// --- Duplicate ids / ordering -----------------------------------------------------

#[test]
fn crafted_duplicate_ids_on_disk_first_wins_never_double_applies() {
    let mut originals = base_ops();
    // A second record reusing id "4" but carrying a different payload: a
    // retried append after a crash-uncertain write (or a hostile edit) must
    // not overwrite or double-apply the first record's effects.
    originals.push(env(
        "4",
        Op::ToolResult {
            call_id: "c1".into(),
            ok: false,
            output_digest: "forged".into(),
            changed_files: vec!["nanok3-forged.rs".into()],
        },
    ));
    let (bytes, _, _) = serialize_lines(&originals);
    let report = parse_journal_bytes(&bytes).expect("duplicate ids are parseable");
    let state = SessionState::fold(&report.envelopes);
    assert!(
        state.changed_files.contains("main.rs"),
        "first record applies"
    );
    assert!(
        !state.changed_files.contains("nanok3-forged.rs"),
        "duplicate id must never double-apply or overwrite"
    );
}

#[test]
fn out_of_order_ops_fold_without_panic_into_safe_states() {
    // TurnEnd before its TurnBegin, ToolResult before its ToolCall,
    // CompactionComplete without a Begin: replay must never panic and the
    // restore invariants (stranded work resets to safe states) must hold.
    let shuffled = vec![
        env(
            "1",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "C:\\repo".into(),
            },
        ),
        env(
            "2",
            Op::TurnEnd {
                turn_id: "t9".into(),
                outcome: TurnOutcome::Completed,
            },
        ),
        env(
            "3",
            Op::ToolResult {
                call_id: "c9".into(),
                ok: true,
                output_digest: "d9".into(),
                changed_files: vec!["late.rs".into()],
            },
        ),
        env(
            "4",
            Op::CompactionComplete {
                compaction_id: "k9".into(),
                summary: "orphan".into(),
                covers_op_ids: vec![],
                changed_files: vec![],
            },
        ),
        env(
            "5",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "still open".into(),
            },
        ),
    ];
    let state = SessionState::fold(&shuffled);
    assert_eq!(state.open_turn.as_ref().unwrap().turn_id, "t1");
    assert!(
        state.turn_interrupted,
        "stranded turn must reset to interrupted"
    );
    assert!(state.changed_files.contains("late.rs"));

    // Fully reversed journal: still no panic, still a safe restore.
    let mut reversed = base_ops();
    reversed.reverse();
    let state = SessionState::fold(&reversed);
    if state.open_turn.is_some() {
        assert!(state.turn_interrupted);
    }
}

// --- Writer behavior against corrupted journals --------------------------------------

#[test]
fn writer_duplicate_id_with_different_payload_is_a_noop() {
    let dir = temp_dir("dup-id");
    let path = dir.join("wire.jsonl");
    let first = env(
        "4",
        Op::ToolResult {
            call_id: "c1".into(),
            ok: true,
            output_digest: "d1".into(),
            changed_files: vec!["main.rs".into()],
        },
    );
    let forged = env(
        "4",
        Op::ToolResult {
            call_id: "c1".into(),
            ok: false,
            output_digest: "forged".into(),
            changed_files: vec!["nanok3-forged.rs".into()],
        },
    );
    {
        let mut writer = JournalWriter::open(&path).unwrap();
        assert!(writer.append(&first).unwrap());
        assert!(
            !writer.append(&forged).unwrap(),
            "same-id append must no-op"
        );
    }
    {
        let mut writer = JournalWriter::open(&path).unwrap();
        assert!(
            !writer.append(&forged).unwrap(),
            "same-id append must stay a no-op across reopen"
        );
    }
    let report = read_journal(&path).unwrap();
    assert_eq!(report.envelopes, vec![first]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writer_append_after_torn_tail_must_stay_recoverable() {
    // Crash window: the writer appended id "4" but the write tore mid-line.
    // On restart the client cannot know whether id "4" landed, so it retries
    // the append. The retry MUST be recoverable — either the torn line is
    // truncated away or the new record starts on a fresh line.
    let originals = base_ops();
    let (mut bytes, _, _) = serialize_lines(&originals[..3]);
    bytes.extend(b"{\"v\":1,\"id\":\"4\",\"ts\":\""); // torn mid-write of id "4"
    let dir = temp_dir("torn-retry");
    let path = dir.join("wire.jsonl");
    std::fs::write(&path, &bytes).unwrap();

    let mut writer = JournalWriter::open(&path).unwrap();
    let appended = writer.append(&originals[3]).unwrap();
    drop(writer);

    // SAFE expectation: the retry lands on a fresh line (or the torn bytes
    // were truncated at open), recovery succeeds, and id "4" replays.
    //
    // ACTUAL (hole, left failing on purpose): the writer opens in pure append
    // mode, so the retry glues onto the torn bytes. The retried record's own
    // trailing '\n' then demotes the glued malformed line from "torn tail" to
    // "malformed MIDDLE line" — every subsequent read_journal hard-errors with
    // an integrity error. One crash-torn write plus one idempotent retry
    // bricks the whole journal: the confirmed Ok(true) record is unreadable
    // and no later record can ever be recovered either.
    let recovery = read_journal(&path);
    let recovered_ids: Vec<String> = recovery
        .as_ref()
        .map(|report| report.envelopes.iter().map(|e| e.id.clone()).collect())
        .unwrap_or_default();
    assert!(
        appended && recovery.is_ok() && recovered_ids.iter().any(|id| id == "4"),
        "HOLE (torn-tail retry): append of id \"4\" returned Ok(true), but the \
         journal no longer recovers — read_journal: {:?}; recovered ids: \
         {recovered_ids:?}. The retried append glued onto the torn tail and \
         turned it into a permanent integrity error.",
        recovery.as_ref().map(|_| "ok").map_err(|e| e.to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writer_on_middle_corrupt_journal_stays_fail_loud() {
    // An integrity-broken journal must keep refusing restore even after the
    // writer appends to it. (JournalWriter::open swallows the integrity error
    // and starts with an empty id set — documented here so the behavior is
    // pinned: the journal stays broken loudly, never silently legitimized.)
    let originals = base_ops();
    let (mut bytes, _, _) = serialize_lines(&originals[..2]);
    bytes.extend(b"not json at all\n");
    bytes.extend(serde_json::to_vec(&originals[2]).unwrap());
    bytes.push(b'\n');
    let dir = temp_dir("middle-corrupt");
    let path = dir.join("wire.jsonl");
    std::fs::write(&path, &bytes).unwrap();

    assert!(
        read_journal(&path).is_err(),
        "broken middle must fail loudly"
    );
    let mut writer = JournalWriter::open(&path).unwrap();
    writer.append(&originals[5]).unwrap();
    drop(writer);
    assert!(
        read_journal(&path).is_err(),
        "appending must never silently legitimize an integrity-broken journal"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Deterministic mini-fuzzer ---------------------------------------------------------

/// xorshift64* — deterministic, dependency-free, seed-pinned.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        (self.next() % n as u64) as usize
    }
}

/// Applies one random corruption; returns the mutated bytes and the index of
/// the first line whose bytes were touched (`line_count` = nothing touched,
/// e.g. pure append after the final newline).
fn corrupt(
    rng: &mut XorShift,
    base: &[u8],
    starts: &[usize],
    line_count: usize,
) -> (Vec<u8>, usize) {
    match rng.below(7) {
        // Bit flip at a random position.
        0 => {
            let pos = rng.below(base.len());
            let mut bytes = base.to_vec();
            bytes[pos] ^= 1 << rng.below(8);
            (bytes, line_index(starts, pos))
        }
        // Random-byte run overwrite.
        1 => {
            let pos = rng.below(base.len());
            let len = 1 + rng.below(16);
            let mut bytes = base.to_vec();
            for i in 0..len {
                if pos + i < bytes.len() {
                    bytes[pos + i] = rng.next() as u8;
                }
            }
            (bytes, line_index(starts, pos))
        }
        // Truncation at a random offset.
        2 => {
            let cut = rng.below(base.len() + 1);
            let affected = if cut >= base.len() {
                line_count
            } else {
                line_index(starts, cut)
            };
            (base[..cut].to_vec(), affected)
        }
        // Random-byte insertion.
        3 => {
            let pos = rng.below(base.len() + 1);
            let len = 1 + rng.below(16);
            let mut bytes = base.to_vec();
            for _ in 0..len {
                bytes.insert(pos.min(bytes.len()), rng.next() as u8);
            }
            let affected = if pos >= base.len() {
                line_count
            } else {
                line_index(starts, pos)
            };
            (bytes, affected)
        }
        // Garbage append (lands after the final newline: a fresh last line).
        4 => {
            let mut bytes = base.to_vec();
            for _ in 0..1 + rng.below(64) {
                bytes.push(rng.next() as u8);
            }
            (bytes, line_count)
        }
        // NUL run overwrite.
        5 => {
            let pos = rng.below(base.len());
            let len = 1 + rng.below(32);
            let mut bytes = base.to_vec();
            for i in 0..len {
                if pos + i < bytes.len() {
                    bytes[pos + i] = 0;
                }
            }
            (bytes, line_index(starts, pos))
        }
        // Newline injection (splits a line into a middle line + tail).
        _ => {
            let pos = rng.below(base.len());
            let mut bytes = base.to_vec();
            bytes.insert(pos, b'\n');
            (bytes, line_index(starts, pos))
        }
    }
}

#[test]
fn seeded_fuzz_1000_corruptions_never_panic_and_never_resurrect() {
    let originals = base_ops();
    let (bytes, starts, _) = serialize_lines(&originals);
    let mut rng = XorShift(0x5EED_5EED_5EED_5EED);

    for iteration in 0..1000u32 {
        let (mutated, first_affected) = corrupt(&mut rng, &bytes, &starts, originals.len());
        let context = format!("iteration {iteration}");
        // Recovery must NEVER panic. It either fails loudly (integrity error)
        // or recovers; when it recovers, no record before the first corrupted
        // byte may be altered and no record may be invented.
        if let Ok(report) = parse_journal_bytes(&mutated) {
            assert_no_resurrection(&report, &originals, first_affected, &context);
            // Replay of whatever recovered must never panic either.
            let state = SessionState::fold(&report.envelopes);
            if state.open_turn.is_some() {
                assert!(
                    state.turn_interrupted,
                    "{context}: stranded turn must restore as interrupted"
                );
            }
        }
    }
}
