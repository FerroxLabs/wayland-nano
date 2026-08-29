//! Nano transport adapter for the sole authenticated activation gate.

use nano_activation::admission::{AdmissionGate, AdmittedToken};
use nano_activation::authority::KeyRole;
use nano_activation::control::ControlOutcome;
use nano_activation::key_provider::load_key_reference;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling};
use nano_activation::receipt::ArtifactIdentity;
use nano_activation::signer_provider::ExternalReceiptSigner;
use nano_activation::{ActivationError, TransportDocument, inspect_transport_frame};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SharedAdmission(Arc<Mutex<AdmissionGate>>);

impl SharedAdmission {
    pub fn open_production(nano_home: &std::path::Path) -> Result<Self, String> {
        let reference = load_key_reference(
            &nano_home.join("activation/receipt-signer.keyref"),
            KeyRole::ReceiptSigner,
        )
        .map_err(|_| "receipt signer reference unavailable".to_string())?;
        let signer = ExternalReceiptSigner::from_key_reference(&reference)
            .map_err(|_| "receipt signer unavailable".to_string())?;
        let executable = std::env::current_exe()
            .and_then(std::fs::read)
            .map_err(|_| "executable identity unavailable".to_string())?;
        let artifact = nano_activation::build_identity::compiled()
            .bind_executable(&hex(&Sha256::digest(executable)))
            .map_err(|_| "compiled activation identity invalid".to_string())?;
        let gate = AdmissionGate::open(nano_home, Box::new(signer), production_ceiling(), artifact)
            .map_err(|_| "activation gate unavailable".to_string())?;
        Ok(Self::from_gate(gate))
    }

    pub fn from_gate(gate: AdmissionGate) -> Self {
        Self(Arc::new(Mutex::new(gate)))
    }

    /// Inspect and authenticate one complete raw ACP line before transport serde.
    pub fn admit_transport(
        &self,
        raw: &[u8],
        now_utc: &str,
    ) -> Result<TransportAdmission, ActivationError> {
        match inspect_transport_frame(raw)? {
            TransportDocument::Activation => {
                let token = self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .admit_raw(raw, now_utc, None)?;
                Ok(TransportAdmission::Activation(Box::new(token)))
            }
            TransportDocument::Control(raw_control) => {
                let decision = self
                    .0
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .apply_control(&raw_control, now_utc)?;
                Ok(TransportAdmission::Control(decision.outcome()))
            }
            TransportDocument::Other => Ok(TransportAdmission::Other),
        }
    }

    pub fn bind_session(
        &self,
        token: &AdmittedToken,
        session_id: &str,
    ) -> Result<String, ActivationError> {
        let fingerprint = hex(&Sha256::digest(token.receipt().as_bytes()));
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .bind_session(token.activation_id(), session_id, &fingerprint)?;
        Ok(fingerprint)
    }

    pub fn recheck_session(&self, session_id: &str) -> Result<(), ActivationError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .session_binding(session_id)
            .map(|_| ())
    }
}

fn production_ceiling() -> PolicyCeiling {
    PolicyCeiling {
        capabilities: [
            EffectiveCapability::FilesystemRead,
            EffectiveCapability::FilesystemWrite,
            EffectiveCapability::ShellExecute,
            EffectiveCapability::NetworkEgress,
            EffectiveCapability::McpInvoke,
            EffectiveCapability::TaskSpawn,
            EffectiveCapability::CheckpointMutate,
            EffectiveCapability::ComputerUse,
        ]
        .into(),
        controls: [EffectiveControl::Cancel, EffectiveControl::Pause].into(),
        budgets: BudgetLimits {
            max_turns: 10_000,
            max_tool_calls: 100_000,
            max_input_tokens: 10_000_000,
            max_output_tokens: 10_000_000,
            max_cost_microcents: 1_000_000_000_000,
            wall_clock_ms: 86_400_000,
        },
        deadline_utc: "9999-12-31T23:59:59Z".into(),
    }
}

#[derive(Debug)]
pub enum TransportAdmission {
    Activation(Box<AdmittedToken>),
    Control(ControlOutcome),
    Other,
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn runtime_authority(
    token: &AdmittedToken,
) -> Result<(ArtifactIdentity, [u64; 4]), &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(token.receipt().as_bytes()).map_err(|_| "receipt malformed")?;
    let string = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or("receipt incomplete")
    };
    let epoch = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or("receipt incomplete")
    };
    Ok((
        ArtifactIdentity {
            source_commit_sha: string("source_commit_sha")?,
            cargo_lock_sha256: string("cargo_lock_sha256")?,
            executable_sha256: string("executable_sha256")?,
        },
        [
            epoch("admin_epoch")?,
            epoch("issuer_epoch")?,
            epoch("grant_epoch")?,
            epoch("revocation_epoch")?,
        ],
    ))
}

pub fn delegated_authority(
    token: &AdmittedToken,
    nano_home: &std::path::Path,
) -> Result<nano_agent::mcp::DelegatedEffectAuthority, &'static str> {
    let (artifact, epochs) = runtime_authority(token)?;
    Ok(nano_agent::mcp::DelegatedEffectAuthority::new(
        token.clone(),
        nano_home,
        artifact,
        epochs,
        now_utc(),
    ))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}
