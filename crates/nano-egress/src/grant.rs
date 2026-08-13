//! Endpoint grants: path+method-scoped egress authority (P3 design §6.3).
//!
//! Host allowlisting authorizes ANY https path on an allowed host; OAuth
//! trust-chain flows need authority capped to exact token/discovery
//! endpoints. An [`EndpointGrant`] is the canonical tuple
//! `(scheme, host, port, method, path)`:
//!
//! - scheme is HTTPS ONLY — plain http never carries an EndpointGrant;
//! - host is IDNA-normalized and lowercased by the `url` crate (the SAME
//!   crate `EgressClient` sends through, via `reqwest::Url`);
//! - port is the EFFECTIVE port: explicit, else the scheme default (443) —
//!   `https://h/token` and `https://h:8443/token` are different grants;
//! - path is the serialized URL path in ONE canonical form: dot-segments
//!   removed by the url-crate parse (never by hand), NO trailing-slash
//!   folding (`/token` and `/token/` are distinct resources), reserved
//!   percent-encodings (`%2F` and friends) NEVER decoded into path
//!   separators, and unreserved escapes canonicalized by exactly one rule
//!   (decode `%41`→`A`-style unreserved octets, uppercase the hex of every
//!   remaining escape);
//! - query, fragment, and userinfo are REJECTED at grant construction
//!   (typed [`InvalidEndpoint`]); a request URL carrying any of them can
//!   match only the host rule, never a grant.
//!
//! Grant construction (`EgressPolicy::allow_endpoint`) and request matching
//! (`EgressPolicy::decide`) run through the SAME [`normalize_endpoint`]
//! function, so both sides normalize identically by construction.

use std::fmt;

/// HTTP methods an endpoint grant can name. Grants are built for the OAuth
/// endpoint set (metadata GET, token/registration POST); the full standard
/// set is represented so a grant is never lossy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }
}

impl std::str::FromStr for HttpMethod {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Ok(match s.to_ascii_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "DELETE" => HttpMethod::Delete,
            "PATCH" => HttpMethod::Patch,
            "HEAD" => HttpMethod::Head,
            "OPTIONS" => HttpMethod::Options,
            _ => return Err(()),
        })
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Grant scheme. HTTPS ONLY: plain http never carries an EndpointGrant (the
/// per-host http opt-in stays a host-rule concept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrantScheme {
    Https,
}

/// The canonical grant tuple. Construct ONLY via [`normalize_endpoint`] so
/// grant side and request side normalize identically.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EndpointGrant {
    pub scheme: GrantScheme,
    /// IDNA-normalized, lowercase (the url crate's host rules).
    pub host: String,
    /// EFFECTIVE port: explicit, else the scheme default (443).
    pub port: u16,
    pub method: HttpMethod,
    /// Serialized URL path in the canonical form (see module docs).
    pub path: String,
}

/// Why a URL cannot be normalized into (or matched against) a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidEndpoint {
    Unparseable,
    SchemeNotHttps,
    MissingHost,
    UserinfoPresent,
    QueryPresent,
    FragmentPresent,
}

impl fmt::Display for InvalidEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            InvalidEndpoint::Unparseable => "unparseable URL",
            InvalidEndpoint::SchemeNotHttps => "endpoint grants are https-only",
            InvalidEndpoint::MissingHost => "URL has no host",
            InvalidEndpoint::UserinfoPresent => "userinfo is rejected, not stripped",
            InvalidEndpoint::QueryPresent => "query strings are not grantable",
            InvalidEndpoint::FragmentPresent => "fragments are not grantable",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for InvalidEndpoint {}

/// The ONE normalizing function for both grant construction and request
/// matching. Returns the canonical tuple for `method` + `url`, or a typed
/// rejection.
pub fn normalize_endpoint(method: HttpMethod, url: &str) -> Result<EndpointGrant, InvalidEndpoint> {
    let parsed = reqwest::Url::parse(url).map_err(|_| InvalidEndpoint::Unparseable)?;
    if parsed.scheme() != "https" {
        return Err(InvalidEndpoint::SchemeNotHttps);
    }
    // Credential-bearing URLs are REJECTED, never stripped (the C4 §3.5 rule
    // applied to grants).
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(InvalidEndpoint::UserinfoPresent);
    }
    if parsed.query().is_some() {
        return Err(InvalidEndpoint::QueryPresent);
    }
    if parsed.fragment().is_some() {
        return Err(InvalidEndpoint::FragmentPresent);
    }
    let host = parsed
        .host_str()
        .ok_or(InvalidEndpoint::MissingHost)?
        .to_string();
    let port = parsed
        .port_or_known_default()
        .ok_or(InvalidEndpoint::MissingHost)?;
    Ok(EndpointGrant {
        scheme: GrantScheme::Https,
        host,
        port,
        method,
        path: canonical_path(parsed.path()),
    })
}

/// The one canonical rule for path comparison: the url crate has already
/// removed dot segments and applied its percent-encode set; on top of that
/// we decode unreserved-character escapes (`%41` → `A`) and uppercase the
/// hex of every remaining escape (`%2f` → `%2F`). Reserved encodings are
/// NEVER decoded — `%2F` is never a path separator.
fn canonical_path(path: &str) -> String {
    // Byte-level rewrite (the only substitutions are ASCII), then revalidate:
    // a url-crate path is always valid UTF-8 and non-ASCII bytes pass through
    // untouched, so this cannot fail — but fail loud rather than corrupt.
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_value(bytes[i + 1]);
            let lo = hex_value(bytes[i + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                let octet = (hi << 4) | lo;
                if is_unreserved(octet) {
                    out.push(octet);
                } else {
                    out.push(b'%');
                    out.push(HEX_UPPER[hi as usize]);
                    out.push(HEX_UPPER[lo as usize]);
                }
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("url-crate path is valid UTF-8; rewrite substitutes ASCII only")
}

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// RFC 3986 §2.3 unreserved characters: ALPHA / DIGIT / "-" / "." / "_" / "~".
fn is_unreserved(octet: u8) -> bool {
    matches!(octet,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(method: HttpMethod, url: &str) -> Result<EndpointGrant, InvalidEndpoint> {
        normalize_endpoint(method, url)
    }

    /// §12 normalization vectors (r3 codex new-4 / claude-N6): the
    /// adversarial equivalence / non-equivalence pairs.
    #[test]
    fn normalization_vectors() {
        // NO trailing-slash folding: /token and /token/ are DISTINCT.
        assert_ne!(
            grant(HttpMethod::Post, "https://h/token").unwrap(),
            grant(HttpMethod::Post, "https://h/token/").unwrap()
        );
        // Dot segments removed by the url-crate parse.
        assert_eq!(
            grant(HttpMethod::Post, "https://h/a/../token").unwrap(),
            grant(HttpMethod::Post, "https://h/token").unwrap()
        );
        // Unreserved escape decoded by the one canonical rule.
        assert_eq!(
            grant(HttpMethod::Post, "https://h/%74oken").unwrap(),
            grant(HttpMethod::Post, "https://h/token").unwrap()
        );
        // A reserved escape is NEVER a path separator.
        assert_ne!(
            grant(HttpMethod::Post, "https://h/%2Ftoken").unwrap(),
            grant(HttpMethod::Post, "https://h//token").unwrap()
        );
        // ...and the escape hex is canonicalized to uppercase on BOTH sides.
        assert_eq!(
            grant(HttpMethod::Post, "https://h/%2ftoken").unwrap(),
            grant(HttpMethod::Post, "https://h/%2Ftoken").unwrap()
        );
        // Effective port is key material.
        assert_ne!(
            grant(HttpMethod::Post, "https://h/token").unwrap(),
            grant(HttpMethod::Post, "https://h:8443/token").unwrap()
        );
        // Explicit default port normalizes to the effective port.
        assert_eq!(
            grant(HttpMethod::Post, "https://h:443/token").unwrap(),
            grant(HttpMethod::Post, "https://h/token").unwrap()
        );
        // Method is key material.
        assert_ne!(
            grant(HttpMethod::Get, "https://h/token").unwrap(),
            grant(HttpMethod::Post, "https://h/token").unwrap()
        );
        // Host case is insignificant (IDNA/lowercase); path case is preserved.
        assert_eq!(
            grant(HttpMethod::Get, "https://H/token").unwrap(),
            grant(HttpMethod::Get, "https://h/token").unwrap()
        );
        assert_ne!(
            grant(HttpMethod::Get, "https://h/Token").unwrap(),
            grant(HttpMethod::Get, "https://h/token").unwrap()
        );
    }

    #[test]
    fn grant_construction_rejects_query_fragment_userinfo() {
        assert_eq!(
            grant(HttpMethod::Get, "https://h/token?x=1"),
            Err(InvalidEndpoint::QueryPresent)
        );
        // A trailing bare '?' is still a query.
        assert_eq!(
            grant(HttpMethod::Get, "https://h/token?"),
            Err(InvalidEndpoint::QueryPresent)
        );
        assert_eq!(
            grant(HttpMethod::Get, "https://h/token#frag"),
            Err(InvalidEndpoint::FragmentPresent)
        );
        assert_eq!(
            grant(HttpMethod::Get, "https://user@h/token"),
            Err(InvalidEndpoint::UserinfoPresent)
        );
        assert_eq!(
            grant(HttpMethod::Get, "https://user:pass@h/token"),
            Err(InvalidEndpoint::UserinfoPresent)
        );
        // http never carries a grant.
        assert_eq!(
            grant(HttpMethod::Get, "http://h/token"),
            Err(InvalidEndpoint::SchemeNotHttps)
        );
        assert_eq!(
            grant(HttpMethod::Get, "not-a-url"),
            Err(InvalidEndpoint::Unparseable)
        );
    }

    #[test]
    fn canonical_tuple_fields() {
        let g = grant(HttpMethod::Post, "https://EXAMPLE.com/token").unwrap();
        assert_eq!(g.scheme, GrantScheme::Https);
        assert_eq!(g.host, "example.com");
        assert_eq!(g.port, 443);
        assert_eq!(g.method, HttpMethod::Post);
        assert_eq!(g.path, "/token");
        // Root path serializes as "/".
        let root = grant(HttpMethod::Get, "https://example.com").unwrap();
        assert_eq!(root.path, "/");
    }
}
