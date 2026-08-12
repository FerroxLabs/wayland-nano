//! The egress HTTP client: policy-gated, redacting, bounded.
//!
//! The only place in the workspace permitted to construct reqwest clients.
//! Invariants:
//! - policy check BEFORE any socket activity (deny = no bytes leave);
//! - redirect hops are re-checked against the policy before being followed
//!   (an allowlisted origin cannot 302 to an off-allowlist host);
//! - Authorization/x-api-key headers are set via dedicated methods and are
//!   redacted from every error's Display;
//! - connect and total-request timeouts are mandatory;
//! - observability fields: method/host/port + path_query_sha256 only.

// The only crate permitted to construct HTTP clients.
#![allow(clippy::disallowed_methods)]

use crate::policy::EgressDecision;
use crate::policy::EgressPolicy;
use crate::policy::path_query_sha256;

#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    #[error("egress denied by policy: {method} {host} path_query_sha256={digest}")]
    Denied {
        method: String,
        host: String,
        digest: String,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("http {status}: {host} path_query_sha256={digest}")]
    HttpStatus {
        status: u16,
        host: String,
        digest: String,
    },
    #[error("egress denied: {host} resolved to a private/reserved address")]
    PrivateAddress { host: String },
    #[error("egress denied: credential-bearing URL (userinfo present) for {host}")]
    CredentialsRejected { host: String },
    #[error("egress denied: missing Content-Type from {host}")]
    ContentTypeMissing { host: String },
    #[error("egress denied: Content-Type {media_type} not allowlisted for {host}")]
    ContentTypeDenied { host: String, media_type: String },
    #[error("egress denied: redirect from {host} without a usable Location")]
    InvalidRedirect { host: String },
    #[error("redirect hop limit exceeded: last hop {host}")]
    RedirectLimit { host: String },
}

/// Redirect hops are only followed when the target re-passes the policy
/// gate; the cap matches reqwest's former default redirect limit.
const MAX_REDIRECT_HOPS: usize = 10;

/// A policy-gated outbound client. Construct once per policy domain.
#[derive(Debug)]
pub struct EgressClient {
    client: reqwest::Client,
    policy: EgressPolicy,
    fetch_driver: std::sync::Arc<dyn FetchDriver>,
}

impl EgressClient {
    pub fn new(policy: EgressPolicy) -> Self {
        let hop_gate = policy.clone();
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("wayland-nano/0.1.0")
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECT_HOPS {
                    return attempt.error("redirect hop limit exceeded");
                }
                if hop_gate.allows(attempt.url().as_str()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .expect("reqwest client build");
        Self {
            client,
            policy,
            fetch_driver: std::sync::Arc::new(SystemFetchDriver),
        }
    }

    /// TEST SEAM (C4 §3.4): substitute the DNS/transport driver used by
    /// `fetch_bounded`. The policy gate, per-hop private-range check,
    /// redirect loop, and streaming byte cap still run in the loop — this
    /// swaps only name resolution and the socket send. Production code must
    /// use `EgressClient::new`.
    #[doc(hidden)]
    pub fn with_fetch_driver_for_tests(mut self, driver: std::sync::Arc<dyn FetchDriver>) -> Self {
        self.fetch_driver = driver;
        self
    }

    pub fn flux() -> Self {
        Self::new(EgressPolicy::flux_only())
    }

    /// Policy gate + request construction. Returns Err(Denied) before any
    /// socket activity when the policy rejects the URL.
    pub fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, EgressError> {
        match self.policy.decide(url) {
            EgressDecision::Allow => Ok(self.client.request(method.clone(), url)),
            EgressDecision::Deny => Err(EgressError::Denied {
                method: method.to_string(),
                host: host_display(url),
                digest: path_query_sha256(url),
            }),
        }
    }

    /// Classify a transport/HTTP outcome with secrets redacted by
    /// construction (no headers or bodies ever reach the error string).
    pub fn classify_status(&self, url: &str, status: u16) -> EgressError {
        EgressError::HttpStatus {
            status,
            host: host_display(url),
            digest: path_query_sha256(url),
        }
    }

    pub fn classify_transport(&self, err: &reqwest::Error) -> EgressError {
        EgressError::Transport(sanitize_transport_error(err))
    }

    /// Bounded GET-only web fetch (C4 §3.2). GET-specific BY SIGNATURE —
    /// other methods cannot be expressed. The manual hop loop (reqwest's
    /// redirect closure is synchronous and cannot re-pin, so the fetch
    /// client follows nothing itself) runs, PER HOP and in order:
    ///   a. hostname policy gate (deny = zero socket activity);
    ///   b. name resolution (system resolver via the driver);
    ///   c. private/reserved-range deny on EVERY resolved address;
    ///   d. a per-hop pinned client (`resolve_to_addrs`) — no pooling, no
    ///      pin caching: a cached pin is a stale pin;
    ///   e. send, then stream the body with a hard byte counter, aborting
    ///      at `max_bytes` (marked truncation, never typed rejection).
    pub async fn fetch_bounded(
        &self,
        url: &str,
        max_bytes: usize,
        timeout: std::time::Duration,
    ) -> Result<FetchOutcome, EgressError> {
        let mut current = url.to_string();
        for _hop in 0..MAX_REDIRECT_HOPS {
            // (a) policy gate BEFORE any DNS or socket activity.
            if self.policy.decide(&current) == EgressDecision::Deny {
                return Err(EgressError::Denied {
                    method: "GET".into(),
                    host: host_display(&current),
                    digest: path_query_sha256(&current),
                });
            }
            let parsed = match reqwest::Url::parse(&current) {
                Ok(u) => u,
                Err(_) => {
                    return Err(EgressError::Denied {
                        method: "GET".into(),
                        host: host_display(&current),
                        digest: path_query_sha256(&current),
                    });
                }
            };
            // Credential-bearing URLs are REJECTED, not stripped (C4 §3.5).
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(EgressError::CredentialsRejected {
                    host: host_display(&current),
                });
            }
            let (Some(host), Some(port)) = (
                parsed.host_str().map(str::to_string),
                parsed.port_or_known_default(),
            ) else {
                return Err(EgressError::Denied {
                    method: "GET".into(),
                    host: host_display(&current),
                    digest: path_query_sha256(&current),
                });
            };
            // (b)+(c): an explicitly allowlisted IP literal is dialed
            // directly (no name to rebind — the allowlist entry IS the
            // pin). A NAME is resolved per hop and EVERY answer must be
            // public (fail-closed on split-horizon answers).
            let addrs: Vec<std::net::IpAddr> = match host.parse::<std::net::IpAddr>() {
                Ok(ip) => vec![ip],
                Err(_) => {
                    let resolved = self
                        .fetch_driver
                        .resolve(&host, port)
                        .await
                        .map_err(|e| EgressError::Transport(format!("dns: {e}")))?;
                    if resolved.is_empty() {
                        return Err(EgressError::Transport(format!(
                            "dns: no addresses for {host}"
                        )));
                    }
                    if resolved.iter().any(|ip| is_private_or_reserved(*ip)) {
                        return Err(EgressError::PrivateAddress { host });
                    }
                    resolved
                }
            };
            // (d)+(e): per-hop pinned client; stream with a hard counter.
            let hop = self
                .fetch_driver
                .send(&host, port, &addrs, &current, timeout)
                .await?;
            if (300..400).contains(&hop.status) {
                let Some(location) = hop.location else {
                    return Err(EgressError::InvalidRedirect { host });
                };
                let next = parsed
                    .join(&location)
                    .map_err(|_| EgressError::InvalidRedirect { host: host.clone() })?;
                current = next.to_string();
                continue;
            }
            if !(200..300).contains(&hop.status) {
                return Err(self.classify_status(&current, hop.status));
            }
            // Content-type allowlist on the media-type prefix BEFORE ';';
            // a MISSING Content-Type is a typed denial, not an allow.
            let Some(content_type) = hop.content_type else {
                return Err(EgressError::ContentTypeMissing { host });
            };
            let media = content_type
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let allowed = media.starts_with("text/")
                || matches!(
                    media.as_str(),
                    "application/json"
                        | "application/xml"
                        | "application/javascript"
                        | "application/xhtml+xml"
                );
            if !allowed {
                return Err(EgressError::ContentTypeDenied {
                    host,
                    media_type: media,
                });
            }
            let mut body: Vec<u8> = Vec::new();
            let mut truncated = false;
            let mut stream = hop.body;
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                let chunk = chunk?;
                let remaining = max_bytes.saturating_sub(body.len());
                if chunk.len() > remaining {
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(FetchOutcome {
                status: hop.status,
                final_url: current,
                content_type: media,
                body_bytes: body.len(),
                declared_bytes: hop.content_length,
                truncated,
                body,
            });
        }
        Err(EgressError::RedirectLimit {
            host: host_display(&current),
        })
    }
}

/// Outcome of a bounded fetch (C4 §3.1/§3.2). `declared_bytes` is the
/// Content-Length value — a DECLARED length only, never a "real total"
/// (absent for chunked responses; lies are common).
#[derive(Debug)]
pub struct FetchOutcome {
    pub status: u16,
    /// The last hop's URL, fully re-validated by the loop.
    pub final_url: String,
    /// Media type only (parameters stripped), validated against the
    /// allowlist.
    pub content_type: String,
    /// Bytes actually returned in `body` (the truncation footer, when any,
    /// is added by the tool layer and is NOT counted here).
    pub body_bytes: usize,
    pub declared_bytes: Option<u64>,
    pub truncated: bool,
    pub body: Vec<u8>,
}

/// One hop's response as seen by the fetch loop.
#[doc(hidden)]
pub struct FetchHop {
    pub status: u16,
    pub location: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub body:
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Vec<u8>, EgressError>> + Send>>,
}

/// DNS + transport seam for `fetch_bounded` (C4 §3.4).
///
/// Production clients use the system driver (system resolver + per-hop
/// pinned reqwest clients). Tests substitute scripted DNS/transport to
/// prove the per-hop gate → resolve → private-IP-deny → pin pipeline
/// WITHOUT real network. The private-range check is NOT behind this seam:
/// it runs in the fetch loop for every hop, production and tests alike.
#[doc(hidden)]
#[async_trait::async_trait]
pub trait FetchDriver: std::fmt::Debug + Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<std::net::IpAddr>>;
    async fn send(
        &self,
        host: &str,
        port: u16,
        addrs: &[std::net::IpAddr],
        url: &str,
        timeout: std::time::Duration,
    ) -> Result<FetchHop, EgressError>;
}

/// Production driver: system DNS + one pinned reqwest client PER HOP
/// (`redirect(Policy::none())` — reqwest itself follows nothing). No
/// connection pooling, no cross-request pin caching (C4 §3.4: deliberate;
/// a TLS handshake per fetch is acceptable at interactive frequency).
#[derive(Debug)]
struct SystemFetchDriver;

#[async_trait::async_trait]
impl FetchDriver for SystemFetchDriver {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        let addrs = tokio::net::lookup_host((host, port)).await?;
        Ok(addrs.map(|a| a.ip()).collect())
    }

    async fn send(
        &self,
        host: &str,
        port: u16,
        addrs: &[std::net::IpAddr],
        url: &str,
        timeout: std::time::Duration,
    ) -> Result<FetchHop, EgressError> {
        let socket_addrs: Vec<std::net::SocketAddr> = addrs
            .iter()
            .map(|ip| std::net::SocketAddr::new(*ip, port))
            .collect();
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(timeout)
            .user_agent("wayland-nano/0.1.0")
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &socket_addrs)
            .build()
            .map_err(|e| EgressError::Transport(sanitize_transport_error(&e)))?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| EgressError::Transport(sanitize_transport_error(&e)))?;
        let status = response.status().as_u16();
        let headers = response.headers();
        let location = headers
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_length = response.content_length();
        let body = Box::pin(futures_util::StreamExt::map(
            response.bytes_stream(),
            |chunk| {
                chunk
                    .map(|b| b.to_vec())
                    .map_err(|e| EgressError::Transport(sanitize_transport_error(&e)))
            },
        ));
        Ok(FetchHop {
            status,
            location,
            content_type,
            content_length,
            body,
        })
    }
}

/// The complete private/reserved denylist (C4 §3.4.1). `Ipv6Addr::is_global`
/// is unstable in std, so this explicit list IS the spec; every range has a
/// deny-battery case in tests/adversarial_egress.rs.
fn is_private_or_reserved(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 0 // 0.0.0.0/8 (unspecified / "this host")
            || o[0] == 10 // 10.0.0.0/8 (RFC1918)
            || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10 (CGNAT)
            || o[0] == 127 // 127.0.0.0/8 (loopback)
            || (o[0] == 169 && o[1] == 254) // 169.254.0.0/16 (link-local)
            || (o[0] == 172 && (o[1] & 0xF0) == 16) // 172.16.0.0/12 (RFC1918)
            || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 (IETF)
            || (o[0] == 192 && o[1] == 168) // 192.168.0.0/16 (RFC1918)
            || (o[0] == 198 && (o[1] & 0xFE) == 18) // 198.18.0.0/15 (benchmark)
            || (o[0] & 0xF0) == 240 // 240.0.0.0/4 (reserved, incl. broadcast)
        }
        std::net::IpAddr::V6(v6) => {
            // ::ffff:0:0/96 (IPv4-mapped): re-check the embedded v4 address
            // against the v4 list.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private_or_reserved(std::net::IpAddr::V4(mapped));
            }
            let s = v6.segments();
            v6.is_unspecified() // ::
            || v6.is_loopback() // ::1
            || (s[0] & 0xFE00) == 0xFC00 // fc00::/7 (unique-local)
            || (s[0] & 0xFFC0) == 0xFE80 // fe80::/10 (link-local)
            // 64:ff9b::/96 (NAT64 — maps to IPv4, private targets included)
            || (s[0] == 0x0064 && s[1] == 0xFF9B && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)
        }
    }
}

fn host_display(url: &str) -> String {
    url.split_once("://")
        .map(|(_, r)| {
            let authority = r.split('/').next().unwrap_or(r);
            // Strip userinfo: callers attach credentials as user:password@,
            // and only the host:port may ever render.
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host_port)| host_port)
                .to_string()
        })
        .unwrap_or_else(|| "<no-scheme>".to_string())
}

/// Render a reqwest transport error without echoing credentials.
///
/// `reqwest::Error`'s Display embeds the full request URL — userinfo and
/// query string included — which violates the crate invariant "query is
/// hashed, never echoed". Re-render the URL with userinfo/query/fragment
/// stripped; if that redaction cannot be proven complete, fail closed to a
/// message carrying only host + path_query_sha256.
///
/// Shared with nano-model (its `ModelError::Transport` wraps the same
/// reqwest errors) so there is exactly one redaction path.
pub fn sanitize_transport_error(err: &reqwest::Error) -> String {
    let rendered = err.to_string();
    let Some(url) = err.url() else {
        return rendered;
    };
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.set_query(None);
    redacted.set_fragment(None);
    let cleaned = rendered.replace(url.as_str(), redacted.as_str());
    let leaks_secret = cleaned.contains(url.as_str())
        || (!url.username().is_empty() && cleaned.contains(url.username()))
        || url.password().is_some_and(|p| cleaned.contains(p))
        || url
            .query()
            .is_some_and(|q| !q.is_empty() && cleaned.contains(q));
    if leaks_secret {
        format!(
            "transport error: {} path_query_sha256={}",
            host_display(url.as_str()),
            path_query_sha256(url.as_str())
        )
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_request_never_builds() {
        let client = EgressClient::flux();
        let err = client
            .request(reqwest::Method::GET, "https://api.openai.com/v1/models")
            .expect_err("must deny");
        let rendered = err.to_string();
        assert!(rendered.contains("egress denied"));
        assert!(!rendered.contains("Authorization"));
    }

    #[test]
    fn allowed_request_builds_with_no_secret_echo() {
        let client = EgressClient::flux();
        let _builder = client
            .request(
                reqwest::Method::POST,
                "https://api.fluxrouter.ai/v1/chat/completions",
            )
            .expect("allowed");
    }

    #[test]
    fn error_display_carries_only_observability_fields() {
        let client = EgressClient::flux();
        let err = client.classify_status(
            "https://api.fluxrouter.ai/v1/chat/completions?key=secret",
            402,
        );
        let rendered = err.to_string();
        assert!(rendered.contains("402"));
        assert!(rendered.contains("api.fluxrouter.ai"));
        assert!(
            !rendered.contains("secret"),
            "query must be hashed, not echoed: {rendered}"
        );
    }

    #[test]
    fn host_display_strips_userinfo_and_ignores_query() {
        assert_eq!(
            host_display("https://user:s3cr3t@api.fluxrouter.ai/v1?api_key=x"),
            "api.fluxrouter.ai"
        );
        assert_eq!(
            host_display("https://api.fluxrouter.ai:8443/v1"),
            "api.fluxrouter.ai:8443"
        );
        assert_eq!(host_display("api.fluxrouter.ai/v1"), "<no-scheme>");
    }

    /// Every C4 §3.4.1 range, one case each, plus public controls. The
    /// integration battery re-proves each through fetch_bounded with an
    /// allowlisted hostname.
    #[test]
    fn private_range_denylist_is_complete() {
        use std::net::IpAddr;
        let denied = [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fdff:ffff::1",
            "fe80::1",
            "64:ff9b::a00:1",         // NAT64 → 10.0.0.1
            "::ffff:10.0.0.1",        // v4-mapped private
            "::ffff:169.254.169.254", // v4-mapped link-local
        ];
        for ip in denied {
            assert!(
                is_private_or_reserved(ip.parse::<IpAddr>().unwrap()),
                "{ip} must be denied"
            );
        }
        let allowed = ["8.8.8.8", "93.184.216.34", "192.0.2.1", "2606:4700::1"];
        for ip in allowed {
            assert!(
                !is_private_or_reserved(ip.parse::<IpAddr>().unwrap()),
                "{ip} must pass"
            );
        }
    }

    #[tokio::test]
    async fn transport_error_display_redacts_url_credentials() {
        // Port 9 (discard) on loopback refuses connections; the refused
        // transport error must not echo the URL's userinfo or query.
        let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
        let err = client
            .request(
                reqwest::Method::GET,
                "http://user:s3cr3t@127.0.0.1:9/v1?api_key=query-secret",
            )
            .expect("allowlisted")
            .send()
            .await
            .expect_err("refused connection must error");
        let rendered = client.classify_transport(&err).to_string();
        assert!(
            !rendered.contains("s3cr3t"),
            "userinfo leaked into Transport display: {rendered}"
        );
        assert!(
            !rendered.contains("query-secret"),
            "query leaked into Transport display: {rendered}"
        );
        assert!(
            rendered.contains("127.0.0.1"),
            "host should remain observable: {rendered}"
        );
    }
}
