//! Session cost meter (P1 §3.2, §4 — panel-certified design
//! `shared/reviews/panel-tui/P1-search-economy-design.md`).
//!
//! One session-scoped [`CostMeter`] (`Arc`-shared state — the same ownership
//! pattern as the C9 steer queue and the C6 shared steps counter,
//! `tasks.rs`) is the SOLE budget authority: provider-reported `cost_usd`
//! stays observability-only and is never mixed into the meter's
//! microcents (§3.2).
//!
//! Hard-cap enforcement is an ATOMIC output reservation under the meter lock
//! before EVERY parent/child/grounding model request (§4.2):
//! `reserve_output(requested) -> Reservation` deducts
//! `granted = min(requested, remaining)` atomically; the clamped grant
//! becomes the request's `max_tokens`; a zero grant is the §4.1 hard stop —
//! never a zero-token request. Every outcome settles: success charges actual
//! input + output and refunds the unspent grant; failure/cancel/missing
//! usage takes the §3.5 conservative charge (input estimate + FULL reserved
//! output — no refund without evidence); an unsettled reservation settles
//! conservatively when its scope ends (a dropped/panicked request cannot
//! leak allowance silently).
//!
//! The guarantee is stated honestly (§4.2): the cap is OUTPUT-BOUNDED,
//! INPUT-BEST-EFFORT. Input tokens are unclampable and can push the meter
//! past the cap; the overshoot bound is the aggregate unclampable INPUT of
//! all concurrently reserved in-flight requests (bounded by the C6 fan-out
//! cap of 4 children plus the parent) plus their clamped (reserved) OUTPUT.
//! Once the meter crosses the cap, further reservations grant zero and every
//! subsequent turn hard-stops. Nothing stronger is claimed.

use nano_model::pricing::PricingCatalog;
use nano_model::types::Usage;
use nano_session::op::{ESTIMATION_METHOD_VERSION, TurnUsage};
use std::sync::{Arc, Mutex, MutexGuard};

/// The UsageSink seam (P1 §3.2) is `nano_model::metering::UsageSink` — the
/// SHARED interface both P1 lanes code against: every model response's
/// usage is recorded PER RESPONSE, never the turn's last-response-only
/// (`TurnResult.usage` is explicitly the last response, `turn.rs`). The
/// session meter implements it below; the Flux grounding path (P1 §2.2,
/// search lane) records through it against the search tool call id so a
/// search-heavy turn cannot evade the session cap.
use nano_model::metering::UsageRecord;

/// The 80% warn payload (P1 §4.1): a typed notice carrying
/// `{limit, observed, pct_used}`. Latest-wins observability; fires once per
/// crossing (re-arms when a grant lifts the effective limit back above the
/// observed total).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetWarn {
    pub limit: u64,
    pub observed: u64,
    pub pct_used: u64,
}

/// A point-in-time read of the meter's budget position (surfaces, tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetState {
    /// The effective limit: configured cap + accepted grants.
    pub limit: u64,
    /// Settled (charged) tokens against it.
    pub observed: u64,
    /// Outstanding reserved output (in-flight requests).
    pub reserved: u64,
}

impl BudgetState {
    pub fn pct_used(&self) -> u64 {
        if self.limit == 0 {
            return 100;
        }
        self.observed.saturating_mul(100) / self.limit
    }
}

/// The warn threshold (P1 §4.1).
const WARN_PCT: u64 = 80;

/// S10 soak fix: the §3.5 median's sample window. The median estimates a
/// TYPICAL recent response size (the conservative charge for a request that
/// under-reported), so a sliding window of the most recent samples keeps the
/// semantics while bounding retention — the old unbounded Vec grew one
/// `u64` per model response for the process lifetime.
const SAMPLE_WINDOW: usize = 64;

#[derive(Debug, Default)]
struct MeterState {
    /// Settled usage totals (token classes, microcents, priced, §3.5
    /// provenance) — the same shape the journal's `TurnUsage` sums carry,
    /// so live meter == journaled sum == replay reconstruction.
    charged: TurnUsage,
    /// Configured session cap (`[budget] session_tokens`); None = uncapped
    /// (back-compat default).
    cap: Option<u64>,
    /// Accepted `/budget continue` grants (P1 §4.3).
    granted_tokens: u64,
    /// Outstanding reserved output across all in-flight reservations.
    reserved_output: u64,
    /// Per-response totals feeding the §3.5 median — the MOST RECENT
    /// [`SAMPLE_WINDOW`] samples of this process (a reseeded meter starts
    /// empty — median 0 until responses accrue). Windowed, not lifetime:
    /// the estimate tracks the session's current response-shape, never
    /// grows unbounded.
    samples: std::collections::VecDeque<u64>,
    /// The 80% warn fired for the current effective-limit position.
    warned: bool,
    /// A warn crossing awaiting emission (drained by the engine, which owns
    /// the observer channel).
    pending_warn: Option<BudgetWarn>,
}

impl MeterState {
    fn effective_limit(&self) -> Option<u64> {
        self.cap.map(|cap| cap.saturating_add(self.granted_tokens))
    }

    /// Re-evaluate the 80% crossing after any charge/grant.
    fn refresh_warn(&mut self) {
        let Some(limit) = self.effective_limit() else {
            self.pending_warn = None;
            self.warned = false;
            return;
        };
        let observed = self.charged.total_tokens();
        let pct = observed
            .saturating_mul(100)
            .checked_div(limit)
            .unwrap_or(100);
        if pct < WARN_PCT {
            // Back below the threshold (a grant lifted the limit): re-arm.
            self.warned = false;
            self.pending_warn = None;
        } else if !self.warned {
            self.warned = true;
            self.pending_warn = Some(BudgetWarn {
                limit,
                observed,
                pct_used: pct,
            });
        }
    }

    fn record_sample(&mut self, total: u64) {
        self.samples.push_back(total);
        while self.samples.len() > SAMPLE_WINDOW {
            self.samples.pop_front();
        }
    }

    fn median_sample(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }
}

/// The session-scoped cost meter: cheap to clone (Arc-shared state), safe to
/// share into C6 child contexts beside the steps counter.
pub struct CostMeter {
    provider: String,
    catalog: Arc<PricingCatalog>,
    state: Arc<Mutex<MeterState>>,
}

impl std::fmt::Debug for CostMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostMeter")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl Clone for CostMeter {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            catalog: self.catalog.clone(),
            state: self.state.clone(),
        }
    }
}

impl CostMeter {
    pub fn new(
        provider: impl Into<String>,
        catalog: Arc<PricingCatalog>,
        cap: Option<u64>,
    ) -> Self {
        Self {
            provider: provider.into(),
            catalog,
            state: Arc::new(Mutex::new(MeterState {
                cap,
                ..Default::default()
            })),
        }
    }

    /// Rebind the pricing provider WITHOUT cloning the shared state: the
    /// returned meter shares totals/reservations with `self` but prices
    /// records against `provider`. Per-turn the engine rebinds to the
    /// turn's resolved binding provider (C8), so a session that switched
    /// providers prices against the right table section (an absent row is
    /// unpriced — honest, never a wrong price).
    pub fn with_provider(&self, provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            catalog: self.catalog.clone(),
            state: self.state.clone(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, MeterState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Price a usage record against the catalog: an unknown model row
    /// resolves `priced: false` (absence is never $0, P1 §3.1).
    fn price(&self, model: &str, input: u64, output: u64, cached: u64) -> (u64, bool) {
        match self.catalog.estimate_cost_with_cache_status(
            &self.provider,
            model,
            input,
            output,
            cached,
            0,
        ) {
            Ok(status) => (status.microcents, status.priced),
            Err(_) => (0, false),
        }
    }

    /// Record provider-reported usage (settled, un-reserved path — the
    /// [`nano_model::metering::UsageSink`] feed). Returns the recorded
    /// delta so the caller's turn-scoped accumulator sums EXACTLY what the
    /// meter charged.
    pub fn record_usage(&self, model: &str, usage: &Usage) -> TurnUsage {
        let cached = usage.cached_input_tokens.unwrap_or(0);
        let reasoning = usage.reasoning_tokens.unwrap_or(0);
        let (microcents, priced) =
            self.price(model, usage.input_tokens, usage.output_tokens, cached);
        let mut delta = TurnUsage::default();
        delta.add_provider_reported(
            usage.input_tokens,
            usage.output_tokens,
            cached,
            reasoning,
            microcents,
            priced,
        );
        let mut state = self.lock();
        state.charged.add_sum(&delta);
        state.record_sample(usage.input_tokens.saturating_add(usage.output_tokens));
        state.refresh_warn();
        delta
    }

    /// Atomically reserve output allowance before a model request (P1 §4.2):
    /// deducts `granted = min(requested, remaining)` under the meter lock.
    /// The grant becomes the request's `max_tokens`; `granted() == 0` is the
    /// §4.1 hard stop — the caller must NOT issue a zero-token request.
    pub fn reserve_output(&self, requested: u64) -> Reservation {
        let mut state = self.lock();
        let granted = match state.effective_limit() {
            None => requested,
            Some(limit) => {
                let remaining = limit
                    .saturating_sub(state.charged.total_tokens())
                    .saturating_sub(state.reserved_output);
                requested.min(remaining)
            }
        };
        state.reserved_output = state.reserved_output.saturating_add(granted);
        Reservation {
            meter: self.clone(),
            granted,
            requested,
            settled: false,
        }
    }

    /// The current budget position (None = uncapped).
    pub fn budget_state(&self) -> Option<BudgetState> {
        let state = self.lock();
        state.effective_limit().map(|limit| BudgetState {
            limit,
            observed: state.charged.total_tokens(),
            reserved: state.reserved_output,
        })
    }

    /// Drain a pending 80% warn crossing (the engine owns the observer
    /// channel and calls this after every settle).
    pub fn take_pending_warn(&self) -> Option<BudgetWarn> {
        self.lock().pending_warn.take()
    }

    /// The session usage totals so far (status surfaces, child rollups).
    pub fn session_usage(&self) -> TurnUsage {
        self.lock().charged.clone()
    }

    /// Apply an ACCEPTED `/budget continue` grant (P1 §4.3) — called by the
    /// host ONLY after `Op::BudgetGrant` landed durably (journal-first).
    /// Returns the new effective limit; None (typed error upstream) when the
    /// session is uncapped.
    pub fn apply_grant(&self, tokens: u64) -> Option<u64> {
        let mut state = self.lock();
        state.cap?;
        state.granted_tokens = state.granted_tokens.saturating_add(tokens);
        state.refresh_warn();
        state.effective_limit()
    }

    /// Re-seed from replay on session/load (P1 §3.3/§4.3): the meter totals
    /// are RECONSTRUCTED from `TurnEnd.usage` + `ChildUsageRollup` (plus the
    /// orphan-child fold) and grants from `Op::BudgetGrant`, so kill-resume
    /// restores the exact budget position.
    pub fn reseed(&self, usage: &TurnUsage, granted_tokens: u64) {
        let mut state = self.lock();
        state.charged = usage.clone();
        state.granted_tokens = granted_tokens;
        state.warned = false;
        state.pending_warn = None;
    }

    /// Settle a reservation on success: charge the ACTUAL input + output and
    /// release the unspent grant back to the allowance (refund). Returns the
    /// recorded delta for the turn-scoped accumulator.
    fn settle_success(&self, granted: u64, model: &str, usage: &Usage) -> TurnUsage {
        let delta = self.record_usage(model, usage);
        let mut state = self.lock();
        state.reserved_output = state.reserved_output.saturating_sub(granted);
        state.refresh_warn();
        delta
    }

    /// Settle a reservation conservatively (P1 §3.5, Q4 RULED): the wire
    /// reported no usage (or the request failed/was cancelled with no
    /// evidence), so charge the GREATER of (a) the request's measured input
    /// estimate plus the FULL reserved output, or (b) the session's median
    /// per-response usage — never zero, no refund without evidence. The
    /// charge carries auditable provenance into the journaled sum.
    fn settle_conservative(&self, granted: u64, model: &str, input_estimate: u64) -> TurnUsage {
        let mut state = self.lock();
        state.reserved_output = state.reserved_output.saturating_sub(granted);
        if granted == 0 && input_estimate == 0 {
            // A zero-grant reservation is the hard stop — NO request was
            // issued, so nothing is charged (the median floor applies only
            // to a request that actually went out and under-reported).
            return TurnUsage::default();
        }
        let request_total = input_estimate.saturating_add(granted);
        let applied = request_total.max(state.median_sample());
        // Attribute the applied charge as the measured input plus output
        // covering the remainder, so charged.total_tokens() == applied.
        let output_charge = applied.saturating_sub(input_estimate);
        let (microcents, priced) = self.price(model, input_estimate, output_charge, 0);
        let mut delta = TurnUsage::default();
        delta.add_estimated(
            input_estimate,
            output_charge,
            microcents,
            priced,
            ESTIMATION_METHOD_VERSION,
            applied,
        );
        state.charged.add_sum(&delta);
        state.refresh_warn();
        delta
    }
    /// Record a §3.5 conservative charge for a call whose wire carried NO
    /// usage (the metering seam's `reported: false` path): the record's
    /// token counts when present, floored at the session's median
    /// per-response sample — never zero — with auditable estimation
    /// provenance. Returns the recorded delta (same contract as
    /// [`Self::record_usage`]).
    pub fn record_estimated(&self, model: &str, usage: &Usage) -> TurnUsage {
        let input = usage.input_tokens;
        let output = usage.output_tokens;
        let mut state = self.lock();
        let applied = input.saturating_add(output).max(state.median_sample());
        // Attribute the applied charge as the measured input plus output
        // covering the remainder, so charged.total_tokens() == applied.
        let output_charge = applied.saturating_sub(input);
        let (microcents, priced) = self.price(model, input, output_charge, 0);
        let mut delta = TurnUsage::default();
        delta.add_estimated(
            input,
            output_charge,
            microcents,
            priced,
            ESTIMATION_METHOD_VERSION,
            applied,
        );
        state.charged.add_sum(&delta);
        state.refresh_warn();
        delta
    }
}

impl nano_model::metering::UsageSink for CostMeter {
    /// The shared P1 seam (`nano_model::metering::UsageSink`): one record
    /// feeds the session meter — provider-reported usage charges actuals;
    /// an unreported wire takes the §3.5 conservative charge (never zero).
    fn record_usage(&self, record: &UsageRecord) {
        if record.reported {
            self.record_usage(&record.model, &record.usage);
        } else {
            self.record_estimated(&record.model, &record.usage);
        }
    }
}

/// The r3 codex-F1 dual feed (P1 §3.2): one [`UsageRecord`] charges the
/// session meter AND lands the EXACT charged delta in the owning turn's
/// accumulator cell (drained into `TurnEnd.usage` /
/// `ChildUsageRollup.usage` before terminal journaling), so live meter ==
/// journaled sum == replay reconstruction, searches included. Wired as the
/// search sink beside every `with_web_search` meter site.
pub struct MeteringTurnSink {
    meter: CostMeter,
    cell: Arc<Mutex<TurnUsage>>,
}

impl MeteringTurnSink {
    pub fn new(meter: CostMeter, cell: Arc<Mutex<TurnUsage>>) -> Self {
        Self { meter, cell }
    }
}

impl std::fmt::Debug for MeteringTurnSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeteringTurnSink")
            .field("meter", &self.meter)
            .finish_non_exhaustive()
    }
}

impl nano_model::metering::UsageSink for MeteringTurnSink {
    fn record_usage(&self, record: &UsageRecord) {
        let delta = if record.reported {
            self.meter.record_usage(&record.model, &record.usage)
        } else {
            self.meter.record_estimated(&record.model, &record.usage)
        };
        self.cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .add_sum(&delta);
    }
}

/// An outstanding output reservation (P1 §4.2): scoped — a reservation
/// dropped without an explicit settle settles CONSERVATIVELY (full grant
/// charged, no refund), so a dropped/panicked request cannot leak allowance
/// silently.
pub struct Reservation {
    meter: CostMeter,
    granted: u64,
    requested: u64,
    settled: bool,
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("granted", &self.granted)
            .field("requested", &self.requested)
            .field("settled", &self.settled)
            .finish()
    }
}

impl Reservation {
    /// The clamped allowance — becomes the request's `max_tokens`. Zero is
    /// the §4.1 hard stop: never issue a zero-token request.
    pub fn granted(&self) -> u64 {
        self.granted
    }

    /// What the caller asked for (clamp notices compare against `granted`).
    pub fn requested(&self) -> u64 {
        self.requested
    }

    /// Success: charge actual input + output, refund the unspent grant.
    /// Returns the recorded delta for the turn-scoped accumulator.
    pub fn settle_success(&mut self, model: &str, usage: &Usage) -> TurnUsage {
        self.settled = true;
        self.meter.settle_success(self.granted, model, usage)
    }

    /// Failure/cancel/missing-usage: the §3.5 conservative charge (input
    /// estimate + FULL reserved output, no refund). Returns the recorded
    /// delta (with provenance) for the turn-scoped accumulator.
    pub fn settle_conservative(&mut self, model: &str, input_estimate: u64) -> TurnUsage {
        self.settled = true;
        self.meter
            .settle_conservative(self.granted, model, input_estimate)
    }
}

impl Drop for Reservation {
    /// An unsettled reservation at scope end settles conservatively: the
    /// full grant is charged (the §3.5 formula with a zero input estimate),
    /// never silently returned to the allowance.
    fn drop(&mut self) {
        if !self.settled {
            let meter = self.meter.clone();
            meter.settle_conservative(self.granted, "unknown", 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Arc<PricingCatalog> {
        let raw = r#"
[metered.model]
input_per_mtok_usd = 1.0
output_per_mtok_usd = 2.0
"#;
        Arc::new(PricingCatalog::from_toml_str(raw).unwrap())
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        }
    }

    #[test]
    fn reserve_clamps_to_remaining_and_settle_refunds() {
        let meter = CostMeter::new("metered", catalog(), Some(1000));
        let mut res = meter.reserve_output(400);
        assert_eq!(res.granted(), 400);
        // Charge 100 in + 100 out: refund 300 of the grant.
        let delta = res.settle_success("model", &usage(100, 100));
        assert_eq!(delta.input_tokens, 100);
        assert_eq!(delta.output_tokens, 100);
        let state = meter.budget_state().unwrap();
        assert_eq!(state.observed, 200);
        assert_eq!(state.reserved, 0);
        // Next reservation sees the settled remainder: 800 left.
        let res = meter.reserve_output(1000);
        assert_eq!(res.granted(), 800);
    }

    /// §4.2 atomicity: concurrent reservations never collectively overshoot
    /// the allowance (check-and-deduct under the one lock).
    #[test]
    fn concurrent_reservations_never_exceed_the_allowance() {
        let meter = CostMeter::new("metered", catalog(), Some(500));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let meter = meter.clone();
            handles.push(std::thread::spawn(move || {
                let res = meter.reserve_output(200);
                let granted = res.granted();
                // Keep the reservation outstanding (drop settles
                // conservatively after the assertion boundary).
                std::mem::forget(res);
                granted
            }));
        }
        let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(
            total <= 500,
            "aggregate granted output must never exceed the allowance"
        );
        // 2 full grants of 200 + one partial of 100, the rest zero.
        assert_eq!(total, 500);
    }

    /// A zero grant is the hard stop signal; nothing is charged for it.
    #[test]
    fn zero_grant_is_the_hard_stop() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        let mut res = meter.reserve_output(100);
        res.settle_success("model", &usage(50, 50));
        let res = meter.reserve_output(10);
        assert_eq!(res.granted(), 0);
        drop(res);
        // The dropped zero-grant reservation charges nothing.
        assert_eq!(meter.budget_state().unwrap().observed, 100);
    }

    /// Failure/cancel/missing usage: full grant charged, no refund (§3.5).
    #[test]
    fn conservative_settle_charges_full_grant_with_provenance() {
        let meter = CostMeter::new("metered", catalog(), Some(10_000));
        let mut res = meter.reserve_output(400);
        let delta = res.settle_conservative("model", 300);
        assert_eq!(delta.input_tokens, 300);
        assert_eq!(delta.output_tokens, 400);
        assert_eq!(delta.usage_source, nano_session::op::UsageSource::Estimated);
        assert_eq!(delta.applied_estimate, Some(700));
        assert_eq!(
            meter.budget_state().unwrap().observed,
            700,
            "input estimate + FULL reserved output, no refund"
        );
    }

    /// §3.5 (Q4): the session median beats the request estimate when larger.
    #[test]
    fn conservative_settle_takes_the_median_floor() {
        let meter = CostMeter::new("metered", catalog(), Some(1_000_000));
        // Seed a median of 1000 per response.
        meter.record_usage("model", &usage(600, 400));
        let mut res = meter.reserve_output(100);
        let delta = res.settle_conservative("model", 50);
        assert_eq!(
            delta.applied_estimate,
            Some(1000),
            "max(request estimate + reserved, session median)"
        );
    }

    /// S10 soak fix: the §3.5 sample window is BOUNDED (no per-process-
    /// lifetime growth) and the median tracks the most recent window —
    /// the "typical recent response" semantics, never a stale lifetime one.
    #[test]
    fn median_samples_are_bounded_to_the_recent_window() {
        let meter = CostMeter::new("metered", catalog(), None);
        // 2× the window of small samples, then a window of large ones.
        for _ in 0..(SAMPLE_WINDOW * 2) {
            meter.record_usage("model", &usage(10, 0));
        }
        for _ in 0..SAMPLE_WINDOW {
            meter.record_usage("model", &usage(500, 500));
        }
        let state = meter.lock();
        assert_eq!(state.samples.len(), SAMPLE_WINDOW, "bounded retention");
        assert_eq!(
            state.median_sample(),
            1000,
            "the median tracks the most recent window only"
        );
    }

    /// An unsettled reservation settles conservatively at scope end (Drop):
    /// the full grant is charged, never silently leaked back.
    #[test]
    fn unsettled_reservation_charges_full_grant_on_drop() {
        let meter = CostMeter::new("metered", catalog(), Some(10_000));
        {
            let res = meter.reserve_output(500);
            assert_eq!(meter.budget_state().unwrap().reserved, 500);
            drop(res);
        }
        let state = meter.budget_state().unwrap();
        assert_eq!(state.reserved, 0);
        assert_eq!(state.observed, 500);
        assert_eq!(
            meter.session_usage().usage_source,
            nano_session::op::UsageSource::Estimated
        );
    }

    /// Uncapped meter: reservations always grant fully, no budget state.
    #[test]
    fn uncapped_meter_never_clamps() {
        let meter = CostMeter::new("metered", catalog(), None);
        let res = meter.reserve_output(u64::MAX / 2);
        assert_eq!(res.granted(), u64::MAX / 2);
        assert!(meter.budget_state().is_none());
    }

    /// 80% warn fires once per crossing and re-arms after a grant.
    #[test]
    fn warn_fires_once_per_crossing_and_rearms_after_grant() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        meter.record_usage("model", &usage(50, 29));
        assert!(
            meter.take_pending_warn().is_none(),
            "79%: below the warn line"
        );
        meter.record_usage("model", &usage(6, 5));
        let warn = meter.take_pending_warn().expect("80% crossing fires");
        assert_eq!(warn.limit, 100);
        assert_eq!(warn.observed, 90);
        assert_eq!(warn.pct_used, 90);
        assert!(meter.take_pending_warn().is_none(), "once per crossing");
        // Grant lifts the limit past the observed total: re-armed.
        assert_eq!(meter.apply_grant(100), Some(200));
        meter.record_usage("model", &usage(100, 0));
        let warn = meter.take_pending_warn().expect("re-crossing fires again");
        assert_eq!(warn.limit, 200);
        assert_eq!(warn.observed, 190);
    }

    /// Grant bookkeeping: after_limit = cap + grants; uncapped rejects.
    #[test]
    fn grant_raises_the_effective_limit() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        assert_eq!(meter.apply_grant(50), Some(150));
        assert_eq!(meter.apply_grant(50), Some(200));
        let uncapped = CostMeter::new("metered", catalog(), None);
        assert_eq!(uncapped.apply_grant(50), None);
    }

    /// Kill-resume: reseed reconstructs the exact budget position.
    #[test]
    fn reseed_restores_the_budget_position() {
        let meter = CostMeter::new("metered", catalog(), Some(1000));
        let mut restored = TurnUsage::default();
        restored.add_provider_reported(400, 300, 0, 0, 0, false);
        meter.reseed(&restored, 200);
        let state = meter.budget_state().unwrap();
        assert_eq!(state.limit, 1200);
        assert_eq!(state.observed, 700);
        // Reservations account against the restored position.
        let res = meter.reserve_output(600);
        assert_eq!(res.granted(), 500);
    }

    /// Provider-reported `cost_usd` is NEVER mixed into meter microcents:
    /// only the catalog price counts (the meter is the budget authority).
    #[test]
    fn provider_cost_usd_never_enters_the_meter() {
        let meter = CostMeter::new("metered", catalog(), None);
        let delta = meter.record_usage(
            "model",
            &Usage {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                cost_usd: Some(999.99),
                ..Default::default()
            },
        );
        // 1M in @ $1/Mtok + 1M out @ $2/Mtok = $3.00 = 300M microcents —
        // the wire's 999.99 plays no role.
        assert_eq!(delta.microcents, 300_000_000);
        assert!(delta.priced);
    }

    /// An unknown model row prices as unpriced, never $0 (P1 §3.1).
    #[test]
    fn unknown_model_is_unpriced() {
        let meter = CostMeter::new("metered", catalog(), None);
        let delta = meter.record_usage("ghost-model", &usage(10, 10));
        assert!(!delta.priced);
        assert_eq!(delta.microcents, 0);
    }
}
