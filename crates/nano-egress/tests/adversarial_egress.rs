//! Adversarial egress tests: bypass attempts against the policy gate.
//!
//! Every hostile endpoint is a local TCP listener on 127.0.0.1 — no real
//! network is ever contacted. "Denied" assertions additionally prove that a
//! denial produces ZERO socket activity (the listener never sees a connection
//! carrying the test's canary), per the crate invariant "deny = no bytes
//! leave".
//!
//! Race hardening: each test drives its listener with a unique canary path
//! (`/nanok3-canary-<test>-<pid>`). A listener counts a connection as a hit
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
                        respond(&mut stream, "404 Not Found", "", "nanok3-stray-probe");
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
    let canary = format!("/nanok3-canary-denied-host-{}", std::process::id());
    let hostile = spawn_listener(&canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nanok3-should-never-see-this");
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
    assert!(policy.allows("https://api.fluxrouter.ai/v1/models"));
    assert!(!policy.allows("https://api.fluxrouter.ai.evil.com/v1/models"));
    assert!(!policy.allows("https://sub.api.fluxrouter.ai/v1/models"));
}

// --- Redirect-following bypass ----------------------------------------------

#[tokio::test]
async fn redirect_to_off_allowlist_host_must_not_be_followed() {
    // Hostile target: only reachable if the client follows a redirect WITHOUT
    // re-checking the egress policy against the redirect target.
    let exfil_canary = format!("/nanok3-canary-redirect-exfil-{}", std::process::id());
    let origin_canary = format!("/nanok3-canary-redirect-origin-{}", std::process::id());
    let exfil = spawn_listener(&exfil_canary, |mut stream, _| {
        respond(&mut stream, "200 OK", "", "nanok3-exfil-reached");
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
    let client = EgressClient::new(EgressPolicy::new().allow_host("localhost"));
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
    let canary = format!("/nanok3-canary-redirect-control-{}", std::process::id());
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
            respond(&mut stream, "200 OK", "", "nanok3-redirect-final");
        }
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host("localhost"));
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
    assert!(body.contains("nanok3-redirect-final"), "final body: {body}");
}

// --- Credential/header redaction on error paths ------------------------------

#[test]
fn denied_error_display_redacts_query_and_userinfo_credentials() {
    let client = EgressClient::flux();
    let err = client
        .request(
            reqwest::Method::GET,
            "https://nanok3-user:nanok3-s3cr3t-password@evil.example.com/v1?api_key=nanok3-query-secret",
        )
        .expect_err("must deny");
    let rendered = err.to_string();
    assert!(
        !rendered.contains("nanok3-s3cr3t-password"),
        "userinfo credential leaked into Denied display: {rendered}"
    );
    assert!(
        !rendered.contains("nanok3-query-secret"),
        "query leaked into Denied display: {rendered}"
    );
}

#[test]
fn http_status_error_display_redacts_query_and_userinfo_credentials() {
    let client = EgressClient::flux();
    let err = client.classify_status(
        "https://nanok3-user:nanok3-s3cr3t-password@api.fluxrouter.ai/v1/chat/completions?api_key=nanok3-query-secret",
        401,
    );
    let rendered = err.to_string();
    assert!(rendered.contains("401"));
    assert!(
        !rendered.contains("nanok3-s3cr3t-password"),
        "userinfo credential leaked into HttpStatus display: {rendered}"
    );
    assert!(
        !rendered.contains("nanok3-query-secret"),
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
    let client = EgressClient::new(EgressPolicy::new().allow_host("127.0.0.1"));
    let url = format!("http://127.0.0.1:{port}/v1?api_key=nanok3-transport-secret");
    let err = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .bearer_auth("nanok3-bearer-secret")
        .send()
        .await
        .expect_err("unanswered connection must error");
    let rendered = client.classify_transport(&err).to_string();
    assert!(
        !rendered.contains("nanok3-transport-secret"),
        "URL query credential leaked into Transport display: {rendered}"
    );
    assert!(
        !rendered.contains("nanok3-bearer-secret"),
        "Authorization header leaked into Transport display: {rendered}"
    );
}
