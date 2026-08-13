//! `wayland-nano auth login|status|logout <server>` (P3 §6.2/§8, F-P3-1) —
//! the PRODUCTION caller of the nano-mcp OAuth machinery (PKCE S256 +
//! RFC 9728/8414 trust chain + keyring/`<VAR>_FILE` storage), which was
//! merged without one.
//!
//! - Server names resolve against `NANO_MCP_SERVERS` (operator config,
//!   `SpecSource::Config`). An unknown name or a non-HTTP spec is a TYPED
//!   refusal (exit 2, kind-named message) — the CLI never invents a server
//!   and OAuth applies to HTTP servers only.
//! - `login` drives the hook-driven `flow::login` with the production
//!   wiring: session transport = an [`EgressTransport`] whose policy allows
//!   EXACTLY the MCP server origin (§6.1/§6.3 — the AS origin is NEVER
//!   host-allowlisted; AS traffic rides the one-shot bootstrap client and
//!   the endpoint-grant-scoped client only); bootstrap factory =
//!   `discovery::BootstrapClient::for_issuer`; scoped factory =
//!   `discovery::scoped_client`; storage = `storage::CredentialStore`
//!   (keyring-primary, `NANO_MCP_OAUTH_REFRESH_FILE_<SERVER>` fallback per
//!   §6.4). Operator approval of the DISCOVERED AS origin is an interactive
//!   console confirm (§6.2 step 2): a non-TTY stdin or any answer other
//!   than an explicit yes DECLINES, fail-closed (`operator_declined`). The
//!   authorize URL is PRINTED for the operator — never auto-fetched, and v1
//!   never shells out to a browser. The grant is journaled journal-first
//!   through `acp_mode::oauth_grant_recorder` (THE one producer) into the
//!   dedicated append-only grants journal `<nano_home>/oauth/grants.jsonl`;
//!   §6.3 step-4 replay-consumption of that file at session construction is
//!   the tracked follow-up (it lands with the dispatcher HTTP binding).
//! - `status` prints the §6.2 tri-state (missing / usable /
//!   authorization_required) — one bounded line per server.
//! - `logout` deletes stored credentials and reports the deletion — never
//!   token material.
//!
//! Every `OAuthError` maps to a printed typed reason plus a nonzero exit;
//! the Display impls are sanitized by construction (bounded reason codes —
//! no URLs with query/userinfo, no tokens, no provider prose).

use nano_agent::mcp::{McpServerSpec, Transport, mint_instance_id};
use nano_mcp::oauth::discovery;
use nano_mcp::oauth::flow::{self, GrantEndpoint, LoginHooks, LoginRequest};
use nano_mcp::oauth::storage::{CredentialStore, StoredTokens, TokenStorage};
use nano_mcp::oauth::{EgressTransport, FailReason, OAuthError, OAuthTransport};
use nano_session::NanoErrorKind;
use std::path::Path;
use std::sync::Arc;

const USAGE: &str = "usage: wayland-nano auth login|status|logout <server>";

/// The parsed command (§8 grammar; §6.2 shows `status` bare — a server name
/// is optional for status only, narrowing the report to that server).
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthCommand {
    Login(String),
    Status(Option<String>),
    Logout(String),
}

fn parse_args(args: &[String]) -> Result<AuthCommand, i32> {
    let usage = || {
        eprintln!("{USAGE}");
        2
    };
    let Some(verb) = args.first().map(String::as_str) else {
        return Err(usage());
    };
    let rest = &args[1..];
    match verb {
        "login" => match rest {
            [server] => Ok(AuthCommand::Login(server.clone())),
            _ => Err(usage()),
        },
        "logout" => match rest {
            [server] => Ok(AuthCommand::Logout(server.clone())),
            _ => Err(usage()),
        },
        "status" => match rest {
            [] => Ok(AuthCommand::Status(None)),
            [server] => Ok(AuthCommand::Status(Some(server.clone()))),
            _ => Err(usage()),
        },
        _ => Err(usage()),
    }
}

/// Resolve `<server>` against the operator-configured specs. Unknown name
/// or a non-HTTP spec ⇒ typed refusal (exit 2, `invalid_params` — §7 routes
/// config-shape failures there).
fn resolve_http_spec(server: &str) -> Result<McpServerSpec, i32> {
    let specs = crate::mcp_specs::mcp_specs_from_env();
    let Some(spec) = specs.into_iter().find(|s| s.name == server) else {
        eprintln!(
            "wayland-nano: {}: unknown MCP server '{server}' (not in NANO_MCP_SERVERS)",
            crate::mcp_specs::kind_token(NanoErrorKind::InvalidParams)
        );
        return Err(2);
    };
    if !matches!(spec.transport, Transport::Http { .. }) {
        eprintln!(
            "wayland-nano: {}: MCP server '{server}' is a stdio server; auth commands apply to HTTP (OAuth) servers only",
            crate::mcp_specs::kind_token(NanoErrorKind::InvalidParams)
        );
        return Err(2);
    }
    Ok(spec)
}

/// The dedicated append-only grants journal for standalone logins
/// (`<nano_home>/oauth/grants.jsonl`; the directory is created on demand).
/// Journal-first is durable here: a login that cannot open/append its grant
/// journal aborts BEFORE any endpoint grant exists in a live policy.
fn open_grants_journal(
    nano_home: &Path,
) -> Result<Arc<nano_session::JournalCoordinator>, OAuthError> {
    let dir = nano_home.join("oauth");
    std::fs::create_dir_all(&dir).map_err(|err| {
        eprintln!(
            "wayland-nano: cannot create the OAuth grants directory {}: {err}",
            dir.display()
        );
        OAuthError::Failed {
            reason: FailReason::JournalUnavailable,
        }
    })?;
    nano_session::JournalCoordinator::open(dir.join("grants.jsonl"))
        .map(Arc::new)
        .map_err(|err| {
            eprintln!("wayland-nano: cannot open the OAuth grants journal: {err}");
            OAuthError::Failed {
                reason: FailReason::JournalUnavailable,
            }
        })
}

/// §6.2 step 2 decision, factored for tests: a non-TTY console, a read
/// failure, or any answer other than an explicit yes DECLINES — fail-closed
/// (the flow turns `false` into typed `operator_declined`, nothing
/// journaled, no listener).
fn as_origin_approved(tty: bool, answer: Option<&str>) -> bool {
    tty && answer
        .map(str::trim)
        .is_some_and(|a| a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes"))
}

/// The production approval surface: the DISCOVERED AS origin is displayed
/// on stderr and must be explicitly confirmed on stdin BEFORE any browser
/// handoff. A non-TTY stdin declines without reading (fail-closed).
fn console_approve_as_origin(server_name: &str, as_origin: &str) -> bool {
    use std::io::IsTerminal;
    eprintln!(
        "wayland-nano: MCP server '{server_name}' discovered authorization server: {as_origin}"
    );
    if !std::io::stdin().is_terminal() {
        eprintln!("wayland-nano: stdin is not a terminal — declining (fail-closed)");
        return as_origin_approved(false, None);
    }
    eprint!("approve OAuth login with this authorization server? [y/N] ");
    let mut line = String::new();
    let answer = std::io::stdin().read_line(&mut line).ok().map(|_| line);
    as_origin_approved(true, answer.as_deref())
}

/// Map an OAuthError to its §7 kind-named line + exit 2. The Display impls
/// are sanitized by construction (bounded reason codes only).
fn report_oauth_error(err: &OAuthError) -> i32 {
    let kind = match err {
        OAuthError::AuthorizationRequired { .. } => NanoErrorKind::McpAuthorizationRequired,
        OAuthError::CredstoreUnavailable { .. } => NanoErrorKind::McpCredstoreUnavailable,
        OAuthError::Failed { .. } => NanoErrorKind::McpOAuthFailed,
        OAuthError::EgressDenied { .. } => NanoErrorKind::EgressDenied,
        OAuthError::Transport { .. } => NanoErrorKind::McpTransport,
    };
    eprintln!(
        "wayland-nano: {}: {err}",
        crate::mcp_specs::kind_token(kind)
    );
    2
}

/// Wall-clock seconds since the Unix epoch (the flow's injected clock).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `wayland-nano auth login <server>` — the full §6.2 authorization-code +
/// PKCE S256 flow with the §6.3 trust chain and egress discipline.
async fn login(nano_home: &Path, server: &str) -> i32 {
    let spec = match resolve_http_spec(server) {
        Ok(spec) => spec,
        Err(code) => return code,
    };
    let Transport::Http { url } = &spec.transport else {
        return 2; // resolve_http_spec already refused non-HTTP specs
    };
    let instance_id = mint_instance_id(&spec);
    let coordinator = match open_grants_journal(nano_home) {
        Ok(coordinator) => coordinator,
        Err(err) => return report_oauth_error(&err),
    };
    // THE journal-first grant producer (F-36); a validation/append failure
    // aborts the login BEFORE the scoped client is built.
    let record_grant = crate::acp_mode::oauth_grant_recorder(coordinator);

    // §6.1/§6.3: the session transport's policy allows EXACTLY the MCP
    // server origin (https host) — the AS origin is NEVER host-allowlisted;
    // AS traffic rides the one-shot bootstrap client (single metadata GET
    // grant, redirects disabled) and the endpoint-grant-scoped client.
    let session_policy = nano_egress::policy::EgressPolicy::new().allow_url(url);
    let session_transport =
        EgressTransport::new(nano_egress::client::EgressClient::new(session_policy));

    let bootstrap_factory = |candidate: &str| -> Result<Box<dyn OAuthTransport>, OAuthError> {
        Ok(Box::new(
            discovery::BootstrapClient::for_issuer(candidate)?.into_transport(),
        ))
    };
    let scoped_factory = |as_origin: &str,
                          endpoints: &[GrantEndpoint]|
     -> Result<Box<dyn OAuthTransport>, OAuthError> {
        let pairs: Vec<(nano_egress::grant::HttpMethod, String)> = endpoints
            .iter()
            .map(|e| (e.method, e.path.clone()))
            .collect();
        Ok(Box::new(EgressTransport::new(discovery::scoped_client(
            as_origin, &pairs,
        )?)))
    };
    let storage = CredentialStore::new();
    let server_name = spec.name.clone();
    let approve_as_origin =
        move |as_origin: &str| console_approve_as_origin(&server_name, as_origin);
    // §6.2 step 4: the browser handoff is a PRINTED URL with instructions —
    // never an auto-fetch; v1 never shells out to a browser.
    let open_authorize_url = |url: &str| {
        eprintln!(
            "wayland-nano: open this URL in your browser to authorize (nano never fetches it):"
        );
        println!("{url}");
    };
    let outcome = flow::login(LoginRequest {
        server_id: &instance_id,
        server_url: url,
        // §6.2 step 7: no static client-id override in v1 — RFC 7591 DCR
        // rides the flow when the AS advertises it.
        static_client_id: None,
        session_transport: &session_transport,
        bootstrap_factory: &bootstrap_factory,
        scoped_factory: &scoped_factory,
        storage: &storage,
        hooks: LoginHooks {
            approve_as_origin: &approve_as_origin,
            open_authorize_url: &open_authorize_url,
            record_grant: &record_grant,
            now_unix: &now_unix,
        },
    })
    .await;
    match outcome {
        Ok(outcome) => {
            println!(
                "logged in to MCP server '{}' ({instance_id}); authorization server {}; dynamic client registration: {}",
                spec.name,
                outcome.as_origin,
                if outcome.dcr_used { "used" } else { "not used" }
            );
            0
        }
        Err(err) => report_oauth_error(&err),
    }
}

/// The §6.2/§6.4 tri-state (`StoredOAuthTokenStatus` mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    Missing,
    Usable,
    AuthorizationRequired,
}

impl AuthStatus {
    fn as_str(&self) -> &'static str {
        match self {
            AuthStatus::Missing => "missing",
            AuthStatus::Usable => "usable",
            AuthStatus::AuthorizationRequired => "authorization_required",
        }
    }
}

/// The tri-state decision, factored for tests (the storage double injects
/// the `Option<&StoredTokens>` directly). Semantics: a set the 30s-skew
/// check would refresh BEFORE use counts as Usable (the refresh path
/// exists); a present set whose access token is missing or expired-ish with
/// NO refresh token is AuthorizationRequired; nothing stored is Missing.
pub fn status_of(tokens: Option<&StoredTokens>, now_unix: u64) -> AuthStatus {
    let Some(tokens) = tokens else {
        return AuthStatus::Missing;
    };
    if flow::token_needs_refresh(tokens, now_unix) {
        // A refresh token exists and the refresh runs before use (§6.5).
        return AuthStatus::Usable;
    }
    match (&tokens.access_token, tokens.expires_at_unix) {
        // No refresh token (token_needs_refresh was false) and the access
        // token is expired or inside the refresh skew: login needed.
        (Some(_), Some(expiry)) if now_unix.saturating_add(flow::REFRESH_SKEW_SECS) >= expiry => {
            AuthStatus::AuthorizationRequired
        }
        (Some(_), _) => AuthStatus::Usable,
        // No access token and no refresh token: nothing usable.
        (None, _) => AuthStatus::AuthorizationRequired,
    }
}

/// `wayland-nano auth status [server]` — one bounded line per server:
/// `<name>\t<instance_id>\t<state>`. Never token material.
fn status(server: Option<&str>) -> i32 {
    let now = now_unix();
    let storage = CredentialStore::new();
    let specs = match server {
        Some(name) => match resolve_http_spec(name) {
            Ok(spec) => vec![spec],
            Err(code) => return code,
        },
        None => crate::mcp_specs::mcp_specs_from_env()
            .into_iter()
            .filter(|s| matches!(s.transport, Transport::Http { .. }))
            .collect(),
    };
    let mut exit = 0;
    for spec in &specs {
        let instance_id = mint_instance_id(spec);
        match storage.load(&instance_id) {
            Ok(tokens) => {
                let state = status_of(tokens.as_ref(), now);
                println!("{}\t{instance_id}\t{}", spec.name, state.as_str());
            }
            Err(err) => {
                // e.g. keyring unavailable AND no refresh file (§6.4): the
                // typed error is the honest status for this server.
                eprintln!(
                    "wayland-nano: {} ({}): {}",
                    spec.name,
                    instance_id,
                    report_oauth_error_line(&err)
                );
                exit = 2;
            }
        }
    }
    if specs.is_empty() {
        eprintln!("wayland-nano: no HTTP MCP servers configured (NANO_MCP_SERVERS)");
    }
    exit
}

/// The kind-named one-line form of an OAuthError (no exit; the caller owns
/// the code). Sanitized Display, same mapping as [`report_oauth_error`].
fn report_oauth_error_line(err: &OAuthError) -> String {
    let kind = match err {
        OAuthError::AuthorizationRequired { .. } => NanoErrorKind::McpAuthorizationRequired,
        OAuthError::CredstoreUnavailable { .. } => NanoErrorKind::McpCredstoreUnavailable,
        OAuthError::Failed { .. } => NanoErrorKind::McpOAuthFailed,
        OAuthError::EgressDenied { .. } => NanoErrorKind::EgressDenied,
        OAuthError::Transport { .. } => NanoErrorKind::McpTransport,
    };
    format!("{}: {err}", crate::mcp_specs::kind_token(kind))
}

/// `wayland-nano auth logout <server>` — delete every stored credential for
/// the server (keyring entry + the refresh file named by
/// `NANO_MCP_OAUTH_REFRESH_FILE_<SERVER>`); report what happened, never
/// token material.
fn logout(server: &str) -> i32 {
    let spec = match resolve_http_spec(server) {
        Ok(spec) => spec,
        Err(code) => return code,
    };
    let instance_id = mint_instance_id(&spec);
    let storage = CredentialStore::new();
    // Load first so the report says whether anything was stored; a load
    // failure is surfaced typed (and the delete is still attempted).
    let had_stored = storage.load(&instance_id).ok().flatten().is_some();
    match storage.delete(&instance_id) {
        Ok(()) => {
            if had_stored {
                println!(
                    "logged out of MCP server '{}' ({instance_id}); stored credentials deleted",
                    spec.name
                );
            } else {
                println!(
                    "MCP server '{}' ({instance_id}): no stored credentials",
                    spec.name
                );
            }
            0
        }
        Err(err) => report_oauth_error(&err),
    }
}

/// CLI entry (main.rs owns the tokio runtime; `login` is async).
pub async fn run(nano_home: &Path, args: &[String]) -> i32 {
    match parse_args(args) {
        Err(code) => code,
        Ok(AuthCommand::Login(server)) => login(nano_home, &server).await,
        Ok(AuthCommand::Status(server)) => status(server.as_deref()),
        Ok(AuthCommand::Logout(server)) => logout(&server),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_mcp::oauth::flow::{GrantEndpoint as FlowEndpoint, GrantRecord};

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    // --- arg parsing / usage exits ----------------------------------------

    #[test]
    fn parse_args_accepts_the_grammar() {
        assert_eq!(
            parse_args(&s(&["login", "srv"])).unwrap(),
            AuthCommand::Login("srv".into())
        );
        assert_eq!(
            parse_args(&s(&["logout", "srv"])).unwrap(),
            AuthCommand::Logout("srv".into())
        );
        assert_eq!(
            parse_args(&s(&["status"])).unwrap(),
            AuthCommand::Status(None)
        );
        assert_eq!(
            parse_args(&s(&["status", "srv"])).unwrap(),
            AuthCommand::Status(Some("srv".into()))
        );
    }

    #[test]
    fn parse_args_rejects_off_grammar() {
        for args in [
            &[][..],
            &["bogus"][..],
            &["login"][..],           // login requires the server
            &["login", "a", "b"][..], // exactly one positional
            &["logout"][..],
            &["status", "a", "b"][..],
        ] {
            assert_eq!(parse_args(&s(args)), Err(2), "args: {args:?}");
        }
    }

    // --- operator approval (§6.2 step 2, fail-closed) ----------------------

    #[test]
    fn as_origin_approval_is_fail_closed() {
        // Non-TTY declines unconditionally (even with a piped "y").
        assert!(!as_origin_approved(false, Some("y")));
        assert!(!as_origin_approved(false, None));
        // TTY: only an explicit yes approves.
        assert!(as_origin_approved(true, Some("y")));
        assert!(as_origin_approved(true, Some(" yes ")));
        assert!(as_origin_approved(true, Some("Y")));
        assert!(!as_origin_approved(true, Some("")));
        assert!(!as_origin_approved(true, Some("n")));
        assert!(!as_origin_approved(true, Some("yes please")));
        assert!(!as_origin_approved(true, None)); // read failure declines
    }

    // --- status tri-state (§6.2/§6.4) --------------------------------------

    fn tokens(
        access: Option<&str>,
        refresh: Option<&str>,
        expires_at_unix: Option<u64>,
    ) -> StoredTokens {
        StoredTokens {
            access_token: access.map(str::to_string),
            refresh_token: refresh.map(str::to_string),
            expires_at_unix,
        }
    }

    #[test]
    fn status_tri_state() {
        const NOW: u64 = 1_800_000_000;
        assert_eq!(status_of(None, NOW), AuthStatus::Missing);
        // Fresh access token (no refresh needed): usable.
        assert_eq!(
            status_of(Some(&tokens(Some("a-CANARY"), None, Some(NOW + 3600))), NOW),
            AuthStatus::Usable
        );
        // No expiry advertised: usable until a 401 says otherwise.
        assert_eq!(
            status_of(Some(&tokens(Some("a-CANARY"), None, None)), NOW),
            AuthStatus::Usable
        );
        // Expired-ish access WITH a refresh token: the refresh runs before
        // use (§6.5) — usable.
        assert_eq!(
            status_of(
                Some(&tokens(Some("a-CANARY"), Some("r-CANARY"), Some(NOW + 10))),
                NOW
            ),
            AuthStatus::Usable
        );
        // File-path load (refresh token only, no access): refreshes before
        // use — usable.
        assert_eq!(
            status_of(Some(&tokens(None, Some("r-CANARY"), None)), NOW),
            AuthStatus::Usable
        );
        // Expired-ish access and NO refresh token: authorization required.
        assert_eq!(
            status_of(Some(&tokens(Some("a-CANARY"), None, Some(NOW + 10))), NOW),
            AuthStatus::AuthorizationRequired
        );
        // Nothing usable at all (no access, no refresh).
        assert_eq!(
            status_of(Some(&tokens(None, None, None)), NOW),
            AuthStatus::AuthorizationRequired
        );
    }

    // --- the standalone grants journal path (§6.3 journal-first) -----------

    /// `auth login`'s record_grant path: the grants journal opens (creating
    /// `<nano_home>/oauth/`), and a valid grant appends through THE
    /// `oauth_grant_recorder` producer and replays as `Op::McpOauthGrant`.
    #[test]
    fn grants_journal_appends_through_the_recorder() {
        let tmp = tempfile::tempdir().unwrap();
        let journal_path = tmp.path().join("oauth").join("grants.jsonl");
        assert!(!journal_path.exists());
        let coordinator = open_grants_journal(tmp.path()).expect("open grants journal");
        assert!(journal_path.exists(), "open creates the dir + journal");

        let recorder = crate::acp_mode::oauth_grant_recorder(coordinator);
        let record = GrantRecord {
            grant_id: "g-auth-cmd-1".into(),
            server_id: "srv_0123456789abcdef".into(),
            as_origin: "https://as.example".into(),
            issuer: "https://as.example".into(),
            endpoints: vec![FlowEndpoint {
                method: nano_egress::grant::HttpMethod::Post,
                path: "/tenant1/token".into(),
            }],
        };
        recorder(&record).expect("grant appends");
        let report = nano_session::reader::read_journal(&journal_path).unwrap();
        let grants: Vec<_> = report
            .envelopes
            .iter()
            .filter_map(|e| match &e.op {
                nano_session::op::Op::McpOauthGrant {
                    grant_id,
                    server_id,
                    ..
                } => Some((grant_id.clone(), server_id.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            grants,
            vec![(
                "g-auth-cmd-1".to_string(),
                "srv_0123456789abcdef".to_string()
            )],
            "the standalone login's grant is durable in the grants journal"
        );
    }

    /// §6.3 step-2 wiring pin: the bootstrap factory product is capped to
    /// EXACTLY the RFC 8414 metadata GET — any other URL or method denies
    /// with zero socket activity (the same discipline the flow relies on).
    #[test]
    fn production_bootstrap_transport_is_one_grant_only() {
        let transport = discovery::BootstrapClient::for_issuer("https://as.example/tenant1")
            .expect("bootstrap")
            .into_transport();
        // Drive it through the trait object exactly like the flow does.
        let transport: Box<dyn OAuthTransport> = Box::new(transport);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let err = transport
                .get_bounded("https://as.example/other")
                .await
                .expect_err("a second path must deny");
            assert!(matches!(err, OAuthError::EgressDenied { .. }));
            let err = transport
                .post_form(
                    "https://as.example/.well-known/oauth-authorization-server/tenant1",
                    &[],
                )
                .await
                .expect_err("a method mismatch must deny");
            assert!(matches!(err, OAuthError::EgressDenied { .. }));
        });
    }
}
