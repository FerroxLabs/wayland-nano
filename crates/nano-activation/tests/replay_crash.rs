mod support;

use nano_activation::RejectReason;
use nano_activation::admission::AdmissionFault;
use nano_activation::receipt::ResultState;
use std::panic::{AssertUnwindSafe, catch_unwind};
use support::{bootstrap, gate, signed_control, signed_frame};

#[test]
fn crash_after_intent_rebuilds_without_redispatch() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, receipt) = bootstrap(home.path());
    let raw = signed_frame(&issuer, "idem-a", "nonce-a", None);
    let mut first = gate(home.path(), receipt.clone());
    let crash = catch_unwind(AssertUnwindSafe(|| {
        let _ = first.admit_raw_with_fault(
            &raw,
            "2026-08-30T10:00:00Z",
            None,
            AdmissionFault::AfterIntent,
        );
    }));
    assert!(crash.is_err());
    drop(first);

    let mut rebuilt = gate(home.path(), receipt);
    let admitted = rebuilt
        .admit_raw(&raw, "2026-08-30T10:00:00Z", None)
        .unwrap();
    rebuilt
        .mark_dispatch_eligible(admitted.activation_id())
        .unwrap();
    rebuilt
        .record_unknown_outcome(admitted.activation_id())
        .unwrap();
    assert_eq!(
        rebuilt.result_state(admitted.activation_id()).unwrap(),
        Some(ResultState::UnknownOutcome)
    );
    assert_eq!(
        rebuilt
            .record_result(admitted.activation_id(), "a")
            .unwrap_err()
            .reason(),
        RejectReason::AmbiguousRecovery
    );
    drop(rebuilt);
    let reopened = gate(home.path(), support::receipt_signer());
    assert_eq!(
        reopened.result_state("act-idem-a").unwrap(),
        Some(ResultState::UnknownOutcome)
    );
}

#[test]
fn crash_after_decision_replays_byte_identical_receipt() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, receipt) = bootstrap(home.path());
    let raw = signed_frame(&issuer, "idem-b", "nonce-b", None);
    let mut first = gate(home.path(), receipt.clone());
    let crash = catch_unwind(AssertUnwindSafe(|| {
        let _ = first.admit_raw_with_fault(
            &raw,
            "2026-08-30T10:00:00Z",
            None,
            AdmissionFault::AfterDecision,
        );
    }));
    assert!(crash.is_err());
    drop(first);
    let journal_before = std::fs::read(home.path().join("activation/admission.jsonl")).unwrap();
    let mut rebuilt = gate(home.path(), receipt);
    let replay = rebuilt
        .admit_raw(&raw, "2026-08-30T10:00:00Z", None)
        .unwrap();
    let journal_after = std::fs::read(home.path().join("activation/admission.jsonl")).unwrap();
    assert_eq!(
        journal_before, journal_after,
        "exact replay must append nothing"
    );
    replay
        .receipt()
        .verify_offline(&support::receipt_key().verifying_key().to_bytes())
        .unwrap();
}

#[test]
fn signed_controls_have_durable_fixed_race_ordering() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, receipt) = bootstrap(home.path());
    let raw = signed_frame(&issuer, "idem-control", "nonce-control", Some("session-a"));
    let mut active_gate = gate(home.path(), receipt);
    let admitted = active_gate
        .admit_raw(&raw, "2026-08-30T10:00:00Z", None)
        .unwrap();
    let cancel = signed_control(
        &issuer,
        admitted.activation_id(),
        "session-a",
        "cancel",
        "control-nonce-a",
    );
    let control_decision = active_gate
        .apply_control(&cancel, "2026-08-30T10:00:01Z")
        .unwrap();
    assert_eq!(
        control_decision.outcome(),
        nano_activation::control::ControlOutcome::Cancelled
    );
    control_decision
        .receipt()
        .verify_offline(&support::receipt_key().verifying_key().to_bytes())
        .unwrap();
    let replayed_control = active_gate
        .apply_control(&cancel, "2026-08-30T10:00:01Z")
        .unwrap();
    assert_eq!(
        replayed_control.receipt().as_bytes(),
        control_decision.receipt().as_bytes()
    );
    assert_eq!(
        active_gate
            .record_result(admitted.activation_id(), "done")
            .unwrap_err()
            .reason(),
        RejectReason::ControlRaceLost
    );
    drop(active_gate);

    let mut reopened = gate(home.path(), support::receipt_signer());
    assert_eq!(
        reopened.result_state("act-idem-control").unwrap(),
        Some(ResultState::Cancelled)
    );
    let unauthorized = signed_control(
        &issuer,
        "act-other",
        "session-a",
        "pause",
        "control-nonce-b",
    );
    assert_eq!(
        reopened
            .apply_control(&unauthorized, "2026-08-30T10:00:01Z")
            .unwrap_err()
            .reason(),
        RejectReason::ControlUnauthorized
    );

    let completed_home = tempfile::tempdir().unwrap();
    let (completed_issuer, completed_receipt) = bootstrap(completed_home.path());
    let completed_raw = signed_frame(
        &completed_issuer,
        "idem-complete",
        "nonce-complete",
        Some("session-b"),
    );
    let mut completed_gate = gate(completed_home.path(), completed_receipt);
    let completed = completed_gate
        .admit_raw(&completed_raw, "2026-08-30T10:00:00Z", None)
        .unwrap();
    completed_gate
        .mark_dispatch_eligible(completed.activation_id())
        .unwrap();
    completed_gate
        .record_result(completed.activation_id(), "result-digest")
        .unwrap();
    let late_cancel = signed_control(
        &completed_issuer,
        completed.activation_id(),
        "session-b",
        "cancel",
        "control-late",
    );
    assert_eq!(
        completed_gate
            .apply_control(&late_cancel, "2026-08-30T10:00:01Z")
            .unwrap()
            .outcome(),
        nano_activation::control::ControlOutcome::RaceLost
    );
}

#[test]
fn torn_final_record_is_ignored_and_prior_decision_remains_authoritative() {
    use std::io::Write as _;
    let home = tempfile::tempdir().unwrap();
    let (issuer, receipt) = bootstrap(home.path());
    let raw = signed_frame(&issuer, "idem-torn", "nonce-torn", None);
    let mut first = gate(home.path(), receipt);
    let admitted = first.admit_raw(&raw, "2026-08-30T10:00:00Z", None).unwrap();
    let receipt_bytes = admitted.receipt().as_bytes().to_vec();
    drop(first);
    let path = home.path().join("activation/admission.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"record_type\":\"result\"")
        .unwrap();
    let mut rebuilt = gate(home.path(), support::receipt_signer());
    let replay = rebuilt
        .admit_raw(&raw, "2026-08-30T10:00:00Z", None)
        .unwrap();
    assert_eq!(replay.receipt().as_bytes(), receipt_bytes);
}
