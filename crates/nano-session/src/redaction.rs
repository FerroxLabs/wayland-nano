//! Pre-persistence secret scan (C1 §8).
//!
//! Model-generated compaction summaries re-enter both the context window and
//! the durable journal. Before a summary is journaled it MUST pass
//! [`scan_for_secrets`]; a summary that trips the scan is never persisted
//! (the compaction attempt fails closed with `CompactionCancel`). This is
//! NEW CODE, named honestly: no reusable scan existed in the tree before C1.
//!
//! The scanner is hand-rolled prefix matching — no regex dependency, no
//! allocation-heavy machinery, and nothing here ever logs or returns the
//! matched text: a hit carries only the [`SecretKind`], so the scan cannot
//! become a secondary secret-persistence channel.
//!
//! The pattern set is pinned by the fixture corpus under
//! `fixtures/redaction/` (positives must trip, negatives must not); the
//! corpus uses synthetic canary strings only — real credentials never
//! appear in this repo.

/// Hard input bound. Summaries are small (a few thousand tokens); anything
/// beyond this is an error the caller must treat as fail-closed (the scan
/// did not complete, so nothing may be persisted on its say-so).
pub const MAX_SCAN_BYTES: usize = 1 << 20;

/// Test-only canary prefix. Engine/integration tests trip the gate with
/// strings like `wayland-nano-canary-a1b2c3d4` — synthetic by construction,
/// never a real credential shape the tree would otherwise carry.
pub const TEST_CANARY_PREFIX: &str = "wayland-nano-canary-";

/// What kind of secret-shaped content was found. Carries no payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// `sk-…` style API token.
    ApiToken,
    /// GitHub token (`ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`/`github_pat_`).
    GithubToken,
    /// AWS access key id (`AKIA…`).
    AwsAccessKey,
    /// Slack token (`xox[baprs]-…`).
    SlackToken,
    /// PEM private-key block header.
    PrivateKeyBlock,
    /// The synthetic test canary ([`TEST_CANARY_PREFIX`]).
    TestCanary,
}

/// Why a scan rejected the input (or could not vouch for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RedactionError {
    #[error("secret-shaped content found: {0:?}")]
    Secret(SecretKind),
    /// The input exceeded [`MAX_SCAN_BYTES`]; the scan did not run to
    /// completion, which is a fail-closed rejection, not a pass.
    #[error("input too large to scan ({bytes} bytes)")]
    TooLarge { bytes: usize },
}

/// Scan `text` for secret-shaped content. `Ok(())` means the text may be
/// persisted; ANY `Err` — a hit OR a scanner limitation — fails closed.
pub fn scan_for_secrets(text: &str) -> Result<(), RedactionError> {
    if text.len() > MAX_SCAN_BYTES {
        return Err(RedactionError::TooLarge { bytes: text.len() });
    }
    if text.contains("PRIVATE KEY-----") && text.contains("-----BEGIN") {
        return Err(RedactionError::Secret(SecretKind::PrivateKeyBlock));
    }
    // (prefix, minimum trailing token length, kind)
    const RULES: &[(&str, usize, SecretKind)] = &[
        (TEST_CANARY_PREFIX, 8, SecretKind::TestCanary),
        ("github_pat_", 20, SecretKind::GithubToken),
        ("ghp_", 20, SecretKind::GithubToken),
        ("gho_", 20, SecretKind::GithubToken),
        ("ghu_", 20, SecretKind::GithubToken),
        ("ghs_", 20, SecretKind::GithubToken),
        ("ghr_", 20, SecretKind::GithubToken),
        ("xoxb-", 10, SecretKind::SlackToken),
        ("xoxp-", 10, SecretKind::SlackToken),
        ("xoxa-", 10, SecretKind::SlackToken),
        ("xoxr-", 10, SecretKind::SlackToken),
        ("xoxs-", 10, SecretKind::SlackToken),
        ("sk-", 20, SecretKind::ApiToken),
        ("AKIA", 16, SecretKind::AwsAccessKey),
    ];
    for (prefix, min_tail, kind) in RULES {
        if has_token_with_prefix(text, prefix, *min_tail) {
            return Err(RedactionError::Secret(*kind));
        }
    }
    Ok(())
}

/// Whether `text` contains `prefix` immediately followed by at least
/// `min_tail` token characters (`[A-Za-z0-9_-]`). Short or punctuation-
/// bounded lookalikes ("sk-impossible", "ghp_editor") do not trip.
fn has_token_with_prefix(text: &str, prefix: &str, min_tail: usize) -> bool {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(found) = text[start..].find(prefix) {
        let tail_start = start + found + prefix.len();
        let tail_len = bytes[tail_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
            .count();
        if tail_len >= min_tail {
            return true;
        }
        start = tail_start;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture corpus is the pinned contract: every positive MUST trip
    /// the scan, every negative MUST pass. A missing corpus file fails the
    /// test loudly (scenario-subject-missing must never silently skip).
    fn corpus(name: &str) -> Vec<String> {
        let path = format!("{}/fixtures/redaction/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("redaction corpus {path} unreadable: {e}"))
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn corpus_positives_all_trip() {
        let positives = corpus("positives.txt");
        assert!(!positives.is_empty(), "positive corpus must not be empty");
        for case in &positives {
            assert!(
                scan_for_secrets(case).is_err(),
                "positive must trip the scan: {case:?}"
            );
        }
    }

    #[test]
    fn corpus_negatives_all_pass() {
        let negatives = corpus("negatives.txt");
        assert!(!negatives.is_empty(), "negative corpus must not be empty");
        for case in &negatives {
            assert_eq!(
                scan_for_secrets(case),
                Ok(()),
                "negative must pass the scan: {case:?}"
            );
        }
    }

    #[test]
    fn hits_carry_no_payload() {
        let Err(RedactionError::Secret(kind)) =
            scan_for_secrets("key: sk-abcdefghijklmnopqrstuvwxyz0123456789")
        else {
            panic!("must trip");
        };
        assert_eq!(kind, SecretKind::ApiToken);
        // The rendered error names the kind only — never the matched text.
        let rendered = RedactionError::Secret(kind).to_string();
        assert!(!rendered.contains("sk-"));
    }

    #[test]
    fn oversized_input_fails_closed() {
        let huge = "x".repeat(MAX_SCAN_BYTES + 1);
        assert_eq!(
            scan_for_secrets(&huge),
            Err(RedactionError::TooLarge {
                bytes: MAX_SCAN_BYTES + 1
            })
        );
    }

    #[test]
    fn lookalikes_do_not_trip() {
        assert_eq!(scan_for_secrets("this task is sk-impossible"), Ok(()));
        assert_eq!(scan_for_secrets("edited ghp_editor.rs"), Ok(()));
        assert_eq!(scan_for_secrets("no secrets here at all"), Ok(()));
    }
}
