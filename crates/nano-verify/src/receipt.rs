//! Standalone red-green receipt storage and read-only preflight primitives.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

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

pub fn canonical_receipt(receipt: &Receipt) -> Result<Vec<u8>, VerifyError> {
    validate_receipt(receipt)?;
    let value = serde_json::to_value(receipt).map_err(invalid_receipt)?;
    let normalized = normalize_value(value)?;
    serde_json::to_vec(&normalized).map_err(invalid_receipt)
}

pub fn mint_receipt(receipt: Receipt) -> Result<Receipt, VerifyError> {
    validate_receipt(&receipt)?;
    Ok(receipt)
}

pub fn read_receipt(path: &Path) -> Result<Receipt, ReceiptPreflight> {
    for attempt in 0..2 {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes)
            && validate_receipt(&receipt).is_ok()
        {
            return Ok(receipt);
        }
        if attempt == 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(ReceiptPreflight::Unverifiable)
}

pub fn write_receipt(_directory: &Path, _receipt: &Receipt) -> Result<PathBuf, VerifyError> {
    Err(VerifyError::Registry(
        "receipt storage not implemented".into(),
    ))
}

fn write_receipt_with_policy(
    directory: &Path,
    receipt: &Receipt,
    _retry: Duration,
    _deadline: Duration,
    _stale_after: Duration,
) -> Result<PathBuf, VerifyError> {
    write_receipt(directory, receipt)
}

fn validate_receipt(receipt: &Receipt) -> Result<(), VerifyError> {
    if receipt.schema != 1
        || receipt.failing_run.exit_code == 0
        || !is_lower_hex(&receipt.failing_run.log_digest, 64)
        || !is_lower_hex(&receipt.gate_closure_digest, 64)
        || !is_lower_hex(&receipt.failing_run.observed_at_commit, 40)
        || !is_lower_hex(&receipt.fix_commit, 40)
        || receipt.requirement.is_empty()
        || receipt.test.is_empty()
        || receipt.gate_id.is_empty()
        || receipt.minted_by.is_empty()
        || !is_rfc3339_utc(&receipt.minted_at)
    {
        return Err(invalid_receipt("invalid schema-1 red receipt"));
    }
    Ok(())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_rfc3339_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || *bytes.last().unwrap_or(&0) != b'Z'
    {
        return false;
    }
    let main = &value[..value.len() - 1];
    let (seconds, fraction) = main[17..].split_once('.').unwrap_or((&main[17..], ""));
    value[..4].bytes().all(|b| b.is_ascii_digit())
        && value[5..7].bytes().all(|b| b.is_ascii_digit())
        && value[8..10].bytes().all(|b| b.is_ascii_digit())
        && value[11..13].bytes().all(|b| b.is_ascii_digit())
        && value[14..16].bytes().all(|b| b.is_ascii_digit())
        && seconds.len() == 2
        && seconds.bytes().all(|b| b.is_ascii_digit())
        && (fraction.is_empty() || fraction.bytes().all(|b| b.is_ascii_digit()))
}

fn normalize_value(value: serde_json::Value) -> Result<serde_json::Value, VerifyError> {
    match value {
        serde_json::Value::String(value) => Ok(serde_json::Value::String(value.nfc().collect())),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(normalize_value)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key.nfc().collect(), normalize_value(value)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        serde_json::Value::Number(ref number) if !number.is_i64() && !number.is_u64() => {
            Err(invalid_receipt("canonical JSON permits integers only"))
        }
        other => Ok(other),
    }
}

fn invalid_receipt(error: impl std::fmt::Display) -> VerifyError {
    VerifyError::StoreIo(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
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

    #[test]
    fn store_lock_contention_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("RCPT-01.receipt.json");
        std::fs::write(target.with_extension("lock"), b"held").unwrap();
        let started = Instant::now();
        let result = write_receipt_with_policy(
            dir.path(),
            &receipt("RCPT-01"),
            Duration::from_millis(10),
            Duration::from_millis(60),
            Duration::from_secs(60),
        );
        assert!(matches!(result, Err(VerifyError::LockHeld(_))));
        assert!(started.elapsed() >= Duration::from_millis(60));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn store_replace_overwrites_existing_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let old = receipt("RCPT-01");
        let target = dir.path().join("RCPT-01.receipt.json");
        std::fs::write(&target, canonical_receipt(&old).unwrap()).unwrap();

        let mut new = old.clone();
        new.fix_commit = "e".repeat(40);
        assert_eq!(write_receipt(dir.path(), &new).unwrap(), target);
        assert_eq!(read_receipt(&target).unwrap(), new);
    }
}
