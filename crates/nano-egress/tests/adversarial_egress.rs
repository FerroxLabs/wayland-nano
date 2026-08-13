//! Adversarial egress tests: bypass attempts against the policy gate.
//!
//! Every hostile endpoint is a local TCP listener on 127.0.0.1 — no real
//! network is ever contacted. "Denied" assertions additionally prove that a
//! denial produces ZERO socket activity (the listener never sees a connection
//! carrying the test's canary), per the crate invariant "deny = no bytes
//! leave".
//!
//! Race hardening: each test drives its listener with a unique canary path
//! (`/nano-canary-<test>-<pid>`). A listener counts a connection as a hit
//! ONLY when the request head carries that test's canary; stray probes
//! (AV/EDR port scans, port reuse by another process under full-workspace
//! parallelism) get a 404 and stay inert instead of producing false-positive
//! "exfiltration" hits.

use nano_egress::client::{EgressClient, EgressError};
use nano_egress::policy::EgressPolicy;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A hostile endpoint: counts canary-carrying connections and answers each
/// with `handler`.
struct HostileListener {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

/// Bind an OS-assigned port (port 0 — never a hardcoded port, so tests cannot
/// collide through TIME_WAIT/reuse) and accept connections in the background.
/// A connection is only counted (and handed to `handler`) when its request
/// head contains `canary`; anything else is a stray probe and is dismissed.
fn spawn_listener(
    canary: &str,
    handler: impl Fn(TcpStream, usize) + Send + 'static,
) -> HostileListener {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let canary = canary.to_owned();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_thread = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let head = read_head(&mut stream);
                    if !head.contains(&canary) {
                        // Stray probe: not this test's traffic — never counted.
                        respond(&mut stream, "404 Not Found", "", "nano-stray-probe");
                        continue;
                    }
                    let n = hits_thread.fetch_add(1, Ordering::SeqCst) + 1;
                    handler(stream, n);
                }
                Err(_) => break,
            }
        }
    });
    HostileListener { addr, hits }
}

fn hit_count(listener: &HostileListener) -> usize {
    listener.hits.load(Ordering::SeqCst)
}

/// Read one HTTP request head; ignore timeouts/truncation (hostile peers).
/// Returns the raw head for canary matching.
fn read_head(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn respond(stream: &mut TcpStream, status: &str, extra_headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// --- Direct bypass attempts -------------------------------------------------

#[tokio::test]
async fn denied_host_produces_zero_socket_activity() {
    let canary = format!("/nano-canary-denied-host-{}", std::process::id());
    let hostile = spawn_listener(&canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nano-should-never-see-this");
    });
    let client = EgressClient::flux();
    let url = format!("http://127.0.0.1:{}{canary}", hostile.addr.port());
    let err = client
        .request(reqwest::Method::GET, &url)
        .expect_err("non-Flux host must be denied");
    assert!(matches!(err, EgressError::Denied { .. }), "err: {err:?}");
    // Give any (buggy) socket activity time to land, then prove none happened.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hit_count(&hostile),
        0,
        "denied request must never reach the network"
    );
}

#[test]
fn lookalike_host_bypass_attempts_are_denied() {
    let client = EgressClient::flux();
    let attempts = [
        "https://api.fluxrouter.ai.evil.com/v1/chat/completions",
        "https://evil.com/v1?next=api.fluxrouter.ai",
        "https://API.FLUXROUTER.AI.evil.com/v1",
        // userinfo tricks: real host is after the '@'
        "https://api.fluxrouter.ai@evil.com/v1",
        "https://api.fluxrouter.ai:password@evil.com/v1",
        // trailing-dot DNS equivalent is NOT the allowlisted host
        "https://api.fluxrouter.ai./v1",
        // schemeless and malformed URLs fail closed
        "api.fluxrouter.ai/v1/chat/completions",
        "//api.fluxrouter.ai/v1",
        "https://api.fluxrouter.ai:0x50/v1",
        // loopback / link-local / decimal-IP literals
        "http://127.0.0.1/latest/meta-data",
        "http://2130706433/latest/meta-data",
        "http://169.254.169.254/latest/meta-data",
        "https://[::1]/v1",
    ];
    for url in attempts {
        let err = client
            .request(reqwest::Method::GET, url)
            .expect_err("bypass attempt must be denied");
        assert!(
            matches!(err, EgressError::Denied { .. }),
            "{url} denied with wrong variant: {err:?}"
        );
    }
}

#[test]
fn flux_allowlist_accepts_only_the_real_host() {
    // Control: the allowlist itself is not over-broad.
    let policy = EgressPolicy::flux_only();
    assert!(policy.allows_host("https://api.fluxrouter.ai/v1/models"));
    assert!(!policy.allows_host("https://api.fluxrouter.ai.evil.com/v1/models"));
    assert!(!policy.allows_host("https://sub.api.fluxrouter.ai/v1/models"));
}

// --- Redirect-following bypass ----------------------------------------------

#[tokio::test]
async fn redirect_to_off_allowlist_host_must_not_be_followed() {
    // Hostile target: only reachable if the client follows a redirect WITHOUT
    // re-checking the egress policy against the redirect target.
    let exfil_canary = format!("/nano-canary-redirect-exfil-{}", std::process::id());
    let origin_canary = format!("/nano-canary-redirect-origin-{}", std::process::id());
    let exfil = spawn_listener(&exfil_canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nano-exfil-reached");
    });
    let exfil_port = exfil.addr.port();
    // Allowed origin: redirects every request to the off-allowlist host,
    // pointing at the exfil listener's canary path so only a real follow
    // registers as a hit.
    let origin = spawn_listener(&origin_canary, move |mut stream, _| {
        respond(
            &mut stream,
            "302 Found",
            &format!("Location: http://127.0.0.1:{exfil_port}{exfil_canary}\r\n"),
            "",
        );
    });
    // "localhost" is allowlisted; "127.0.0.1" deliberately is not — the two
    // names resolve to the same loopback, so only policy re-checking on the
    // redirect target can stop the exfiltration.
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("localhost"));
    let url = format!("http://localhost:{}{origin_canary}", origin.addr.port());
    let builder = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted origin must build");
    let _ = builder.send().await;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hit_count(&exfil),
        0,
        "SECURITY HOLE: redirect to off-allowlist host was followed — \
         the policy gate only checks the initial URL"
    );
}

#[tokio::test]
async fn redirect_within_allowlisted_host_is_allowed() {
    // Control: proves the harness above can observe a redirect when policy
    // permits it, so a green off-allowlist test is not a false negative.
    // Both the initial request and the redirect target use the canary path,
    // so each follow is distinguishable from stray traffic.
    let canary = format!("/nano-canary-redirect-control-{}", std::process::id());
    let location = canary.clone();
    let origin = spawn_listener(&canary, move |mut stream, n| {
        if n == 1 {
            respond(
                &mut stream,
                "302 Found",
                &format!("Location: {location}\r\n"),
                "",
            );
        } else {
            respond(&mut stream, "200 OK", "", "nano-redirect-final");
        }
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("localhost"));
    let url = format!("http://localhost:{}{canary}", origin.addr.port());
    let response = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .send()
        .await
        .expect("send");
    let body = response.text().await.expect("body");
    assert_eq!(
        hit_count(&origin),
        2,
        "same-host redirect should be followed"
    );
    assert!(body.contains("nano-redirect-final"), "final body: {body}");
}

// --- Credential/header redaction on error paths ------------------------------

#[test]
fn denied_error_display_redacts_query_and_userinfo_credentials() {
    let client = EgressClient::flux();
    let err = client
        .request(
            reqwest::Method::GET,
            "https://nano-user:nano-s3cr3t-password@evil.example.com/v1?api_key=nano-query-secret",
        )
        .expect_err("must deny");
    let rendered = err.to_string();
    assert!(
        !rendered.contains("nano-s3cr3t-password"),
        "userinfo credential leaked into Denied display: {rendered}"
    );
    assert!(
        !rendered.contains("nano-query-secret"),
        "query leaked into Denied display: {rendered}"
    );
}

#[test]
fn http_status_error_display_redacts_query_and_userinfo_credentials() {
    let client = EgressClient::flux();
    let err = client.classify_status(
        "https://nano-user:nano-s3cr3t-password@api.fluxrouter.ai/v1/chat/completions?api_key=nano-query-secret",
        401,
    );
    let rendered = err.to_string();
    assert!(rendered.contains("401"));
    assert!(
        !rendered.contains("nano-s3cr3t-password"),
        "userinfo credential leaked into HttpStatus display: {rendered}"
    );
    assert!(
        !rendered.contains("nano-query-secret"),
        "query leaked into HttpStatus display: {rendered}"
    );
}

#[tokio::test]
async fn transport_error_display_redacts_url_credentials() {
    // A failed transport must not echo the URL (query/userinfo) or headers
    // that a caller attached credentials to.
    //
    // Hardening choice: instead of "bind port 0, drop, then reuse the port"
    // (whose close window another process can win under parallel load,
    // turning a deterministic refusal into a surprise successful request),
    // the listener stays BOUND and accepts-then-drops every connection
    // without answering. What this test proves about EgressClient is that
    // `classify_transport` redacts credentials from ANY transport error, not
    // specifically ECONNREFUSED — and the accept-then-drop peer yields that
    // error deterministically while the bound port can never be stolen.
    let sink = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = sink.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in sink.incoming() {
            drop(stream); // accept, answer with nothing: deterministic error
        }
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
    let url = format!("http://127.0.0.1:{port}/v1?api_key=nano-transport-secret");
    let err = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .bearer_auth("nano-bearer-secret")
        .send()
        .await
        .expect_err("unanswered connection must error");
    let rendered = client.classify_transport(&err).to_string();
    assert!(
        !rendered.contains("nano-transport-secret"),
        "URL query credential leaked into Transport display: {rendered}"
    );
    assert!(
        !rendered.contains("nano-bearer-secret"),
        "Authorization header leaked into Transport display: {rendered}"
    );
}

// --- C4: bounded web fetch (fetch_bounded) -----------------------------------
//
// Two layers of proof:
//  1. Scripted FetchDriver (DNS + transport seam): private-range battery,
//     split-horizon answers, DNS-rebind flips mid-loop, hop limits, scheme
//     and content-type rules — with a send spy proving denial = zero sends.
//     The private-range check itself runs in the fetch loop, NOT behind the
//     seam.
//  2. Real TCP against loopback listeners via explicitly allowlisted IP
//     literals (no name to rebind — the allowlist entry is the pin): the
//     production SystemFetchDriver path end-to-end.

use nano_egress::client::{FetchDriver, FetchHop};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::IpAddr;
use std::sync::Mutex;

/// Scripted DNS + transport. `answers[host]` is a QUEUE so successive
/// resolutions of the same name can differ (DNS-rebind simulation). Every
/// send is recorded; a denied hop must produce ZERO sends.
#[derive(Debug, Default)]
struct ScriptedDriver {
    answers: Mutex<HashMap<String, VecDeque<io::Result<Vec<IpAddr>>>>>,
    responses: Mutex<HashMap<String, ScriptedHop>>,
    sends: std::sync::Arc<Mutex<Vec<String>>>,
}

#[derive(Debug, Clone)]
struct ScriptedHop {
    status: u16,
    location: Option<String>,
    content_type: Option<String>,
    content_length: Option<u64>,
    chunks: Vec<Vec<u8>>,
}

impl ScriptedDriver {
    fn with_answer(self, host: &str, answers: Vec<&str>) -> Self {
        let queue: VecDeque<io::Result<Vec<IpAddr>>> = answers
            .into_iter()
            .map(|a| Ok(vec![a.parse::<IpAddr>().expect("test ip")]))
            .collect();
        self.answers
            .lock()
            .unwrap()
            .entry(host.to_string())
            .or_default()
            .extend(queue);
        self
    }

    fn with_response(self, url: &str, hop: ScriptedHop) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), hop);
        self
    }
}

fn scripted_ok(body: &str) -> ScriptedHop {
    ScriptedHop {
        status: 200,
        location: None,
        content_type: Some("text/plain".into()),
        content_length: Some(body.len() as u64),
        chunks: vec![body.as_bytes().to_vec()],
    }
}

#[async_trait::async_trait]
impl FetchDriver for ScriptedDriver {
    async fn resolve(&self, host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
        self.answers
            .lock()
            .unwrap()
            .get_mut(host)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no scripted answer for {host}"),
                ))
            })
    }

    async fn send(
        &self,
        _host: &str,
        _port: u16,
        _addrs: &[IpAddr],
        url: &str,
        _timeout: Duration,
    ) -> Result<FetchHop, EgressError> {
        self.sends.lock().unwrap().push(url.to_string());
        let hop = self
            .responses
            .lock()
            .unwrap()
            .get(url)
            .unwrap_or_else(|| panic!("no scripted response for {url}"))
            .clone();
        Ok(FetchHop {
            status: hop.status,
            location: hop.location,
            content_type: hop.content_type,
            content_length: hop.content_length,
            body: Box::pin(futures_util::stream::iter(
                hop.chunks.into_iter().map(Ok::<Vec<u8>, EgressError>),
            )),
        })
    }
}

fn scripted_client(policy: EgressPolicy, driver: ScriptedDriver) -> EgressClient {
    EgressClient::new(policy).with_fetch_driver_for_tests(std::sync::Arc::new(driver))
}

/// One deny case per C4 §3.4.1 private-range class: the hostname IS
/// allowlisted; the resolution to a private/reserved address must deny at
/// check time with ZERO sends.
#[tokio::test]
async fn fetch_private_range_battery_denied_at_check() {
    let cases = [
        "0.0.0.0",
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.0.0.1",
        "192.168.0.1",
        "198.18.0.1",
        "240.0.0.1",
        "::",
        "::1",
        "fc00::1",
        "fe80::1",
        "64:ff9b::a00:1",  // NAT64 → 10.0.0.1
        "::ffff:10.0.0.1", // v4-mapped private
    ];
    for addr in cases {
        let driver = ScriptedDriver::default().with_answer("allowed.example", vec![addr]);
        let sends = std::sync::Arc::clone(&driver.sends);
        let client = scripted_client(EgressPolicy::new().allow_host("allowed.example"), driver);
        let err = client
            .fetch_bounded("https://allowed.example/x", 1024, Duration::from_secs(5))
            .await
            .expect_err("private resolution must deny");
        assert!(
            matches!(err, EgressError::PrivateAddress { .. }),
            "{addr}: wrong variant: {err:?}"
        );
        assert_eq!(sends.lock().unwrap().len(), 0, "{addr}: zero sends on deny");
    }
}

/// Split-horizon: ONE private answer among public ones denies the hop
/// (fail-closed on split answers).
#[tokio::test]
async fn fetch_split_dns_answers_deny_the_hop() {
    #[derive(Debug)]
    struct SplitDriver;
    #[async_trait::async_trait]
    impl FetchDriver for SplitDriver {
        async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
            Ok(vec![
                "93.184.216.34".parse().unwrap(),
                "192.168.1.1".parse().unwrap(),
            ])
        }
        async fn send(
            &self,
            _host: &str,
            _port: u16,
            _addrs: &[IpAddr],
            _url: &str,
            _timeout: Duration,
        ) -> Result<FetchHop, EgressError> {
            panic!("send must never run when an answer is private");
        }
    }
    let client = EgressClient::new(EgressPolicy::new().allow_host("split.example"))
        .with_fetch_driver_for_tests(std::sync::Arc::new(SplitDriver));
    let err = client
        .fetch_bounded("https://split.example/", 1024, Duration::from_secs(5))
        .await
        .expect_err("split answers must deny");
    assert!(matches!(err, EgressError::PrivateAddress { .. }));
}

/// DNS-rebind-style flip MID-LOOP: hop 1 resolves public and returns a
/// same-host redirect; hop 2's re-resolution flips to private and must be
/// caught by the per-hop re-check. The send spy proves exactly one request
/// left the loop.
#[tokio::test]
async fn fetch_dns_rebind_flip_caught_on_second_resolution() {
    let driver = ScriptedDriver::default()
        .with_answer("flip.example", vec!["93.184.216.34", "10.0.0.1"])
        .with_response(
            "https://flip.example/a",
            ScriptedHop {
                status: 302,
                location: Some("/b".into()),
                content_type: None,
                content_length: None,
                chunks: vec![],
            },
        );
    let sends = std::sync::Arc::clone(&driver.sends);
    let client = scripted_client(EgressPolicy::new().allow_host("flip.example"), driver);
    let err = client
        .fetch_bounded("https://flip.example/a", 1024, Duration::from_secs(5))
        .await
        .expect_err("the flipped second resolution must deny");
    assert!(
        matches!(err, EgressError::PrivateAddress { .. }),
        "err: {err:?}"
    );
    assert_eq!(
        sends.lock().unwrap().len(),
        1,
        "exactly hop 1 left the loop"
    );
}

/// Scheme and credential hardening (C4 §3.5).
#[tokio::test]
async fn fetch_scheme_and_userinfo_rules() {
    let client = EgressClient::new(EgressPolicy::new().allow_host("example.com"));
    // ftp:// denied at decide() even for an allowlisted host
    let err = client
        .fetch_bounded("ftp://example.com/f", 1024, Duration::from_secs(5))
        .await
        .expect_err("ftp must deny");
    assert!(matches!(err, EgressError::Denied { .. }), "err: {err:?}");
    // http:// without the per-host opt-in denied at decide()
    let err = client
        .fetch_bounded("http://example.com/f", 1024, Duration::from_secs(5))
        .await
        .expect_err("http without opt-in must deny");
    assert!(matches!(err, EgressError::Denied { .. }), "err: {err:?}");
    // userinfo is REJECTED, not stripped
    let err = client
        .fetch_bounded(
            "https://user:s3cr3t@example.com/f",
            1024,
            Duration::from_secs(5),
        )
        .await
        .expect_err("userinfo must be rejected");
    assert!(
        matches!(err, EgressError::CredentialsRejected { .. }),
        "err: {err:?}"
    );
    assert!(!err.to_string().contains("s3cr3t"));
}

/// Content-type allowlist: prefix match before ';'; octet-stream denied;
/// a MISSING Content-Type is a typed denial, not an allow.
#[tokio::test]
async fn fetch_content_type_rules() {
    let base = EgressPolicy::new().allow_host("ct.example");
    let driver = || ScriptedDriver::default().with_answer("ct.example", vec!["93.184.216.34"]);

    // text/html; charset=utf-8 allowed (prefix match)
    let client = scripted_client(
        base.clone(),
        driver().with_response(
            "https://ct.example/html",
            ScriptedHop {
                status: 200,
                location: None,
                content_type: Some("text/html; charset=utf-8".into()),
                content_length: Some(5),
                chunks: vec![b"<html".to_vec()],
            },
        ),
    );
    let outcome = client
        .fetch_bounded("https://ct.example/html", 1024, Duration::from_secs(5))
        .await
        .expect("text/html allowed");
    assert_eq!(outcome.content_type, "text/html");
    assert_eq!(outcome.body, b"<html");
    assert_eq!(outcome.declared_bytes, Some(5));

    // application/octet-stream denied
    let client = scripted_client(
        base.clone(),
        driver().with_response(
            "https://ct.example/bin",
            ScriptedHop {
                status: 200,
                location: None,
                content_type: Some("application/octet-stream".into()),
                content_length: Some(2),
                chunks: vec![b"\x00\x01".to_vec()],
            },
        ),
    );
    let err = client
        .fetch_bounded("https://ct.example/bin", 1024, Duration::from_secs(5))
        .await
        .expect_err("octet-stream must deny");
    assert!(matches!(err, EgressError::ContentTypeDenied { .. }));

    // MISSING Content-Type: typed denial
    let client = scripted_client(
        base.clone(),
        driver().with_response(
            "https://ct.example/none",
            ScriptedHop {
                status: 200,
                location: None,
                content_type: None,
                content_length: None,
                chunks: vec![b"data".to_vec()],
            },
        ),
    );
    let err = client
        .fetch_bounded("https://ct.example/none", 1024, Duration::from_secs(5))
        .await
        .expect_err("missing content-type must deny");
    assert!(matches!(err, EgressError::ContentTypeMissing { .. }));
}

/// Body caps: over-cap streams abort AT the cap with marked truncation;
/// chunked responses (no Content-Length → declared_bytes absent) are capped
/// identically.
#[tokio::test]
async fn fetch_body_cap_truncates_and_labels_declared_bytes() {
    let big = vec![b'x'; 100 * 1024];
    let driver = ScriptedDriver::default()
        .with_answer("cap.example", vec!["93.184.216.34", "93.184.216.34"])
        .with_response(
            "https://cap.example/declared",
            ScriptedHop {
                status: 200,
                location: None,
                content_type: Some("text/plain".into()),
                content_length: Some(100 * 1024),
                chunks: vec![big.clone()],
            },
        )
        .with_response(
            "https://cap.example/chunked",
            ScriptedHop {
                status: 200,
                location: None,
                content_type: Some("text/plain".into()),
                content_length: None, // chunked: no Content-Length
                chunks: vec![big[..40 * 1024].to_vec(), big[40 * 1024..].to_vec()],
            },
        );
    let client = scripted_client(EgressPolicy::new().allow_host("cap.example"), driver);

    let outcome = client
        .fetch_bounded(
            "https://cap.example/declared",
            64 * 1024,
            Duration::from_secs(5),
        )
        .await
        .expect("fetch");
    assert!(outcome.truncated);
    assert_eq!(outcome.body_bytes, 64 * 1024);
    assert_eq!(outcome.declared_bytes, Some(100 * 1024));

    let outcome = client
        .fetch_bounded(
            "https://cap.example/chunked",
            64 * 1024,
            Duration::from_secs(5),
        )
        .await
        .expect("fetch");
    assert!(outcome.truncated);
    assert_eq!(outcome.body_bytes, 64 * 1024);
    assert_eq!(
        outcome.declared_bytes, None,
        "chunked has no declared length"
    );
}

/// Redirect loop rules: a 10-hop chain within the allowlist succeeds; the
/// 11th hop is a typed error; relative Locations resolve; the loop (not
/// reqwest) follows — the fetch client is built with Policy::none().
#[tokio::test]
async fn fetch_redirect_chain_succeeds_and_hop_limit_is_typed() {
    // 9 redirects + final 200 = 10 hops: succeeds.
    let mut driver = ScriptedDriver::default();
    for _ in 0..10 {
        driver = driver.with_answer("chain.example", vec!["93.184.216.34"]);
    }
    for i in 0..9 {
        driver = driver.with_response(
            &format!("https://chain.example/hop-{i}"),
            ScriptedHop {
                status: 302,
                location: Some(format!("/hop-{}", i + 1)), // relative
                content_type: None,
                content_length: None,
                chunks: vec![],
            },
        );
    }
    driver = driver.with_response("https://chain.example/hop-9", scripted_ok("final"));
    let client = scripted_client(EgressPolicy::new().allow_host("chain.example"), driver);
    let outcome = client
        .fetch_bounded("https://chain.example/hop-0", 1024, Duration::from_secs(5))
        .await
        .expect("10-hop chain succeeds");
    assert_eq!(outcome.final_url, "https://chain.example/hop-9");
    assert_eq!(outcome.body, b"final");

    // 10 redirects: the 11th hop never happens — typed RedirectLimit.
    let mut driver = ScriptedDriver::default();
    for _ in 0..11 {
        driver = driver.with_answer("loop.example", vec!["93.184.216.34"]);
    }
    for i in 0..11 {
        driver = driver.with_response(
            &format!("https://loop.example/hop-{i}"),
            ScriptedHop {
                status: 302,
                location: Some(format!("https://loop.example/hop-{}", i + 1)),
                content_type: None,
                content_length: None,
                chunks: vec![],
            },
        );
    }
    let sends = std::sync::Arc::clone(&driver.sends);
    let client = scripted_client(EgressPolicy::new().allow_host("loop.example"), driver);
    let err = client
        .fetch_bounded("https://loop.example/hop-0", 1024, Duration::from_secs(5))
        .await
        .expect_err("hop 11 must be a typed error");
    assert!(
        matches!(err, EgressError::RedirectLimit { .. }),
        "err: {err:?}"
    );
    assert_eq!(sends.lock().unwrap().len(), 10, "exactly 10 hops ran");
}

/// flux_only() regression: the fetch path honors the SAME policy gate —
/// non-Flux hosts deny before any DNS or socket activity.
#[tokio::test]
async fn fetch_bounded_under_flux_only_denies_the_world() {
    let client = EgressClient::flux();
    let err = client
        .fetch_bounded("https://example.com/", 1024, Duration::from_secs(5))
        .await
        .expect_err("non-Flux host must deny");
    assert!(matches!(err, EgressError::Denied { .. }));
}

// --- Real-TCP fetch paths (allowlisted IP literal = the pin) -----------------

/// End-to-end over real TCP: allowlisted IP literal + per-host http opt-in
/// (the local-endpoint story of C4 §3.5). Proves the SystemFetchDriver path:
/// per-hop pinned client, Policy::none (reqwest follows nothing itself),
/// the manual loop with relative-Location resolution, content-type and
/// declared length surfaced.
#[tokio::test]
async fn fetch_real_http_optin_roundtrip_and_relative_redirect() {
    let canary = format!("/nano-canary-fetch-real-{}", std::process::id());
    let final_path = format!("{canary}-final");
    let origin = spawn_listener(&canary, move |mut stream, n| {
        if n == 1 {
            respond(
                &mut stream,
                "302 Found",
                &format!("Location: {final_path}\r\n"),
                "",
            );
        } else {
            respond(
                &mut stream,
                "200 OK",
                "Content-Type: text/plain\r\n",
                "nano-fetch-final",
            )
        }
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
    let outcome = client
        .fetch_bounded(
            &format!("http://127.0.0.1:{}{canary}", origin.addr.port()),
            64 * 1024,
            Duration::from_secs(5),
        )
        .await
        .expect("fetch over real TCP");
    assert_eq!(hit_count(&origin), 2, "the manual loop followed one hop");
    assert_eq!(outcome.status, 200);
    assert_eq!(outcome.body, b"nano-fetch-final");
    assert_eq!(outcome.content_type, "text/plain");
    assert_eq!(
        outcome.declared_bytes,
        Some("nano-fetch-final".len() as u64)
    );
    assert!(outcome.final_url.ends_with("-final"));
    assert!(!outcome.truncated);
}

/// Real redirect to a NON-allowlisted host: typed denial at the hop gate,
/// zero bytes to the target — the fetch client follows nothing by itself.
#[tokio::test]
async fn fetch_real_redirect_off_allowlist_denied_zero_bytes() {
    let exfil_canary = format!("/nano-canary-fetch-exfil-{}", std::process::id());
    let exfil = spawn_listener(&exfil_canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nano-exfil-reached");
    });
    let exfil_port = exfil.addr.port();
    let origin_canary = format!("/nano-canary-fetch-origin-{}", std::process::id());
    let origin = spawn_listener(&origin_canary, move |mut stream, _| {
        respond(
            &mut stream,
            "302 Found",
            &format!("Location: http://localhost:{exfil_port}{exfil_canary}\r\n"),
            "",
        );
    });
    // 127.0.0.1 allowlisted (with http opt-in); "localhost" deliberately not.
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
    let err = client
        .fetch_bounded(
            &format!("http://127.0.0.1:{}{origin_canary}", origin.addr.port()),
            1024,
            Duration::from_secs(5),
        )
        .await
        .expect_err("redirect off the allowlist must deny");
    assert!(matches!(err, EgressError::Denied { .. }), "err: {err:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(hit_count(&exfil), 0, "zero bytes to the redirect target");
}

/// Real streaming over cap: the body read aborts at max_bytes with marked
/// truncation; declared_bytes still reports the server's Content-Length.
#[tokio::test]
async fn fetch_real_over_cap_stream_aborts_with_marked_truncation() {
    let canary = format!("/nano-canary-fetch-cap-{}", std::process::id());
    let big = "y".repeat(128 * 1024);
    let origin = spawn_listener(&canary, move |mut stream, _| {
        respond(&mut stream, "200 OK", "Content-Type: text/plain\r\n", &big)
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
    let outcome = client
        .fetch_bounded(
            &format!("http://127.0.0.1:{}{canary}", origin.addr.port()),
            64 * 1024,
            Duration::from_secs(5),
        )
        .await
        .expect("fetch");
    assert!(outcome.truncated, "over-cap body must be marked truncated");
    assert_eq!(outcome.body_bytes, 64 * 1024);
    assert_eq!(outcome.declared_bytes, Some(128 * 1024));
}

/// Redirect to a DIFFERENT allowlisted name whose resolution is loopback:
/// denied at the pin stage of that hop; exactly one request left the loop.
#[tokio::test]
async fn fetch_redirect_to_allowlisted_name_resolving_loopback_denied() {
    let driver = ScriptedDriver::default()
        .with_answer("entry.example", vec!["93.184.216.34"])
        .with_answer("rebind.example", vec!["127.0.0.1"])
        .with_response(
            "https://entry.example/start",
            ScriptedHop {
                status: 302,
                location: Some("https://rebind.example/x".into()),
                content_type: None,
                content_length: None,
                chunks: vec![],
            },
        );
    let sends = std::sync::Arc::clone(&driver.sends);
    let client = scripted_client(
        EgressPolicy::new()
            .allow_host("entry.example")
            .allow_host("rebind.example"),
        driver,
    );
    let err = client
        .fetch_bounded("https://entry.example/start", 1024, Duration::from_secs(5))
        .await
        .expect_err("loopback resolution must deny at the pin stage");
    assert!(
        matches!(err, EgressError::PrivateAddress { .. }),
        "err: {err:?}"
    );
    assert_eq!(sends.lock().unwrap().len(), 1, "only hop 1 left the loop");
}

// --- P3 §6.3: grant-bearing policies through the redirect-following path ----

/// A policy carrying an EndpointGrant routes `request()` through the
/// per-request method-aware redirect client. This integration leg proves the
/// mechanism end-to-end over plain HTTP (grants themselves are https-only):
/// a 303 on a POST downgrades to GET, the hop is re-gated, and the
/// allowlisted-host follow completes — while the unit-level
/// `redirect_gate_re_gates_downgraded_hops_as_get` pins the exact
/// grant/tuple semantics the network can't express without TLS.
#[tokio::test]
async fn grant_bearing_policy_redirect_roundtrip_via_method_aware_client() {
    let canary = format!("/nano-canary-grant-redirect-{}", std::process::id());
    let final_path = format!("{canary}-final");
    let origin = spawn_listener(&canary, move |mut stream, n| {
        if n == 1 {
            respond(
                &mut stream,
                "303 See Other",
                &format!("Location: {final_path}\r\n"),
                "",
            );
        } else {
            respond(
                &mut stream,
                "200 OK",
                "Content-Type: text/plain\r\n",
                "nano-grant-final",
            );
        }
    });
    let policy = EgressPolicy::new()
        .allow_host_with_http("127.0.0.1")
        // A grant forces the per-request method-aware redirect gate; the
        // host rule then authorizes both hops (method-agnostic).
        .allow_endpoint(
            nano_egress::grant::HttpMethod::Post,
            "https://as.example/token",
        )
        .expect("grant");
    let client = EgressClient::new(policy);
    let url = format!("http://127.0.0.1:{}{canary}", origin.addr.port());
    let response = client
        .request(reqwest::Method::POST, &url)
        .expect("allowlisted")
        .send()
        .await
        .expect("send");
    assert_eq!(response.status().as_u16(), 200);
    let body = response.text().await.expect("body");
    assert_eq!(hit_count(&origin), 2, "the downgraded hop was followed");
    assert!(body.contains("nano-grant-final"), "final body: {body}");
}

/// The without_redirects constructor (§6.3: OAuth bootstrap/scoped
/// clients): a 302 is RETURNED, never followed — the redirect target sees
/// zero connections.
#[tokio::test]
async fn without_redirects_returns_3xx_and_never_follows() {
    let exfil_canary = format!("/nano-canary-noredir-exfil-{}", std::process::id());
    let exfil = spawn_listener(&exfil_canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nano-exfil-reached");
    });
    let exfil_port = exfil.addr.port();
    let origin_canary = format!("/nano-canary-noredir-origin-{}", std::process::id());
    let origin = spawn_listener(&origin_canary, move |mut stream, _| {
        respond(
            &mut stream,
            "302 Found",
            &format!("Location: http://127.0.0.1:{exfil_port}{exfil_canary}\r\n"),
            "",
        );
    });
    let client =
        EgressClient::without_redirects(EgressPolicy::new().allow_host_with_http("127.0.0.1"));
    let url = format!("http://127.0.0.1:{}{origin_canary}", origin.addr.port());
    let response = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .send()
        .await
        .expect("send");
    assert_eq!(
        response.status().as_u16(),
        302,
        "3xx returned, not followed"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(hit_count(&exfil), 0, "redirect target saw zero bytes");
}

/// The HIGH-1 regression pin (§6.3): a `without_redirects` client carrying
/// an EndpointGrant (OAuth's bootstrap/scoped clients ALWAYS do) must keep
/// the no-follow posture through `request_client` — a 307 to its own URL is
/// RETURNED unfollowed, and the server sees exactly one request. Before the
/// fix, grant-bearing policies were silently re-routed through the
/// redirect-FOLLOW gate, so this test's second hit landed and the final
/// status was 200.
#[tokio::test]
async fn without_redirects_grant_bearing_returns_3xx_unfollowed() {
    let canary = format!("/nano-canary-noredir-grant-{}", std::process::id());
    let redirect_target = canary.clone();
    let origin = spawn_listener(&canary, move |mut stream, n| {
        if n == 1 {
            // 307 to ITSELF (method-preserving): a follow gate would loop
            // back here; the no-follow posture must return it as-is.
            respond(
                &mut stream,
                "307 Temporary Redirect",
                &format!("Location: {redirect_target}\r\n"),
                "",
            );
        } else {
            respond(&mut stream, "200 OK", "", "nano-followed-hop");
        }
    });
    let policy = EgressPolicy::new()
        .allow_host_with_http("127.0.0.1")
        // A grant forces grant-bearing routing; the no-follow posture must
        // still win over the method-aware follow gate.
        .allow_endpoint(
            nano_egress::grant::HttpMethod::Post,
            "https://as.example/token",
        )
        .expect("grant");
    let client = EgressClient::without_redirects(policy);
    let url = format!("http://127.0.0.1:{}{canary}", origin.addr.port());
    let response = client
        .request(reqwest::Method::POST, &url)
        .expect("allowlisted")
        .send()
        .await
        .expect("send");
    assert_eq!(
        response.status().as_u16(),
        307,
        "the 307 must be returned, not followed"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(hit_count(&origin), 1, "zero second request may be sent");
}
