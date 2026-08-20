//! Fail-closed gate execution and independently preflightable red-green receipts.
//!
//! This crate is intentionally bottom-of-graph. It contains the WP-1 verification
//! primitives without depending on other Wayland Nano crates or exposing the WP-2
//! climb engine and WP-3 CLI surfaces.

pub mod climb;
pub mod engine;
pub mod error;
pub mod gate;
pub mod receipt;
pub mod registry;

pub use climb::{
    Candidate, ClimbConfig, ClimbOutcome, ClimbState, ClimbStep, LogCode, LogEntry, Phase,
    RunDeadline, StepResult, StopReason, TerminalState, Tier, apply_result, better_candidate,
    next_step,
};

pub use engine::{
    CandidateDiff, ChangeKind, ExpectedChange, ExpectedChangeManifest, derive_expected_changes,
    parse_candidate_diff,
};
pub use error::VerifyError;
pub use gate::{
    CandidateArtifact, CheckVerdict, FailCategory, FailClosedReason, GateEvidence, GateInvocation,
    GateOutcome, parse_gate_output, run_gate,
};
pub use receipt::{
    FailingRun, Receipt, ReceiptPreflight, VerifyVerdict, canonical_receipt, mint_receipt,
    preflight_receipt, read_receipt, write_receipt,
};
pub use registry::{
    CwdPolicy, GateClosure, GateRegistry, GateRegistryEntry, ToolPin, check_closure_pin,
    check_inventory, closure_digest, gate_for_requirement, load_registry,
};
