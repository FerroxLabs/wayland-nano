//! Telemetry facade — replaces `codex-otel` for ported sandbox code.
//!
//! The donor wires OTel/Statsig product metrics through setup paths as an
//! optional hook (`Option<&StatsigMetricsSettings>`). Nano wants structured
//! events, not OTel product wiring: callers receive an optional sink and
//! emit named counter-style events with string fields. Absent sink = no-op,
//! matching the donor's library-path behavior (`otel: None`).
//!
//! This is original Nano code implementing the donor's *seam*, not a port
//! of codex-otel itself.

/// A minimal metrics/event sink. Implementations must never panic and never
/// log secret payloads; fields are pre-scrubbed by callers.
pub trait MetricsSink: Send + Sync {
    fn emit(&self, metric: &str, fields: &[(&str, &str)]);
}

/// Optional metrics hook, mirroring the donor's `Option<&StatsigMetricsSettings>`.
pub type MetricsHook<'a> = Option<&'a dyn MetricsSink>;

/// Emit a metric if a sink is installed; otherwise a no-op.
pub fn emit_safely(hook: MetricsHook<'_>, metric: &str, fields: &[(&str, &str)]) {
    if let Some(sink) = hook {
        sink.emit(metric, fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Recorded = (String, Vec<(String, String)>);
    struct Recording(Mutex<Vec<Recorded>>);
    impl MetricsSink for Recording {
        fn emit(&self, metric: &str, fields: &[(&str, &str)]) {
            self.0.lock().unwrap().push((
                metric.to_string(),
                fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
        }
    }

    #[test]
    fn absent_sink_is_noop() {
        emit_safely(None, "sandbox.test", &[("k", "v")]); // must not panic
    }

    #[test]
    fn present_sink_receives_metric_and_fields() {
        let rec = Recording(Mutex::new(Vec::new()));
        emit_safely(Some(&rec), "sandbox.setup.start", &[("identity", "offline")]);
        let got = rec.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "sandbox.setup.start");
        assert_eq!(got[0].1, vec![("identity".to_string(), "offline".to_string())]);
    }
}

/// Serializable telemetry settings carried on the setup wire (orchestrator →
/// elevated helper), replacing the donor's `StatsigMetricsSettings` field.
/// `None` on all current paths — the helper emits through the facade only
/// when settings are present.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TelemetrySettings {
    pub environment: String,
    pub service_name: String,
}

/// Global telemetry settings for setup payloads. None by default — Nano does
/// not wire product analytics into provisioning.
pub fn global_telemetry_settings() -> Option<TelemetrySettings> {
    None
}
