//! Loop protection: repeated-action breaker, no-progress detection, budgets.
//!
//! Provenance:
//! - mechanical repeat-breaker ported from Kimi Code `toolDedupeService.ts`
//!   (exact-args streak: reminders at 3/5/8, force stop at 12) — cheap,
//!   model-agnostic, test-covered semantics;
//! - no-progress detection is Nano-owned (no donor implements it): observable
//!   signals only — files changed, process outcome changed, new information;
//! - budgets per the plan (turns, tool calls, wall time).

use std::collections::BTreeMap;

/// Canonical key for a tool call: name + canonicalized arguments
/// (JSON object keys sorted recursively so {"a":1,"b":2} == {"b":2,"a":1}).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ToolCallKey(String);

impl ToolCallKey {
    pub fn new(name: &str, arguments: &serde_json::Value) -> Self {
        let canonical = canonical_json(arguments);
        Self(format!(
            "{name}:{}",
            serde_json::to_string(&canonical).unwrap_or_default()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonical_json(v)))
                .collect();
            let mut out = serde_json::Map::new();
            for (k, v) in sorted {
                out.insert(k, v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonical_json).collect())
        }
        other => other.clone(),
    }
}

/// What the loop should do about a repeated tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum RepeatAction {
    Allow,
    /// Inject this reminder to the model before the next step.
    Remind(String),
    /// Hard stop the turn with this reason.
    ForceStop(String),
}

const REMIND_AT: &[u32] = &[3, 5, 8];
const FORCE_STOP_STREAK: u32 = 12;

/// Kimi-mechanics repeat breaker: tracks the consecutive streak of the
/// current identical tool call. Resets whenever the key changes.
#[derive(Default)]
pub struct RepeatBreaker {
    current: Option<(ToolCallKey, u32)>,
}

impl RepeatBreaker {
    pub fn check(&mut self, key: &ToolCallKey) -> RepeatAction {
        let streak = match &mut self.current {
            Some((k, streak)) if k == key => {
                *streak += 1;
                *streak
            }
            slot => {
                *slot = Some((key.clone(), 1));
                1
            }
        };
        if streak >= FORCE_STOP_STREAK {
            return RepeatAction::ForceStop(format!(
                "identical tool call repeated {streak} times consecutively — stopping turn (force-stop at {FORCE_STOP_STREAK})"
            ));
        }
        if REMIND_AT.contains(&streak) {
            return RepeatAction::Remind(format!(
                "You have called the exact same tool with the exact same arguments {streak} times in a row. It returned the same result each time. Do something different: change the arguments, use a different tool, or explain what you are actually trying to achieve."
            ));
        }
        RepeatAction::Allow
    }
}

/// Observable progress signals for one loop step (external state only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgressSignals {
    pub files_changed: bool,
    pub process_outcome_changed: bool,
    pub new_information: bool,
}

impl ProgressSignals {
    pub fn any_progress(&self) -> bool {
        self.files_changed || self.process_outcome_changed || self.new_information
    }
}

/// What the loop should do about a no-progress streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressAction {
    Continue,
    Replan,
    Stop,
}

const REPLAN_AT_STREAK: u32 = 4;
const STOP_AT_STREAK: u32 = 6;

#[derive(Default)]
pub struct NoProgressTracker {
    zero_signal_streak: u32,
}

impl NoProgressTracker {
    pub fn observe(&mut self, signals: &ProgressSignals) -> ProgressAction {
        if signals.any_progress() {
            self.zero_signal_streak = 0;
            return ProgressAction::Continue;
        }
        self.zero_signal_streak += 1;
        if self.zero_signal_streak >= STOP_AT_STREAK {
            ProgressAction::Stop
        } else if self.zero_signal_streak >= REPLAN_AT_STREAK {
            ProgressAction::Replan
        } else {
            ProgressAction::Continue
        }
    }

    pub fn streak(&self) -> u32 {
        self.zero_signal_streak
    }
}

/// Per-turn budgets. Exceeding any of them is a typed stop, not a warning.
#[derive(Debug, Clone)]
pub struct TurnBudget {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub max_wall_time: std::time::Duration,
}

impl Default for TurnBudget {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_tool_calls: 100,
            max_wall_time: std::time::Duration::from_secs(1800),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetExhausted {
    Steps { used: u32, max: u32 },
    ToolCalls { used: u32, max: u32 },
    WallTime { elapsed_ms: u64, max_ms: u64 },
}

#[derive(Default)]
pub struct BudgetTracker {
    steps: u32,
    tool_calls: u32,
    started: Option<std::time::Instant>,
}

impl BudgetTracker {
    pub fn start_turn(&mut self) {
        self.started = Some(std::time::Instant::now());
        self.steps = 0;
        self.tool_calls = 0;
    }

    pub fn record_step(&mut self) {
        self.steps += 1;
    }

    pub fn record_tool_call(&mut self) {
        self.tool_calls += 1;
    }

    pub fn steps_count(&self) -> u32 {
        self.steps
    }

    pub fn tool_calls_count(&self) -> u32 {
        self.tool_calls
    }

    pub fn check(&self, budget: &TurnBudget) -> Result<(), BudgetExhausted> {
        // Wall time first: it exhausts independently of the counters.
        if let Some(started) = self.started {
            let elapsed = started.elapsed();
            if elapsed > budget.max_wall_time {
                return Err(BudgetExhausted::WallTime {
                    elapsed_ms: elapsed.as_millis() as u64,
                    max_ms: budget.max_wall_time.as_millis() as u64,
                });
            }
        }
        if self.steps >= budget.max_steps {
            return Err(BudgetExhausted::Steps {
                used: self.steps,
                max: budget.max_steps,
            });
        }
        if self.tool_calls >= budget.max_tool_calls {
            return Err(BudgetExhausted::ToolCalls {
                used: self.tool_calls,
                max: budget.max_tool_calls,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, args: serde_json::Value) -> ToolCallKey {
        ToolCallKey::new(name, &args)
    }

    #[test]
    fn canonical_key_ignores_argument_order() {
        let a = key("fs_read", serde_json::json!({"path": "x", "offset": 1}));
        let b = key("fs_read", serde_json::json!({"offset": 1, "path": "x"}));
        assert_eq!(a, b);
        let c = key("fs_read", serde_json::json!({"path": "y", "offset": 1}));
        assert_ne!(a, c);
    }

    #[test]
    fn streak_reminds_at_3_5_8_and_allows_otherwise() {
        let mut breaker = RepeatBreaker::default();
        let k = key("shell", serde_json::json!({"cmd": "cargo test"}));
        assert_eq!(breaker.check(&k), RepeatAction::Allow);
        assert_eq!(breaker.check(&k), RepeatAction::Allow);
        assert!(matches!(breaker.check(&k), RepeatAction::Remind(_))); // 3
        assert_eq!(breaker.check(&k), RepeatAction::Allow); // 4
        assert!(matches!(breaker.check(&k), RepeatAction::Remind(_))); // 5
        assert_eq!(breaker.check(&k), RepeatAction::Allow); // 6
        assert_eq!(breaker.check(&k), RepeatAction::Allow); // 7
        assert!(matches!(breaker.check(&k), RepeatAction::Remind(_))); // 8
    }

    #[test]
    fn force_stop_at_12_and_reset_on_key_change() {
        let mut breaker = RepeatBreaker::default();
        let k = key("fs_read", serde_json::json!({"path": "x"}));
        for _ in 0..11 {
            breaker.check(&k);
        }
        assert!(matches!(breaker.check(&k), RepeatAction::ForceStop(_)));

        let mut breaker = RepeatBreaker::default();
        for _ in 0..5 {
            breaker.check(&k);
        }
        let other = key("fs_write", serde_json::json!({"path": "x"}));
        assert_eq!(breaker.check(&other), RepeatAction::Allow);
        assert_eq!(
            breaker.check(&k),
            RepeatAction::Allow,
            "streak resets on key change"
        );
    }

    #[test]
    fn no_progress_streak_replans_then_stops() {
        let mut tracker = NoProgressTracker::default();
        let nothing = ProgressSignals::default();
        for expected in [
            ProgressAction::Continue,
            ProgressAction::Continue,
            ProgressAction::Continue,
            ProgressAction::Replan,
            ProgressAction::Replan,
            ProgressAction::Stop,
        ] {
            assert_eq!(tracker.observe(&nothing), expected);
        }

        let mut tracker = NoProgressTracker::default();
        tracker.observe(&nothing);
        tracker.observe(&nothing);
        let progress = ProgressSignals {
            files_changed: true,
            ..Default::default()
        };
        assert_eq!(tracker.observe(&progress), ProgressAction::Continue);
        assert_eq!(tracker.streak(), 0);
    }

    #[test]
    fn budgets_stop_typed() {
        let budget = TurnBudget {
            max_steps: 2,
            max_tool_calls: 1,
            max_wall_time: std::time::Duration::from_millis(10),
        };
        let mut tracker = BudgetTracker::default();
        tracker.start_turn();
        tracker.record_step();
        assert!(tracker.check(&budget).is_ok());
        tracker.record_tool_call();
        assert_eq!(
            tracker.check(&budget),
            Err(BudgetExhausted::ToolCalls { used: 1, max: 1 })
        );
        tracker.record_step();
        assert_eq!(
            tracker.check(&budget),
            Err(BudgetExhausted::Steps { used: 2, max: 2 })
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
        assert!(matches!(
            tracker.check(&budget),
            Err(BudgetExhausted::WallTime { .. })
        ));
    }

    /// P3 §3.2/§12: the repeat-breaker fires on identical re-searches —
    /// asserted, not trusted (tool_search is an ordinary tool call to the
    /// breaker: the LOADED status line says re-searching returns the same
    /// result, and the breaker is the hard backstop).
    #[test]
    fn p3_identical_tool_search_calls_trip_the_breaker() {
        let mut breaker = RepeatBreaker::default();
        let key = ToolCallKey::new("tool_search", &serde_json::json!({"query": "fs read"}));
        assert_eq!(breaker.check(&key), RepeatAction::Allow);
        assert_eq!(breaker.check(&key), RepeatAction::Allow);
        assert!(matches!(breaker.check(&key), RepeatAction::Remind(_)));
        for _ in 4..12 {
            let _ = breaker.check(&key);
        }
        assert!(matches!(breaker.check(&key), RepeatAction::ForceStop(_)));
    }
}
