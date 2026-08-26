use crate::{
    DecisionWrite, EpisodeWrite, FactWrite, MemoryResult, MemoryStore, ProcedureWrite, SourceTrust,
};
use nano_session::{redaction::redact_secrets, scan_for_secrets};

#[derive(Debug, Clone)]
pub enum ProposalKind {
    Fact(FactWrite),
    Decision(DecisionWrite),
    Episode(EpisodeWrite),
    Procedure(ProcedureWrite),
}
#[derive(Debug, Clone)]
pub struct MemoryProposal {
    pub kind: ProposalKind,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReceipt {
    pub write_id: String,
    pub agent_id: String,
    pub message: String,
}

impl MemoryStore {
    /// The single model-proposes/host-commits authority boundary.
    pub fn commit_proposal(&mut self, proposal: MemoryProposal) -> MemoryResult<MemoryReceipt> {
        let (id, agent) = match proposal.kind {
            ProposalKind::Fact(mut v) => {
                v.subject = screen(&v.subject)?;
                v.predicate = screen(&v.predicate)?;
                v.object = screen(&v.object)?;
                v.source_trust = SourceTrust::ModelInference;
                let id = v.id.clone();
                let agent = v.agent_id.clone();
                self.commit_mediated_fact(v)?;
                (id, agent)
            }
            ProposalKind::Decision(mut v) => {
                v.summary = screen(&v.summary)?;
                v.why = screen(&v.why)?;
                v.how_to_apply = screen(&v.how_to_apply)?;
                v.tags = v
                    .tags
                    .into_iter()
                    .map(|tag| screen(&tag))
                    .collect::<MemoryResult<Vec<_>>>()?;
                v.source_trust = SourceTrust::ModelInference;
                let id = v.id.clone();
                let agent = v.agent_id.clone();
                self.commit_mediated_decision(v)?;
                (id, agent)
            }
            ProposalKind::Episode(mut v) => {
                v.content = screen(&v.content)?;
                v.source = "model".into();
                v.source_product = "wayland-nano".into();
                v.source_trust = SourceTrust::ModelInference;
                let id = v.id.clone();
                let agent = v.agent_id.clone();
                self.commit_mediated_episode(v)?;
                (id, agent)
            }
            ProposalKind::Procedure(mut v) => {
                v.title = screen(&v.title)?;
                v.steps = screen(&v.steps)?;
                v.source_trust = SourceTrust::ModelInference;
                let id = v.id.clone();
                let agent = v.agent_id.clone();
                self.commit_mediated_procedure(v)?;
                (id, agent)
            }
        };
        let message = format!("memory updated for {agent}");
        Ok(MemoryReceipt {
            write_id: id,
            agent_id: agent,
            message,
        })
    }
}
fn screen(text: &str) -> MemoryResult<String> {
    let redacted = redact_secrets(text).map_err(|_| crate::MemoryError::ScreeningRejected)?;
    scan_for_secrets(&redacted).map_err(|_| crate::MemoryError::ScreeningRejected)?;
    Ok(redacted)
}
