use crate::embed::{Embedder, HashedEmbedder, vector_json};
use crate::resolver::{ContradictionResolution, ResolverCandidate, resolve_contradiction};
use crate::schema;
use crate::types::*;
use nano_session::{FileLock, JournalWriter, Op, OpEnvelope, read_journal};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct MemoryStore {
    db: Connection,
    journal: JournalWriter,
    embedder: HashedEmbedder,
    policy: MemoryPolicy,
    configured_agents: Option<ConfiguredAgents>,
    next_op: u64,
    _writer_lock: FileLock,
}

impl MemoryStore {
    pub fn open(
        nano_home: &Path,
        journal_path: &Path,
        policy: MemoryPolicy,
        active_agent: &str,
        configured_agents: ConfiguredAgents,
    ) -> MemoryResult<Self> {
        Self::open_inner(
            nano_home,
            journal_path,
            policy,
            Some(active_agent),
            Some(configured_agents),
        )
    }

    fn open_inner(
        nano_home: &Path,
        journal_path: &Path,
        policy: MemoryPolicy,
        active_agent: Option<&str>,
        configured_agents: Option<ConfiguredAgents>,
    ) -> MemoryResult<Self> {
        reject_network_path(nano_home)?;
        validate_policy(&policy)?;
        validate_active_agent(active_agent, configured_agents.as_ref())?;
        crate::register_sqlite_vec();
        let path = memory_db_path(nano_home);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer_lock = FileLock::try_acquire(&path.with_extension("memory.lock"))
            .map_err(|e| MemoryError::Contention(e.to_string()))?;
        let next_op = next_op_id(journal_path)?;
        let db = Connection::open(path)?;
        schema::migrate(&db)?;
        let store = Self {
            db,
            journal: JournalWriter::open(journal_path)?,
            embedder: HashedEmbedder,
            policy,
            configured_agents,
            next_op,
            _writer_lock: writer_lock,
        };
        Ok(store)
    }

    pub fn open_at(
        db_path: &Path,
        journal_path: &Path,
        policy: MemoryPolicy,
        active_agent: &str,
        configured_agents: ConfiguredAgents,
    ) -> MemoryResult<Self> {
        Self::open_at_inner(
            db_path,
            journal_path,
            policy,
            Some(active_agent),
            Some(configured_agents),
        )
    }

    fn open_at_inner(
        db_path: &Path,
        journal_path: &Path,
        policy: MemoryPolicy,
        active_agent: Option<&str>,
        configured_agents: Option<ConfiguredAgents>,
    ) -> MemoryResult<Self> {
        reject_network_path(db_path)?;
        validate_policy(&policy)?;
        validate_active_agent(active_agent, configured_agents.as_ref())?;
        crate::register_sqlite_vec();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer_lock = FileLock::try_acquire(&db_path.with_extension("memory.lock"))
            .map_err(|e| MemoryError::Contention(e.to_string()))?;
        let next_op = next_op_id(journal_path)?;
        let db = Connection::open(db_path)?;
        schema::migrate(&db)?;
        let store = Self {
            db,
            journal: JournalWriter::open(journal_path)?,
            embedder: HashedEmbedder,
            policy,
            configured_agents,
            next_op,
            _writer_lock: writer_lock,
        };
        Ok(store)
    }

    pub fn write_fact(&mut self, fact: FactWrite) -> MemoryResult<ContradictionResolution> {
        if fact.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.commit_fact(fact, false, || {})
    }

    /// Contracted fault-injection seam: the hook runs after the synced journal append
    /// and before the SQLite transaction, so a child process can terminate itself.
    pub fn write_fact_with_fault_injection<F: FnOnce()>(
        &mut self,
        fact: FactWrite,
        hook: F,
    ) -> MemoryResult<ContradictionResolution> {
        if fact.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.commit_fact(fact, false, hook)
    }

    pub(crate) fn commit_mediated_fact(
        &mut self,
        mut fact: FactWrite,
    ) -> MemoryResult<ContradictionResolution> {
        fact.source_trust = SourceTrust::ModelInference;
        self.commit_fact(fact, true, || {})
    }

    fn commit_fact<F: FnOnce()>(
        &mut self,
        fact: FactWrite,
        mediated: bool,
        hook: F,
    ) -> MemoryResult<ContradictionResolution> {
        self.check_write()?;
        validate_partition(&fact.project, &fact.agent_id)?;
        self.check_configured_agent(&fact.agent_id)?;
        self.ensure_id_available(&fact.id)?;
        if !mediated && fact.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        let resolution = self.fact_resolution(&fact)?;
        let op = Op::MemoryWriteFact {
            fact_id: fact.id.clone(),
            subject: fact.subject.clone(),
            predicate: fact.predicate.clone(),
            object: fact.object.clone(),
            confidence_micros: (fact.confidence.clamp(0.0, 1.0) * 1_000_000.0).round() as u32,
            source_episode: fact.source_episode.clone(),
            valid_from: fact.valid_from.clone(),
            valid_to: fact.valid_to.clone(),
            source_trust: fact.source_trust.as_str().into(),
            project: fact.project.clone(),
            agent_id: fact.agent_id.clone(),
            session_id: self.write_session_id()?,
            resolver_outcome: format!("{resolution:?}").to_lowercase(),
        };
        let system_time = self.append(op)?;
        if mediated {
            let message = format!("memory updated for {}", fact.agent_id);
            self.append_receipt(&fact.id, &fact.agent_id, &message)?;
        }
        hook();
        self.apply_fact_at(&fact, resolution, &system_time)?;
        Ok(resolution)
    }

    pub fn write_decision(&mut self, value: DecisionWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteDecision {
            decision_id: value.id.clone(),
            summary: value.summary.clone(),
            why: value.why.clone(),
            how_to_apply: value.how_to_apply.clone(),
            tags: value.tags.clone(),
            source_episode: value.source_episode.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        self.apply_decision_at(&value, &system_time)
    }
    pub(crate) fn commit_mediated_decision(
        &mut self,
        mut value: DecisionWrite,
    ) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteDecision {
            decision_id: value.id.clone(),
            summary: value.summary.clone(),
            why: value.why.clone(),
            how_to_apply: value.how_to_apply.clone(),
            tags: value.tags.clone(),
            source_episode: value.source_episode.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_decision_at(&value, &system_time)
    }
    pub fn write_episode(&mut self, value: EpisodeWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteEpisode {
            episode_id: value.id.clone(),
            content: value.content.clone(),
            source: value.source.clone(),
            source_product: value.source_product.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        self.apply_episode_at(&value, &system_time)
    }
    pub(crate) fn commit_mediated_episode(&mut self, mut value: EpisodeWrite) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteEpisode {
            episode_id: value.id.clone(),
            content: value.content.clone(),
            source: value.source.clone(),
            source_product: value.source_product.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_episode_at(&value, &system_time)
    }
    pub fn write_procedure(&mut self, value: ProcedureWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteProcedure {
            procedure_id: value.id.clone(),
            title: value.title.clone(),
            steps: value.steps.clone(),
            created_by: value.created_by.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        self.apply_procedure_at(&value, &system_time)
    }
    pub(crate) fn commit_mediated_procedure(
        &mut self,
        mut value: ProcedureWrite,
    ) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.check_configured_agent(&value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        let system_time = self.append(Op::MemoryWriteProcedure {
            procedure_id: value.id.clone(),
            title: value.title.clone(),
            steps: value.steps.clone(),
            created_by: value.created_by.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
            session_id: self.write_session_id()?,
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_procedure_at(&value, &system_time)
    }

    pub(crate) fn append_receipt(
        &mut self,
        write_id: &str,
        agent_id: &str,
        message: &str,
    ) -> MemoryResult<()> {
        self.append(Op::MemoryWriteReceipt {
            write_id: write_id.into(),
            agent_id: agent_id.into(),
            message: message.into(),
        })?;
        Ok(())
    }

    fn check_write(&self) -> MemoryResult<()> {
        if !self.policy.enabled || self.policy.write == WriteScope::Off {
            Err(MemoryError::Disabled)
        } else {
            Ok(())
        }
    }
    fn check_configured_agent(&self, agent_id: &str) -> MemoryResult<()> {
        if self
            .configured_agents
            .as_ref()
            .is_some_and(|configured| !configured.contains(agent_id))
        {
            Err(MemoryError::UnconfiguredAgent(agent_id.to_owned()))
        } else {
            Ok(())
        }
    }
    fn ensure_id_available(&self, id: &str) -> MemoryResult<()> {
        let exists: i64 = self.db.query_row("SELECT EXISTS(SELECT 1 FROM facts WHERE id=?1 UNION SELECT 1 FROM episodes WHERE id=?1 UNION SELECT 1 FROM decisions WHERE id=?1 UNION SELECT 1 FROM procedures WHERE id=?1)",[id],|r|r.get(0))?;
        if exists != 0 {
            Err(MemoryError::InvalidValue {
                field: "memory id",
                value: id.into(),
            })
        } else {
            Ok(())
        }
    }
    fn append(&mut self, op: Op) -> MemoryResult<String> {
        self.next_op += 1;
        let id = format!("memory-{}", self.next_op);
        let system_time = now();
        if !self
            .journal
            .append(&OpEnvelope::new(id, system_time.clone(), op))?
        {
            return Err(MemoryError::JournalIntegrity(
                "duplicate memory op id".into(),
            ));
        }
        Ok(system_time)
    }
    fn write_session_id(&self) -> MemoryResult<Option<String>> {
        match self.policy.write {
            WriteScope::SessionOnly => self.policy.session_id.clone().map(Some).ok_or_else(|| {
                MemoryError::UnsupportedPolicy("SessionOnly requires session_id".into())
            }),
            WriteScope::SessionAndProject => Ok(None),
            WriteScope::Off => Err(MemoryError::Disabled),
        }
    }
    fn write_session_key(&self) -> &str {
        match self.policy.write {
            WriteScope::SessionOnly => self.policy.session_id.as_deref().unwrap_or(""),
            WriteScope::SessionAndProject | WriteScope::Off => "",
        }
    }
    fn fact_resolution(&self, new: &FactWrite) -> MemoryResult<ContradictionResolution> {
        let old=self.db.query_row("SELECT object,confidence,source_trust FROM facts WHERE project=?1 AND agent_id=?2 AND session_id=?3 AND subject=?4 AND predicate=?5 AND valid_to IS NULL ORDER BY system_time DESC LIMIT 1",params![new.project,new.agent_id,self.write_session_key(),new.subject,new.predicate],|r|Ok((r.get::<_,String>(0)?,r.get::<_,f64>(1)?,r.get::<_,String>(2)?))).optional()?;
        match old {
            None => Ok(ContradictionResolution::Coexist),
            Some((object, confidence, tier)) => Ok(resolve_contradiction(
                ResolverCandidate {
                    object: &object,
                    confidence,
                    source_trust: SourceTrust::parse(&tier)?,
                },
                ResolverCandidate {
                    object: &new.object,
                    confidence: new.confidence,
                    source_trust: new.source_trust,
                },
            )),
        }
    }
    fn apply_fact_at(
        &mut self,
        v: &FactWrite,
        res: ContradictionResolution,
        system_time: &str,
    ) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let mut stored = v.clone();
        if res == ContradictionResolution::KeepExisting && stored.valid_to.is_none() {
            stored.valid_to = Some(stored.valid_from.clone());
        }
        let session_key = self.write_session_key().to_owned();
        let tx = self.db.transaction()?;
        if res == ContradictionResolution::Supersede {
            tx.execute("UPDATE facts SET valid_to=?1,superseded_by=?2 WHERE project=?3 AND agent_id=?4 AND session_id=?5 AND subject=?6 AND predicate=?7 AND valid_to IS NULL",params![v.valid_from,v.id,v.project,v.agent_id,session_key,v.subject,v.predicate])?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO facts VALUES(?1,?2,?3,?4,?5,?6,NULL,?7,?8,?9,?10,?11,?12,?13)",
            params![
                stored.id,
                stored.subject,
                stored.predicate,
                stored.object,
                stored.confidence,
                stored.source_episode,
                stored.valid_from,
                stored.valid_to,
                system_time,
                stored.source_trust.as_str(),
                stored.project,
                stored.agent_id,
                session_key
            ],
        )?;
        let text = format!("{} {} {}", v.subject, v.predicate, v.object);
        index(
            &tx,
            &v.id,
            "fact",
            &v.project,
            &v.agent_id,
            &session_key,
            &text,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_decision_at(&mut self, v: &DecisionWrite, system_time: &str) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let session_key = self.write_session_key().to_owned();
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO decisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                v.id,
                v.summary,
                v.why,
                v.how_to_apply,
                serde_json::to_string(&v.tags).unwrap_or_default(),
                v.source_episode,
                v.valid_from,
                v.valid_to,
                system_time,
                v.source_trust.as_str(),
                v.project,
                v.agent_id,
                session_key
            ],
        )?;
        let text = format!(
            "{} {} {} {}",
            v.summary,
            v.why,
            v.how_to_apply,
            v.tags.join(" ")
        );
        index(
            &tx,
            &v.id,
            "decision",
            &v.project,
            &v.agent_id,
            &session_key,
            &text,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_episode_at(&mut self, v: &EpisodeWrite, system_time: &str) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let session_key = self.write_session_key().to_owned();
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO episodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                v.id,
                v.content,
                v.source,
                v.source_product,
                v.valid_from,
                v.valid_to,
                system_time,
                v.source_trust.as_str(),
                v.project,
                v.agent_id,
                session_key
            ],
        )?;
        index(
            &tx,
            &v.id,
            "episode",
            &v.project,
            &v.agent_id,
            &session_key,
            &v.content,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_procedure_at(&mut self, v: &ProcedureWrite, system_time: &str) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let session_key = self.write_session_key().to_owned();
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO procedures VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                v.id,
                v.title,
                v.steps,
                v.created_by,
                v.valid_from,
                v.valid_to,
                system_time,
                v.source_trust.as_str(),
                v.project,
                v.agent_id,
                session_key
            ],
        )?;
        let text = format!("{} {}", v.title, v.steps);
        index(
            &tx,
            &v.id,
            "procedure",
            &v.project,
            &v.agent_id,
            &session_key,
            &text,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }

    fn enforce_caps(&self, project: &str, agent: &str) -> MemoryResult<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO retention_control VALUES(?1,?2,?3,?4,?5)",
            params![
                project,
                agent,
                self.policy.retention.episodes,
                self.policy.retention.facts,
                self.policy.retention.bytes
            ],
        )?;
        self.db.execute("DELETE FROM facts WHERE id IN(SELECT id FROM facts WHERE project=?1 AND agent_id=?2 ORDER BY (valid_to IS NULL),confidence,system_time LIMIT MAX(0,(SELECT COUNT(*) FROM facts WHERE project=?1 AND agent_id=?2)-?3))",params![project,agent,self.policy.retention.facts])?;
        self.db.execute("DELETE FROM episodes WHERE id IN(SELECT id FROM episodes WHERE project=?1 AND agent_id=?2 ORDER BY (valid_to IS NULL),system_time LIMIT MAX(0,(SELECT COUNT(*) FROM episodes WHERE project=?1 AND agent_id=?2)-?3))",params![project,agent,self.policy.retention.episodes])?;
        let mut statement=self.db.prepare("SELECT id,length(CAST(subject AS BLOB))+length(CAST(predicate AS BLOB))+length(CAST(object AS BLOB)),CASE WHEN valid_to IS NULL THEN 1 ELSE 0 END,confidence,system_time FROM facts WHERE project=?1 AND agent_id=?2 UNION ALL SELECT id,length(CAST(content AS BLOB)),CASE WHEN valid_to IS NULL THEN 1 ELSE 0 END,1.0,system_time FROM episodes WHERE project=?1 AND agent_id=?2 UNION ALL SELECT id,length(CAST(summary AS BLOB))+length(CAST(why AS BLOB))+length(CAST(how_to_apply AS BLOB)),CASE WHEN valid_to IS NULL THEN 1 ELSE 0 END,1.0,system_time FROM decisions WHERE project=?1 AND agent_id=?2 UNION ALL SELECT id,length(CAST(title AS BLOB))+length(CAST(steps AS BLOB)),CASE WHEN valid_to IS NULL THEN 1 ELSE 0 END,1.0,system_time FROM procedures WHERE project=?1 AND agent_id=?2 ORDER BY 3,4,5")?;
        let rows = statement
            .query_map(params![project, agent], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut total: u64 = rows.iter().map(|(_, bytes)| *bytes).sum();
        for (id, bytes) in rows {
            if total <= self.policy.retention.bytes {
                break;
            }
            self.delete_indexed_row(&id)?;
            total = total.saturating_sub(bytes);
        }
        self.db.execute("DELETE FROM memory_fts WHERE id NOT IN(SELECT id FROM facts UNION SELECT id FROM episodes UNION SELECT id FROM decisions UNION SELECT id FROM procedures)",[])?;
        self.db.execute("DELETE FROM memory_vec WHERE record_id NOT IN(SELECT id FROM facts UNION SELECT id FROM episodes UNION SELECT id FROM decisions UNION SELECT id FROM procedures)",[])?;
        Ok(())
    }

    fn delete_indexed_row(&self, id: &str) -> MemoryResult<()> {
        for table in ["facts", "episodes", "decisions", "procedures"] {
            self.db
                .execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
        }
        Ok(())
    }

    pub fn retrieve(&self, q: &RetrieveQuery) -> MemoryResult<Vec<RetrieveHit>> {
        Ok(self.retrieve_with_evidence(q)?.assembled)
    }

    /// Apply authoritative memory journal records to this already-open store
    /// without appending them again. Used by crash recovery and independent
    /// replay verification; the configured-agent boundary remains active.
    pub fn replay_journals(&mut self, journals: &[PathBuf]) -> MemoryResult<()> {
        replay_journals_into_store(self, journals)
    }

    pub fn retrieve_with_evidence(&self, q: &RetrieveQuery) -> MemoryResult<RetrievalEvidence> {
        if !self.policy.enabled {
            return Err(MemoryError::Disabled);
        }
        if self.policy.read_scope == ReadScope::Session && self.policy.session_id.is_none() {
            return Err(MemoryError::UnsupportedPolicy(
                "Session read scope requires session_id".into(),
            ));
        }
        validate_partition(&q.project, &q.agent_id)?;
        if matches!(q.agent_scope, AgentScope::Explicit(_)) {
            return Err(MemoryError::InvalidValue {
                field: "agent_scope",
                value: "explicit cross-agent reads are not active".into(),
            });
        }
        let ids = q.agent_scope.ids(&q.agent_id);
        if ids.is_empty() {
            return Ok(RetrievalEvidence {
                fts_hits: 0,
                knn_hits: 0,
                fts_ids: Vec::new(),
                knn_ids: Vec::new(),
                assembled: Vec::new(),
            });
        }
        let agents = serde_json::to_string(&ids)
            .map_err(|e| MemoryError::JournalIntegrity(e.to_string()))?;
        let mut session_keys = vec![String::new()];
        if let Some(session_id) = &self.policy.session_id {
            if self.policy.read_scope == ReadScope::Session {
                session_keys.clear();
            }
            session_keys.push(session_id.clone());
        }
        let session_keys_json = serde_json::to_string(&session_keys)
            .map_err(|e| MemoryError::JournalIntegrity(e.to_string()))?;
        let match_query = q
            .text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut fts_ids = Vec::new();
        if !match_query.is_empty() {
            let mut s=self.db.prepare("SELECT id FROM memory_fts WHERE memory_fts MATCH ?1 AND project=?2 AND agent_id IN(SELECT value FROM json_each(?3)) AND session_id IN(SELECT value FROM json_each(?4)) ORDER BY bm25(memory_fts) LIMIT 100")?;
            for (rank, id) in s
                .query_map(
                    params![match_query, q.project, agents, session_keys_json],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .enumerate()
            {
                fts_ids.push(id.clone());
                *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
            }
        }
        let fts_evidence = self.checkpoint_identities(&fts_ids)?;
        self.assert_checkpoint_partitions(&fts_evidence, q, &ids, &session_keys)?;
        let vector = vector_json(&self.embedder.embed(&q.text));
        let mut knn = Vec::new();
        for agent in &ids {
            for session_key in &session_keys {
                let mut s=self.db.prepare("SELECT record_id,distance FROM memory_vec WHERE embedding MATCH ?1 AND k=100 AND project=?2 AND agent_id=?3 AND session_id=?4 ORDER BY distance")?;
                knn.extend(
                    s.query_map(params![vector, q.project, agent, session_key], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?,
                );
            }
        }
        knn.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let knn_ids = knn.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let knn_evidence = self.checkpoint_identities(&knn_ids)?;
        self.assert_checkpoint_partitions(&knn_evidence, q, &ids, &session_keys)?;
        for (rank, (id, _)) in knn.into_iter().enumerate() {
            *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
        }
        let mut hits = Vec::new();
        for (id, score) in scores {
            if let Some((mut hit, session_id)) = self.load_hit(&id)? {
                if hit.project != q.project || !ids.contains(&hit.agent_id) {
                    return Err(MemoryError::JournalIntegrity(
                        "partition assertion failed".into(),
                    ));
                }
                if !session_keys.contains(&session_id) {
                    return Err(MemoryError::JournalIntegrity(
                        "session partition assertion failed".into(),
                    ));
                }
                let floor = self.policy.min_tier.rank().max(q.min_tier.rank());
                if hit.valid_to.is_none() && hit.source_trust.rank() >= floor {
                    hit.score = score * hit.source_trust.weight();
                    hits.push(hit);
                }
            }
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        let mut source_counts: HashMap<String, usize> = HashMap::new();
        let mut tokens = 0usize;
        hits.retain(|hit| {
            if let Some(source) = &hit.source_episode {
                let count = source_counts.entry(source.clone()).or_default();
                if *count >= 2 {
                    return false;
                }
                *count += 1;
            }
            let item_tokens = hit.text.split_whitespace().count();
            if tokens.saturating_add(item_tokens) > q.token_budget {
                return false;
            }
            tokens += item_tokens;
            true
        });
        hits.truncate(q.limit);
        Ok(RetrievalEvidence {
            fts_hits: fts_ids.len(),
            knn_hits: knn_ids.len(),
            fts_ids: fts_evidence,
            knn_ids: knn_evidence,
            assembled: hits,
        })
    }
    fn checkpoint_identities(&self, record_ids: &[String]) -> MemoryResult<Vec<RetrievalIdentity>> {
        record_ids
            .iter()
            .map(|id| {
                let Some((hit, session_id)) = self.load_hit(id)? else {
                    return Err(MemoryError::JournalIntegrity(
                        "retrieval checkpoint references a missing row".into(),
                    ));
                };
                Ok(RetrievalIdentity {
                    id: hit.id,
                    project: hit.project,
                    agent_id: hit.agent_id,
                    session_id,
                })
            })
            .collect()
    }
    fn assert_checkpoint_partitions(
        &self,
        identities: &[RetrievalIdentity],
        query: &RetrieveQuery,
        agent_ids: &[String],
        session_keys: &[String],
    ) -> MemoryResult<()> {
        for identity in identities {
            if identity.project != query.project || !agent_ids.contains(&identity.agent_id) {
                return Err(MemoryError::JournalIntegrity(
                    "retrieval checkpoint partition assertion failed".into(),
                ));
            }
            if !session_keys.contains(&identity.session_id) {
                return Err(MemoryError::JournalIntegrity(
                    "retrieval checkpoint session assertion failed".into(),
                ));
            }
        }
        Ok(())
    }
    fn load_hit(&self, id: &str) -> MemoryResult<Option<(RetrieveHit, String)>> {
        self.db.query_row("SELECT id,kind,text,confidence,source_episode,valid_from,valid_to,source_trust,project,agent_id,session_id FROM (SELECT id,'fact' kind,subject||' '||predicate||' '||object text,confidence,source_episode,valid_from,valid_to,source_trust,project,agent_id,session_id FROM facts UNION ALL SELECT id,'decision',summary||' '||why||' '||how_to_apply,1.0,source_episode,valid_from,valid_to,source_trust,project,agent_id,session_id FROM decisions UNION ALL SELECT id,'episode',content,1.0,NULL,valid_from,valid_to,source_trust,project,agent_id,session_id FROM episodes UNION ALL SELECT id,'procedure',title||' '||steps,1.0,NULL,valid_from,valid_to,source_trust,project,agent_id,session_id FROM procedures) WHERE id=?1",[id],|r|Ok((RetrieveHit{id:r.get(0)?,kind:r.get(1)?,text:r.get(2)?,score:0.0,confidence:r.get(3)?,source_episode:r.get(4)?,valid_from:r.get(5)?,valid_to:r.get(6)?,source_trust:SourceTrust::parse(&r.get::<_,String>(7)?).map_err(|_|rusqlite::Error::InvalidQuery)?,project:r.get(8)?,agent_id:r.get(9)?}, r.get(10)?))).optional().map_err(Into::into)
    }

    pub fn current_facts(&self) -> MemoryResult<Vec<FactState>> {
        let mut stmt=self.db.prepare("SELECT id,valid_from,valid_to,source_trust,project,agent_id FROM facts WHERE valid_to IS NULL ORDER BY id")?;
        stmt.query_map([], |r| {
            Ok(FactState {
                id: r.get(0)?,
                valid_from: r.get(1)?,
                valid_to: r.get(2)?,
                source_trust: SourceTrust::parse(&r.get::<_, String>(3)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                project: r.get(4)?,
                agent_id: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
}

#[allow(clippy::too_many_arguments)]
fn index(
    db: &Connection,
    id: &str,
    kind: &str,
    project: &str,
    agent: &str,
    session_id: &str,
    text: &str,
    embedder: &dyn Embedder,
) -> MemoryResult<()> {
    db.execute(
        "INSERT INTO memory_fts(id,kind,project,agent_id,session_id,text) VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, kind, project, agent, session_id, text],
    )?;
    db.execute(
        "INSERT INTO memory_vec(record_id,project,agent_id,session_id,embedding) VALUES(?1,?2,?3,?4,?5)",
        params![id, project, agent, session_id, vector_json(&embedder.embed(text))],
    )?;
    Ok(())
}
fn now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

fn next_op_id(path: &Path) -> MemoryResult<u64> {
    Ok(read_journal(path)?
        .envelopes
        .iter()
        .filter_map(|e| e.id.strip_prefix("memory-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0))
}

#[derive(Debug, Clone)]
pub struct LegacyMigrationWrite {
    pub fact: FactWrite,
    pub session_id: String,
    pub receipt_message: String,
}

#[derive(Debug, Clone)]
pub struct LegacyMigrationCompletion {
    pub agent_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMigrationResult {
    pub ingested_ids: Vec<String>,
    pub skipped_ids: Vec<String>,
}

#[derive(Debug)]
pub enum LegacyMigrationError {
    Memory(MemoryError),
    AlreadyCompleted,
    CompletionCollision,
    InvalidJournal(String),
}

impl std::fmt::Display for LegacyMigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory(error) => write!(formatter, "{error}"),
            Self::AlreadyCompleted => formatter.write_str("legacy migration already completed"),
            Self::CompletionCollision => {
                formatter.write_str("legacy migration completion collision")
            }
            Self::InvalidJournal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for LegacyMigrationError {}

impl From<MemoryError> for LegacyMigrationError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<std::io::Error> for LegacyMigrationError {
    fn from(error: std::io::Error) -> Self {
        Self::Memory(MemoryError::Journal(error))
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "migration binds path, policy, identity, candidate, completion, and fault authorities independently"
)]
pub fn migrate_legacy_facts_with_fault_injection<F>(
    nano_home: &Path,
    journal_path: &Path,
    policy: MemoryPolicy,
    active_agent: &str,
    configured_agents: ConfiguredAgents,
    writes: &[LegacyMigrationWrite],
    completion: Option<&LegacyMigrationCompletion>,
    fault: F,
) -> Result<LegacyMigrationResult, LegacyMigrationError>
where
    F: FnOnce() -> MemoryResult<()>,
{
    reject_network_path(nano_home)?;
    validate_policy(&policy)?;
    validate_active_agent(Some(active_agent), Some(&configured_agents))?;
    for write in writes {
        validate_partition(&write.fact.project, &write.fact.agent_id)?;
        validate_recovery_partition(
            &write.fact.project,
            &write.fact.agent_id,
            Some(&write.session_id),
        )?;
        if !configured_agents.contains(&write.fact.agent_id) {
            return Err(MemoryError::UnconfiguredAgent(write.fact.agent_id.clone()).into());
        }
        if write.fact.source_trust != SourceTrust::ModelInference {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "legacy migration candidate {} is not ModelInference",
                write.fact.id
            )));
        }
    }

    let db_path = memory_db_path(nano_home);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _target_lock = FileLock::try_acquire(&db_path.with_extension("memory.lock"))
        .map_err(|error| MemoryError::Contention(error.to_string()))?;
    let snapshot = read_journal(journal_path)?.envelopes;
    validate_migration_completion(&snapshot, completion)?;

    let candidates = writes
        .iter()
        .map(|write| (write.fact.id.as_str(), write))
        .collect::<HashMap<_, _>>();
    if candidates.len() != writes.len() {
        return Err(LegacyMigrationError::InvalidJournal(
            "duplicate legacy migration candidate id".into(),
        ));
    }

    for candidate in writes {
        let canonical_id = migration_write_envelope_id(&candidate.fact.id);
        let matching = snapshot
            .iter()
            .filter(|row| {
                matches!(&row.op, Op::MemoryWriteFact { fact_id, .. }
                    if fact_id == &candidate.fact.id)
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 || matching.first().is_some_and(|row| row.id != canonical_id) {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "candidate fact {} has noncanonical write authority",
                candidate.fact.id
            )));
        }
    }

    let mut existing = HashMap::<String, (usize, &LegacyMigrationWrite, bool)>::new();
    for (index, envelope) in snapshot.iter().enumerate() {
        if !envelope.id.starts_with("legacy-migration-write-") {
            continue;
        }
        let fact_id = match &envelope.op {
            Op::MemoryWriteFact { fact_id, .. } => fact_id,
            _ => {
                return Err(LegacyMigrationError::InvalidJournal(format!(
                    "reserved migration write id {} carries another op",
                    envelope.id
                )));
            }
        };
        let candidate = candidates.get(fact_id.as_str()).copied().ok_or_else(|| {
            LegacyMigrationError::InvalidJournal(format!(
                "orphaned existing migration write {fact_id}"
            ))
        })?;
        let expected = resolve_legacy_write_at_prefix(
            &db_path,
            &snapshot[..index],
            &policy,
            &configured_agents,
            candidate,
        )?;
        if envelope.id != expected.id || envelope.op != expected.op {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "resolver outcome mismatch for existing migration write {fact_id}"
            )));
        }
        let receipts = snapshot
            .iter()
            .enumerate()
            .filter(|(_, row)| migration_receipt_targets(row, candidate))
            .collect::<Vec<_>>();
        if receipts.len() > 1
            || receipts
                .first()
                .is_some_and(|(_, receipt)| !migration_receipt_matches(receipt, candidate))
            || receipts
                .first()
                .is_some_and(|(receipt_index, _)| *receipt_index != index + 1)
        {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "invalid or noncausal migration receipt for {fact_id}"
            )));
        }
        if existing
            .insert(fact_id.clone(), (index, candidate, !receipts.is_empty()))
            .is_some()
        {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "duplicate existing migration write {fact_id}"
            )));
        }
    }

    for candidate in writes {
        let write_id = migration_write_envelope_id(&candidate.fact.id);
        if let Some(row) = snapshot.iter().find(|row| row.id == write_id)
            && !matches!(&row.op, Op::MemoryWriteFact { fact_id, .. } if fact_id == &candidate.fact.id)
        {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "authoritative envelope id collision for {}",
                candidate.fact.id
            )));
        }
        let receipt_id = migration_receipt_envelope_id(&candidate.fact.id);
        if let Some(row) = snapshot.iter().find(|row| row.id == receipt_id)
            && !migration_receipt_matches(row, candidate)
        {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "invalid reserved migration receipt for {}",
                candidate.fact.id
            )));
        }
        let targeted = snapshot
            .iter()
            .filter(|row| migration_receipt_targets(row, candidate))
            .collect::<Vec<_>>();
        if targeted.len() > 1
            || targeted
                .first()
                .is_some_and(|row| !migration_receipt_matches(row, candidate))
        {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "invalid migration receipt identity for {}",
                candidate.fact.id
            )));
        }
    }

    let unreceipted = existing
        .iter()
        .filter(|(_, (_, _, receipted))| !receipted)
        .collect::<Vec<_>>();
    if unreceipted.len() > 1 {
        return Err(LegacyMigrationError::InvalidJournal(
            "multiple unreceipted migration writes".into(),
        ));
    }
    let torn_fact_id = if let Some((fact_id, (index, _, _))) = unreceipted.first() {
        if *index + 1 != snapshot.len() {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "unreceipted migration write {fact_id} is not the journal tail"
            )));
        }
        Some((*fact_id).clone())
    } else {
        None
    };

    let mut working = snapshot;
    let mut writer = JournalWriter::open(journal_path)?;
    let mut ingested_ids = Vec::new();
    let mut skipped_ids = Vec::new();
    if let Some(fact_id) = torn_fact_id {
        let candidate = candidates[fact_id.as_str()];
        let receipt = migration_receipt_envelope(candidate);
        if !writer.append(&receipt)? {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "missing receipt for {fact_id} could not be repaired"
            )));
        }
        working.push(receipt);
        skipped_ids.push(fact_id);
    }
    for candidate in writes {
        if existing.contains_key(&candidate.fact.id) {
            if !skipped_ids.contains(&candidate.fact.id) {
                skipped_ids.push(candidate.fact.id.clone());
            }
            continue;
        }
        let authoritative = resolve_legacy_write_at_prefix(
            &db_path,
            &working,
            &policy,
            &configured_agents,
            candidate,
        )?;
        if !writer.append(&authoritative)? {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "authoritative envelope id collision for {}",
                candidate.fact.id
            )));
        }
        working.push(authoritative);
        let receipt = migration_receipt_envelope(candidate);
        if !writer.append(&receipt)? {
            return Err(LegacyMigrationError::InvalidJournal(format!(
                "migration receipt id collision for {}",
                candidate.fact.id
            )));
        }
        working.push(receipt);
        ingested_ids.push(candidate.fact.id.clone());
    }
    drop(writer);
    fault()?;
    rebuild_from_journals_locked(
        &db_path,
        std::slice::from_ref(&journal_path.to_path_buf()),
        policy,
        configured_agents,
    )?;
    if let Some(completion) = completion {
        let envelope = migration_completion_envelope(completion);
        let appended = JournalWriter::open(journal_path)?.append(&envelope)?;
        if !appended {
            let exact = read_journal(journal_path)?
                .envelopes
                .into_iter()
                .any(|row| row == envelope);
            if !exact {
                return Err(LegacyMigrationError::CompletionCollision);
            }
        }
    }
    Ok(LegacyMigrationResult {
        ingested_ids,
        skipped_ids,
    })
}

fn migration_write_envelope_id(fact_id: &str) -> String {
    format!("legacy-migration-write-{fact_id}")
}

fn migration_receipt_envelope_id(fact_id: &str) -> String {
    format!("legacy-migration-receipt-{fact_id}")
}

fn migration_receipt_targets(envelope: &OpEnvelope, write: &LegacyMigrationWrite) -> bool {
    matches!(&envelope.op, Op::MemoryWriteReceipt { write_id, agent_id, .. }
        if write_id == &write.fact.id && agent_id == &write.fact.agent_id)
}

fn migration_receipt_matches(envelope: &OpEnvelope, write: &LegacyMigrationWrite) -> bool {
    envelope.id == migration_receipt_envelope_id(&write.fact.id)
        && matches!(&envelope.op, Op::MemoryWriteReceipt { write_id, agent_id, message }
            if write_id == &write.fact.id
                && agent_id == &write.fact.agent_id
                && message == &write.receipt_message)
}

fn migration_receipt_envelope(write: &LegacyMigrationWrite) -> OpEnvelope {
    OpEnvelope::new(
        migration_receipt_envelope_id(&write.fact.id),
        now(),
        Op::MemoryWriteReceipt {
            write_id: write.fact.id.clone(),
            agent_id: write.fact.agent_id.clone(),
            message: write.receipt_message.clone(),
        },
    )
}

fn migration_completion_envelope(completion: &LegacyMigrationCompletion) -> OpEnvelope {
    OpEnvelope::new(
        "legacy-migration-complete",
        now(),
        Op::MemoryWriteReceipt {
            write_id: "legacy-migration-complete".into(),
            agent_id: completion.agent_id.clone(),
            message: completion.message.clone(),
        },
    )
}

fn validate_migration_completion(
    snapshot: &[OpEnvelope],
    completion: Option<&LegacyMigrationCompletion>,
) -> Result<(), LegacyMigrationError> {
    let Some(row) = snapshot
        .iter()
        .find(|row| row.id == "legacy-migration-complete")
    else {
        if snapshot.iter().any(|row| matches!(&row.op, Op::MemoryWriteReceipt { write_id, .. } if write_id == "legacy-migration-complete")) {
            return Err(LegacyMigrationError::CompletionCollision);
        }
        return Ok(());
    };
    let Some(completion) = completion else {
        return Err(LegacyMigrationError::AlreadyCompleted);
    };
    if row.op == migration_completion_envelope(completion).op {
        Err(LegacyMigrationError::AlreadyCompleted)
    } else {
        Err(LegacyMigrationError::CompletionCollision)
    }
}

fn resolve_legacy_write_at_prefix(
    db_path: &Path,
    prefix: &[OpEnvelope],
    policy: &MemoryPolicy,
    configured_agents: &ConfiguredAgents,
    write: &LegacyMigrationWrite,
) -> Result<OpEnvelope, LegacyMigrationError> {
    let suffix = format!("migration-resolve-{}-{}", std::process::id(), now());
    let scratch_db = db_path.with_extension(format!("{suffix}.db"));
    let prefix_journal = db_path.with_extension(format!("{suffix}.prefix.jsonl"));
    let output_journal = db_path.with_extension(format!("{suffix}.output.jsonl"));
    let scratch = MigrationScratch::new(scratch_db, prefix_journal, output_journal);
    let mut prefix_writer = JournalWriter::open(&scratch.prefix_journal)?;
    for envelope in prefix {
        prefix_writer.append(envelope)?;
    }
    drop(prefix_writer);
    rebuild_from_journals(
        &scratch.db,
        std::slice::from_ref(&scratch.prefix_journal),
        policy.clone(),
        configured_agents.clone(),
    )?;
    let mut store = MemoryStore::open_at(
        &scratch.db,
        &scratch.output_journal,
        policy.clone(),
        &write.fact.agent_id,
        configured_agents.clone(),
    )?;
    store.commit_mediated_fact(write.fact.clone())?;
    drop(store);
    let mut expected = read_journal(&scratch.output_journal)?
        .envelopes
        .into_iter()
        .find(|row| matches!(&row.op, Op::MemoryWriteFact { fact_id, .. } if fact_id == &write.fact.id))
        .ok_or_else(|| {
            LegacyMigrationError::InvalidJournal(format!(
                "resolver emitted no write for {}",
                write.fact.id
            ))
        })?;
    expected.id = migration_write_envelope_id(&write.fact.id);
    if let Op::MemoryWriteFact { session_id, .. } = &mut expected.op {
        *session_id = Some(write.session_id.clone());
    }
    Ok(expected)
}

struct MigrationScratch {
    db: PathBuf,
    prefix_journal: PathBuf,
    output_journal: PathBuf,
}

impl MigrationScratch {
    fn new(db: PathBuf, prefix_journal: PathBuf, output_journal: PathBuf) -> Self {
        Self {
            db,
            prefix_journal,
            output_journal,
        }
    }
}

impl Drop for MigrationScratch {
    fn drop(&mut self) {
        for path in [
            self.db.clone(),
            self.db.with_extension("memory.lock"),
            self.prefix_journal.clone(),
            self.output_journal.clone(),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn rebuild_from_journals(
    db_path: &Path,
    journals: &[PathBuf],
    policy: MemoryPolicy,
    configured_agents: ConfiguredAgents,
) -> MemoryResult<()> {
    reject_network_path(db_path)?;
    validate_policy(&policy)?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Acquire the authoritative target lock before creating, removing, or
    // replacing any database artifact. Contention therefore cannot mutate the
    // original database by even one byte.
    let _target_lock = FileLock::try_acquire(&db_path.with_extension("memory.lock"))
        .map_err(|e| MemoryError::Contention(e.to_string()))?;
    rebuild_from_journals_locked(db_path, journals, policy, configured_agents)
}

fn rebuild_from_journals_locked(
    db_path: &Path,
    journals: &[PathBuf],
    policy: MemoryPolicy,
    configured_agents: ConfiguredAgents,
) -> MemoryResult<()> {
    let suffix = format!("rebuild-{}", std::process::id());
    let temp_db = db_path.with_extension(format!("{suffix}.db"));
    let temp_journal = db_path.with_extension(format!("{suffix}.jsonl"));
    let temp_lock = temp_db.with_extension("memory.lock");
    if temp_db.exists() || temp_journal.exists() {
        return Err(MemoryError::Contention(
            "stale rebuild sibling exists".into(),
        ));
    }
    let result = rebuild_into_sibling(&temp_db, &temp_journal, journals, policy, configured_agents);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp_db);
        let _ = std::fs::remove_file(&temp_journal);
        let _ = std::fs::remove_file(&temp_lock);
        return Err(error);
    }
    let synced_db = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp_db)
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("open rebuilt database for sync: {error}"),
            )
        })?;
    synced_db.sync_all().map_err(|error| {
        std::io::Error::new(error.kind(), format!("sync rebuilt database: {error}"))
    })?;
    drop(synced_db);
    if let Err(error) = platform_replace(&temp_db, db_path) {
        let _ = std::fs::remove_file(&temp_db);
        let _ = std::fs::remove_file(&temp_journal);
        let _ = std::fs::remove_file(&temp_lock);
        return Err(error);
    }
    sync_parent(db_path)?;
    let _ = std::fs::remove_file(&temp_journal);
    let _ = std::fs::remove_file(&temp_lock);
    Ok(())
}

fn rebuild_into_sibling(
    temp_db: &Path,
    temp_journal: &Path,
    journals: &[PathBuf],
    policy: MemoryPolicy,
    configured_agents: ConfiguredAgents,
) -> MemoryResult<()> {
    let mut store =
        MemoryStore::open_at_inner(temp_db, temp_journal, policy, None, Some(configured_agents))?;
    replay_journals_into_store(&mut store, journals)?;
    drop(store);
    Ok(())
}

fn replay_journals_into_store(store: &mut MemoryStore, journals: &[PathBuf]) -> MemoryResult<()> {
    for path in journals {
        let report = read_journal(path)?;
        let receipts: HashSet<(String, String)> = report
            .envelopes
            .iter()
            .filter_map(|e| match &e.op {
                Op::MemoryWriteReceipt {
                    write_id, agent_id, ..
                } => Some((write_id.clone(), agent_id.clone())),
                _ => None,
            })
            .collect();
        for envelope in report.envelopes {
            match envelope.op {
                // Session policy records are audit-only. The explicitly
                // supplied resolved policy is the sole rebuild authority.
                Op::MemoryPolicyResolved { .. } => {}
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
                    validate_recovery_partition(&project, &agent_id, session_id.as_deref())?;
                    store.check_configured_agent(&agent_id)?;
                    if source_trust == "ModelInference"
                        && !receipts.contains(&(fact_id.clone(), agent_id.clone()))
                    {
                        continue;
                    }
                    set_recovery_session(&mut store.policy, session_id);
                    let r = match resolver_outcome.as_str() {
                        "supersede" => ContradictionResolution::Supersede,
                        "keepexisting" | "keep_existing" => ContradictionResolution::KeepExisting,
                        _ => ContradictionResolution::Coexist,
                    };
                    store.apply_fact_at(
                        &FactWrite {
                            id: fact_id,
                            subject,
                            predicate,
                            object,
                            confidence: f64::from(confidence_micros) / 1_000_000.0,
                            source_episode,
                            valid_from,
                            valid_to,
                            source_trust: SourceTrust::parse(&source_trust)?,
                            project,
                            agent_id,
                        },
                        r,
                        &envelope.ts,
                    )?;
                }
                Op::MemoryWriteDecision {
                    decision_id,
                    summary,
                    why,
                    how_to_apply,
                    tags,
                    source_episode,
                    valid_from,
                    valid_to,
                    source_trust,
                    project,
                    agent_id,
                    session_id,
                } => {
                    validate_recovery_partition(&project, &agent_id, session_id.as_deref())?;
                    store.check_configured_agent(&agent_id)?;
                    if source_trust == "ModelInference"
                        && !receipts.contains(&(decision_id.clone(), agent_id.clone()))
                    {
                        continue;
                    }
                    set_recovery_session(&mut store.policy, session_id);
                    store.apply_decision_at(
                        &DecisionWrite {
                            id: decision_id,
                            summary,
                            why,
                            how_to_apply,
                            tags,
                            source_episode,
                            valid_from,
                            valid_to,
                            source_trust: SourceTrust::parse(&source_trust)?,
                            project,
                            agent_id,
                        },
                        &envelope.ts,
                    )?
                }
                Op::MemoryWriteEpisode {
                    episode_id,
                    content,
                    source,
                    source_product,
                    valid_from,
                    valid_to,
                    source_trust,
                    project,
                    agent_id,
                    session_id,
                } => {
                    validate_recovery_partition(&project, &agent_id, session_id.as_deref())?;
                    store.check_configured_agent(&agent_id)?;
                    if source_trust == "ModelInference"
                        && !receipts.contains(&(episode_id.clone(), agent_id.clone()))
                    {
                        continue;
                    }
                    set_recovery_session(&mut store.policy, session_id);
                    store.apply_episode_at(
                        &EpisodeWrite {
                            id: episode_id,
                            content,
                            source,
                            source_product,
                            valid_from,
                            valid_to,
                            source_trust: SourceTrust::parse(&source_trust)?,
                            project,
                            agent_id,
                        },
                        &envelope.ts,
                    )?
                }
                Op::MemoryWriteProcedure {
                    procedure_id,
                    title,
                    steps,
                    created_by,
                    valid_from,
                    valid_to,
                    source_trust,
                    project,
                    agent_id,
                    session_id,
                } => {
                    validate_recovery_partition(&project, &agent_id, session_id.as_deref())?;
                    store.check_configured_agent(&agent_id)?;
                    if source_trust == "ModelInference"
                        && !receipts.contains(&(procedure_id.clone(), agent_id.clone()))
                    {
                        continue;
                    }
                    set_recovery_session(&mut store.policy, session_id);
                    store.apply_procedure_at(
                        &ProcedureWrite {
                            id: procedure_id,
                            title,
                            steps,
                            created_by,
                            valid_from,
                            valid_to,
                            source_trust: SourceTrust::parse(&source_trust)?,
                            project,
                            agent_id,
                        },
                        &envelope.ts,
                    )?
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn set_recovery_session(policy: &mut MemoryPolicy, session_id: Option<String>) {
    policy.session_id = session_id;
}

fn validate_recovery_partition(
    project: &str,
    agent: &str,
    session: Option<&str>,
) -> MemoryResult<()> {
    validate_partition(project, agent)?;
    if let Some(session) = session {
        if session.is_empty() || session.len() > 128 || session.chars().any(char::is_control) {
            return Err(MemoryError::InvalidValue {
                field: "session_id",
                value: session.into(),
            });
        }
    }
    Ok(())
}

fn validate_policy(policy: &MemoryPolicy) -> MemoryResult<()> {
    if policy.deletion == DeletionRule::HardDelete {
        return Err(MemoryError::UnsupportedPolicy(
            "HardDelete requires a journaled explicit delete op, which P-MEM-1 does not expose"
                .into(),
        ));
    }
    if matches!(policy.write, WriteScope::SessionOnly)
        || matches!(policy.read_scope, ReadScope::Session)
    {
        validate_recovery_partition("policy", "main", policy.session_id.as_deref())?;
        if policy.session_id.is_none() {
            return Err(MemoryError::UnsupportedPolicy(
                "session scope requires session_id".into(),
            ));
        }
    }
    Ok(())
}

fn validate_active_agent(
    active_agent: Option<&str>,
    configured_agents: Option<&ConfiguredAgents>,
) -> MemoryResult<()> {
    match (active_agent, configured_agents) {
        (None, None | Some(_)) => Ok(()),
        (Some(agent_id), Some(configured)) => {
            validate_partition("active-agent", agent_id)?;
            if configured.contains(agent_id) {
                Ok(())
            } else {
                Err(MemoryError::UnconfiguredAgent(agent_id.to_owned()))
            }
        }
        (Some(_), None) => Err(MemoryError::UnsupportedPolicy(
            "active_agent and configured_agents must be supplied together".into(),
        )),
    }
}

#[cfg(windows)]
fn platform_replace(source: &Path, target: &Path) -> MemoryResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        return Err(std::io::Error::new(
            error.kind(),
            format!("atomically replace rebuilt database: {error}"),
        )
        .into());
    }
    Ok(())
}

#[cfg(unix)]
fn platform_replace(source: &Path, target: &Path) -> MemoryResult<()> {
    std::fs::rename(source, target)?;
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn platform_replace(_source: &Path, _target: &Path) -> MemoryResult<()> {
    Err(MemoryError::UnsupportedPolicy(
        "no atomic replacement primitive".into(),
    ))
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> MemoryResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> MemoryResult<()> {
    Ok(())
}
