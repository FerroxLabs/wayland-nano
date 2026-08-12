//! Clock abstraction (C11): every time-dependent trigger (goal wall-clock
//! budgets, cron ticks) drives a `Clock`, never the wall clock directly —
//! wcore's explicit anti-flake rule (`wcore-cron/src/runner.rs`), adopted
//! verbatim.

/// Millisecond-resolution clock. `SystemClock` in production; tests drive a
/// `TestClock`.
pub trait Clock: std::fmt::Debug + Send + Sync {
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Fully deterministic test clock: time advances only when the test says so.
#[derive(Debug, Default)]
pub struct TestClock {
    now: std::sync::atomic::AtomicU64,
}

impl TestClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(start_ms),
        }
    }

    pub fn advance_ms(&self, delta: u64) {
        self.now
            .fetch_add(delta, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}
