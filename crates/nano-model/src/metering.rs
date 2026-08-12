//! P1 usage-metering seam (design note §3.2): the `UsageSink` handle every
//! token-bearing model call feeds — turn responses AND the Flux web_search
//! grounding round-trip alike.
//!
//! Lane split: Lane B owns the session `CostMeter` (pricing, atomic output
//! reservations, the turn-scoped accumulator). This module fixes the
//! SHARED signature both lanes code against, plus a feature-complete local
//! stub ([`StubCostMeter`]) that accumulates real records so the seam is
//! exercisable end-to-end until Lane B's meter lands. The stub is a
//! stand-in for the seam ONLY — it has no pricing, no reservations, and no
//! cap authority; nothing budget-bearing may rely on it.

use crate::types::Usage;

/// One recorded model call's usage with its provenance (design §3.2).
/// Numbers and bounded strings only — never content.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub usage: Usage,
    /// The model that produced the usage: the session model for turn
    /// responses, `flux-fast` (pinned, Q3) for the grounding round-trip.
    pub model: String,
    /// The owning tool call id when the usage came from a tool's internal
    /// model call (the web_search grounding round-trip is recorded against
    /// the search tool call id); `None` for ordinary turn responses.
    pub tool_call_id: Option<String>,
    /// False when the wire carried NO usage for the call: the meter then
    /// applies the §3.5 conservative estimate (never zero), with journaled
    /// provenance. True when `usage` is provider-reported.
    pub reported: bool,
}

/// The session meter handle (design §3.2/§2.5). Threaded into the turn
/// engine (per response, per step — never last-response-only) and into
/// `RealToolExecutor` beside the web_search slot (`with_web_search`).
pub trait UsageSink: Send + Sync {
    fn record_usage(&self, record: &UsageRecord);
}

/// The dual feed (r3 codex-F1): one grounding-usage record feeds BOTH the
/// session meter and the owning turn's `record_usage` accumulator, so the
/// live meter, the journaled turn sum, and replay reconstruction agree —
/// searches included. Construction order is preserved on the feed.
#[derive(Default)]
pub struct FanoutUsageSink {
    sinks: Vec<std::sync::Arc<dyn UsageSink>>,
}

impl FanoutUsageSink {
    pub fn new(sinks: Vec<std::sync::Arc<dyn UsageSink>>) -> Self {
        Self { sinks }
    }

    pub fn push(&mut self, sink: std::sync::Arc<dyn UsageSink>) {
        self.sinks.push(sink);
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl std::fmt::Debug for FanoutUsageSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanoutUsageSink")
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl UsageSink for FanoutUsageSink {
    fn record_usage(&self, record: &UsageRecord) {
        for sink in &self.sinks {
            sink.record_usage(record);
        }
    }
}

/// Feature-complete local stub for the P1 seam (see module docs): a real
/// accumulating meter minus pricing/reservations/cap authority. Lane B's
/// session `CostMeter` replaces it at the wiring sites.
#[derive(Debug, Default)]
pub struct StubCostMeter {
    records: std::sync::Mutex<Vec<UsageRecord>>,
}

impl StubCostMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every record fed so far, in feed order (test/diagnostic surface).
    pub fn records(&self) -> Vec<UsageRecord> {
        self.records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn total_input_tokens(&self) -> u64 {
        self.records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|r| r.usage.input_tokens)
            .sum()
    }

    pub fn total_output_tokens(&self) -> u64 {
        self.records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|r| r.usage.output_tokens)
            .sum()
    }
}

impl UsageSink for StubCostMeter {
    fn record_usage(&self, record: &UsageRecord) {
        self.records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(input: u64, output: u64) -> UsageRecord {
        UsageRecord {
            usage: Usage {
                input_tokens: input,
                output_tokens: output,
                ..Usage::default()
            },
            model: "flux-fast".into(),
            tool_call_id: Some("call-1".into()),
            reported: true,
        }
    }

    #[test]
    fn stub_accumulates_real_records() {
        let meter = StubCostMeter::new();
        meter.record_usage(&record(10, 5));
        meter.record_usage(&record(20, 7));
        assert_eq!(meter.total_input_tokens(), 30);
        assert_eq!(meter.total_output_tokens(), 12);
        assert_eq!(meter.records().len(), 2);
        assert_eq!(meter.records()[0].model, "flux-fast");
    }

    /// r3 codex-F1: the fanout feeds BOTH sinks from the one record.
    #[test]
    fn fanout_feeds_every_sink_with_the_same_record() {
        let a = std::sync::Arc::new(StubCostMeter::new());
        let b = std::sync::Arc::new(StubCostMeter::new());
        let fanout = FanoutUsageSink::new(vec![a.clone(), b.clone()]);
        assert!(!fanout.is_empty());
        fanout.record_usage(&record(10, 5));
        assert_eq!(a.records(), b.records());
        assert_eq!(a.total_input_tokens(), 10);
        assert!(FanoutUsageSink::default().is_empty());
    }
}
