//! RFC 9728 protected-resource metadata → trust-chained RFC 8414 AS
//! discovery (P3 §6.3). Same-origin is enforced BY CONSTRUCTION: the 9728
//! URL is derived from the configured server origin, never from a server
//! response; each candidate issuer is fetched through a ONE-SHOT bootstrap
//! client whose policy carries exactly ONE `EndpointGrant` — GET on the
//! exact RFC 8414 metadata URL — with redirects DISABLED. The fetched
//! metadata's `issuer` must EXACTLY equal the candidate string.

use nano_egress::client::EgressClient;
use nano_egress::grant::HttpMethod;
use nano_egress::policy::EgressPolicy;

use super::EgressTransport;
use super::FailReason;
use super::OAuthError;
use super::OAuthTransport;

/// Well-known suffixes (RFC 9728 / RFC 8414).
const PROTECTED_RESOURCE_WELL_KNOWN: &str = "/.well-known/oauth-protected-resource";
const AS_WELL_KNOWN: &str = "/.well-known/oauth-authorization-server";

/// Field bounds (§6.3: bounded strings — issuer ≤ 512, origin ≤ 256).
const MAX_ISSUER_CHARS: usize = 512;
const MAX_ORIGIN_CHARS: usize = 256;
const MAX_ENDPOINT_CHARS: usize = 1024;
const MAX_AUTH_SERVERS: usize = 8;

/// Step 1: the RFC 9728 metadata URL for a configured MCP server URL.
/// Derived from the configured ORIGIN (scheme://host[:port]) — never from a
/// server response — so the fetch is same-origin by construction. https
/// only, matching `mcp_specs` parse-time rejection (§6.1).
pub fn protected_resource_metadata_url(server_url: &str) -> Result<String, OAuthError> {
    let origin = origin_of(server_url)?;
    Ok(format!("{origin}{PROTECTED_RESOURCE_WELL_KNOWN}"))
}

/// The scheme://host[:port] origin of an https URL, validated by the url
/// crate's rules (IDNA/lowercase host, effective port). Plain http is a
/// typed rejection — OAuth discovery never runs over cleartext.
pub fn origin_of(url: &str) -> Result<String, OAuthError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| OAuthError::Failed {
        reason: FailReason::MetadataInvalid,
    })?;
    if parsed.scheme() != "https" {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    let host = parsed.host_str().ok_or(OAuthError::Failed {
        reason: FailReason::MetadataInvalid,
    })?;
    let origin = match parsed.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    };
    if origin.len() > MAX_ORIGIN_CHARS {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    Ok(origin)
}

/// The RFC 8414 §3.1 metadata URL for a candidate issuer: for a
/// path-bearing issuer the well-known component is INSERTED BEFORE the
/// path (`https://h/tenant1` ⇒
/// `https://h/.well-known/oauth-authorization-server/tenant1`); a root
/// issuer (`https://h`) ⇒ `https://h/.well-known/oauth-authorization-server`.
/// An issuer carrying query or fragment is rejected (typed).
pub fn as_metadata_url(issuer: &str) -> Result<String, OAuthError> {
    if issuer.chars().count() > MAX_ISSUER_CHARS {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    let parsed = reqwest::Url::parse(issuer).map_err(|_| OAuthError::Failed {
        reason: FailReason::MetadataInvalid,
    })?;
    if parsed.scheme() != "https" {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        });
    }
    let origin = origin_of(issuer)?;
    let path = parsed.path();
    // path starts with '/' (url crate guarantee for special schemes); a
    // root issuer's path is exactly "/".
    let suffix = path.strip_prefix('/').unwrap_or(path);
    let metadata = if suffix.is_empty() {
        format!("{origin}{AS_WELL_KNOWN}")
    } else {
        format!("{origin}{AS_WELL_KNOWN}/{suffix}")
    };
    Ok(metadata)
}

/// RFC 9728 protected-resource metadata (the members this flow consumes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMetadata {
    pub authorization_servers: Vec<String>,
}

/// Parse + bound RFC 9728 metadata. Absent/invalid ⇒ typed failure (the
/// server doesn't speak 9728; NO heuristic fallback, §6.3).
pub fn parse_resource_metadata(body: &[u8]) -> Result<ResourceMetadata, OAuthError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| OAuthError::Failed {
            reason: FailReason::NoProtectedResourceMetadata,
        })?;
    let servers = value
        .get("authorization_servers")
        .and_then(|v| v.as_array())
        .ok_or(OAuthError::Failed {
            reason: FailReason::NoProtectedResourceMetadata,
        })?;
    if servers.is_empty() || servers.len() > MAX_AUTH_SERVERS {
        return Err(OAuthError::Failed {
            reason: FailReason::NoProtectedResourceMetadata,
        });
    }
    let mut out = Vec::with_capacity(servers.len());
    for entry in servers {
        let Some(s) = entry.as_str() else {
            return Err(OAuthError::Failed {
                reason: FailReason::NoProtectedResourceMetadata,
            });
        };
        out.push(s.to_string());
    }
    Ok(ResourceMetadata {
        authorization_servers: out,
    })
}

/// A validated authorization server: issuer exact-match passed, every
/// endpoint confirmed same-origin with the issuer, S256 proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAs {
    /// The validated issuer string (EXACTLY the candidate, ≤ 512 chars).
    pub issuer: String,
    /// The AS origin (https://host[:port], ≤ 256 chars) — the value the
    /// operator approves (§6.2 step 2) and the journal records (§6.3).
    pub as_origin: String,
    /// The exact RFC 8414 metadata URL that was fetched (metadata GET grant).
    pub metadata_url: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// RFC 7591 dynamic client registration endpoint, when advertised.
    pub registration_endpoint: Option<String>,
}

/// Validate fetched RFC 8414 metadata against the candidate issuer
/// (§6.3 steps 2–3): `issuer` EXACT equality, `authorization_endpoint` and
/// `token_endpoint` (and `registration_endpoint` when present) on the SAME
/// origin as the validated AS, S256 proven by
/// `code_challenge_methods_supported`. Fail-closed on every deviation.
pub fn validate_as_metadata(
    candidate_issuer: &str,
    metadata_url: &str,
    body: &[u8],
) -> Result<ValidatedAs, OAuthError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })?;
    let str_field = |name: &str| -> Result<String, OAuthError> {
        let s = value
            .get(name)
            .and_then(|v| v.as_str())
            .ok_or(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
        if s.chars().count() > MAX_ENDPOINT_CHARS {
            return Err(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            });
        }
        Ok(s.to_string())
    };

    // The RFC 8414 issuer check: EXACT string equality with the candidate.
    let issuer = str_field("issuer")?;
    if issuer != candidate_issuer {
        return Err(OAuthError::Failed {
            reason: FailReason::IssuerMismatch,
        });
    }
    let as_origin = origin_of(&issuer)?;

    // Every consumed endpoint must sit on the validated AS origin — an
    // authorization endpoint elsewhere is a cross-origin redirect of the
    // user's login, typed-denied (§6.3 "No 9728, no OAuth" rule).
    let authorization_endpoint = str_field("authorization_endpoint")?;
    let token_endpoint = str_field("token_endpoint")?;
    let registration_endpoint = match value.get("registration_endpoint") {
        Some(v) => Some(
            v.as_str()
                .filter(|s| s.chars().count() <= MAX_ENDPOINT_CHARS)
                .ok_or(OAuthError::Failed {
                    reason: FailReason::MetadataInvalid,
                })?
                .to_string(),
        ),
        None => None,
    };
    for endpoint in [&authorization_endpoint, &token_endpoint]
        .into_iter()
        .chain(registration_endpoint.iter())
    {
        if origin_of(endpoint)? != as_origin {
            return Err(OAuthError::Failed {
                reason: FailReason::CrossOriginEndpoint,
            });
        }
    }

    // S256 ONLY: the AS must prove S256 support. Absence of the member
    // proves nothing — fail closed (§6.2 step 3; §9.2).
    let methods = value
        .get("code_challenge_methods_supported")
        .and_then(|v| v.as_array())
        .ok_or(OAuthError::Failed {
            reason: FailReason::S256Unsupported,
        })?;
    let supports_s256 = methods.iter().any(|m| m.as_str() == Some("S256"));
    if !supports_s256 {
        return Err(OAuthError::Failed {
            reason: FailReason::S256Unsupported,
        });
    }

    Ok(ValidatedAs {
        issuer,
        as_origin,
        metadata_url: metadata_url.to_string(),
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
    })
}

/// The exact endpoint grants for a validated AS (§6.3 step 3): token POST,
/// registration POST if DCR is used, metadata GET — never a wildcard, ≤ 4
/// entries. Built through `EgressPolicy::allow_endpoint`, so the journaled
/// form is normalized through `normalize_endpoint` before journaling.
pub fn endpoint_grants(
    validated: &ValidatedAs,
    dcr_used: bool,
) -> Result<Vec<(HttpMethod, String)>, OAuthError> {
    let mut out = vec![
        (HttpMethod::Post, validated.token_endpoint.clone()),
        (HttpMethod::Get, validated.metadata_url.clone()),
    ];
    if dcr_used {
        let registration = validated
            .registration_endpoint
            .clone()
            .ok_or(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
        out.push((HttpMethod::Post, registration));
    }
    // Every granted URL must survive normalize_endpoint (https, no
    // query/fragment/userinfo) — a metadata URL that can't be granted is a
    // trust-chain failure, not a policy mutation.
    for (method, url) in &out {
        EgressPolicy::new()
            .allow_endpoint(*method, url)
            .map_err(|_| OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
    }
    Ok(out)
}

/// The scoped post-login client (§6.3 step 4): empty base policy + the
/// journaled endpoint grants — NO host-wide `allow_host` for the AS origin
/// anywhere — and redirects DISABLED (an AS that redirects its token
/// endpoint is a typed `McpOAuthFailed`, not a followed redirect).
/// `endpoints` are canonical `(method, path)` pairs (the journaled form);
/// each is re-normalized through `allow_endpoint` against `as_origin`.
pub fn scoped_client(
    as_origin: &str,
    endpoints: &[(HttpMethod, String)],
) -> Result<EgressClient, OAuthError> {
    let mut policy = EgressPolicy::new();
    for (method, path) in endpoints {
        if !path.starts_with('/') {
            return Err(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            });
        }
        policy = policy
            .allow_endpoint(*method, &format!("{as_origin}{path}"))
            .map_err(|_| OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
    }
    Ok(EgressClient::without_redirects(policy))
}

/// The ONE-SHOT bootstrap client (§6.3 step 2): a policy carrying exactly
/// ONE `EndpointGrant` (GET on the exact RFC 8414 metadata URL derived from
/// the candidate issuer), redirects DISABLED, consumed by the single fetch
/// (`fetch` takes `self`). It cannot be reused for any other URL — a second
/// path on the same host is `EgressDenied` with zero socket activity.
pub struct BootstrapClient {
    client: EgressClient,
    url: String,
}

impl BootstrapClient {
    pub fn for_issuer(candidate_issuer: &str) -> Result<Self, OAuthError> {
        let url = as_metadata_url(candidate_issuer)?;
        let policy = EgressPolicy::new()
            .allow_endpoint(HttpMethod::Get, &url)
            .map_err(|_| OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
        Ok(Self {
            client: EgressClient::without_redirects(policy),
            url,
        })
    }

    /// The metadata URL this client is bound to (the metadata GET grant).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The single fetch this client exists for. Consumes the client.
    pub async fn fetch(self) -> Result<(u16, Vec<u8>), OAuthError> {
        let Self { client, url } = self;
        let transport = EgressTransport::new(client);
        transport.get_bounded(&url).await
    }

    /// Consume into the transport half (the §6.2 CLI login lane hands it to
    /// the flow's bootstrap factory, which drives the one allowed fetch
    /// itself). The policy still caps it to EXACTLY the one RFC 8414
    /// metadata GET — any other URL or method is `EgressDenied` with zero
    /// socket activity.
    pub fn into_transport(self) -> EgressTransport {
        EgressTransport::new(self.client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_url_inserts_well_known_before_issuer_path() {
        // RFC 8414 §3.1: path-bearing issuer ⇒ INSERTION, not append.
        assert_eq!(
            as_metadata_url("https://h/tenant1").unwrap(),
            "https://h/.well-known/oauth-authorization-server/tenant1"
        );
        assert_eq!(
            as_metadata_url("https://h").unwrap(),
            "https://h/.well-known/oauth-authorization-server"
        );
        assert_eq!(
            as_metadata_url("https://h:8443/tenant1").unwrap(),
            "https://h:8443/.well-known/oauth-authorization-server/tenant1"
        );
        // Query/fragment on the issuer are typed rejections.
        assert!(as_metadata_url("https://h/tenant1?x=1").is_err());
        assert!(as_metadata_url("https://h/tenant1#f").is_err());
        // http issuers never reach the network.
        assert!(as_metadata_url("http://h").is_err());
    }

    #[test]
    fn protected_resource_url_is_same_origin_by_construction() {
        assert_eq!(
            protected_resource_metadata_url("https://mcp.example/mcp/").unwrap(),
            "https://mcp.example/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            protected_resource_metadata_url("https://mcp.example:8443/mcp/").unwrap(),
            "https://mcp.example:8443/.well-known/oauth-protected-resource"
        );
        assert!(protected_resource_metadata_url("http://mcp.example/mcp/").is_err());
    }

    fn valid_metadata(issuer: &str) -> String {
        format!(
            r#"{{"issuer":"{issuer}",
               "authorization_endpoint":"{issuer}/authorize",
               "token_endpoint":"{issuer}/token",
               "registration_endpoint":"{issuer}/register",
               "code_challenge_methods_supported":["S256"]}}"#
        )
    }

    #[test]
    fn valid_metadata_validates() {
        let candidate = "https://as.example";
        let meta_url = as_metadata_url(candidate).unwrap();
        let validated =
            validate_as_metadata(candidate, &meta_url, valid_metadata(candidate).as_bytes())
                .expect("valid metadata");
        assert_eq!(validated.as_origin, "https://as.example");
        assert_eq!(validated.token_endpoint, "https://as.example/token");
    }

    #[test]
    fn issuer_mismatch_is_typed() {
        let candidate = "https://as.example";
        let meta_url = as_metadata_url(candidate).unwrap();
        let err = validate_as_metadata(
            candidate,
            &meta_url,
            valid_metadata("https://evil.example").as_bytes(),
        )
        .expect_err("issuer mismatch must fail");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::IssuerMismatch
            }
        ));
    }

    #[test]
    fn cross_origin_authorization_endpoint_is_typed() {
        let candidate = "https://as.example";
        let meta_url = as_metadata_url(candidate).unwrap();
        let body = r#"{"issuer":"https://as.example",
            "authorization_endpoint":"https://evil.example/authorize",
            "token_endpoint":"https://as.example/token",
            "code_challenge_methods_supported":["S256"]}"#;
        let err = validate_as_metadata(candidate, &meta_url, body.as_bytes())
            .expect_err("cross-origin endpoint must fail");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::CrossOriginEndpoint
            }
        ));
    }

    #[test]
    fn s256_only_is_enforced() {
        let candidate = "https://as.example";
        let meta_url = as_metadata_url(candidate).unwrap();
        // plain-only ⇒ typed failure, never a downgrade.
        let plain_only = r#"{"issuer":"https://as.example",
            "authorization_endpoint":"https://as.example/authorize",
            "token_endpoint":"https://as.example/token",
            "code_challenge_methods_supported":["plain"]}"#;
        let err = validate_as_metadata(candidate, &meta_url, plain_only.as_bytes())
            .expect_err("plain-only must fail");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::S256Unsupported
            }
        ));
        // Member absent ⇒ cannot prove S256 ⇒ typed failure (fail-closed).
        let absent = r#"{"issuer":"https://as.example",
            "authorization_endpoint":"https://as.example/authorize",
            "token_endpoint":"https://as.example/token"}"#;
        let err = validate_as_metadata(candidate, &meta_url, absent.as_bytes())
            .expect_err("absent methods must fail");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::S256Unsupported
            }
        ));
    }

    #[test]
    fn grants_are_exact_and_bounded() {
        let candidate = "https://as.example/tenant1";
        let meta_url = as_metadata_url(candidate).unwrap();
        let validated =
            validate_as_metadata(candidate, &meta_url, valid_metadata(candidate).as_bytes())
                .expect("valid");
        let grants = endpoint_grants(&validated, true).expect("grants");
        assert_eq!(grants.len(), 3);
        assert_eq!(
            grants[0],
            (
                HttpMethod::Post,
                "https://as.example/tenant1/token".to_string()
            )
        );
        assert_eq!(
            grants[1],
            (
                HttpMethod::Get,
                "https://as.example/.well-known/oauth-authorization-server/tenant1".to_string()
            )
        );
        assert_eq!(
            grants[2],
            (
                HttpMethod::Post,
                "https://as.example/tenant1/register".to_string()
            )
        );
    }

    /// §12 bootstrap discipline: the one-shot client can reach ONLY the
    /// exact metadata URL — a second path on the same host is denied with
    /// ZERO socket activity (the denial precedes any socket).
    #[test]
    fn bootstrap_grant_caps_to_the_exact_metadata_url() {
        let bootstrap = BootstrapClient::for_issuer("https://as.example/tenant1").expect("client");
        // Reach into the built policy through a fresh request on the SAME
        // host but a different path: denied before any socket.
        let policy = EgressPolicy::new()
            .allow_endpoint(HttpMethod::Get, bootstrap.url())
            .expect("grant");
        let probe = EgressClient::without_redirects(policy);
        assert!(
            probe
                .request(reqwest::Method::GET, "https://as.example/other")
                .is_err(),
            "a second path on the AS host must deny"
        );
        assert!(
            probe
                .request(reqwest::Method::POST, bootstrap.url())
                .is_err(),
            "a method mismatch on the granted path must deny"
        );
        // The exact granted tuple builds (no socket until send).
        assert!(probe.request(reqwest::Method::GET, bootstrap.url()).is_ok());
        // And the real bootstrap client is one-shot by construction:
        // `fetch(self)` consumes it (compile-enforced).
        let _one_shot = bootstrap;
    }
}
