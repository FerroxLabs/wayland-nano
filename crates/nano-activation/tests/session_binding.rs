mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::RejectReason;
use nano_activation::admission::AdmissionGate;
use nano_activation::authority::AuthorityCommand;
use nano_activation::store::AuthorityStore;
use serde_json::json;
use support::{artifact_with_exe, bootstrap, ceiling, gate as open_gate, signed_frame};

const NOW: &str = "2026-08-30T10:00:00Z";

#[test]
fn binding_replays_after_reopen_and_resume_needs_no_transcript() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, signer) = bootstrap(home.path());
    let mut gate = open_gate(home.path(), signer.clone());
    let admitted = gate
        .admit_raw(
            &signed_frame(&issuer, "fresh", "nonce-fresh", Some("session-a")),
            NOW,
            None,
        )
        .unwrap();
    let fingerprint = "a".repeat(64);
    let first = gate
        .bind_session(admitted.activation_id(), "session-a", &fingerprint)
        .unwrap();
    let replay = gate
        .bind_session(admitted.activation_id(), "session-a", &fingerprint)
        .unwrap();
    assert_eq!(first, replay);
    drop(gate);

    let mut reopened = open_gate(home.path(), signer);
    assert_eq!(reopened.session_binding("session-a").unwrap(), first);
    reopened
        .admit_raw(
            &resume_frame(&issuer, "session-a", &fingerprint, "resume", "nonce-resume"),
            NOW,
            None,
        )
        .unwrap();
}

#[test]
fn conflicting_rebind_and_fingerprint_drift_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, signer) = bootstrap(home.path());
    let mut gate = open_gate(home.path(), signer);
    let admitted = gate
        .admit_raw(
            &signed_frame(&issuer, "fresh", "nonce-fresh", Some("session-a")),
            NOW,
            None,
        )
        .unwrap();
    gate.bind_session(admitted.activation_id(), "session-a", &"a".repeat(64))
        .unwrap();
    assert_eq!(
        gate.bind_session(admitted.activation_id(), "session-a", &"b".repeat(64))
            .unwrap_err()
            .reason(),
        RejectReason::ResumeDrift
    );
    assert_eq!(
        gate.admit_raw(
            &resume_frame(
                &issuer,
                "session-a",
                &"b".repeat(64),
                "resume",
                "nonce-resume"
            ),
            NOW,
            None,
        )
        .unwrap_err()
        .reason(),
        RejectReason::ResumeDrift
    );
}

#[test]
fn revocation_and_artifact_drift_invalidate_binding() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, signer) = bootstrap(home.path());
    let mut gate = open_gate(home.path(), signer.clone());
    let admitted = gate
        .admit_raw(
            &signed_frame(&issuer, "fresh", "nonce-fresh", Some("session-a")),
            NOW,
            None,
        )
        .unwrap();
    gate.bind_session(admitted.activation_id(), "session-a", &"a".repeat(64))
        .unwrap();
    drop(gate);

    let drifted = AdmissionGate::open(
        home.path(),
        Box::new(signer.clone()),
        ceiling(),
        artifact_with_exe('f'),
    )
    .unwrap();
    assert_eq!(
        drifted.session_binding("session-a").unwrap_err().reason(),
        RejectReason::ResumeDrift
    );
    drop(drifted);

    let mut authority = AuthorityStore::open(home.path()).unwrap();
    authority
        .commit(AuthorityCommand::RevokeIssuer {
            operation_id: "revoke-session-issuer".into(),
            issuer_id: "desktop".into(),
        })
        .unwrap();
    drop(authority);
    assert_eq!(
        open_gate(home.path(), signer)
            .session_binding("session-a")
            .unwrap_err()
            .reason(),
        RejectReason::ResumeDrift
    );
}

fn resume_frame(
    key: &SigningKey,
    session_id: &str,
    fingerprint: &str,
    idem: &str,
    nonce: &str,
) -> Vec<u8> {
    let mut carrier = json!({"activation_id":format!("act-{idem}"),"alg":"Ed25519","budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":2,"max_turns":2,"wall_clock_ms":100},"capabilities":[],"continuity":{"fallback":"none","resume_fingerprint":fingerprint,"strategy":"session_resume"},"controls":["cancel","pause"],"deadline":"2026-08-30T10:20:00Z","idempotency_key":idem,"issued_at":"2026-08-30T09:59:59Z","issuer_id":"desktop","key_id":"desktop-key-1","nonce":nonce,"not_after":"2026-08-30T10:05:00Z","not_before":"2026-08-30T09:59:55Z","principal_id":"main","product_subject_id":"subject-a","project_id":"project-a","schema":"wayland.nano.activation/v1","session_id":session_id});
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":"session/load","params":{"_meta":{"waylandNanoActivation":carrier}}})).unwrap()
}
