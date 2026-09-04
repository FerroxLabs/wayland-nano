//! Explicit migration of the quarantined Markdown memory store.
//!
//! The dedicated memory journal remains authoritative: every accepted legacy
//! entry and its mediation receipt are synced before `memory.db` is rebuilt.

use nano_memory::{FactWrite, MemoryProposal, MemoryStore, ProposalKind, rebuild_from_journals};
use nano_session::{JournalWriter, Op, OpEnvelope, read_journal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

const COMPLETION_ID: &str = "legacy-migration-complete";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationReceipt {
    v: u8,
    outcome: MigrationOutcome,
    ingested: usize,
    skipped: usize,
    refused: usize,
    journal_paths: Vec<PathBuf>,
    entries: Vec<EntryReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MigrationOutcome {
    Complete,
    Incomplete,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryReceipt {
    name: String,
    content_sha256: Option<String>,
    status: EntryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<EntryErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryStatus {
    Ingested,
    Skipped,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryErrorKind {
    InvalidTimestamp,
    NotPlainFile,
    ReadFailed,
    InvalidUtf8,
    SecretScreening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MigrationErrorKind {
    Usage,
    AlreadyCompleted,
    PolicyInvalid,
    PolicyDisabled,
    InvalidSessionId,
    JournalInvalid,
    RebuildFailed,
    CompletionCollision,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationFailure {
    v: u8,
    error_kind: MigrationErrorKind,
    message: String,
}

#[derive(Debug)]
enum MigrationError {
    Usage,
    AlreadyCompleted,
    Policy(String),
    PolicyDisabled,
    InvalidSessionId,
    Journal(String),
    Rebuild(String),
    CompletionCollision,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage => write!(
                f,
                "usage: wayland-nano memory migrate --project <project> --agent-id <id> --session-id <id>"
            ),
            Self::AlreadyCompleted => write!(f, "legacy memory migration already completed"),
            Self::Policy(message) => write!(f, "memory migration policy refusal: {message}"),
            Self::PolicyDisabled => write!(f, "resolved memory policy disables migration writes"),
            Self::InvalidSessionId => write!(f, "invalid migration session_id"),
            Self::Journal(message) => write!(f, "memory migration journal error: {message}"),
            Self::Rebuild(message) => write!(f, "memory migration rebuild error: {message}"),
            Self::CompletionCollision => {
                write!(
                    f,
                    "migration completion envelope collides with different content"
                )
            }
        }
    }
}

impl MigrationError {
    const fn kind(&self) -> MigrationErrorKind {
        match self {
            Self::Usage => MigrationErrorKind::Usage,
            Self::AlreadyCompleted => MigrationErrorKind::AlreadyCompleted,
            Self::Policy(_) => MigrationErrorKind::PolicyInvalid,
            Self::PolicyDisabled => MigrationErrorKind::PolicyDisabled,
            Self::InvalidSessionId => MigrationErrorKind::InvalidSessionId,
            Self::Journal(_) => MigrationErrorKind::JournalInvalid,
            Self::Rebuild(_) => MigrationErrorKind::RebuildFailed,
            Self::CompletionCollision => MigrationErrorKind::CompletionCollision,
        }
    }
}

struct Params {
    project: String,
    agent_id: String,
    session_id: String,
}

pub(crate) fn run(
    nano_home: &Path,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let result = parse(args).and_then(|params| migrate(nano_home, &params));
    match result {
        Ok(receipt) => {
            let exit = if receipt.outcome == MigrationOutcome::Complete {
                0
            } else {
                3
            };
            if let Err(error) = serde_json::to_writer(&mut *out, &receipt) {
                let _ = write_failure(
                    err,
                    MigrationErrorKind::JournalInvalid,
                    format!("memory migration output error: {error}"),
                );
                return 3;
            }
            if let Err(error) = writeln!(out) {
                let _ = writeln!(err, "wayland-nano: memory migration output error: {error}");
                return 3;
            }
            exit
        }
        Err(error) => {
            let _ = write_failure(err, error.kind(), error.to_string());
            if matches!(error, MigrationError::Usage) {
                2
            } else {
                3
            }
        }
    }
}

fn write_failure(
    err: &mut dyn Write,
    error_kind: MigrationErrorKind,
    message: String,
) -> std::io::Result<()> {
    serde_json::to_writer(
        &mut *err,
        &MigrationFailure {
            v: 1,
            error_kind,
            message,
        },
    )
    .map_err(std::io::Error::other)?;
    writeln!(err)
}

fn parse(args: &[String]) -> Result<Params, MigrationError> {
    if args.first().map(String::as_str) != Some("migrate") {
        return Err(MigrationError::Usage);
    }
    let mut project = None;
    let mut agent_id = None;
    let mut session_id = None;
    let mut index = 1;
    while let Some(flag) = args.get(index) {
        let target = match flag.as_str() {
            "--project" => &mut project,
            "--agent-id" => &mut agent_id,
            "--session-id" => &mut session_id,
            _ => return Err(MigrationError::Usage),
        };
        if target.is_some() {
            return Err(MigrationError::Usage);
        }
        *target = Some(args.get(index + 1).cloned().ok_or(MigrationError::Usage)?);
        index += 2;
    }
    let params = Params {
        project: project.ok_or(MigrationError::Usage)?,
        agent_id: agent_id.ok_or(MigrationError::Usage)?,
        session_id: session_id.ok_or(MigrationError::Usage)?,
    };
    if params.project.is_empty() {
        return Err(MigrationError::Usage);
    }
    validate_session_id(&params.session_id)?;
    Ok(params)
}

fn validate_session_id(session_id: &str) -> Result<(), MigrationError> {
    let valid = !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid.then_some(()).ok_or(MigrationError::InvalidSessionId)
}

fn migrate(nano_home: &Path, params: &Params) -> Result<MigrationReceipt, MigrationError> {
    let resolved = nano_cli::memory_policy::resolve(nano_home)
        .map_err(|error| MigrationError::Policy(error.to_string()))?;
    let mut policy = resolved.policy().clone();
    if !policy.enabled || policy.write == nano_memory::WriteScope::Off {
        return Err(MigrationError::PolicyDisabled);
    }
    if policy.write == nano_memory::WriteScope::SessionOnly {
        policy.session_id = Some(params.session_id.clone());
    }
    if !resolved.configured_agents().contains(&params.agent_id) {
        return Err(MigrationError::Policy(format!(
            "unconfigured memory agent: {}",
            params.agent_id
        )));
    }
    validate_agent_id(&params.agent_id)?;

    let journal_path = nano_home.join("memory.jsonl");
    let existing =
        read_journal(&journal_path).map_err(|error| MigrationError::Journal(error.to_string()))?;
    if let Some(entry) = existing
        .envelopes
        .iter()
        .find(|entry| entry.id == COMPLETION_ID)
    {
        if matches_completion(entry, params) {
            return Err(MigrationError::AlreadyCompleted);
        }
        return Err(MigrationError::CompletionCollision);
    }
    if existing.envelopes.iter().any(|entry| {
        matches!(&entry.op, Op::MemoryWriteReceipt { write_id, .. } if write_id == COMPLETION_ID)
    }) {
        return Err(MigrationError::CompletionCollision);
    }
    let journaled_fact_ids = existing
        .envelopes
        .iter()
        .filter_map(|entry| match &entry.op {
            Op::MemoryWriteFact { fact_id, .. } => Some(fact_id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    let mut receipts = Vec::new();
    let mut accepted = Vec::new();
    for path in legacy_entries(&nano_home.join("memory"))? {
        match prepare_entry(&path, params) {
            Ok(entry) => {
                let skipped = journaled_fact_ids.contains(&entry.fact.id);
                receipts.push(EntryReceipt {
                    name: entry.name.clone(),
                    content_sha256: Some(entry.content_sha256.clone()),
                    status: if skipped {
                        EntryStatus::Skipped
                    } else {
                        EntryStatus::Ingested
                    },
                    error_kind: None,
                    message: None,
                });
                accepted.push(entry);
            }
            Err(receipt) => receipts.push(receipt),
        }
    }
    let refused = receipts
        .iter()
        .filter(|entry| entry.status == EntryStatus::Refused)
        .count();
    if accepted.is_empty() && refused > 0 {
        return Ok(build_receipt(
            MigrationOutcome::Incomplete,
            journal_path,
            receipts,
        ));
    }
    for entry in &accepted {
        let matching = existing
            .envelopes
            .iter()
            .filter(|envelope| {
                matches!(&envelope.op, Op::MemoryWriteFact { fact_id, .. } if fact_id == &entry.fact.id)
            })
            .collect::<Vec<_>>();
        if matching.len() > 1
            || matching
                .first()
                .is_some_and(|envelope| !matches_existing_write(envelope, entry, params))
        {
            return Err(MigrationError::Journal(format!(
                "conflicting authoritative write for {}",
                entry.fact.id
            )));
        }
        if let Some(envelope) = existing
            .envelopes
            .iter()
            .find(|envelope| envelope.id == entry.receipt_envelope.id)
            && !matches_existing_receipt(envelope, entry)
        {
            return Err(MigrationError::Journal(format!(
                "conflicting migration receipt for {}",
                entry.fact.id
            )));
        }
    }

    let shadow = ShadowPaths::new(nano_home);
    rebuild_from_journals(
        &shadow.db,
        std::slice::from_ref(&journal_path),
        policy.clone(),
        resolved.configured_agents().clone(),
    )
    .map_err(|error| MigrationError::Rebuild(error.to_string()))?;
    let mut shadow_store = MemoryStore::open_at(
        &shadow.db,
        &shadow.journal,
        policy.clone(),
        &params.agent_id,
        resolved.configured_agents().clone(),
    )
    .map_err(|error| MigrationError::Rebuild(error.to_string()))?;
    let mut authority_writer = JournalWriter::open(&journal_path)
        .map_err(|error| MigrationError::Journal(error.to_string()))?;
    for entry in &accepted {
        if journaled_fact_ids.contains(&entry.fact.id) {
            authority_writer
                .append(&entry.receipt_envelope)
                .map_err(|error| MigrationError::Journal(error.to_string()))?;
            continue;
        }
        shadow_store
            .commit_proposal(MemoryProposal {
                kind: ProposalKind::Fact(entry.fact.clone()),
            })
            .map_err(|error| MigrationError::Rebuild(error.to_string()))?;
        let mut authoritative = read_journal(&shadow.journal)
            .map_err(|error| MigrationError::Journal(error.to_string()))?
            .envelopes
            .into_iter()
            .rev()
            .find(|envelope| {
                matches!(&envelope.op, Op::MemoryWriteFact { fact_id, .. } if fact_id == &entry.fact.id)
            })
            .ok_or_else(|| {
                MigrationError::Journal(format!(
                    "resolver emitted no write for {}",
                    entry.fact.id
                ))
            })?;
        authoritative.id = format!("legacy-migration-write-{}", entry.fact.id);
        if let Op::MemoryWriteFact { session_id, .. } = &mut authoritative.op {
            *session_id = Some(params.session_id.clone());
        }
        authority_writer
            .append(&authoritative)
            .map_err(|error| MigrationError::Journal(error.to_string()))?;
        authority_writer
            .append(&entry.receipt_envelope)
            .map_err(|error| MigrationError::Journal(error.to_string()))?;
    }
    drop(authority_writer);
    drop(shadow_store);
    drop(shadow);

    if std::env::var_os("NANO_TEST_MEMORY_MIGRATE_STOP_AFTER_JOURNAL").is_some() {
        return Err(MigrationError::Rebuild(
            "test fault after journal sync and before database rebuild".into(),
        ));
    }

    rebuild_from_journals(
        &nano_home.join("memory/memory.db"),
        std::slice::from_ref(&journal_path),
        policy,
        resolved.configured_agents().clone(),
    )
    .map_err(|error| MigrationError::Rebuild(error.to_string()))?;

    if refused > 0 {
        return Ok(build_receipt(
            MigrationOutcome::Incomplete,
            journal_path,
            receipts,
        ));
    }

    let completion = OpEnvelope::new(
        COMPLETION_ID,
        chrono::Utc::now().to_rfc3339(),
        Op::MemoryWriteReceipt {
            write_id: COMPLETION_ID.into(),
            agent_id: params.agent_id.clone(),
            message: completion_message(params),
        },
    );
    let appended = JournalWriter::open(&journal_path)
        .and_then(|mut writer| writer.append(&completion))
        .map_err(|error| MigrationError::Journal(error.to_string()))?;
    if !appended {
        let exact = read_journal(&journal_path)
            .map_err(|error| MigrationError::Journal(error.to_string()))?
            .envelopes
            .into_iter()
            .any(|existing| existing == completion);
        if !exact {
            return Err(MigrationError::CompletionCollision);
        }
    }

    Ok(build_receipt(
        MigrationOutcome::Complete,
        journal_path,
        receipts,
    ))
}

fn build_receipt(
    outcome: MigrationOutcome,
    journal_path: PathBuf,
    entries: Vec<EntryReceipt>,
) -> MigrationReceipt {
    MigrationReceipt {
        v: 1,
        outcome,
        ingested: entries
            .iter()
            .filter(|entry| entry.status == EntryStatus::Ingested)
            .count(),
        skipped: entries
            .iter()
            .filter(|entry| entry.status == EntryStatus::Skipped)
            .count(),
        refused: entries
            .iter()
            .filter(|entry| entry.status == EntryStatus::Refused)
            .count(),
        journal_paths: vec![journal_path],
        entries,
    }
}

fn completion_message(params: &Params) -> String {
    format!(
        "legacy migration complete for project {} and agent {}",
        params.project, params.agent_id
    )
}

fn matches_completion(envelope: &OpEnvelope, params: &Params) -> bool {
    matches!(
        &envelope.op,
        Op::MemoryWriteReceipt {
            write_id,
            agent_id,
            message,
        } if write_id == COMPLETION_ID
            && agent_id == &params.agent_id
            && message == &completion_message(params)
    )
}

fn validate_agent_id(agent_id: &str) -> Result<(), MigrationError> {
    let valid = agent_id.len() <= 64
        && agent_id
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && agent_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(MigrationError::Policy(format!(
            "invalid agent_id: {agent_id}"
        )))
    }
}

fn legacy_entries(root: &Path) -> Result<Vec<PathBuf>, MigrationError> {
    match std::fs::read_dir(root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(MigrationError::Policy(error.to_string())),
    }
    Ok(
        nano_agent::memory::MemoryStore::from_dir(root.to_path_buf())
            .list()
            .into_iter()
            .map(|name| root.join(name))
            .collect(),
    )
}

struct PreparedEntry {
    name: String,
    content_sha256: String,
    fact: FactWrite,
    receipt_envelope: OpEnvelope,
}

fn matches_existing_write(envelope: &OpEnvelope, entry: &PreparedEntry, params: &Params) -> bool {
    let expected_id = format!("legacy-migration-write-{}", entry.fact.id);
    match &envelope.op {
        Op::MemoryWriteFact {
            fact_id,
            subject,
            predicate,
            object,
            confidence_micros,
            source_episode,
            valid_from,
            valid_to,
            source_trust,
            project,
            agent_id,
            session_id,
            resolver_outcome,
        } => {
            envelope.id == expected_id
                && fact_id == &entry.fact.id
                && subject == &entry.fact.subject
                && predicate == &entry.fact.predicate
                && object == &entry.fact.object
                && *confidence_micros == 1_000_000
                && source_episode.is_none()
                && valid_from == &entry.fact.valid_from
                && valid_to.is_none()
                && source_trust == "ModelInference"
                && project == &entry.fact.project
                && agent_id == &entry.fact.agent_id
                && session_id.as_deref() == Some(params.session_id.as_str())
                && matches!(
                    resolver_outcome.as_str(),
                    "coexist" | "supersede" | "keepexisting" | "keep_existing"
                )
        }
        _ => false,
    }
}

fn matches_existing_receipt(envelope: &OpEnvelope, entry: &PreparedEntry) -> bool {
    match (&envelope.op, &entry.receipt_envelope.op) {
        (
            Op::MemoryWriteReceipt {
                write_id,
                agent_id,
                message,
            },
            Op::MemoryWriteReceipt {
                write_id: expected_write,
                agent_id: expected_agent,
                message: expected_message,
            },
        ) => {
            write_id == expected_write && agent_id == expected_agent && message == expected_message
        }
        _ => false,
    }
}

struct ShadowPaths {
    db: PathBuf,
    journal: PathBuf,
}

impl ShadowPaths {
    fn new(nano_home: &Path) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stem = format!(".migration-shadow-{}-{nonce}", std::process::id());
        Self {
            db: nano_home.join("memory").join(format!("{stem}.db")),
            journal: nano_home.join("memory").join(format!("{stem}.jsonl")),
        }
    }
}

impl Drop for ShadowPaths {
    fn drop(&mut self) {
        for path in [
            self.db.clone(),
            self.db.with_extension("memory.lock"),
            self.journal.clone(),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn prepare_entry(path: &Path, params: &Params) -> Result<PreparedEntry, EntryReceipt> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<non-utf8>")
        .to_owned();
    let refusal = |hash, error_kind, message| EntryReceipt {
        name: name.clone(),
        content_sha256: hash,
        status: EntryStatus::Refused,
        error_kind: Some(error_kind),
        message: Some(message),
    };
    let metadata = path.symlink_metadata().map_err(|error| {
        refusal(
            None,
            EntryErrorKind::ReadFailed,
            format!("metadata: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(refusal(
            None,
            EntryErrorKind::NotPlainFile,
            "entry is not a plain file".into(),
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| refusal(None, EntryErrorKind::ReadFailed, format!("read: {error}")))?;
    let content_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        refusal(
            Some(content_sha256.clone()),
            EntryErrorKind::InvalidUtf8,
            format!("utf-8: {error}"),
        )
    })?;
    let content = nano_session::redaction::redact_secrets(text).map_err(|error| {
        refusal(
            Some(content_sha256.clone()),
            EntryErrorKind::SecretScreening,
            format!("secret screening: {error}"),
        )
    })?;
    nano_session::scan_for_secrets(&content).map_err(|error| {
        refusal(
            Some(content_sha256.clone()),
            EntryErrorKind::SecretScreening,
            format!("secret screening: {error}"),
        )
    })?;
    let valid_from = timestamp_from_name(&name).map_err(|message| {
        refusal(
            Some(content_sha256.clone()),
            EntryErrorKind::InvalidTimestamp,
            message,
        )
    })?;
    let mut identity = Sha256::new();
    identity.update(name.as_bytes());
    identity.update([0]);
    identity.update(&bytes);
    let fact_id = format!("legacy-{:x}", identity.finalize());
    let now = chrono::Utc::now().to_rfc3339();
    let receipt_id = format!("legacy-migration-receipt-{fact_id}");
    let fact = FactWrite {
        id: fact_id.clone(),
        subject: "legacy-memory-entry".into(),
        predicate: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        object: content,
        confidence: 1.0,
        source_episode: None,
        valid_from: valid_from.clone(),
        valid_to: None,
        source_trust: nano_memory::SourceTrust::ModelInference,
        project: params.project.clone(),
        agent_id: params.agent_id.clone(),
    };
    Ok(PreparedEntry {
        name,
        content_sha256: content_sha256.clone(),
        fact,
        receipt_envelope: OpEnvelope::new(
            receipt_id,
            now,
            Op::MemoryWriteReceipt {
                write_id: fact_id,
                agent_id: params.agent_id.clone(),
                message: format!(
                    "migrated legacy entry {content_sha256} (sha256:{content_sha256})"
                ),
            },
        ),
    })
}

fn timestamp_from_name(name: &str) -> Result<String, String> {
    let prefix = name
        .get(..19)
        .filter(|_| name.as_bytes().get(19) == Some(&b'-'))
        .ok_or_else(|| "invalid legacy filename timestamp".to_owned())?;
    let timestamp = chrono::NaiveDateTime::parse_from_str(prefix, "%Y-%m-%dT%H-%M-%S")
        .map_err(|error| format!("invalid legacy filename timestamp: {error}"))?;
    Ok(timestamp
        .and_utc()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_receipt_and_failure_wire_types_round_trip() {
        let receipt = MigrationReceipt {
            v: 1,
            outcome: MigrationOutcome::Incomplete,
            ingested: 1,
            skipped: 2,
            refused: 1,
            journal_paths: vec![PathBuf::from("memory.jsonl")],
            entries: vec![EntryReceipt {
                name: "2026-01-02T03-04-05-entry.md".into(),
                content_sha256: Some("a".repeat(64)),
                status: EntryStatus::Refused,
                error_kind: Some(EntryErrorKind::InvalidTimestamp),
                message: Some("invalid timestamp".into()),
            }],
        };
        let bytes = serde_json::to_vec(&receipt).unwrap();
        assert_eq!(
            serde_json::from_slice::<MigrationReceipt>(&bytes).unwrap(),
            receipt
        );
        let mut value = serde_json::to_value(&receipt).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MigrationReceipt>(value).is_err());

        let failure = MigrationFailure {
            v: 1,
            error_kind: MigrationErrorKind::PolicyDisabled,
            message: "disabled".into(),
        };
        let bytes = serde_json::to_vec(&failure).unwrap();
        assert_eq!(
            serde_json::from_slice::<MigrationFailure>(&bytes).unwrap(),
            failure
        );
        let mut value = serde_json::to_value(&failure).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MigrationFailure>(value).is_err());
    }
}
