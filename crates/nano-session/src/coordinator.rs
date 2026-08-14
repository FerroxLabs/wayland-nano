//! Session-scoped serialization for every journal append and compaction cut
//! (P3 design §3.3 [r4 codex new-1]).
//!
//! One `JournalCoordinator` per session, created beside the session state
//! where the journal path is first known, and shared with the turn loop, the
//! MCP hydration/elicitation/grant append paths, and the compaction builder:
//!
//! (i)   EVERY journal append routes through `coordinator.append(envelope)` —
//!       no caller opens a `JournalWriter` directly on a live session journal;
//! (ii)  appends serialize on ONE std `Mutex` guard (FIFO, single writer);
//! (iii) the compaction critical section holds that guard CONTINUOUSLY across
//!       watermark capture, carry construction, the durable
//!       `CompactionComplete` append, and publication of the covered-prefix
//!       drop, so the snapshot is exact by construction;
//! (iv)  a failed ordinary append returns the io error to the caller (the
//!       typed `JournalUnavailable` mapping is the caller's), live state
//!       unchanged; a failed append INSIDE the critical section aborts the
//!       compaction — nothing is published, the session continues
//!       uncompacted.

use crate::compact::compacted_prefix;
use crate::op::{
    HydrationCarryEntry, MAX_HYDRATION_TOOL_NAMES, OpEnvelope, validate_hydration_carry_entry,
};
use crate::replay::SessionState;
use crate::writer::JournalWriter;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The sole in-process append authority for one session journal.
pub struct JournalCoordinator {
    path: PathBuf,
    writer: Mutex<JournalWriter>,
}

impl std::fmt::Debug for JournalCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalCoordinator")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl JournalCoordinator {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            writer: Mutex::new(JournalWriter::open(&path)?),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Durably append (fsync per the writer discipline) while holding the
    /// session's single serialization lock. `Ok(false)` = the idempotent
    /// no-op of an already-durable id (a retried append). Fail-closed on a
    /// torn journal path: if the path no longer names a regular file
    /// (deleted/replaced out from under the session), the append fails
    /// typed rather than writing into a delete-pending handle.
    pub fn append(&self, envelope: &OpEnvelope) -> io::Result<bool> {
        let mut writer = self.lock()?;
        check_journal_path(&self.path)?;
        writer.append(envelope)
    }

    /// Enter the compaction critical section. Keep this guard alive across
    /// watermark capture, carry construction, durable append, and covered-
    /// prefix publication — no append can interleave between capture and
    /// publish, so the snapshot is exact by construction.
    pub fn compaction(&self) -> io::Result<CompactionGuard<'_>> {
        Ok(CompactionGuard {
            writer: self.lock()?,
            path: &self.path,
        })
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, JournalWriter>> {
        self.writer
            .lock()
            .map_err(|_| io::Error::other("journal coordinator poisoned"))
    }
}

/// The held compaction critical-section guard (§3.3 rule iii).
pub struct CompactionGuard<'a> {
    writer: MutexGuard<'a, JournalWriter>,
    path: &'a Path,
}

impl CompactionGuard<'_> {
    /// The watermark read: the full envelope stream, taken UNDER the guard,
    /// so the op-id prefix the caller names in `covers_op_ids` cannot race
    /// with any other append.
    pub fn snapshot(&self) -> io::Result<Vec<OpEnvelope>> {
        Ok(crate::reader::read_journal(self.path)?.envelopes)
    }

    /// The durable `CompactionComplete` append. An error leaves the
    /// uncompacted prefix authoritative; callers must publish no in-memory
    /// cut unless this append succeeds (§3.3 rule iv).
    pub fn append_complete(&mut self, envelope: &OpEnvelope) -> io::Result<bool> {
        check_journal_path(self.path)?;
        self.writer.append(envelope)
    }

    /// Best-effort `CompactionCancel` inside the section (the journal may be
    /// the thing that failed — the caller decides whether to keep it).
    pub fn append_cancel(&mut self, envelope: &OpEnvelope) -> io::Result<bool> {
        check_journal_path(self.path)?;
        self.writer.append(envelope)
    }
}

/// The journal path must still name a regular file at append time (fail-
/// closed on a torn/replaced journal — never write into a delete-pending
/// handle).
fn check_journal_path(path: &Path) -> io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(io::Error::other("journal path is no longer a file")),
        Err(err) => Err(err),
    }
}

/// The hydration state equivalent AT the watermark (§3.3):
/// `carry(W) = prior_carry ⊕ covered_hydration_suffix`, computed EXACTLY as
/// the fold of the replay input of `envelopes` (the snapshot taken under the
/// guard), so `carry(W) ≡ fold(replay input at W)` holds by construction —
/// the equivalence §12 asserts, including across consecutive compactions
/// (carry feeding carry). Returns `None` when nothing is hydrated (the field
/// stays omitted, byte-minimal).
///
/// The replay-input fold is what makes this exact for BOTH coverage shapes:
/// the manual path's full-prefix watermark and the in-turn path's
/// turn-scoped watermark — replay's clear-and-install carry arm replaces the
/// fold of any surviving hydration ops with this full at-W state.
///
/// F-P3-8 degradation: when a server's hydrated UNION exceeds
/// `MAX_HYDRATION_TOOL_NAMES` (legal — the cap is per op, the union is
/// not), the entry degrades to digest/summary form (names dropped, digest +
/// churn window carried) rather than aborting the compaction forever;
/// resume re-exposes nothing for that server and tool_search re-hydrates.
pub fn hydration_carry_at(
    envelopes: &[OpEnvelope],
) -> io::Result<Option<Vec<HydrationCarryEntry>>> {
    let replay_input: Vec<OpEnvelope> = compacted_prefix(envelopes).into_iter().cloned().collect();
    let state = SessionState::fold(&replay_input);
    if state.mcp_hydrated.is_empty()
        && state.mcp_tools_digest.is_empty()
        && state.mcp_recent_digests.is_empty()
    {
        return Ok(None);
    }
    let mut server_ids: Vec<String> = state
        .mcp_hydrated
        .keys()
        .chain(state.mcp_tools_digest.keys())
        .chain(state.mcp_recent_digests.keys())
        .cloned()
        .collect();
    server_ids.sort();
    server_ids.dedup();
    let mut carry = Vec::with_capacity(server_ids.len());
    for server_id in server_ids {
        let mut entry = HydrationCarryEntry {
            server_id: server_id.clone(),
            tool_names: state
                .mcp_hydrated
                .get(&server_id)
                .map(|names| names.iter().cloned().collect())
                .unwrap_or_default(),
            tools_digest: state
                .mcp_tools_digest
                .get(&server_id)
                .cloned()
                .unwrap_or_default(),
            recent_digests: state
                .mcp_recent_digests
                .get(&server_id)
                .cloned()
                .unwrap_or_default(),
        };
        // F-P3-8: the per-server union across hydration ops can exceed the
        // per-entry name cap even though every journaled op was legal, and
        // aborting here bricks EVERY later compaction against the same wall.
        // Overflow degrades to the digest/summary form instead: the names
        // are dropped (never a silently truncated subset), the tools_digest
        // and churn window ride, and resume treats the server as
        // digest-verified with nothing hydrated — the tools defer and
        // tool_search re-hydrates. Compaction never bricks.
        if entry.tool_names.len() > MAX_HYDRATION_TOOL_NAMES {
            entry.tool_names = Vec::new();
        }
        // The carry must itself be a legal journal payload: a state that
        // cannot be re-journaled bounded aborts the compaction fail-safe
        // (typed error — the caller publishes nothing), never silently
        // truncated.
        if let Err(rule) = validate_hydration_carry_entry(&entry) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("hydration carry entry out of bounds: {rule}"),
            ));
        }
        carry.push(entry);
    }
    Ok(Some(carry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{HydrationEntry, Op};

    fn digest(byte: u8) -> String {
        format!("{:064x}", byte)
    }

    fn hydration(id: &str, server: &str, tools: &[&str], dg: &str) -> OpEnvelope {
        OpEnvelope::new(
            id,
            "now",
            Op::McpToolHydration {
                hydration_id: format!("{id}-h"),
                entries: vec![HydrationEntry {
                    server_id: server.to_string(),
                    tool_names: tools.iter().map(|t| t.to_string()).collect(),
                    tools_digest: dg.to_string(),
                }],
            },
        )
    }

    #[test]
    fn carry_is_none_without_hydration() {
        let envelopes = vec![OpEnvelope::new(
            "1",
            "now",
            Op::SessionBegin {
                session_id: "s".into(),
                cwd: "/tmp".into(),
            },
        )];
        assert!(hydration_carry_at(&envelopes).expect("carry").is_none());
    }

    #[test]
    fn carry_captures_state_at_watermark() {
        let envelopes = vec![
            OpEnvelope::new(
                "1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "/tmp".into(),
                },
            ),
            hydration("2", "fs", &["read", "write"], &digest(1)),
        ];
        let carry = hydration_carry_at(&envelopes)
            .expect("carry")
            .expect("present");
        assert_eq!(carry.len(), 1);
        assert_eq!(carry[0].server_id, "fs");
        assert_eq!(carry[0].tool_names, vec!["read", "write"]);
        assert_eq!(carry[0].tools_digest, digest(1));
        assert_eq!(carry[0].recent_digests, vec![digest(1)]);
        for entry in &carry {
            validate_hydration_carry_entry(entry).expect("carry is a legal payload");
        }
    }

    #[test]
    fn carry_equals_replay_input_fold_under_prior_compaction() {
        // First compaction covers the original hydration op and carries the
        // state; a second hydration lands; the SECOND carry must equal
        // first-carry ⊕ its covered suffix (carry feeding carry).
        let prefix = vec![
            OpEnvelope::new(
                "1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "/tmp".into(),
                },
            ),
            hydration("2", "fs", &["read"], &digest(1)),
        ];
        let first_complete = OpEnvelope::new(
            "3",
            "now",
            Op::CompactionComplete {
                compaction_id: "c1".into(),
                summary: "s".into(),
                covers_op_ids: vec!["1".into(), "2".into()],
                changed_files: vec![],
                image_influenced: false,
                mcp_hydration: hydration_carry_at(&prefix).expect("carry"),
            },
        );
        let journal = vec![
            prefix[0].clone(),
            prefix[1].clone(),
            first_complete,
            hydration("4", "fs", &["write"], &digest(2)),
            hydration("5", "web", &["fetch"], &digest(3)),
        ];
        let carry = hydration_carry_at(&journal)
            .expect("carry")
            .expect("present");
        let fs = carry.iter().find(|e| e.server_id == "fs").expect("fs");
        assert_eq!(fs.tool_names, vec!["read", "write"]);
        assert_eq!(fs.tools_digest, digest(2));
        assert_eq!(fs.recent_digests, vec![digest(1), digest(2)]);
        let web = carry.iter().find(|e| e.server_id == "web").expect("web");
        assert_eq!(web.tool_names, vec!["fetch"]);
        assert_eq!(web.recent_digests, vec![digest(3)]);
    }

    /// F-P3-8 regression pin: two legal hydration ops (≤ 64 names each)
    /// whose per-server UNION is 70 names. Pre-fix the carry validated
    /// against the per-op cap, failed, and every later compaction hit the
    /// same wall. Now the entry degrades to digest/summary form, the
    /// compaction proceeds, replay stays consistent, and the NEXT
    /// compaction carries again.
    #[test]
    fn carry_degrades_when_hydrated_union_exceeds_the_name_cap() {
        let first: Vec<String> = (0..40).map(|i| format!("tool_{i:03}")).collect();
        let second: Vec<String> = (30..70).map(|i| format!("tool_{i:03}")).collect();
        let envelopes = vec![
            OpEnvelope::new(
                "1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "/tmp".into(),
                },
            ),
            hydration(
                "2",
                "fs",
                &first.iter().map(String::as_str).collect::<Vec<_>>(),
                &digest(1),
            ),
            hydration(
                "3",
                "fs",
                &second.iter().map(String::as_str).collect::<Vec<_>>(),
                &digest(2),
            ),
        ];
        let carry = hydration_carry_at(&envelopes)
            .expect("overflow must degrade, never brick the compaction")
            .expect("present");
        assert_eq!(carry.len(), 1);
        let entry = &carry[0];
        assert!(
            entry.tool_names.is_empty(),
            "overflow drops the names (never a truncated subset)"
        );
        assert_eq!(entry.tools_digest, digest(2));
        assert_eq!(entry.recent_digests, vec![digest(1), digest(2)]);
        validate_hydration_carry_entry(entry).expect("degraded carry is a legal payload");

        // Replay consistency: the compaction arm installs the carry, so the
        // folded post-compaction state IS the degraded carry — digest +
        // churn window ride, no names are phantom-exposed.
        let complete = OpEnvelope::new(
            "4",
            "now",
            Op::CompactionComplete {
                compaction_id: "c1".into(),
                summary: "s".into(),
                covers_op_ids: vec!["1".into(), "2".into(), "3".into()],
                changed_files: vec![],
                image_influenced: false,
                mcp_hydration: Some(carry.clone()),
            },
        );
        let mut journal = envelopes.clone();
        journal.push(complete);
        let state = SessionState::fold(&journal);
        assert!(
            state.mcp_hydrated.get("fs").expect("fs").is_empty(),
            "no phantom exposure after the degraded carry"
        );
        assert_eq!(state.mcp_tools_digest.get("fs").expect("fs"), &digest(2));
        assert_eq!(
            state.mcp_recent_digests.get("fs").expect("fs"),
            &vec![digest(1), digest(2)]
        );
        // The next compaction over the compacted journal carries again.
        let second_carry = hydration_carry_at(&journal)
            .expect("the next compaction must not brick either")
            .expect("present");
        assert_eq!(second_carry, carry, "carry feeding carry stays stable");
    }

    #[test]
    fn coordinator_serializes_appends_and_compaction_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "wayland-nano-coordinator-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.jsonl");
        let coordinator = JournalCoordinator::open(&path).expect("open");
        coordinator
            .append(&OpEnvelope::new(
                "1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "/tmp".into(),
                },
            ))
            .expect("append");
        // The idempotent re-append reports the no-op distinctly.
        let reapplied = coordinator
            .append(&OpEnvelope::new(
                "1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "/tmp".into(),
                },
            ))
            .expect("append");
        assert!(!reapplied, "idempotent re-append reports the no-op");
        {
            let mut guard = coordinator.compaction().expect("guard");
            let snapshot = guard.snapshot().expect("snapshot");
            assert_eq!(snapshot.len(), 1);
            guard
                .append_complete(&hydration("2", "fs", &["read"], &digest(1)))
                .expect("complete append");
        }
        let report = crate::reader::read_journal(&path).expect("read");
        assert_eq!(report.envelopes.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
