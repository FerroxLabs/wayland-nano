//! Nano transport adapter for the sole authenticated activation gate.

use nano_activation::admission::{AdmissionGate, AdmissionRefusal, AdmittedToken};
use nano_activation::authority::KeyRole;
use nano_activation::control::ControlOutcome;
use nano_activation::key_provider::load_key_reference;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling};
use nano_activation::receipt::ArtifactIdentity;
use nano_activation::signer_provider::ExternalActivationSigner;
use nano_activation::signer_provider::ExternalReceiptSigner;
use nano_activation::store::AuthorityStore;
use nano_activation::{ActivationError, TransportDocument, inspect_transport_frame};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SharedAdmission(Arc<Mutex<AdmissionGate>>);

/// Opaque memory identity bound once from an admitted activation token.
/// Downstream seams can read but cannot construct or mutate these bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedMemoryIdentity {
    project_id: String,
    agent_id: String,
}

impl AdmittedMemoryIdentity {
    pub fn bind(token: &AdmittedToken) -> Self {
        Self {
            project_id: token.project_id().to_owned(),
            agent_id: token.principal_id().to_owned(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    #[cfg(test)]
    pub(crate) fn test_only(project_id: &str, agent_id: &str) -> Self {
        Self {
            project_id: project_id.into(),
            agent_id: agent_id.into(),
        }
    }
}

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
        let gate = AdmissionGate::open_enabled(
            nano_home,
            Box::new(signer),
            production_ceiling(),
            artifact,
        )
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
    ) -> Result<TransportAdmission, TransportRefusal> {
        let inspected = inspect_transport_frame(raw);
        let mut gate = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match inspected {
            Err(error) => Err(TransportRefusal::from_admission(gate.sign_refusal(
                raw,
                now_utc,
                error.reason(),
            ))),
            Ok(TransportDocument::Activation) => {
                let token = gate
                    .admit_raw_with_receipt(raw, now_utc, None)
                    .map_err(TransportRefusal::from_admission)?;
                Ok(TransportAdmission::Activation(Box::new(token)))
            }
            Ok(TransportDocument::Control(raw_control)) => {
                match gate.apply_control(&raw_control, now_utc) {
                    Ok(decision) => Ok(TransportAdmission::Control(decision.outcome())),
                    Err(error) => Err(TransportRefusal::from_admission(gate.sign_refusal(
                        raw,
                        now_utc,
                        error.reason(),
                    ))),
                }
            }
            Ok(TransportDocument::Other) => Ok(TransportAdmission::Other),
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

    /// Bind a fork child from a currently valid parent binding. Every durable
    /// authority field comes from the parent record; the live token is used
    /// only to prove that the caller still has the same identity, artifact,
    /// and epochs. No child field is caller-selected beyond the session id
    /// minted by the fork implementation.
    pub fn bind_forked_session(
        &self,
        token: &AdmittedToken,
        parent_session_id: &str,
        child_session_id: &str,
    ) -> Result<String, String> {
        let mut gate = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let parent = gate
            .session_binding_at(parent_session_id, &now_utc())
            .map_err(|error| error.reason().to_string())?;
        let (artifact, epochs) = runtime_authority(token).map_err(str::to_owned)?;
        if token.issuer_id() != parent.issuer_id
            || token.product_subject_id() != parent.product_subject_id
            || token.principal_id() != parent.principal_id
            || token.project_id() != parent.project_id
            || token
                .session_id()
                .is_some_and(|session_id| session_id != parent_session_id)
            || artifact != parent.artifact
            || epochs
                != [
                    parent.admin_epoch,
                    parent.issuer_epoch,
                    parent.issuer_epoch,
                    parent.issuer_epoch,
                ]
        {
            return Err("resume_drift".into());
        }
        let child = gate
            .bind_session(&parent.activation_id, child_session_id, &parent.fingerprint)
            .map_err(|error| error.reason().to_string())?;
        let mut expected = parent.clone();
        expected.session_id = child_session_id.to_owned();
        if child != expected {
            return Err("resume_drift".into());
        }
        Ok(child.fingerprint)
    }

    pub fn recheck_session(&self, session_id: &str) -> Result<(), ActivationError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .session_binding_at(session_id, &now_utc())
            .map(|_| ())
    }

    pub fn mark_dispatch_eligible(&self, token: &AdmittedToken) -> Result<(), ActivationError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_dispatch_eligible(token.activation_id())
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

#[derive(Debug)]
pub struct TransportRefusal {
    reason: nano_activation::RejectReason,
    kind: nano_session::NanoErrorKind,
    receipt: Option<Vec<u8>>,
}

impl TransportRefusal {
    fn from_admission(refusal: AdmissionRefusal) -> Self {
        Self {
            reason: refusal.reason(),
            kind: refusal.kind(),
            receipt: refusal.receipt().map(|receipt| receipt.as_bytes().to_vec()),
        }
    }
    pub fn reason(&self) -> nano_activation::RejectReason {
        self.reason
    }
    pub fn kind(&self) -> nano_session::NanoErrorKind {
        self.kind
    }
    pub fn receipt(&self) -> Option<&[u8]> {
        self.receipt.as_deref()
    }
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// 03-02 (D3-04/D3-05): resolve the one typed host memory-policy handle
/// (strict `$NANO_HOME/memory-policy.toml` + the §6.8 agent registry) at host
/// startup. Resolution only — 03-03's seam owns the store-open validation and
/// the journaled policy record; no store or journal is opened here.
pub fn resolve_memory_policy(
    nano_home: &std::path::Path,
) -> Result<crate::memory_policy::ResolvedMemoryPolicy, crate::memory_policy::MemoryPolicyError> {
    crate::memory_policy::resolve(nano_home)
}

pub fn emit_receipt(receipt: &[u8]) {
    use std::io::Write as _;
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(b"wayland-nano-activation-receipt: ");
    let _ = stderr.write_all(receipt);
    let _ = stderr.write_all(b"\n");
}

pub fn mint_local_cli_request(
    nano_home: &std::path::Path,
    params: &crate::exec_mode::LocalActivationParams,
    mode: nano_protocol::permission_mode::PermissionMode,
) -> Result<Vec<u8>, String> {
    validate_cli_id(&params.issuer_id)?;
    validate_cli_id(&params.key_id)?;
    validate_cli_id(&params.project_id)?;
    if let Some(session_id) = &params.session_id {
        validate_cli_id(session_id)?;
    }
    if let Some(fingerprint) = &params.resume_fingerprint
        && !is_lower_hex(fingerprint, 64)
    {
        return Err("local activation resume fingerprint is invalid".into());
    }
    let reference = load_key_reference(&params.key_reference, KeyRole::LocalCliIssuer)
        .map_err(|_| "local CLI issuer reference unavailable".to_string())?;
    let signer = ExternalActivationSigner::from_key_reference(&reference)
        .map_err(|_| "local CLI issuer unavailable".to_string())?;
    let authority = AuthorityStore::open(nano_home)
        .map_err(|_| "activation authority unavailable".to_string())?;
    let snapshot = authority
        .snapshot()
        .map_err(|_| "activation authority unavailable".to_string())?;
    if snapshot.local_cli_public_key != Some(signer.public_key()) {
        return Err("local CLI issuer is not the enrolled local issuer".into());
    }
    let authorized_key = authority
        .authorize(
            &params.issuer_id,
            &params.key_id,
            "main",
            "main",
            &params.project_id,
        )
        .map_err(|_| "local CLI issuer mapping is not authorized".to_string())?;
    if authorized_key != signer.public_key() {
        return Err("local CLI issuer key does not match the enrolled issuer key".into());
    }
    let issued = chrono::Utc::now();
    let not_after = issued + chrono::Duration::minutes(5);
    let unique = format!(
        "{}-{}",
        std::process::id(),
        issued.timestamp_nanos_opt().unwrap_or_default()
    );
    let capabilities: Vec<&str> = match mode {
        nano_protocol::permission_mode::PermissionMode::ReadOnly => vec!["filesystem.read"],
        nano_protocol::permission_mode::PermissionMode::Default => {
            vec!["filesystem.read", "filesystem.write"]
        }
        nano_protocol::permission_mode::PermissionMode::FullAuto => vec![
            "filesystem.read",
            "filesystem.write",
            "shell.execute",
            "network.egress",
            "mcp.invoke",
            "task.spawn",
            "checkpoint.mutate",
            "computer.use",
        ],
    };
    let mut carrier = json!({
        "activation_id": format!("cli-{unique}"), "alg":"Ed25519",
        "budgets":{"max_cost_microcents":1_000_000_000_000u64,"max_input_tokens":10_000_000u64,"max_output_tokens":10_000_000u64,"max_tool_calls":100_000u64,"max_turns":10_000u64,"wall_clock_ms":86_400_000u64},
        "capabilities":capabilities,
        "continuity":{"fallback":"none","resume_fingerprint":params.resume_fingerprint,"strategy":if params.session_id.is_some() { "session_resume" } else { "fresh" }},
        "controls":["cancel","pause"], "deadline":not_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "idempotency_key":format!("cli-{unique}"), "issued_at":issued.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "issuer_id":params.issuer_id, "key_id":params.key_id, "nonce":format!("cli-{unique}"),
        "not_after":not_after.format("%Y-%m-%dT%H:%M:%SZ").to_string(), "not_before":issued.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "principal_id":"main", "product_subject_id":"main", "project_id":params.project_id,
        "schema":"wayland.nano.activation/v1", "session_id":params.session_id
    });
    signer
        .sign_activation_carrier(&mut carrier)
        .map_err(|_| "local CLI issuer unavailable".to_string())?;
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":format!("cli-{unique}"),"method":"session/new","params":{"_meta":{"waylandNanoActivation":carrier}}}))
        .map_err(|_| "local activation encoding failed".to_string())
}

fn validate_cli_id(value: &str) -> Result<(), String> {
    if (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        Err("local activation identifier is invalid".into())
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn run_activation_command(
    home: &std::path::Path,
    args: &[String],
    out: &mut dyn std::io::Write,
) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("admin-bootstrap") => bootstrap_admin_cli(home, &args[1..]),
        Some("admin-apply") => apply_admin_cli(home, &args[1..]),
        Some("enable-apply") => apply_enable_cli(home, &args[1..]),
        Some("receipt-verify") => verify_receipt_cli(&args[1..]),
        _ => Err("usage: wayland-nano activation admin-bootstrap --admin-id <id> --admin-root-keyref <file> --recovery-root-keyref <file> --receipt-signer-keyref <file> --local-cli-keyref <file> | admin-apply --request <file> --command <file> | enable-apply --request <file> --command <file> | receipt-verify --receipt <file> --public-key <64-hex>".into()),
    };
    match result {
        Ok(message) => {
            let _ = writeln!(out, "{message}");
            0
        }
        Err(message) => {
            eprintln!("wayland-nano: {message}");
            2
        }
    }
}

pub fn run_admin_command(
    home: &std::path::Path,
    args: &[String],
    out: &mut dyn std::io::Write,
) -> i32 {
    if matches!(
        args.first().map(String::as_str),
        Some("offline-bootstrap-challenge" | "offline-bootstrap-apply")
    ) {
        return nano_activation::run_phase2_offline_bootstrap_command(home, args, out);
    }
    let result = match args.first().map(String::as_str) {
        Some("bootstrap") => bootstrap_admin_cli(home, &args[1..]),
        _ => Err(admin_usage().into()),
    };
    match result {
        Ok(message) => {
            let _ = writeln!(out, "{message}");
            0
        }
        Err(message) => {
            eprintln!("wayland-nano: {message}");
            2
        }
    }
}

fn admin_usage() -> &'static str {
    "usage: wayland-nano admin bootstrap --admin-id <id> --admin-root-keyref <file> --recovery-root-keyref <file> --receipt-signer-keyref <file> --local-cli-keyref <file> | offline-bootstrap-challenge --admin-root-keyref <file> --recovery-root-keyref <file> --receipt-signer-keyref <file> --local-cli-keyref <file> --output <owner-only-file> | offline-bootstrap-apply --admin-root-keyref <file> --recovery-root-keyref <file> --receipt-signer-keyref <file> --local-cli-keyref <file> --authorization <owner-only-file>"
}

fn bootstrap_admin_cli(home: &std::path::Path, args: &[String]) -> Result<String, String> {
    let mut values = std::collections::BTreeMap::<&str, &str>::new();
    let mut chunks = args.chunks_exact(2);
    for pair in &mut chunks {
        let name = pair[0].as_str();
        if !matches!(
            name,
            "--admin-id"
                | "--admin-root-keyref"
                | "--recovery-root-keyref"
                | "--receipt-signer-keyref"
                | "--local-cli-keyref"
        ) || values.insert(name, pair[1].as_str()).is_some()
        {
            return Err("invalid admin bootstrap arguments".into());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("invalid admin bootstrap arguments".into());
    }
    let required = |name: &str| {
        values
            .get(name)
            .copied()
            .ok_or_else(|| format!("admin bootstrap requires {name}"))
    };
    let admin_id = required("--admin-id")?;
    validate_cli_id(admin_id).map_err(|_| "admin bootstrap administrator id is invalid")?;
    let paths = nano_activation::admin::BootstrapKeyPaths {
        admin_root: required("--admin-root-keyref")?.into(),
        recovery_root: required("--recovery-root-keyref")?.into(),
        receipt_signer: required("--receipt-signer-keyref")?.into(),
        local_cli_issuer: required("--local-cli-keyref")?.into(),
    };
    let proof =
        nano_activation::admin::attest_interactive_owner(admin_id).map_err(bootstrap_refusal)?;
    let store = nano_activation::admin::bootstrap(home, &paths, admin_id, proof)
        .map_err(bootstrap_refusal)?;
    let receipt = store
        .bootstrap_receipt()
        .ok_or("admin bootstrap receipt unavailable")?;
    let receipt = std::str::from_utf8(receipt).map_err(|_| "admin bootstrap receipt invalid")?;
    Ok(format!("activation administrator bootstrapped\n{receipt}"))
}

fn bootstrap_refusal(error: nano_activation::admin::BootstrapError) -> String {
    use nano_activation::admin::BootstrapError;
    match error {
        BootstrapError::ConfirmationRequired => "admin bootstrap confirmation required",
        BootstrapError::NoControllingTty => "admin bootstrap requires controlling TTY",
        BootstrapError::RemoteSession => "admin bootstrap refuses remote session",
        BootstrapError::AlreadyBootstrapped => "admin bootstrap authority already exists",
        BootstrapError::InsecureHome => "admin bootstrap Nano home is not owner-only",
        BootstrapError::RoleKeyReuse => "admin bootstrap role keys must be distinct",
        BootstrapError::KeyProvider(_) => "admin bootstrap key reference refused",
        BootstrapError::SignerProvider(_) => "admin bootstrap key binding refused",
        BootstrapError::Receipt => "admin bootstrap receipt signing refused",
        BootstrapError::Authority(_) => "admin bootstrap authority commit refused",
    }
    .into()
}

fn two_paths(args: &[String]) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    if args.len() != 4 || args[0] != "--request" || args[2] != "--command" {
        return Err("invalid activation command arguments".into());
    }
    Ok((args[1].clone().into(), args[3].clone().into()))
}
fn apply_admin_cli(home: &std::path::Path, args: &[String]) -> Result<String, String> {
    let (request, command) = two_paths(args)?;
    let raw = std::fs::read(request).map_err(|_| "admin request unavailable")?;
    let command: nano_activation::authority::AuthorityCommand =
        serde_json::from_slice(&std::fs::read(command).map_err(|_| "admin command unavailable")?)
            .map_err(|_| "admin command invalid")?;
    let mut store = AuthorityStore::open(home).map_err(|_| "activation authority unavailable")?;
    nano_activation::admin::apply_signed_admin(&mut store, &raw, command, &now_utc())
        .map_err(|_| "admin command refused")?;
    Ok("activation admin command applied".into())
}
fn apply_enable_cli(home: &std::path::Path, args: &[String]) -> Result<String, String> {
    let (request, command) = two_paths(args)?;
    let raw = std::fs::read(request).map_err(|_| "enablement request unavailable")?;
    let command: nano_activation::enablement::EnablementCommand = serde_json::from_slice(
        &std::fs::read(command).map_err(|_| "enablement command unavailable")?,
    )
    .map_err(|_| "enablement command invalid")?;
    let store = nano_activation::enablement::EnablementStore::open(home)
        .map_err(|_| "enablement store unavailable")?;
    store
        .apply_signed(
            &raw,
            &command,
            &now_utc(),
            nano_activation::enablement::EnablementFault::None,
        )
        .map_err(|_| "enablement command refused")?;
    Ok("activation enablement command applied".into())
}
fn verify_receipt_cli(args: &[String]) -> Result<String, String> {
    if args.len() != 4 || args[0] != "--receipt" || args[2] != "--public-key" {
        return Err("invalid receipt verification arguments".into());
    }
    let raw = std::fs::read(&args[1]).map_err(|_| "activation receipt unavailable")?;
    let key = decode_public_key(&args[3])?;
    nano_activation::verify_receipt(&raw, &key).map_err(|_| "activation receipt invalid")?;
    Ok("activation receipt valid".into())
}
fn decode_public_key(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("receipt public key must be 64 lowercase hex characters".into());
    }
    let mut out = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| "receipt public key invalid")?;
        if !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("receipt public key invalid".into());
        }
        out[index] = u8::from_str_radix(text, 16).map_err(|_| "receipt public key invalid")?;
    }
    Ok(out)
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
    Ok(nano_agent::mcp::DelegatedEffectAuthority::new_live(
        token.clone(),
        nano_home,
        artifact,
        epochs,
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
