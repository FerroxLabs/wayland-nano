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
    next_op: u64,
    _writer_lock: FileLock,
}

impl MemoryStore {
    pub fn open(nano_home: &Path, journal_path: &Path, policy: MemoryPolicy) -> MemoryResult<Self> {
        reject_network_path(nano_home)?;
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
        Ok(Self {
            db,
            journal: JournalWriter::open(journal_path)?,
            embedder: HashedEmbedder,
            policy,
            next_op,
            _writer_lock: writer_lock,
        })
    }

    pub fn open_at(
        db_path: &Path,
        journal_path: &Path,
        policy: MemoryPolicy,
    ) -> MemoryResult<Self> {
        reject_network_path(db_path)?;
        crate::register_sqlite_vec();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer_lock = FileLock::try_acquire(&db_path.with_extension("memory.lock"))
            .map_err(|e| MemoryError::Contention(e.to_string()))?;
        let next_op = next_op_id(journal_path)?;
        let db = Connection::open(db_path)?;
        schema::migrate(&db)?;
        Ok(Self {
            db,
            journal: JournalWriter::open(journal_path)?,
            embedder: HashedEmbedder,
            policy,
            next_op,
            _writer_lock: writer_lock,
        })
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
            resolver_outcome: format!("{resolution:?}").to_lowercase(),
        };
        self.append(op)?;
        if mediated {
            let message = format!("memory updated for {}", fact.agent_id);
            self.append_receipt(&fact.id, &fact.agent_id, &message)?;
        }
        hook();
        self.apply_fact(&fact, resolution)?;
        Ok(resolution)
    }

    pub fn write_decision(&mut self, value: DecisionWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteDecision {
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
        })?;
        self.apply_decision(&value)
    }
    pub(crate) fn commit_mediated_decision(
        &mut self,
        mut value: DecisionWrite,
    ) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteDecision {
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
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_decision(&value)
    }
    pub fn write_episode(&mut self, value: EpisodeWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteEpisode {
            episode_id: value.id.clone(),
            content: value.content.clone(),
            source: value.source.clone(),
            source_product: value.source_product.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
        })?;
        self.apply_episode(&value)
    }
    pub(crate) fn commit_mediated_episode(&mut self, mut value: EpisodeWrite) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteEpisode {
            episode_id: value.id.clone(),
            content: value.content.clone(),
            source: value.source.clone(),
            source_product: value.source_product.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_episode(&value)
    }
    pub fn write_procedure(&mut self, value: ProcedureWrite) -> MemoryResult<()> {
        if value.source_trust == SourceTrust::ModelInference {
            return Err(MemoryError::MediationRequired);
        }
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteProcedure {
            procedure_id: value.id.clone(),
            title: value.title.clone(),
            steps: value.steps.clone(),
            created_by: value.created_by.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
        })?;
        self.apply_procedure(&value)
    }
    pub(crate) fn commit_mediated_procedure(
        &mut self,
        mut value: ProcedureWrite,
    ) -> MemoryResult<()> {
        value.source_trust = SourceTrust::ModelInference;
        self.check_write()?;
        validate_partition(&value.project, &value.agent_id)?;
        self.ensure_id_available(&value.id)?;
        self.append(Op::MemoryWriteProcedure {
            procedure_id: value.id.clone(),
            title: value.title.clone(),
            steps: value.steps.clone(),
            created_by: value.created_by.clone(),
            valid_from: value.valid_from.clone(),
            valid_to: value.valid_to.clone(),
            source_trust: value.source_trust.as_str().into(),
            project: value.project.clone(),
            agent_id: value.agent_id.clone(),
        })?;
        let message = format!("memory updated for {}", value.agent_id);
        self.append_receipt(&value.id, &value.agent_id, &message)?;
        self.apply_procedure(&value)
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
        })
    }

    fn check_write(&self) -> MemoryResult<()> {
        if !self.policy.enabled || self.policy.write != WriteScope::SessionAndProject {
            Err(MemoryError::Disabled)
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
    fn append(&mut self, op: Op) -> MemoryResult<()> {
        self.next_op += 1;
        let id = format!("memory-{}", self.next_op);
        if !self.journal.append(&OpEnvelope::new(id, now(), op))? {
            return Err(MemoryError::JournalIntegrity(
                "duplicate memory op id".into(),
            ));
        }
        Ok(())
    }
    fn fact_resolution(&self, new: &FactWrite) -> MemoryResult<ContradictionResolution> {
        let old=self.db.query_row("SELECT object,confidence,source_trust FROM facts WHERE project=?1 AND agent_id=?2 AND subject=?3 AND predicate=?4 AND valid_to IS NULL ORDER BY system_time DESC LIMIT 1",params![new.project,new.agent_id,new.subject,new.predicate],|r|Ok((r.get::<_,String>(0)?,r.get::<_,f64>(1)?,r.get::<_,String>(2)?))).optional()?;
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
    fn apply_fact(&mut self, v: &FactWrite, res: ContradictionResolution) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let mut stored = v.clone();
        if res == ContradictionResolution::KeepExisting && stored.valid_to.is_none() {
            stored.valid_to = Some(stored.valid_from.clone());
        }
        let tx = self.db.transaction()?;
        if res == ContradictionResolution::Supersede {
            tx.execute("UPDATE facts SET valid_to=?1,superseded_by=?2 WHERE project=?3 AND agent_id=?4 AND subject=?5 AND predicate=?6 AND valid_to IS NULL",params![v.valid_from,v.id,v.project,v.agent_id,v.subject,v.predicate])?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO facts VALUES(?1,?2,?3,?4,?5,?6,NULL,?7,?8,?9,?10,?11,?12)",
            params![
                stored.id,
                stored.subject,
                stored.predicate,
                stored.object,
                stored.confidence,
                stored.source_episode,
                stored.valid_from,
                stored.valid_to,
                now(),
                stored.source_trust.as_str(),
                stored.project,
                stored.agent_id
            ],
        )?;
        let text = format!("{} {} {}", v.subject, v.predicate, v.object);
        index(
            &tx,
            &v.id,
            "fact",
            &v.project,
            &v.agent_id,
            &text,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_decision(&mut self, v: &DecisionWrite) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO decisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                v.id,
                v.summary,
                v.why,
                v.how_to_apply,
                serde_json::to_string(&v.tags).unwrap_or_default(),
                v.source_episode,
                v.valid_from,
                v.valid_to,
                now(),
                v.source_trust.as_str(),
                v.project,
                v.agent_id
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
            &text,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_episode(&mut self, v: &EpisodeWrite) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO episodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                v.id,
                v.content,
                v.source,
                v.source_product,
                v.valid_from,
                v.valid_to,
                now(),
                v.source_trust.as_str(),
                v.project,
                v.agent_id
            ],
        )?;
        index(
            &tx,
            &v.id,
            "episode",
            &v.project,
            &v.agent_id,
            &v.content,
            &self.embedder,
        )?;
        tx.commit()?;
        self.enforce_caps(&v.project, &v.agent_id)
    }
    fn apply_procedure(&mut self, v: &ProcedureWrite) -> MemoryResult<()> {
        self.ensure_id_available(&v.id)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO procedures VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                v.id,
                v.title,
                v.steps,
                v.created_by,
                v.valid_from,
                v.valid_to,
                now(),
                v.source_trust.as_str(),
                v.project,
                v.agent_id
            ],
        )?;
        let text = format!("{} {}", v.title, v.steps);
        index(
            &tx,
            &v.id,
            "procedure",
            &v.project,
            &v.agent_id,
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
        if !self.policy.enabled || self.policy.read_scope != ReadScope::SessionAndProject {
            return Err(MemoryError::Disabled);
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
            return Ok(vec![]);
        }
        let agents = serde_json::to_string(&ids)
            .map_err(|e| MemoryError::JournalIntegrity(e.to_string()))?;
        let match_query = q
            .text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut scores: HashMap<String, f64> = HashMap::new();
        if !match_query.is_empty() {
            let mut s=self.db.prepare("SELECT id FROM memory_fts WHERE memory_fts MATCH ?1 AND project=?2 AND agent_id IN(SELECT value FROM json_each(?3)) ORDER BY bm25(memory_fts) LIMIT 100")?;
            for (rank, id) in s
                .query_map(params![match_query, q.project, agents], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .enumerate()
            {
                *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
            }
        }
        let vector = vector_json(&self.embedder.embed(&q.text));
        let mut knn = Vec::new();
        for agent in &ids {
            let mut s=self.db.prepare("SELECT record_id,distance FROM memory_vec WHERE embedding MATCH ?1 AND k=100 AND project=?2 AND agent_id=?3 ORDER BY distance")?;
            knn.extend(
                s.query_map(params![vector, q.project, agent], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?,
            );
        }
        knn.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        for (rank, (id, _)) in knn.into_iter().enumerate() {
            *scores.entry(id).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
        }
        let mut hits = Vec::new();
        for (id, score) in scores {
            if let Some(mut hit) = self.load_hit(&id)? {
                if hit.project != q.project || !ids.contains(&hit.agent_id) {
                    return Err(MemoryError::JournalIntegrity(
                        "partition assertion failed".into(),
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
        Ok(hits)
    }
    fn load_hit(&self, id: &str) -> MemoryResult<Option<RetrieveHit>> {
        self.db.query_row("SELECT id,kind,text,confidence,source_episode,valid_from,valid_to,source_trust,project,agent_id FROM (SELECT id,'fact' kind,subject||' '||predicate||' '||object text,confidence,source_episode,valid_from,valid_to,source_trust,project,agent_id FROM facts UNION ALL SELECT id,'decision',summary||' '||why||' '||how_to_apply,1.0,source_episode,valid_from,valid_to,source_trust,project,agent_id FROM decisions UNION ALL SELECT id,'episode',content,1.0,NULL,valid_from,valid_to,source_trust,project,agent_id FROM episodes UNION ALL SELECT id,'procedure',title||' '||steps,1.0,NULL,valid_from,valid_to,source_trust,project,agent_id FROM procedures) WHERE id=?1",[id],|r|Ok(RetrieveHit{id:r.get(0)?,kind:r.get(1)?,text:r.get(2)?,score:0.0,confidence:r.get(3)?,source_episode:r.get(4)?,valid_from:r.get(5)?,valid_to:r.get(6)?,source_trust:SourceTrust::parse(&r.get::<_,String>(7)?).map_err(|_|rusqlite::Error::InvalidQuery)?,project:r.get(8)?,agent_id:r.get(9)?})).optional().map_err(Into::into)
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

fn index(
    db: &Connection,
    id: &str,
    kind: &str,
    project: &str,
    agent: &str,
    text: &str,
    embedder: &dyn Embedder,
) -> MemoryResult<()> {
    db.execute(
        "INSERT INTO memory_fts(id,kind,project,agent_id,text) VALUES(?1,?2,?3,?4,?5)",
        params![id, kind, project, agent, text],
    )?;
    db.execute(
        "INSERT INTO memory_vec(record_id,project,agent_id,embedding) VALUES(?1,?2,?3,?4)",
        params![id, project, agent, vector_json(&embedder.embed(text))],
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

pub fn rebuild_from_journals(
    db_path: &Path,
    journals: &[PathBuf],
    policy: MemoryPolicy,
) -> MemoryResult<()> {
    if db_path.exists() {
        std::fs::remove_file(db_path)?;
    }
    let temp_journal = db_path.with_extension("rebuild.jsonl");
    let mut store = MemoryStore::open_at(db_path, &temp_journal, policy)?;
    for path in journals {
        let report = read_journal(path)?;
        let receipts: HashSet<String> = report
            .envelopes
            .iter()
            .filter_map(|e| match &e.op {
                Op::MemoryWriteReceipt { write_id, .. } => Some(write_id.clone()),
                _ => None,
            })
            .collect();
        for envelope in report.envelopes {
            match envelope.op {
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
                    resolver_outcome,
                } => {
                    if source_trust == "ModelInference" && !receipts.contains(&fact_id) {
                        continue;
                    }
                    let r = match resolver_outcome.as_str() {
                        "supersede" => ContradictionResolution::Supersede,
                        "keepexisting" | "keep_existing" => ContradictionResolution::KeepExisting,
                        _ => ContradictionResolution::Coexist,
                    };
                    store.apply_fact(
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
                } => {
                    if source_trust == "ModelInference" && !receipts.contains(&decision_id) {
                        continue;
                    }
                    store.apply_decision(&DecisionWrite {
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
                    })?
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
                } => {
                    if source_trust == "ModelInference" && !receipts.contains(&episode_id) {
                        continue;
                    }
                    store.apply_episode(&EpisodeWrite {
                        id: episode_id,
                        content,
                        source,
                        source_product,
                        valid_from,
                        valid_to,
                        source_trust: SourceTrust::parse(&source_trust)?,
                        project,
                        agent_id,
                    })?
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
                } => {
                    if source_trust == "ModelInference" && !receipts.contains(&procedure_id) {
                        continue;
                    }
                    store.apply_procedure(&ProcedureWrite {
                        id: procedure_id,
                        title,
                        steps,
                        created_by,
                        valid_from,
                        valid_to,
                        source_trust: SourceTrust::parse(&source_trust)?,
                        project,
                        agent_id,
                    })?
                }
                _ => {}
            }
        }
    }
    drop(store);
    let _ = std::fs::remove_file(temp_journal);
    Ok(())
}
