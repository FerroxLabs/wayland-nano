//! Adversarial egress tests: bypass attempts against the policy gate.
//!
//! Every hostile endpoint is a local TCP listener on 127.0.0.1 — no real
//! network is ever contacted. "Denied" assertions additionally prove that a
//! denial produces ZERO socket activity (the listener never sees a
//! connection), per the crate invariant "deny = no bytes leave".

use nano_egress::client::{EgressClient, EgressError};
use nano_egress::policy::EgressPolicy;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A hostile endpoint: counts connections and answers each with `handler`.
struct HostileListener {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

fn spawn_listener(handler: impl Fn(TcpStream, usize) + Send + 'static) -> HostileListener {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_thread = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
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
fn read_head(stream: &mut TcpStream) {
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
    let hostile = spawn_listener(|mut stream, _| {
        read_head(&mut stream);
        respond(&mut stream, "200 OK", "", "nanok3-should-never-see-this");
    });
    let client = EgressClient::flux();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", hostile.addr.port());
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
    let exfil = spawn_listener(|mut stream, _| {
        read_head(&mut stream);
        respond(&mut stream, "200 OK", "", "nanok3-exfil-reached");
    });
    let exfil_port = exfil.addr.port();
    // Allowed origin: redirects every request to the off-allowlist host.
    let origin = spawn_listener(move |mut stream, _| {
        read_head(&mut stream);
        respond(
            &mut stream,
            "302 Found",
            &format!("Location: http://127.0.0.1:{exfil_port}/exfil\r\n"),
            "",
        );
    });
    // "localhost" is allowlisted; "127.0.0.1" deliberately is not — the two
    // names resolve to the same loopback, so only policy re-checking on the
    // redirect target can stop the exfiltration.
    let client = EgressClient::new(EgressPolicy::new().allow_host("localhost"));
    let url = format!("http://localhost:{}/start", origin.addr.port());
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
    let origin = spawn_listener(move |mut stream, n| {
        read_head(&mut stream);
        if n == 1 {
            respond(
                &mut stream,
                "302 Found",
                "Location: /final\r\n",
                "",
            );
        } else {
            respond(&mut stream, "200 OK", "", "nanok3-redirect-final");
        }
    });
    let client = EgressClient::new(EgressPolicy::new().allow_host("localhost"));
    let url = format!("http://localhost:{}/start", origin.addr.port());
    let response = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .send()
        .await
        .expect("send");
    let body = response.text().await.expect("body");
    assert_eq!(hit_count(&origin), 2, "same-host redirect should be followed");
    assert!(
        body.contains("nanok3-redirect-final"),
        "final body: {body}"
    );
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
    // A refused connection must not echo the URL (query/userinfo) that a
    // caller attached credentials to.
    let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = probe.local_addr().expect("addr").port();
    drop(probe); // port now (almost certainly) refuses connections
    let client = EgressClient::new(EgressPolicy::new().allow_host("127.0.0.1"));
    let url = format!("http://127.0.0.1:{port}/v1?api_key=nanok3-transport-secret");
    let err = client
        .request(reqwest::Method::GET, &url)
        .expect("allowlisted")
        .bearer_auth("nanok3-bearer-secret")
        .send()
        .await
        .expect_err("refused connection must error");
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
