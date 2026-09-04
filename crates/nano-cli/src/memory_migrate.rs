//! Explicit migration of the quarantined Markdown memory store.
//!
//! The dedicated memory journal remains authoritative: every accepted legacy
//! entry and its mediation receipt are synced before `memory.db` is rebuilt.

use nano_memory::{
    FactWrite, MemoryPolicy, MemoryProposal, MemoryStore, ProposalKind, rebuild_from_journals,
};
use nano_session::{JournalWriter, Op, OpEnvelope, read_journal};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

const COMPLETION_ID: &str = "legacy-migration-complete";

#[derive(Debug, Serialize)]
struct MigrationReceipt {
    ingested: usize,
    skipped: usize,
    refused: usize,
    journal_paths: Vec<PathBuf>,
    entries: Vec<EntryReceipt>,
}

#[derive(Debug, Serialize)]
struct EntryReceipt {
    name: String,
    content_sha256: Option<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug)]
enum MigrationError {
    Usage,
    AlreadyCompleted,
    Policy(String),
    Journal(String),
    Rebuild(String),
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
            Self::Journal(message) => write!(f, "memory migration journal error: {message}"),
            Self::Rebuild(message) => write!(f, "memory migration rebuild error: {message}"),
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
            if let Err(error) = serde_json::to_writer(&mut *out, &receipt) {
                let _ = writeln!(err, "wayland-nano: memory migration output error: {error}");
                return 3;
            }
            if let Err(error) = writeln!(out) {
                let _ = writeln!(err, "wayland-nano: memory migration output error: {error}");
                return 3;
            }
            0
        }
        Err(error) => {
            let _ = writeln!(err, "wayland-nano: {error}");
            if matches!(error, MigrationError::Usage) {
                2
            } else {
                3
            }
        }
    }
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
    if params.project.is_empty() || params.session_id.is_empty() {
        return Err(MigrationError::Usage);
    }
    Ok(params)
}

fn migrate(nano_home: &Path, params: &Params) -> Result<MigrationReceipt, MigrationError> {
    let resolved = nano_cli::memory_policy::resolve(nano_home)
        .map_err(|error| MigrationError::Policy(error.to_string()))?;
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
    if existing.envelopes.iter().any(|entry| {
        matches!(&entry.op, Op::MemoryWriteReceipt { write_id, .. } if write_id == COMPLETION_ID)
    }) {
        return Err(MigrationError::AlreadyCompleted);
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
                    status: if skipped { "skipped" } else { "ingested" },
                    error: None,
                });
                accepted.push(entry);
            }
            Err(receipt) => receipts.push(receipt),
        }
    }

    let shadow = ShadowPaths::new(nano_home);
    rebuild_from_journals(
        &shadow.db,
        std::slice::from_ref(&journal_path),
        MemoryPolicy::default(),
        resolved.configured_agents().clone(),
    )
    .map_err(|error| MigrationError::Rebuild(error.to_string()))?;
    let mut shadow_store = MemoryStore::open_at(
        &shadow.db,
        &shadow.journal,
        MemoryPolicy::default(),
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
        MemoryPolicy::default(),
        resolved.configured_agents().clone(),
    )
    .map_err(|error| MigrationError::Rebuild(error.to_string()))?;

    let completion = OpEnvelope::new(
        COMPLETION_ID,
        chrono::Utc::now().to_rfc3339(),
        Op::MemoryWriteReceipt {
            write_id: COMPLETION_ID.into(),
            agent_id: params.agent_id.clone(),
            message: format!(
                "legacy migration complete for project {}: {} entries examined",
                params.project,
                receipts.len()
            ),
        },
    );
    JournalWriter::open(&journal_path)
        .and_then(|mut writer| writer.append(&completion))
        .map_err(|error| MigrationError::Journal(error.to_string()))?;

    Ok(MigrationReceipt {
        ingested: receipts
            .iter()
            .filter(|entry| entry.status == "ingested")
            .count(),
        skipped: receipts
            .iter()
            .filter(|entry| entry.status == "skipped")
            .count(),
        refused: receipts
            .iter()
            .filter(|entry| entry.status == "refused")
            .count(),
        journal_paths: vec![journal_path],
        entries: receipts,
    })
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
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(MigrationError::Policy(error.to_string())),
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| MigrationError::Policy(error.to_string()))?;
    paths.retain(|path| path.extension().is_some_and(|extension| extension == "md"));
    paths.sort();
    Ok(paths)
}

struct PreparedEntry {
    name: String,
    content_sha256: String,
    fact: FactWrite,
    receipt_envelope: OpEnvelope,
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
    let refusal = |hash, message| EntryReceipt {
        name: name.clone(),
        content_sha256: hash,
        status: "refused",
        error: Some(message),
    };
    let metadata = path
        .symlink_metadata()
        .map_err(|error| refusal(None, format!("metadata: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(refusal(None, "entry is not a plain file".into()));
    }
    let bytes = std::fs::read(path).map_err(|error| refusal(None, format!("read: {error}")))?;
    let content_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| refusal(Some(content_sha256.clone()), format!("utf-8: {error}")))?;
    let content = nano_session::redaction::redact_secrets(text).map_err(|error| {
        refusal(
            Some(content_sha256.clone()),
            format!("secret screening: {error}"),
        )
    })?;
    nano_session::scan_for_secrets(&content).map_err(|error| {
        refusal(
            Some(content_sha256.clone()),
            format!("secret screening: {error}"),
        )
    })?;
    let valid_from = timestamp_from_name(&name)
        .map_err(|message| refusal(Some(content_sha256.clone()), message))?;
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
