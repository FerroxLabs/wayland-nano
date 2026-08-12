//! P1 §8 unit battery (search half): args/render clamps, Flux grounding
//! isolation + metering precondition, the cancellation battery (send /
//! body-read / retry-sleep / chain-terminal), chain behavior, and the
//! deny-by-default posture at the tool layer.

use super::*;
use nano_model::metering::{StubCostMeter, UsageSink};
use nano_model::retry::{ReconnectPolicy, RetryConfig, RetryPolicy};

fn args_json(extra: serde_json::Value) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("query".into(), serde_json::json!("wayland nano"));
    if let serde_json::Value::Object(obj) = extra {
        map.extend(obj);
    }
    serde_json::Value::Object(map)
}

// ── args / render (§8 "Search args/render") ─────────────────────────────

#[test]
fn args_defaults_and_limit_clamp() {
    let args = SearchArgs::parse(&args_json(serde_json::json!({}))).unwrap();
    assert_eq!(args.query, "wayland nano");
    assert_eq!(args.limit, SEARCH_DEFAULT_LIMIT);
    assert_eq!(args.allowed_domains, None);

    let args = SearchArgs::parse(&args_json(serde_json::json!({"limit": 9999}))).unwrap();
    assert_eq!(args.limit, SEARCH_MAX_LIMIT);
    let args = SearchArgs::parse(&args_json(serde_json::json!({"limit": 0}))).unwrap();
    assert_eq!(args.limit, SEARCH_MIN_LIMIT);
}

#[test]
fn args_reject_bad_types() {
    let err = SearchArgs::parse(&serde_json::json!({"limit": 5}))
        .expect_err("missing query is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
    let err = SearchArgs::parse(&args_json(serde_json::json!({"query": "   "})))
        .expect_err("blank query is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
    let err = SearchArgs::parse(&args_json(serde_json::json!({"limit": -1})))
        .expect_err("negative limit is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
    let err = SearchArgs::parse(&args_json(serde_json::json!({"limit": "five"})))
        .expect_err("non-integer limit is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
}

#[test]
fn args_query_cap_is_typed() {
    let within = "q".repeat(SEARCH_QUERY_MAX_BYTES);
    let args = SearchArgs::parse(&serde_json::json!({"query": within})).unwrap();
    assert_eq!(args.query.len(), SEARCH_QUERY_MAX_BYTES);
    let over = "q".repeat(SEARCH_QUERY_MAX_BYTES + 1);
    let err = SearchArgs::parse(&serde_json::json!({"query": over}))
        .expect_err("over-cap query is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
}

#[test]
fn args_domain_allowlist_bounds() {
    let domains: Vec<String> = (0..SEARCH_MAX_DOMAINS)
        .map(|i| format!("d{i}.example"))
        .collect();
    let args =
        SearchArgs::parse(&args_json(serde_json::json!({"allowed_domains": domains}))).unwrap();
    assert_eq!(
        args.allowed_domains.as_ref().unwrap().len(),
        SEARCH_MAX_DOMAINS
    );

    let too_many: Vec<String> = (0..=SEARCH_MAX_DOMAINS)
        .map(|i| format!("d{i}.ex"))
        .collect();
    let err = SearchArgs::parse(&args_json(serde_json::json!({"allowed_domains": too_many})))
        .expect_err("too many domains is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));

    let long = "d".repeat(SEARCH_DOMAIN_MAX_CHARS + 1);
    let err = SearchArgs::parse(&args_json(serde_json::json!({"allowed_domains": [long]})))
        .expect_err("over-long domain is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));

    let err = SearchArgs::parse(&args_json(
        serde_json::json!({"allowed_domains": "example.com"}),
    ))
    .expect_err("non-array domains is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
    let err = SearchArgs::parse(&args_json(serde_json::json!({"allowed_domains": [7]})))
        .expect_err("non-string domain is a typed error");
    assert!(matches!(err, WebSearchError::Args(_)));
}

fn outcome(results: Vec<SearchHit>, citations: Vec<String>, backend: &str) -> SearchOutcome {
    SearchOutcome {
        results,
        citations,
        grounding_usage: None,
        backend: backend.into(),
    }
}

#[test]
fn render_carries_backend_label_untrusted_and_reminder() {
    let hits = vec![
        SearchHit {
            title: "Alpha".into(),
            url: "https://example.com/a".into(),
            snippet: "alpha snippet".into(),
            date: Some("2026-08-01".into()),
        },
        SearchHit {
            title: "Beta".into(),
            url: "https://example.org/b".into(),
            snippet: "beta snippet".into(),
            date: None,
        },
    ];
    let out = render_search_output(&outcome(hits, vec!["https://example.com/a".into()], "flux"));
    assert!(out.starts_with("backend: flux\n"), "{out}");
    assert!(out.contains("untrusted remote content"), "{out}");
    assert!(out.contains("Title: Alpha\nURL: https://example.com/a\nDate: 2026-08-01\nSnippet: alpha snippet\n---"), "{out}");
    // No Date line when the hit carries none.
    assert!(
        out.contains("Title: Beta\nURL: https://example.org/b\nSnippet: beta snippet\n---"),
        "{out}"
    );
    assert!(
        out.contains("Citations:\n[1] https://example.com/a"),
        "{out}"
    );
    assert!(out.contains("cite sources inline"), "{out}");
}

#[test]
fn render_empty_ok_is_a_no_results_line() {
    let out = render_search_output(&outcome(Vec::new(), Vec::new(), "brave"));
    assert!(out.starts_with("backend: brave\n"), "{out}");
    assert!(out.contains("no results"), "{out}");
    assert!(out.contains("untrusted remote content"), "{out}");
}

#[test]
fn urlencode_encodes_reserved_bytes() {
    assert_eq!(urlencode("wayland nano"), "wayland%20nano");
    assert_eq!(urlencode("a+b&c=d"), "a%2Bb%26c%3Dd");
    assert_eq!(urlencode("plain-ok_1.0~"), "plain-ok_1.0~");
}

// ── scripted backends for the chain battery ──────────────────────────────

struct ScriptedBackend {
    id: &'static str,
    outcome: std::sync::Mutex<Option<Result<SearchOutcome, SearchBackendError>>>,
    calls: std::sync::atomic::AtomicU32,
}

impl ScriptedBackend {
    fn failing(id: &'static str) -> Self {
        Self {
            id,
            outcome: std::sync::Mutex::new(Some(Err(SearchBackendError::Backend {
                backend: id.into(),
                kind: BackendErrorKind::Parse("scripted down".into()),
            }))),
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn cancelling(id: &'static str) -> Self {
        Self {
            id,
            outcome: std::sync::Mutex::new(Some(Err(SearchBackendError::Cancelled))),
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn serving(id: &'static str, hits: Vec<SearchHit>) -> Self {
        Self {
            id,
            outcome: std::sync::Mutex::new(Some(Ok(SearchOutcome {
                results: hits,
                citations: Vec::new(),
                grounding_usage: None,
                backend: id.into(),
            }))),
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SearchBackend for ScriptedBackend {
    async fn search(
        &self,
        _args: &SearchArgs,
        _cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<SearchOutcome, SearchBackendError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.outcome
            .lock()
            .unwrap()
            .take()
            .expect("scripted outcome")
    }

    fn backend_id(&self) -> &str {
        self.id
    }
}

fn parsed_args() -> SearchArgs {
    SearchArgs::parse(&args_json(serde_json::json!({}))).unwrap()
}

/// §8 chain behavior: a failing first tier falls to the next, and the tool
/// result names the SERVING backend.
#[tokio::test]
async fn chain_falls_through_on_backend_error_and_names_the_server() {
    let down = std::sync::Arc::new(ScriptedBackend::failing("flux"));
    let up = std::sync::Arc::new(ScriptedBackend::serving(
        "brave",
        vec![SearchHit {
            title: "t".into(),
            url: "u".into(),
            snippet: "s".into(),
            date: None,
        }],
    ));
    let chain = ChainedSearchBackend::new(vec![down.clone(), up.clone()]);
    let outcome = chain.search(&parsed_args(), None).await.expect("served");
    assert_eq!(outcome.backend, "brave");
    assert_eq!(down.calls(), 1);
    assert_eq!(up.calls(), 1);
}

/// §8: `Ok` — even empty — is final; the next tier is never invoked.
#[tokio::test]
async fn chain_empty_ok_never_falls_through() {
    let empty = std::sync::Arc::new(ScriptedBackend::serving("flux", Vec::new()));
    let next = std::sync::Arc::new(ScriptedBackend::serving("brave", Vec::new()));
    let chain = ChainedSearchBackend::new(vec![empty.clone(), next.clone()]);
    let outcome = chain.search(&parsed_args(), None).await.expect("empty Ok");
    assert!(outcome.results.is_empty());
    assert_eq!(outcome.backend, "flux");
    assert_eq!(next.calls(), 0, "an Ok tier is final");
}

/// §8 cancellation battery (r2 codex-F3, D11): a cancelling first tier and
/// a healthy second tier — NO fall-through; the second backend is never
/// invoked; the result is typed Cancelled (no further network I/O).
#[tokio::test]
async fn chain_cancel_is_terminal_never_falls_through() {
    let cancelling = std::sync::Arc::new(ScriptedBackend::cancelling("flux"));
    let healthy = std::sync::Arc::new(ScriptedBackend::serving("brave", Vec::new()));
    let chain = ChainedSearchBackend::new(vec![cancelling.clone(), healthy.clone()]);
    let err = chain
        .search(&parsed_args(), None)
        .await
        .expect_err("cancelled propagates");
    assert!(matches!(err, SearchBackendError::Cancelled));
    assert_eq!(
        healthy.calls(),
        0,
        "a cancelled search fires no further I/O"
    );
}

/// §8: all tiers down → the typed terminal Unavailable from the tail (the
/// executor maps it to `is_error = true`; never a silent empty success).
#[tokio::test]
async fn chain_all_down_is_typed_unavailable() {
    let a = std::sync::Arc::new(ScriptedBackend::failing("flux"));
    let b = std::sync::Arc::new(ScriptedBackend::failing("brave"));
    let tail = std::sync::Arc::new(UnavailableSearchBackend::new(
        "looked for: flux (FLUX_API_KEY), brave (BRAVE_SEARCH_API_KEY)",
    ));
    let chain = ChainedSearchBackend::new(vec![a, b, tail]);
    let err = chain
        .search(&parsed_args(), None)
        .await
        .expect_err("typed unavailable");
    assert!(matches!(err, SearchBackendError::Unavailable(_)));
    assert!(err.to_string().contains("FLUX_API_KEY"), "{err}");
}

/// The tool layer maps backend errors 1:1 and reports the resolved chain id.
#[tokio::test]
async fn tool_wraps_the_chain_and_names_it() {
    let chain = ChainedSearchBackend::new(vec![std::sync::Arc::new(
        UnavailableSearchBackend::new("nothing resolved"),
    )]);
    let tool = WebSearchTool::new(std::sync::Arc::new(chain));
    assert_eq!(tool.backend_id(), "chained");
    let err = tool.search(&parsed_args(), None).await.expect_err("typed");
    assert!(matches!(err, WebSearchError::Unavailable(_)));
}

// ── loopback mock server ────────────────────────────────────────────────

/// One accepted connection: read the request head (+ any body bytes already
/// buffered), run the scripted responder, close. The request text is
/// captured for shape assertions.
struct MockServer {
    pub base: String,
    request: std::sync::Arc<std::sync::Mutex<String>>,
    // The accept thread is deliberately NOT joined: tests whose posture is
    // "zero socket activity" never connect, so the listener stays parked in
    // `accept()` and dies with the process. Joining would hang the test.
    _join: std::thread::JoinHandle<()>,
}

impl MockServer {
    /// `respond` receives the captured request and returns the full HTTP
    /// response bytes.
    fn serve_once(respond: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let request = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let capture = request.clone();
        let join = std::thread::spawn(move || {
            use std::io::Read;
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read the head; for POSTs keep going until Content-Length
            // bytes of body are in.
            let mut content_length = 0usize;
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(head_end) = text.find("\r\n\r\n") {
                            if content_length == 0 {
                                for line in text[..head_end].lines() {
                                    if let Some(v) =
                                        line.to_ascii_lowercase().strip_prefix("content-length:")
                                    {
                                        content_length = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                            if buf.len() >= head_end + 4 + content_length {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&buf).to_string();
            *capture.lock().unwrap() = text.clone();
            let bytes = respond(&text);
            use std::io::Write;
            stream.write_all(&bytes).ok();
            stream.flush().ok();
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            request,
            _join: join,
        }
    }

    /// Accepts the connection and NEVER answers — the cancel-during-send
    /// harness (the client's `send()` await parks on the response head).
    fn serve_silent() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let request = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let join = std::thread::spawn(move || {
            use std::io::Read;
            let (mut stream, _) = listener.accept().expect("accept");
            // Drain the request, then hold the connection open silently
            // until the client gives up (cancel closes the socket).
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(30)))
                .ok();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            request,
            _join: join,
        }
    }

    /// Sends the response head, then drips the body in small chunks with
    /// real delays — the cancel-during-body-read harness.
    fn serve_dripping(body: &'static [u8]) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let request = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let join = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().expect("accept");
            let mut sink = std::io::sink();
            // Drain whatever the client sent; it is not asserted here.
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(500)))
                .ok();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        sink.write_all(&chunk[..n]).ok();
                        if chunk[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).ok();
            stream.flush().ok();
            for piece in body.chunks(8) {
                std::thread::sleep(std::time::Duration::from_millis(80));
                if stream.write_all(piece).is_err() {
                    return; // the client aborted (cancelled) mid-body
                }
                stream.flush().ok();
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            request,
            _join: join,
        }
    }

    fn captured(&self) -> String {
        self.request.lock().unwrap().clone()
    }
}

fn http_ok(content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn http_status(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} ERR\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn loopback_egress() -> EgressClient {
    EgressClient::new(nano_egress::policy::EgressPolicy::new().allow_host_with_http("127.0.0.1"))
}

fn flux_backend(
    base: &str,
    meter: std::sync::Arc<dyn UsageSink>,
) -> Result<FluxSearchBackend, WebSearchError> {
    let client = OpenAiCompletionsClient::new(loopback_egress()).with_base_url(base);
    FluxSearchBackend::new(client, "sk-test-not-a-real-key", Some(meter))
}

// ── Flux backend (§8 "Flux grounding unit/fixture") ─────────────────────

/// r2 claude-F2: no meter handle ⇒ typed refusal at construction — and
/// with no client interaction possible, zero socket activity.
#[test]
fn flux_backend_without_meter_is_a_typed_refusal() {
    let client =
        OpenAiCompletionsClient::new(loopback_egress()).with_base_url("http://127.0.0.1:1"); // nothing listens here
    let err =
        FluxSearchBackend::new(client, "sk-test", None).expect_err("no handle ⇒ typed refusal");
    assert!(matches!(err, WebSearchError::Unmetered(_)));
    assert!(err.to_string().contains("unmetered"), "{err}");
}

/// The full wire round-trip against the loopback mock: the captured
/// request is the isolated grounding shape (pinned flux-fast, one user
/// message, the grounding flag, NO conversation context), and the response
/// normalizes into hits + citations + reported usage.
#[tokio::test]
async fn flux_backend_round_trip_is_isolated_and_normalized() {
    let body = serde_json::json!({
        "choices": [{"message": {"content": "answer [1]"}}],
        "citations": ["https://example.com/a"],
        "search_results": [
            {"title": "A", "url": "https://example.com/a", "snippet": "alpha", "date": "2026-08-01"}
        ],
        "usage": {"prompt_tokens": 9, "completion_tokens": 4}
    })
    .to_string();
    let server = MockServer::serve_once(move |_req| http_ok("application/json", &body));
    let meter = std::sync::Arc::new(StubCostMeter::new());
    let backend = flux_backend(&server.base, meter).expect("metered");
    let outcome = backend.search(&parsed_args(), None).await.expect("ok");
    assert_eq!(outcome.backend, "flux");
    assert_eq!(outcome.results.len(), 1);
    assert_eq!(outcome.results[0].title, "A");
    assert_eq!(outcome.citations, ["https://example.com/a"]);
    let usage = outcome.grounding_usage.expect("flux carries usage");
    assert!(usage.reported);
    assert_eq!(usage.usage.input_tokens, 9);

    let captured = server.captured();
    let body_start = captured.find("\r\n\r\n").expect("request head");
    let request_body: serde_json::Value =
        serde_json::from_str(&captured[body_start + 4..]).expect("json body");
    assert_eq!(request_body["model"], "flux-fast");
    assert_eq!(
        request_body["tools"],
        serde_json::json!([{"type": "web_search"}])
    );
    let messages = request_body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "no conversation context ever");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "wayland nano");
    assert!(request_body.get("system").is_none());
    assert!(request_body["max_tokens"].as_u64().unwrap() <= 1024);
}

/// A prose-only grounding answer (no search_results) is a typed Backend
/// parse failure — NEVER fabricated hits (design §2.2).
#[tokio::test]
async fn flux_backend_prose_only_is_a_typed_parse_failure() {
    let body = serde_json::json!({
        "choices": [{"message": {"content": "no structured results here"}}]
    })
    .to_string();
    let server = MockServer::serve_once(move |_req| http_ok("application/json", &body));
    let meter = std::sync::Arc::new(StubCostMeter::new());
    let backend = flux_backend(&server.base, meter).expect("metered");
    let err = backend
        .search(&parsed_args(), None)
        .await
        .expect_err("typed parse failure");
    match err {
        SearchBackendError::Backend { backend, kind } => {
            assert_eq!(backend, "flux");
            assert!(matches!(
                kind,
                BackendErrorKind::Model(ModelError::Protocol(_))
            ));
        }
        other => panic!("expected typed Backend parse failure, got {other:?}"),
    }
}

/// §8 cancellation battery: cancel during the grounding SEND → typed
/// Cancelled, promptly (the silent server never answers, so the `send()`
/// await parks until the flag fires).
#[tokio::test]
async fn flux_backend_cancel_during_send_is_prompt_cancelled() {
    let server = MockServer::serve_silent();
    let meter = std::sync::Arc::new(StubCostMeter::new());
    let backend = flux_backend(&server.base, meter).expect("metered");
    let flag = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let parsed = parsed_args();
    let driver = backend.search(&parsed, Some(&flag));
    let canceller = async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(driver, canceller);
    let err = result.expect_err("cancelled mid-send");
    assert!(matches!(err, SearchBackendError::Cancelled));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "cancel must be prompt"
    );
}

/// §8 cancellation battery: cancel during the grounding BODY-READ → typed
/// Cancelled, promptly (the drip server keeps sending for seconds).
#[tokio::test]
async fn flux_backend_cancel_during_body_read_is_prompt_cancelled() {
    let body = serde_json::json!({
        "search_results": [{"title": "t", "url": "u", "snippet": "s"}],
        "citations": []
    })
    .to_string();
    let leaked: &'static [u8] = Box::leak(body.into_bytes().into_boxed_slice());
    let server = MockServer::serve_dripping(leaked);
    let meter = std::sync::Arc::new(StubCostMeter::new());
    let backend = flux_backend(&server.base, meter).expect("metered");
    let flag = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let parsed = parsed_args();
    let driver = backend.search(&parsed, Some(&flag));
    let canceller = async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(driver, canceller);
    let err = result.expect_err("cancelled mid-body-read");
    assert!(matches!(err, SearchBackendError::Cancelled));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "cancel must be prompt"
    );
}

/// §8 cancellation battery: cancel during a RETRY SLEEP → typed Cancelled
/// (the sleep_or_cancel precedent; connection-refused drives the
/// cancel-selectable reconnect class).
#[tokio::test]
async fn flux_backend_cancel_during_retry_sleep_is_prompt_cancelled() {
    // Nothing accepts on this port: every connect is refused (Reconnect
    // class, 30 s cancel-selectable sleeps).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let client = OpenAiCompletionsClient::new(loopback_egress())
        .with_base_url(format!("http://127.0.0.1:{port}"))
        .with_retry_config(RetryConfig {
            fast: RetryPolicy {
                max_attempts: 1,
                base_delay_ms: 1,
                max_delay_ms: 1,
            },
            reconnect: ReconnectPolicy {
                max_retries: 3,
                initial_delay: std::time::Duration::from_secs(30),
                max_delay: std::time::Duration::from_secs(30),
                deadline: std::time::Duration::from_secs(300),
            },
        });
    let meter: std::sync::Arc<dyn UsageSink> = std::sync::Arc::new(StubCostMeter::new());
    let backend = FluxSearchBackend::new(client, "sk-test", Some(meter)).expect("metered");
    let flag = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let parsed = parsed_args();
    let driver = backend.search(&parsed, Some(&flag));
    let canceller = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(driver, canceller);
    let err = result.expect_err("cancelled during the retry sleep");
    assert!(matches!(err, SearchBackendError::Cancelled));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "cancel preempts the 30s reconnect sleep promptly"
    );
}

/// A pre-fired flag is honoured BEFORE the first byte leaves: typed
/// Cancelled with zero socket activity (the mock is never contacted).
#[tokio::test]
async fn flux_backend_pre_fired_flag_short_circuits() {
    let server = MockServer::serve_once(|_req| unreachable!("no request may arrive"));
    let meter = std::sync::Arc::new(StubCostMeter::new());
    let backend = flux_backend(&server.base, meter).expect("metered");
    let flag = std::sync::atomic::AtomicBool::new(true);
    let err = backend
        .search(&parsed_args(), Some(&flag))
        .await
        .expect_err("pre-fired cancel");
    assert!(matches!(err, SearchBackendError::Cancelled));
}

// ── Brave / Tavily (§8 chain + normalization legs) ───────────────────────

#[tokio::test]
async fn brave_round_trip_normalizes_and_sends_the_key_header() {
    let body = serde_json::json!({
        "web": {"results": [
            {"title": "A", "url": "https://example.com/a", "description": "alpha"},
            {"title": "B", "url": "https://example.org/b", "description": "beta"}
        ]}
    })
    .to_string();
    let server = MockServer::serve_once(move |_req| http_ok("application/json", &body));
    let backend = BraveSearchBackend::new(loopback_egress(), "sk-test-brave")
        .with_endpoint_for_tests(format!("{}/res/v1/web/search", server.base));
    let outcome = backend.search(&parsed_args(), None).await.expect("ok");
    assert_eq!(outcome.backend, "brave");
    assert_eq!(outcome.results.len(), 2);
    assert_eq!(outcome.results[0].snippet, "alpha");
    assert!(outcome.grounding_usage.is_none(), "brave counts no tokens");

    let captured = server.captured();
    assert!(
        captured.contains("x-subscription-token: sk-test-brave"),
        "{captured}"
    );
    assert!(
        captured.starts_with("GET /res/v1/web/search?q=wayland%20nano&count=5"),
        "{captured}"
    );
}

/// §8: loopback Brave returning 500 → structured typed Err (the chain
/// falls through on exactly this class — covered above).
#[tokio::test]
async fn brave_500_is_a_structured_backend_error() {
    let server = MockServer::serve_once(|_req| http_status(500, "boom"));
    let backend = BraveSearchBackend::new(loopback_egress(), "sk-test-brave")
        .with_endpoint_for_tests(format!("{}/res/v1/web/search", server.base));
    let err = backend
        .search(&parsed_args(), None)
        .await
        .expect_err("typed 500");
    match err {
        SearchBackendError::Backend { backend, kind } => {
            assert_eq!(backend, "brave");
            assert!(matches!(
                kind,
                BackendErrorKind::Egress(nano_egress::client::EgressError::HttpStatus {
                    status: 500,
                    ..
                })
            ));
        }
        other => panic!("expected typed backend error, got {other:?}"),
    }
}

/// Deny-by-default at the backend's own policy: a Brave backend whose
/// policy domain lacks the host is inert — typed egress denial BEFORE any
/// socket activity (the mock never accepts).
#[tokio::test]
async fn brave_with_empty_policy_is_inert() {
    let server = MockServer::serve_once(|_req| unreachable!("policy denies before connect"));
    // Deliberately NOT allowlisting 127.0.0.1: the single-host policy is
    // the key gate — without the host, nothing resolves.
    let client = EgressClient::new(nano_egress::policy::EgressPolicy::new());
    let backend = BraveSearchBackend::new(client, "sk-test-brave")
        .with_endpoint_for_tests(format!("{}/res/v1/web/search", server.base));
    let err = backend
        .search(&parsed_args(), None)
        .await
        .expect_err("denied");
    assert!(matches!(
        err,
        SearchBackendError::Backend {
            kind: BackendErrorKind::Egress(nano_egress::client::EgressError::Denied { .. }),
            ..
        }
    ));
}

#[tokio::test]
async fn tavily_round_trip_normalizes_and_maps_domain_filters() {
    let body = serde_json::json!({
        "results": [
            {"title": "A", "url": "https://example.com/a", "content": "alpha"}
        ]
    })
    .to_string();
    let server = MockServer::serve_once(move |_req| http_ok("application/json", &body));
    let backend = TavilySearchBackend::new(loopback_egress(), "sk-test-tavily")
        .with_endpoint_for_tests(format!("{}/search", server.base));
    let args = SearchArgs::parse(&args_json(
        serde_json::json!({"allowed_domains": ["example.com"]}),
    ))
    .unwrap();
    let outcome = backend.search(&args, None).await.expect("ok");
    assert_eq!(outcome.backend, "tavily");
    assert_eq!(outcome.results[0].snippet, "alpha");

    let captured = server.captured();
    assert!(
        captured.contains("authorization: Bearer sk-test-tavily"),
        "{captured}"
    );
    let body_start = captured.find("\r\n\r\n").expect("head");
    let request_body: serde_json::Value =
        serde_json::from_str(&captured[body_start + 4..]).expect("json");
    assert_eq!(request_body["query"], "wayland nano");
    assert_eq!(request_body["max_results"], 5);
    assert_eq!(request_body["search_depth"], "basic");
    assert_eq!(
        request_body["include_domains"],
        serde_json::json!(["example.com"])
    );
}

/// Cancel flags are honored within the direct backends too (r2 codex-F3):
/// a pre-fired flag never sends a byte.
#[tokio::test]
async fn tavily_pre_fired_flag_is_terminal_cancel() {
    let server = MockServer::serve_once(|_req| unreachable!("no request may arrive"));
    let backend = TavilySearchBackend::new(loopback_egress(), "sk-test-tavily")
        .with_endpoint_for_tests(format!("{}/search", server.base));
    let flag = std::sync::atomic::AtomicBool::new(true);
    let err = backend
        .search(&parsed_args(), Some(&flag))
        .await
        .expect_err("cancelled");
    assert!(matches!(err, SearchBackendError::Cancelled));
}

/// The Unavailable tier names what was looked for — env-var NAMES, never
/// values (D3 / wcore NullWebBackend posture).
#[tokio::test]
async fn unavailable_backend_names_env_vars_not_values() {
    let backend = UnavailableSearchBackend::new(
        "no search backend resolved (looked for: flux via FLUX_API_KEY, brave via BRAVE_SEARCH_API_KEY, tavily via TAVILY_API_KEY)",
    );
    let err = backend
        .search(&parsed_args(), None)
        .await
        .expect_err("typed");
    assert!(matches!(err, SearchBackendError::Unavailable(_)));
    let text = err.to_string();
    assert!(text.contains("BRAVE_SEARCH_API_KEY"), "{text}");
    assert!(text.contains("TAVILY_API_KEY"), "{text}");
}
