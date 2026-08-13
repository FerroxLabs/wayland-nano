//! PKCE (RFC 7636) primitives, S256 ONLY (P3 §6.2 step 3): a server
//! offering only `plain` is a typed `McpOAuthFailed`, never a downgrade.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::Digest;

/// RFC 7636 §4.1 verifier alphabet (url-safe, unreserved).
const VERIFIER_ALPHABET: &[u8; 66] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// Verifier length: 64 chars (the note's constant; RFC allows 43–128).
const VERIFIER_LEN: usize = 64;

/// A PKCE verifier/challenge pair. The verifier is memory-only and dropped
/// after the token exchange (§6.2 step 6).
#[derive(Debug)]
pub struct PkceChallenge {
    verifier: String,
    challenge: String,
}

impl PkceChallenge {
    /// `code_verifier` = 64 random url-safe chars; `code_challenge` =
    /// BASE64URL_NOPAD(SHA256(verifier)) — the S256 method, the only one
    /// this client ever sends.
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let verifier: String = (0..VERIFIER_LEN)
            .map(|_| {
                let idx = rng.gen_range(0..VERIFIER_ALPHABET.len());
                VERIFIER_ALPHABET[idx] as char
            })
            .collect();
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
        }
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// 128 bits of OS randomness as lowercase hex (state nonce, callback path
/// component, login attempt id).
pub fn random_token_128() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_s256_of_verifier() {
        let pkce = PkceChallenge::generate();
        assert_eq!(pkce.verifier().len(), VERIFIER_LEN);
        assert!(
            pkce.verifier()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')),
            "verifier must stay in the RFC 7636 alphabet"
        );
        // Recompute S256 independently.
        let digest = sha2::Sha256::digest(pkce.verifier().as_bytes());
        assert_eq!(URL_SAFE_NO_PAD.encode(digest), pkce.challenge());
        // base64url-NOPAD shape: no padding, no +//= characters.
        assert!(!pkce.challenge().contains(['=', '+', '/']));
    }

    #[test]
    fn pairs_are_unique() {
        assert_ne!(
            PkceChallenge::generate().verifier(),
            PkceChallenge::generate().verifier()
        );
        assert_ne!(random_token_128(), random_token_128());
    }

    /// RFC 7636 appendix B known vector.
    #[test]
    fn s256_matches_rfc_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        assert_eq!(
            URL_SAFE_NO_PAD.encode(digest),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
