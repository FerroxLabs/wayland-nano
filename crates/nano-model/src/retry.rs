//! Bounded provider retry (Kimi stepRetry policy, ported) plus the C9
//! codex-parity split into two typed classes with exactly ONE decision
//! point per model call:
//!
//! - **Reconnect (slow) class** — `Transport` with phase Connect / Tls /
//!   BeforeFirstByte (no response byte observed): codex's 5s→doubling→60s
//!   cap schedule, bounded by a single 5-minute wall-clock deadline per
//!   model call (INCLUDING connection attempts and sleeps, clock starting
//!   at the first slow-class failure) AND max 8 retries (= up to 9 wire
//!   requests), whichever trips first. Cancel preempts the sleep.
//! - **Request-failure (fast) class** — RateLimited (Retry-After wins),
//!   5xx, and `Transport { MidStream }`: the unchanged Nano `RetryPolicy`
//!   (500ms→32s, 25% jitter, 6 attempts).
//!
//! The classes never compound and never nest: slow retries bypass
//! `RetryPolicy` entirely (they neither consume its attempts nor are passed
//! to `decide`); a call switches failure class AT MOST ONCE — a fast-class
//! error after reconnect attempts continues under the fast policy with its
//! OWN fresh attempt budget; a second switch is terminal.
//!
//! Classification consumes ONLY typed provenance (`TransportPhase`, HTTP
//! status) — never string/error-chain inspection.

use crate::types::{CallHooks, ModelError, ModelObservation, TransportPhase};

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            base_delay_ms: 500,
            max_delay_ms: 32_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RetryAction {
    Retry { attempt: u32, delay_ms: u64 },
    GiveUp,
}

/// The two typed retry classes (C9 §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Connection failure, no response byte observed: the slow reconnect
    /// loop, bypassing the fast budget entirely.
    Reconnect,
    /// Rate-limited / 5xx / mid-stream transport failure: the fast policy.
    Fast,
}

/// Type-level classification. `None` = never retried. This is the ONLY
/// classifier; no string inspection anywhere in the retry path.
pub fn classify(err: &ModelError) -> Option<RetryClass> {
    match err {
        ModelError::RateLimited { .. } => Some(RetryClass::Fast),
        ModelError::Server { status, .. } if *status >= 500 => Some(RetryClass::Fast),
        ModelError::Transport { phase, .. } => match phase {
            TransportPhase::Connect | TransportPhase::Tls | TransportPhase::BeforeFirstByte => {
                Some(RetryClass::Reconnect)
            }
            TransportPhase::MidStream => Some(RetryClass::Fast),
        },
        // C7: the egress-wrapped forms of the SAME transient classes retry
        // identically — the error-code table (nano-protocol error_codes)
        // pins agreement for every variant, so the wrapper cannot silently
        // downgrade a transient to terminal. The egress wrapper carries no
        // typed phase provenance, so its transport form maps to the fast
        // class (pre-C9 retry behavior preserved).
        ModelError::Egress(nano_egress::client::EgressError::Transport(_)) => {
            Some(RetryClass::Fast)
        }
        ModelError::Egress(nano_egress::client::EgressError::HttpStatus { status, .. })
            if *status >= 500 =>
        {
            Some(RetryClass::Fast)
        }
        _ => None,
    }
}

/// Fast-class retryability, preserving the pre-C9 signature for the
/// fixture-pinned callers: `Some(retry_after)` for the fast class, `None`
/// otherwise. Reconnect-class errors are NOT fast-retryable.
pub fn is_retryable(err: &ModelError) -> Option<Option<u64>> {
    if classify(err) != Some(RetryClass::Fast) {
        return None;
    }
    match err {
        ModelError::RateLimited { retry_after_ms } => Some(*retry_after_ms),
        _ => Some(None),
    }
}

/// Reconnect-class policy (C9 Q2 RULED): codex's schedule, Nano's bounds.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Max reconnect RETRIES (the initial request is attempt 0; max 8
    /// retries = up to 9 requests on the wire).
    pub max_retries: u32,
    /// First reconnect sleep (codex: 5s), doubling per retry…
    pub initial_delay: std::time::Duration,
    /// …up to this cap (codex: 60s).
    pub max_delay: std::time::Duration,
    /// Single wall-clock deadline per model call, evaluated BEFORE each
    /// retry sleep and again after waking; a retry whose scheduled wake
    /// would cross the deadline is not started.
    pub deadline: std::time::Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_retries: 8,
            initial_delay: std::time::Duration::from_secs(5),
            max_delay: std::time::Duration::from_secs(60),
            deadline: std::time::Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReconnectAction {
    Retry {
        retry: u32,
        delay: std::time::Duration,
    },
    GiveUp,
}

impl ReconnectPolicy {
    /// The sleep before retry `index` (0-based): 5s, 10s, 20s, 40s, 60s…
    pub fn delay_for(&self, index: u32) -> std::time::Duration {
        let shift = index.min(10);
        (self.initial_delay.saturating_mul(1u32 << shift)).min(self.max_delay)
    }

    /// The single reconnect decision: both bounds checked BEFORE the sleep.
    pub fn decide(&self, retries_used: u32, elapsed: std::time::Duration) -> ReconnectAction {
        if retries_used >= self.max_retries || elapsed >= self.deadline {
            return ReconnectAction::GiveUp;
        }
        let delay = self.delay_for(retries_used);
        if elapsed.saturating_add(delay) > self.deadline {
            // The scheduled wake would cross the deadline: do not start it.
            return ReconnectAction::GiveUp;
        }
        ReconnectAction::Retry {
            retry: retries_used + 1,
            delay,
        }
    }
}

impl RetryPolicy {
    pub fn decide(&self, attempt: u32, err: &ModelError) -> RetryAction {
        if attempt >= self.max_attempts {
            return RetryAction::GiveUp;
        }
        let Some(retry_after) = is_retryable(err) else {
            return RetryAction::GiveUp;
        };
        let delay = match retry_after {
            // Retry-After wins, always honored first.
            Some(ms) => ms,
            None => {
                let exp = self.base_delay_ms.saturating_mul(1u64 << attempt.min(6));
                let capped = exp.min(self.max_delay_ms);
                // 25% deterministic jitter (xorshift on attempt) — avoids herd.
                let jitter_seed = (attempt as u64).wrapping_mul(0x9E3779B97F4A7C15) >> 59;
                let jitter = capped * (jitter_seed as u32 % 25) as u64 / 100;
                capped + jitter
            }
        };
        RetryAction::Retry {
            attempt: attempt + 1,
            delay_ms: delay,
        }
    }
}

/// The full per-call retry configuration: fast + reconnect policies.
#[derive(Debug, Clone, Default)]
pub struct RetryConfig {
    pub fast: RetryPolicy,
    pub reconnect: ReconnectPolicy,
}

/// A cancel-selectable sleep: returns true when the cancel flag fired
/// mid-sleep (cancel preempts sleeps — Q2 mandatory). Polls the flag in
/// small chunks so a fired flag aborts promptly even out of a long wait.
async fn sleep_or_cancel(
    duration: std::time::Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    const CHUNK: std::time::Duration = std::time::Duration::from_millis(50);
    let fired = |cancel: Option<&std::sync::atomic::AtomicBool>| {
        cancel.is_some_and(|f| f.load(std::sync::atomic::Ordering::SeqCst))
    };
    let mut remaining = duration;
    while !remaining.is_zero() {
        if fired(cancel) {
            return true;
        }
        let step = remaining.min(CHUNK);
        tokio::time::sleep(step).await;
        remaining = remaining.saturating_sub(step);
    }
    fired(cancel)
}

/// THE single retry decision point per model call (C9 §2.1). All three
/// wire surfaces run their attempt closure through here; within-call
/// retries re-invoke the closure with a byte-identical request and never
/// mutate history.
pub async fn run_with_retries<T, F, Fut>(
    config: &RetryConfig,
    hooks: &CallHooks<'_>,
    mut attempt: F,
) -> Result<T, ModelError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ModelError>>,
{
    let mut fast_attempt = 0u32;
    let mut reconnect_retries = 0u32;
    let mut reconnect_started: Option<std::time::Instant> = None;
    let mut last_class: Option<RetryClass> = None;
    let mut switches = 0u32;
    loop {
        if hooks.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        let err = match attempt().await {
            Ok(response) => return Ok(response),
            Err(err) => err,
        };
        let Some(class) = classify(&err) else {
            return Err(err);
        };
        // Class-switch accounting (no nested multiplication): at most one
        // switch per call; switching to the fast class after reconnect
        // attempts restarts the fast budget FRESH.
        if let Some(previous) = last_class
            && previous != class
        {
            switches += 1;
            if switches > 1 {
                return Err(err);
            }
            if class == RetryClass::Fast {
                fast_attempt = 0;
            }
        }
        match class {
            RetryClass::Reconnect => {
                let started = *reconnect_started.get_or_insert_with(std::time::Instant::now);
                match config
                    .reconnect
                    .decide(reconnect_retries, started.elapsed())
                {
                    ReconnectAction::Retry { retry, delay } => {
                        hooks.observe(ModelObservation::Reconnecting {
                            attempt: retry,
                            next_delay_ms: delay.as_millis() as u64,
                            deadline_remaining_ms: config
                                .reconnect
                                .deadline
                                .saturating_sub(started.elapsed())
                                .as_millis()
                                as u64,
                        });
                        if sleep_or_cancel(delay, hooks.cancel).await {
                            return Err(ModelError::Cancelled);
                        }
                        reconnect_retries = retry;
                        // Deadline re-evaluated after waking: a retry that
                        // woke past the deadline does not fire.
                        if started.elapsed() >= config.reconnect.deadline {
                            return Err(err);
                        }
                        last_class = Some(RetryClass::Reconnect);
                    }
                    ReconnectAction::GiveUp => return Err(err),
                }
            }
            RetryClass::Fast => match config.fast.decide(fast_attempt, &err) {
                RetryAction::Retry {
                    attempt: next,
                    delay_ms,
                } => {
                    // P1 (r2 codex-F3): the fast-class sleep rides the same
                    // cancel-selectable wait as the reconnect class — EVERY
                    // retry-sleep surface preempts promptly on cancel.
                    if sleep_or_cancel(std::time::Duration::from_millis(delay_ms), hooks.cancel)
                        .await
                    {
                        return Err(ModelError::Cancelled);
                    }
                    fast_attempt = next;
                    last_class = Some(RetryClass::Fast);
                }
                RetryAction::GiveUp => return Err(err),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelResponse;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn retry_after_always_wins() {
        let policy = RetryPolicy::default();
        let err = ModelError::RateLimited {
            retry_after_ms: Some(7_500),
        };
        assert_eq!(
            policy.decide(3, &err),
            RetryAction::Retry {
                attempt: 4,
                delay_ms: 7_500
            }
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy::default();
        let err = ModelError::Server {
            status: 503,
            message: "x".into(),
        };
        let RetryAction::Retry { delay_ms: d0, .. } = policy.decide(0, &err) else {
            panic!()
        };
        let RetryAction::Retry { delay_ms: d3, .. } = policy.decide(3, &err) else {
            panic!()
        };
        assert!((500..2000).contains(&d0));
        assert!(d3 > d0);
        let long_policy = RetryPolicy {
            max_attempts: 12,
            ..RetryPolicy::default()
        };
        let RetryAction::Retry { delay_ms: d9, .. } = long_policy.decide(9, &err) else {
            panic!()
        };
        assert!(d9 <= 32_000 + 8_000);
    }

    #[test]
    fn non_retryable_gives_up_immediately() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.decide(
                0,
                &ModelError::Auth {
                    message: "bad key".into(),
                    status: Some(401)
                }
            ),
            RetryAction::GiveUp
        );
        assert_eq!(
            policy.decide(0, &ModelError::Cancelled),
            RetryAction::GiveUp
        );
    }

    #[test]
    fn budget_exhaustion_gives_up() {
        let policy = RetryPolicy::default();
        let err = ModelError::Transport {
            phase: TransportPhase::MidStream,
            message: "reset".into(),
        };
        assert_eq!(
            policy.decide(policy.max_attempts, &err),
            RetryAction::GiveUp
        );
    }

    // ── C9 classification (type-level, no string inspection) ─────────────

    #[test]
    fn classification_is_type_level() {
        let slow = |phase| ModelError::Transport {
            phase,
            message: String::new(),
        };
        assert_eq!(
            classify(&slow(TransportPhase::Connect)),
            Some(RetryClass::Reconnect)
        );
        assert_eq!(
            classify(&slow(TransportPhase::Tls)),
            Some(RetryClass::Reconnect)
        );
        assert_eq!(
            classify(&slow(TransportPhase::BeforeFirstByte)),
            Some(RetryClass::Reconnect)
        );
        // Mid-stream failures are NEVER reclassified into the slow loop.
        assert_eq!(
            classify(&slow(TransportPhase::MidStream)),
            Some(RetryClass::Fast)
        );
        assert_eq!(
            classify(&ModelError::RateLimited {
                retry_after_ms: None
            }),
            Some(RetryClass::Fast)
        );
        assert_eq!(
            classify(&ModelError::Server {
                status: 503,
                message: String::new()
            }),
            Some(RetryClass::Fast)
        );
        assert_eq!(
            classify(&ModelError::Server {
                status: 400,
                message: String::new()
            }),
            None
        );
        assert_eq!(
            classify(&ModelError::Auth {
                message: String::new(),
                status: Some(401)
            }),
            None
        );
    }

    // ── C9 reconnect schedule + boundary pins (Q2) ───────────────────────

    #[test]
    fn reconnect_schedule_is_codex_5_10_20_40_60_capped() {
        let policy = ReconnectPolicy::default();
        let secs: Vec<u64> = (0..8).map(|i| policy.delay_for(i).as_secs()).collect();
        assert_eq!(secs, vec![5, 10, 20, 40, 60, 60, 60, 60]);
    }

    #[test]
    fn wall_clock_deadline_trips_first_on_the_default_schedule() {
        // Q2 boundary pin: on the 5/10/20/40/60/60/60 schedule the deadline
        // (300s) trips at the DECISION for retry 8 — cumulative sleeps reach
        // 255s after 7 retries and the 8th wake (255+60=315s) would cross
        // the deadline. So the default schedule yields 7 retries = 8 wire
        // requests, and the count bound stays the untripped backstop.
        let policy = ReconnectPolicy::default();
        let mut elapsed = std::time::Duration::ZERO;
        let mut retries = 0;
        while let ReconnectAction::Retry { retry, delay } = policy.decide(retries, elapsed) {
            retries = retry;
            elapsed += delay;
        }
        assert_eq!(retries, 7, "wall-clock trips first: 7 retries = 8 requests");
        assert_eq!(elapsed, std::time::Duration::from_secs(255));
        assert!(retries < policy.max_retries);
    }

    #[test]
    fn count_bound_is_the_backstop_8_retries_9_wire_requests() {
        // With a deadline that cannot trip, exactly 8 retries run = 9
        // requests on the wire; the 9th retry decision gives up.
        let policy = ReconnectPolicy {
            deadline: std::time::Duration::from_secs(86_400),
            ..ReconnectPolicy::default()
        };
        let mut elapsed = std::time::Duration::ZERO;
        for expected_retry in 1..=8 {
            match policy.decide(expected_retry - 1, elapsed) {
                ReconnectAction::Retry { retry, delay } => {
                    assert_eq!(retry, expected_retry);
                    elapsed += delay;
                }
                ReconnectAction::GiveUp => panic!("retry {expected_retry} must run"),
            }
        }
        assert_eq!(policy.decide(8, elapsed), ReconnectAction::GiveUp);
    }

    #[test]
    fn deadline_evaluated_before_each_sleep() {
        let policy = ReconnectPolicy::default();
        // 296s elapsed: even the smallest scheduled wake (5s → 301s) would
        // cross the deadline, so no retry starts.
        assert_eq!(
            policy.decide(0, std::time::Duration::from_secs(296)),
            ReconnectAction::GiveUp
        );
        assert!(matches!(
            policy.decide(0, std::time::Duration::from_secs(294)),
            ReconnectAction::Retry { .. }
        ));
    }

    // ── C9 single decision point: driver-level fault injection ──────────

    fn transport(phase: TransportPhase) -> ModelError {
        ModelError::Transport {
            phase,
            message: "injected".into(),
        }
    }

    /// Tiny reconnect policy for async driver tests (no real waiting).
    fn tiny_config() -> RetryConfig {
        RetryConfig {
            fast: RetryPolicy {
                max_attempts: 3,
                base_delay_ms: 1,
                max_delay_ms: 2,
            },
            reconnect: ReconnectPolicy {
                max_retries: 8,
                initial_delay: std::time::Duration::from_millis(1),
                max_delay: std::time::Duration::from_millis(2),
                deadline: std::time::Duration::from_secs(60),
            },
        }
    }

    #[tokio::test]
    async fn reconnect_class_does_not_consume_the_fast_budget() {
        // 4 slow failures then success: 5 calls total, fast budget untouched.
        let calls = Mutex::new(0u32);
        let config = tiny_config();
        let result = run_with_retries(&config, &CallHooks::none(), || {
            let calls = &calls;
            async move {
                *calls.lock().unwrap() += 1;
                if *calls.lock().unwrap() <= 4 {
                    Err(transport(TransportPhase::Connect))
                } else {
                    Ok(ModelResponse {
                        events: Vec::new(),
                        usage: Default::default(),
                        stop_reason: "stop".into(),
                    })
                }
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(*calls.lock().unwrap(), 5);
    }

    #[tokio::test]
    async fn exhaustion_of_the_slow_class_returns_the_typed_error() {
        let calls = Mutex::new(0u32);
        let config = tiny_config();
        let result = run_with_retries(&config, &CallHooks::none(), || {
            let calls = &calls;
            async move {
                *calls.lock().unwrap() += 1;
                Err::<ModelResponse, _>(transport(TransportPhase::BeforeFirstByte))
            }
        })
        .await;
        // 8 retries = 9 wire requests, then the typed Transport error.
        assert_eq!(*calls.lock().unwrap(), 9);
        assert!(matches!(
            result,
            Err(ModelError::Transport {
                phase: TransportPhase::BeforeFirstByte,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn mid_stream_failures_consume_the_fast_budget_only() {
        let calls = Mutex::new(0u32);
        let config = tiny_config();
        let result = run_with_retries(&config, &CallHooks::none(), || {
            let calls = &calls;
            async move {
                *calls.lock().unwrap() += 1;
                Err::<ModelResponse, _>(transport(TransportPhase::MidStream))
            }
        })
        .await;
        // fast policy: max_attempts 3 → attempts 1..=3 then GiveUp (decide
        // at attempt 3 >= max_attempts).
        assert_eq!(*calls.lock().unwrap(), 4);
        assert!(matches!(
            result,
            Err(ModelError::Transport {
                phase: TransportPhase::MidStream,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn slow_to_fast_switch_gets_a_fresh_fast_budget_at_most_once() {
        // slow, slow, then persistent fast failure: the fast budget restarts
        // fresh (attempt 0) — the classes never compound.
        let calls = Mutex::new(0u32);
        let config = tiny_config();
        let result = run_with_retries(&config, &CallHooks::none(), || {
            let calls = &calls;
            async move {
                let n = {
                    let mut guard = calls.lock().unwrap();
                    *guard += 1;
                    *guard
                };
                if n <= 2 {
                    Err::<ModelResponse, _>(transport(TransportPhase::Connect))
                } else {
                    Err(transport(TransportPhase::MidStream))
                }
            }
        })
        .await;
        // 2 slow retries + 4 fast-path calls (attempts 0..3 + give-up check)
        // = 6 calls; a compound budget would have quit far earlier.
        assert_eq!(*calls.lock().unwrap(), 6);
        assert!(matches!(result, Err(ModelError::Transport { .. })));

        // A SECOND class switch is terminal (no oscillation).
        let calls = Mutex::new(0u32);
        let result = run_with_retries(&tiny_config(), &CallHooks::none(), || {
            let calls = &calls;
            async move {
                let n = {
                    let mut guard = calls.lock().unwrap();
                    *guard += 1;
                    *guard
                };
                match n % 2 {
                    1 => Err::<ModelResponse, _>(transport(TransportPhase::Connect)),
                    _ => Err(transport(TransportPhase::MidStream)),
                }
            }
        })
        .await;
        // slow(1) → fast(2, switch 1) → slow(3, switch 2) → terminal.
        assert_eq!(*calls.lock().unwrap(), 3);
        assert!(matches!(result, Err(ModelError::Transport { .. })));
    }

    #[tokio::test]
    async fn cancel_preempts_a_reconnect_sleep() {
        let cancel = AtomicBool::new(false);
        let config = RetryConfig {
            reconnect: ReconnectPolicy {
                initial_delay: std::time::Duration::from_secs(30),
                max_delay: std::time::Duration::from_secs(30),
                ..tiny_config().reconnect
            },
            ..tiny_config()
        };
        let hooks = CallHooks {
            cancel: Some(&cancel),
            observer: None,
        };
        let started = std::time::Instant::now();
        let flag = &cancel;
        let driver = run_with_retries(&config, &hooks, || async {
            Err::<ModelResponse, _>(transport(TransportPhase::Tls))
        });
        tokio::pin!(driver);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let canceller = async {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            };
            tokio::join!(driver, canceller).0
        })
        .await
        .expect("cancel must preempt the 30s sleep promptly");
        assert!(matches!(result, Err(ModelError::Cancelled)));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// P1 (r2 codex-F3): the FAST-class retry sleep is cancel-selectable
    /// too — every retry-sleep surface preempts promptly with typed
    /// Cancelled (the design note's grounding-cancellation invariant).
    #[tokio::test]
    async fn cancel_preempts_a_fast_sleep() {
        let cancel = AtomicBool::new(false);
        let config = RetryConfig {
            fast: RetryPolicy {
                max_attempts: 3,
                base_delay_ms: 30_000,
                max_delay_ms: 30_000,
            },
            ..tiny_config()
        };
        let hooks = CallHooks {
            cancel: Some(&cancel),
            observer: None,
        };
        let started = std::time::Instant::now();
        let flag = &cancel;
        let driver = run_with_retries(&config, &hooks, || async {
            Err::<ModelResponse, _>(ModelError::RateLimited {
                retry_after_ms: None,
            })
        });
        tokio::pin!(driver);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let canceller = async {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            };
            tokio::join!(driver, canceller).0
        })
        .await
        .expect("cancel must preempt the 30s fast sleep promptly");
        assert!(matches!(result, Err(ModelError::Cancelled)));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn reconnect_attempts_emit_typed_observations() {
        let observations = Mutex::new(Vec::new());
        let config = tiny_config();
        let hooks = CallHooks {
            cancel: None,
            observer: Some(&|obs| observations.lock().unwrap().push(obs)),
        };
        let calls = Mutex::new(0u32);
        let _ = run_with_retries(&config, &hooks, || {
            let calls = &calls;
            async move {
                *calls.lock().unwrap() += 1;
                if *calls.lock().unwrap() <= 2 {
                    Err(transport(TransportPhase::Connect))
                } else {
                    Ok(ModelResponse {
                        events: Vec::new(),
                        usage: Default::default(),
                        stop_reason: "stop".into(),
                    })
                }
            }
        })
        .await;
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        for (index, obs) in observations.iter().enumerate() {
            let ModelObservation::Reconnecting {
                attempt,
                next_delay_ms,
                deadline_remaining_ms,
            } = obs
            else {
                panic!("typed reconnect observation expected: {obs:?}");
            };
            assert_eq!(*attempt, index as u32 + 1);
            assert!(*next_delay_ms > 0);
            assert!(*deadline_remaining_ms > 0);
        }
    }
}
