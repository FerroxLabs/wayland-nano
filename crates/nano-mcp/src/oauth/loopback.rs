//! The loopback callback listener (P3 §6.2 step 4), fully bound:
//! `127.0.0.1`, OS-assigned port, in the Nano process. The callback path is
//! random per attempt (`/oauth/callback/<128-bit>`); the listener accepts
//! EXACTLY ONE GET on that exact path with `Host: 127.0.0.1:<port>`;
//! request line + headers are capped at 4 KiB; `code`/`state` params are
//! capped at 2,048 chars each; a duplicate callback after consumption is
//! rejected (the listener is consumed by the valid callback and torn down —
//! a later connection is refused); teardown on completion OR expiry.
//!
//! The `state` is bound to {login attempt id, server instance_id, callback
//! origin, callback path, expiry} in [`LoopbackBinding`]; a state mismatch
//! on the exact bound path is a typed rejection AND closes the listener
//! (fail-closed: a wrong state on the right path is an attack signal, not
//! noise). Mismatched method/path/Host are bounded 4xx rejections and the
//! listener keeps waiting for the one valid callback until expiry.

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use super::FailReason;
use super::OAuthError;
use super::bounded_error_code;
use super::pkce::random_token_128;

/// Listener lifetime (§6.2: torn down on completion OR 180s expiry).
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
/// Request line + headers cap (§6.2 step 4).
pub const MAX_HEAD_BYTES: usize = 4 * 1024;
/// Per-param cap for `code`/`state` (§6.2 step 4).
pub const MAX_PARAM_CHARS: usize = 2048;
/// Per-connection read deadline (hostile peers get no listener time).
const CONN_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The binding record for one login attempt: state bound to attempt +
/// server + callback origin + callback path + expiry. SESSION-VOLATILE
/// (§10): never journaled, invalidated by kill.
pub struct LoopbackBinding {
    /// Random 128-bit login attempt id (hex).
    pub attempt_id: String,
    /// The stable server instance id this attempt belongs to.
    pub server_id: String,
    /// `http://127.0.0.1:<port>` — the callback origin.
    pub callback_origin: String,
    /// The exact random callback path (`/oauth/callback/<128-bit>`).
    pub callback_path: String,
    /// The random 128-bit state (hex).
    pub state: String,
    expires_at: Instant,
    receiver: mpsc::Receiver<CallbackOutcome>,
}

impl LoopbackBinding {
    /// The redirect_uri sent to the AS (origin + exact random path).
    pub fn redirect_uri(&self) -> String {
        format!("{}{}", self.callback_origin, self.callback_path)
    }

    /// Await the single valid callback; returns the authorization `code`.
    /// Blocks the calling thread — async callers must offload (the login
    /// flow uses `tokio::task::spawn_blocking`).
    pub fn await_callback(self) -> Result<String, OAuthError> {
        let remaining = self.expires_at.saturating_duration_since(Instant::now());
        let outcome = self
            .receiver
            .recv_timeout(remaining.max(Duration::from_millis(1)))
            .map_err(|_| OAuthError::Failed {
                reason: FailReason::CallbackTimeout,
            })?;
        match outcome {
            CallbackOutcome::Code(code) => Ok(code),
            CallbackOutcome::Failed(reason) => Err(OAuthError::Failed { reason }),
        }
    }
}

enum CallbackOutcome {
    Code(String),
    Failed(FailReason),
}

/// Bind the listener and spawn its accept loop. No async runtime (D1: std
/// threads/channels in the MCP IO paths).
pub fn bind(server_id: &str) -> Result<LoopbackBinding, OAuthError> {
    bind_with_timeout(server_id, CALLBACK_TIMEOUT)
}

/// Test hook: same binding with a shortened expiry (§12 shortens the clock).
pub(crate) fn bind_with_timeout(
    server_id: &str,
    timeout: Duration,
) -> Result<LoopbackBinding, OAuthError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| OAuthError::Transport {
        detail: format!("loopback bind: {e}"),
    })?;
    let addr = listener.local_addr().map_err(|e| OAuthError::Transport {
        detail: format!("loopback addr: {e}"),
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| OAuthError::Transport {
            detail: format!("loopback nonblocking: {e}"),
        })?;
    let expires_at = Instant::now() + timeout;
    let binding_values = (
        random_token_128(),
        format!("http://127.0.0.1:{}", addr.port()),
        format!("/oauth/callback/{}", random_token_128()),
        random_token_128(),
    );
    let (tx, rx) = mpsc::channel();
    {
        let (_, origin, path, state) = &binding_values;
        let host_header = origin.trim_start_matches("http://").to_string();
        let path = path.clone();
        let state = state.clone();
        std::thread::spawn(move || accept_loop(listener, tx, host_header, path, state, expires_at));
    }
    Ok(LoopbackBinding {
        attempt_id: binding_values.0,
        server_id: server_id.to_string(),
        callback_origin: binding_values.1,
        callback_path: binding_values.2,
        state: binding_values.3,
        expires_at,
        receiver: rx,
    })
}

/// The accept loop: exactly ONE success (200 + teardown); bounded 4xx for
/// mismatches; typed failure + teardown on state mismatch or expiry.
/// Teardown is ordered BEFORE the outcome is signaled on every terminal
/// path (the listener is dropped first, then the channel sent): once
/// `await_callback` returns, the port must already be closed — a later
/// connect is refused and a queued-but-unaccepted duplicate is reset.
fn accept_loop(
    listener: TcpListener,
    tx: mpsc::Sender<CallbackOutcome>,
    host_header: String,
    path: String,
    state: String,
    expires_at: Instant,
) {
    loop {
        if Instant::now() >= expires_at {
            drop(listener);
            let _ = tx.send(CallbackOutcome::Failed(FailReason::CallbackTimeout));
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => match handle_connection(stream, &host_header, &path, &state) {
                ConnResult::Success(code) => {
                    drop(listener); // consumed: teardown before signaling
                    let _ = tx.send(CallbackOutcome::Code(code));
                    return;
                }
                ConnResult::StateMismatch => {
                    drop(listener); // fail-closed teardown before signaling
                    let _ = tx.send(CallbackOutcome::Failed(FailReason::StateMismatch));
                    return;
                }
                ConnResult::ProviderError(code) => {
                    // §6.2 step 5: the provider answered with an `error`
                    // param on a validly-bound state — typed failure, fast
                    // teardown (never the 180s CallbackTimeout hang).
                    drop(listener);
                    let _ = tx.send(CallbackOutcome::Failed(FailReason::ProviderError(code)));
                    return;
                }
                ConnResult::Rejected => continue, // bounded 4xx already sent
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                drop(listener);
                let _ = tx.send(CallbackOutcome::Failed(FailReason::CallbackRejected));
                return;
            }
        }
    }
}

enum ConnResult {
    Success(String),
    StateMismatch,
    /// §6.2 step 5: provider `error` param on a validly-bound state,
    /// carrying the sanitized bounded code.
    ProviderError(String),
    Rejected,
}

/// Handle one connection: bounded read of the request head, then the full
/// method/path/Host/param gauntlet. Every rejection sends a bounded 4xx.
fn handle_connection(
    mut stream: TcpStream,
    host_header: &str,
    path: &str,
    state: &str,
) -> ConnResult {
    // The LISTENER is nonblocking (the accept loop polls). On BSD-derived
    // platforms (macOS) an accepted socket inherits O_NONBLOCK — the
    // canonical BSD sockets semantics, unlike Linux/Windows — so restore
    // blocking mode explicitly; SO_RCVTIMEO below is meaningless on a
    // nonblocking socket, and a read issued before the peer's bytes land
    // would otherwise fail instantly with WouldBlock. (`read_head` also
    // tolerates WouldBlock itself, in case this call ever fails.)
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(CONN_READ_TIMEOUT));
    let Some(head) = read_head(&mut stream) else {
        let _ = respond(&mut stream, "400 Bad Request", "malformed request");
        close_gracefully(&mut stream);
        return ConnResult::Rejected;
    };
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" {
        let _ = respond(&mut stream, "405 Method Not Allowed", "GET only");
        close_gracefully(&mut stream);
        return ConnResult::Rejected;
    }
    // Host header: exact `127.0.0.1:<port>` (case-insensitive header NAME,
    // exact value).
    let host_ok = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .any(|(name, value)| name.eq_ignore_ascii_case("host") && value.trim() == host_header);
    if !host_ok {
        let _ = respond(&mut stream, "400 Bad Request", "bad Host");
        close_gracefully(&mut stream);
        return ConnResult::Rejected;
    }
    let (req_path, query) = target.split_once('?').unwrap_or((target, ""));
    if req_path != path {
        let _ = respond(&mut stream, "404 Not Found", "unknown path");
        close_gracefully(&mut stream);
        return ConnResult::Rejected;
    }
    let mut code: Option<String> = None;
    let mut seen_state: Option<String> = None;
    let mut provider_error: Option<String> = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = form_decode(v);
        if v.chars().count() > MAX_PARAM_CHARS {
            let _ = respond(&mut stream, "400 Bad Request", "oversized parameter");
            close_gracefully(&mut stream);
            return ConnResult::Rejected;
        }
        match k {
            "code" => code = Some(v),
            "state" => seen_state = Some(v),
            "error" => provider_error = Some(v),
            _ => {}
        }
    }
    // §6.2 step 5: the provider answered with an `error` param. Only a
    // validly-bound state makes it the typed ProviderError failure (bounded
    // code only, `error_description` never crosses the boundary); anything
    // else falls through to the code/state gauntlet below.
    if let (Some(raw_code), Some(seen)) = (&provider_error, &seen_state) {
        if *seen == state {
            let _ = respond(
                &mut stream,
                "200 OK",
                "<html><body>Wayland Nano login failed; you can close this tab.</body></html>",
            );
            close_gracefully(&mut stream);
            return ConnResult::ProviderError(bounded_error_code(raw_code));
        }
    }
    match (code, seen_state) {
        (Some(code), Some(seen)) if seen == state => {
            if respond(
                &mut stream,
                "200 OK",
                "<html><body>Wayland Nano login complete; you can close this tab.</body></html>",
            )
            .is_ok()
            {
                ConnResult::Success(code)
            } else {
                ConnResult::Rejected
            }
        }
        (Some(_), Some(_)) => {
            // Right path, wrong state: attack signal — reject AND tear down.
            let _ = respond(&mut stream, "400 Bad Request", "state mismatch");
            close_gracefully(&mut stream);
            ConnResult::StateMismatch
        }
        _ => {
            let _ = respond(&mut stream, "400 Bad Request", "missing code/state");
            close_gracefully(&mut stream);
            ConnResult::Rejected
        }
    }
}

/// Bounded head read: at most MAX_HEAD_BYTES, stopping at the header
/// terminator. Oversized or unreadable ⇒ None (the caller 4xx's).
/// WouldBlock is "not yet", never "never": the deadline for the next bytes
/// is refreshed on every successful read (the same per-read discipline
/// SO_RCVTIMEO gave on blocking sockets), so a peer whose head arrives in
/// split segments — or a socket still in inherited nonblocking mode — is
/// waited on, not rejected as "malformed".
fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let mut deadline = Instant::now() + CONN_READ_TIMEOUT;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                deadline = Instant::now() + CONN_READ_TIMEOUT;
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEAD_BYTES {
                    let _ = respond(
                        stream,
                        "431 Request Header Fields Too Large",
                        "oversized head",
                    );
                    return None;
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
    String::from_utf8(buf).ok()
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Graceful close for a rejected connection: FIN our side (ordered AFTER
/// the response just written), then drain any request bytes still in
/// flight before the socket drops. Closing a socket with unread receive
/// data is a TCP RST, and that reset can overtake or abort the queued
/// response — the peer then sees a bare connection-reset with ZERO
/// response bytes (observed as the `oversized_head_is_rejected` flake
/// under load: the listener cut the head at the cap while the pad tail
/// was still arriving, and the close reset the connection before the 431
/// reached the client). Draining removes the unread-data condition so the
/// close is FIN-ordered and the typed rejection is always delivered.
/// Bounded twice over: the per-connection read deadline
/// (CONN_READ_TIMEOUT) caps each read, and the byte cap stops the drain
/// even against a flooding peer. NOT used on the success path: a valid
/// callback's head is consumed to the terminator by construction, and the
/// login result must not wait on the browser closing its socket.
fn close_gracefully(stream: &mut TcpStream) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 1024];
    let mut drained = 0usize;
    while drained <= MAX_HEAD_BYTES {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

/// application/x-www-form-urlencoded decode for the two callback params:
/// `%XX` decoding plus `+` → space.
fn form_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(((hi << 4) | lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one raw request at a binding; returns the status line (or the
    /// connect error for a torn-down listener). An empty response is
    /// reported WITH ITS CAUSE (clean EOF vs reset) instead of "" so a
    /// load-transient failure is self-explaining; assertions still fail on
    /// it — nothing is retried or tolerated here.
    fn raw_request(binding_addr: &str, request: &str) -> String {
        match TcpStream::connect(binding_addr) {
            Ok(mut s) => {
                s.write_all(request.as_bytes()).unwrap();
                s.flush().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    match s.read(&mut chunk) {
                        Ok(0) => {
                            if buf.is_empty() {
                                return "<clean EOF with zero response bytes>".to_string();
                            }
                            break;
                        }
                        Err(e) => {
                            if buf.is_empty() {
                                return format!("<read error with zero response bytes: {e}>");
                            }
                            break;
                        }
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                String::from_utf8_lossy(&buf)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string()
            }
            Err(e) => format!("CONNECT-ERR {e}"),
        }
    }

    fn addr_of(binding: &LoopbackBinding) -> String {
        binding
            .callback_origin
            .trim_start_matches("http://")
            .to_string()
    }

    #[test]
    fn valid_callback_roundtrip() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!("GET {path}?code=authcode123&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(status.contains("200"), "status: {status}");
        let code = handle.join().expect("join").expect("callback");
        assert_eq!(code, "authcode123");
    }

    /// Regression pin for the CI flake on macOS (run fe6d621): the request
    /// head can arrive in SPLIT TCP segments, and on BSD-derived platforms
    /// the accepted socket may report WouldBlock before the first byte
    /// lands (inherited nonblocking mode). A partial or delayed head is
    /// NOT "malformed": the listener must wait for the rest and complete
    /// the login. (On Windows/Linux the blocking read already waited, so
    /// pre-fix this passed there — it discriminates where the defect
    /// lives.)
    #[test]
    fn split_head_across_segments_still_completes() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        let handle = std::thread::spawn(move || binding.await_callback());
        let mut s = TcpStream::connect(&addr).expect("connect");
        s.write_all(format!("GET {path}?code=authcode123&state={state} HTTP/1.1\r\n").as_bytes())
            .unwrap();
        s.flush().unwrap();
        // Force the server to read a partial head before the rest arrives.
        std::thread::sleep(Duration::from_millis(200));
        s.write_all(format!("Host: {host}\r\n\r\n").as_bytes())
            .unwrap();
        s.flush().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let status = String::from_utf8_lossy(&buf)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        assert!(status.contains("200"), "status: {status}");
        assert_eq!(handle.join().unwrap().unwrap(), "authcode123");
    }

    #[test]
    fn wrong_method_path_host_are_rejected_and_listener_survives() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        // POST instead of GET ⇒ 405.
        assert!(
            raw_request(
                &addr,
                &format!("POST {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")
            )
            .contains("405")
        );
        // Wrong path ⇒ 404.
        assert!(
            raw_request(
                &addr,
                &format!("GET /other HTTP/1.1\r\nHost: {host}\r\n\r\n")
            )
            .contains("404")
        );
        // Wrong Host ⇒ 400.
        assert!(
            raw_request(
                &addr,
                &format!("GET {path} HTTP/1.1\r\nHost: evil.example\r\n\r\n")
            )
            .contains("400")
        );
        // The listener still serves the one valid callback after the noise.
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!("GET {path}?code=c&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(status.contains("200"), "status: {status}");
        assert_eq!(handle.join().unwrap().unwrap(), "c");
    }

    #[test]
    fn state_mismatch_rejects_and_closes_the_listener() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!("GET {path}?code=c&state=wrongstate HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(status.contains("400"), "status: {status}");
        let err = handle.join().unwrap().expect_err("state mismatch is typed");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::StateMismatch
            }
        ));
        // Teardown proof: the port no longer accepts connections.
        let later = raw_request(&addr, "GET / HTTP/1.1\r\n\r\n");
        assert!(
            later.starts_with("CONNECT-ERR"),
            "listener must be closed after state mismatch: {later}"
        );
    }

    #[test]
    fn duplicate_callback_after_consumption_is_rejected() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!("GET {path}?code=c&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(status.contains("200"));
        assert_eq!(handle.join().unwrap().unwrap(), "c");
        // The listener was consumed and torn down: a duplicate is refused.
        let dup = raw_request(
            &addr,
            &format!("GET {path}?code=c2&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(
            dup.starts_with("CONNECT-ERR"),
            "duplicate callback must be rejected (listener consumed): {dup}"
        );
    }

    #[test]
    fn expiry_tears_down_with_typed_timeout() {
        let binding = bind_with_timeout("srv_test", Duration::from_millis(300)).expect("bind");
        let err = binding.await_callback().expect_err("must time out");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::CallbackTimeout
            }
        ));
    }

    #[test]
    fn oversized_head_is_rejected() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        let pad = "x".repeat(MAX_HEAD_BYTES + 10);
        let status = raw_request(
            &addr,
            &format!(
                "GET {path}?code=c&state={state} HTTP/1.1\r\nHost: {host}\r\nX-Pad: {pad}\r\n\r\n"
            ),
        );
        assert!(
            status.contains("431") || status.contains("400"),
            "oversized head must be rejected: {status}"
        );
        // Listener survives; the valid callback still lands.
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!("GET {path}?code=c&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(status.contains("200"), "status: {status}");
        assert_eq!(handle.join().unwrap().unwrap(), "c");
    }

    #[test]
    fn form_decode_handles_plus_and_percent() {
        assert_eq!(form_decode("a+b%20c%2Fd"), "a b c/d");
        assert_eq!(form_decode("plain"), "plain");
    }

    /// §6.2 step 5: a provider `error` param on a validly-bound state is the
    /// typed ProviderError failure — fast (no 180s CallbackTimeout hang),
    /// bounded code only, and the listener is torn down.
    #[test]
    fn provider_error_with_valid_state_fails_typed_and_tears_down() {
        let binding = bind_with_timeout("srv_test", Duration::from_secs(10)).expect("bind");
        let addr = addr_of(&binding);
        let host = addr.clone();
        let path = binding.callback_path.clone();
        let state = binding.state.clone();
        let handle = std::thread::spawn(move || binding.await_callback());
        let status = raw_request(
            &addr,
            &format!(
                "GET {path}?error=Access_Denied&error_description=ignored+prose&state={state} HTTP/1.1\r\nHost: {host}\r\n\r\n"
            ),
        );
        assert!(
            status.contains("200"),
            "browser gets a clean answer: {status}"
        );
        let err = handle
            .join()
            .expect("join")
            .expect_err("provider error is typed, not a timeout");
        match err {
            OAuthError::Failed {
                reason: FailReason::ProviderError(code),
            } => assert_eq!(code, "access_denied", "bounded lowercase code"),
            other => panic!("expected ProviderError, got: {other}"),
        }
        // Teardown proof: the port no longer accepts connections.
        let later = raw_request(&addr, "GET / HTTP/1.1\r\n\r\n");
        assert!(
            later.starts_with("CONNECT-ERR"),
            "listener must be closed after a provider error: {later}"
        );
    }
}
