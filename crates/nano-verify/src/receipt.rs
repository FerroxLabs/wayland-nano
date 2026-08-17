//! Standalone red-green receipt storage and read-only preflight primitives.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::VerifyError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub requirement: String,
    pub test: String,
    pub gate_id: String,
    pub gate_closure_digest: String,
    pub failing_run: FailingRun,
    pub fix_commit: String,
    pub minted_at: String,
    pub minted_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailingRun {
    pub exit_code: i64,
    pub log_digest: String,
    pub observed_at_commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyVerdict {
    Valid,
    NeverRed,
    FabricatedCommit,
    GateMismatch,
    AncestryUnproven,
    Unverifiable,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptPreflight {
    Ready,
    NeverRed,
    FabricatedCommit,
    GateMismatch,
    AncestryUnproven,
    Unverifiable,
}

pub fn canonical_receipt(_receipt: &Receipt) -> Result<Vec<u8>, VerifyError> {
    Err(VerifyError::Registry(
        "canonical receipt not implemented".into(),
    ))
}

pub fn mint_receipt(_receipt: Receipt) -> Result<Receipt, VerifyError> {
    Err(VerifyError::Registry(
        "receipt validation not implemented".into(),
    ))
}

pub fn read_receipt(_path: &Path) -> Result<Receipt, ReceiptPreflight> {
    Err(ReceiptPreflight::Unverifiable)
}

pub fn write_receipt(_directory: &Path, _receipt: &Receipt) -> Result<PathBuf, VerifyError> {
    Err(VerifyError::Registry(
        "receipt storage not implemented".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn receipt(requirement: &str) -> Receipt {
        Receipt {
            schema: 1,
            requirement: requirement.into(),
            test: "tests/red.rs".into(),
            gate_id: "gate-a".into(),
            gate_closure_digest: "a".repeat(64),
            failing_run: FailingRun {
                exit_code: 1,
                log_digest: "b".repeat(64),
                observed_at_commit: "c".repeat(40),
            },
            fix_commit: "d".repeat(40),
            minted_at: "2026-08-17T00:00:00Z".into(),
            minted_by: "wayland-nano 0.1.1".into(),
        }
    }

    #[test]
    fn canonical_schema_and_mint_validation() {
        let valid = receipt("RCPT-01");
        let bytes = canonical_receipt(&valid).expect("valid receipt canonicalizes");
        assert!(!bytes.ends_with(b"\n"));
        assert_eq!(serde_json::from_slice::<Receipt>(&bytes).unwrap(), valid);

        let mut green = receipt("RCPT-01");
        green.failing_run.exit_code = 0;
        assert!(mint_receipt(green).is_err());
    }

    #[test]
    fn store_reader_retry_then_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.receipt.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let started = Instant::now();
        assert_eq!(read_receipt(&path), Err(ReceiptPreflight::Unverifiable));
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "reader returned before the required retry delay"
        );
    }
}
