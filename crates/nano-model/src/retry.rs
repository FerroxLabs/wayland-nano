//! Bounded provider retry (Kimi stepRetry policy, ported):
//! Retry-After honored first, exponential backoff 500ms x2 capped 32s with
//! 25% jitter, bounded attempts consuming the step budget.

use crate::types::ModelError;

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

pub fn is_retryable(err: &ModelError) -> Option<Option<u64>> {
    match err {
        ModelError::RateLimited { retry_after_ms } => Some(*retry_after_ms),
        ModelError::Server { status, .. } if *status >= 500 => Some(None),
        ModelError::Transport(_) => Some(None),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            policy.decide(0, &ModelError::Auth("bad key".into())),
            RetryAction::GiveUp
        );
        assert_eq!(policy.decide(0, &ModelError::Cancelled), RetryAction::GiveUp);
    }

    #[test]
    fn budget_exhaustion_gives_up() {
        let policy = RetryPolicy::default();
        let err = ModelError::Transport("reset".into());
        assert_eq!(
            policy.decide(policy.max_attempts, &err),
            RetryAction::GiveUp
        );
    }
}
