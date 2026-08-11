//! WFP filter installation with metrics and panic containment.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/wfp_setup.rs` @
//! 646f7c0a. Transformations: OTel/Statsig provider -> telemetry facade
//! `MetricsHook` (D-facade); metric names rebranded
//! `codex.*` -> `nano.*`; codex_home -> nano_home.

use crate::setup_error::sanitize_setup_metric_tag_value;
use crate::telemetry;
use crate::wfp::install_wfp_filters_for_account;
use crate::wfp::uninstall_wfp_filters as remove_wfp_objects;
use anyhow::Result;
use std::path::Path;

const WFP_SETUP_SUCCESS_METRIC: &str = "nano.windows_sandbox.wfp_setup_success";
const WFP_SETUP_FAILURE_METRIC: &str = "nano.windows_sandbox.wfp_setup_failure";
const WFP_UNINSTALL_SUCCESS_METRIC: &str = "nano.windows_sandbox.wfp_uninstall_success";
const WFP_UNINSTALL_FAILURE_METRIC: &str = "nano.windows_sandbox.wfp_uninstall_failure";

#[derive(Debug, Clone, Copy)]
enum WfpSetupMetricOutcome {
    Success,
    Failure,
}

struct WfpSetupMetric {
    outcome: WfpSetupMetricOutcome,
    target_account: String,
    installed_filter_count: usize,
    error: Option<String>,
}

fn panic_payload_to_string(panic_payload: Box<dyn std::any::Any + Send>) -> String {
    match panic_payload.downcast::<String>() {
        Ok(message) => *message,
        Err(panic_payload) => match panic_payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "unknown panic payload".to_string(),
        },
    }
}

fn emit_wfp_setup_metric(
    metrics_hook: telemetry::MetricsHook<'_>,
    metric: &WfpSetupMetric,
) -> Result<()> {
    emit_wfp_metric(
        metrics_hook,
        metric,
        WFP_SETUP_SUCCESS_METRIC,
        WFP_SETUP_FAILURE_METRIC,
    )
}

fn emit_wfp_metric(
    metrics_hook: telemetry::MetricsHook<'_>,
    metric: &WfpSetupMetric,
    success_metric: &str,
    failure_metric: &str,
) -> Result<()> {
    let target_account = sanitize_setup_metric_tag_value(&metric.target_account);
    match metric.outcome {
        WfpSetupMetricOutcome::Success => {
            let installed_filter_count = metric.installed_filter_count.to_string();
            telemetry::emit_safely(
                metrics_hook,
                success_metric,
                &[
                    ("target_account", target_account.as_str()),
                    ("installed_filter_count", installed_filter_count.as_str()),
                ],
            );
        }
        WfpSetupMetricOutcome::Failure => {
            let error_tag = metric.error.as_deref().map(sanitize_setup_metric_tag_value);
            let error_owned;
            let tags: &[(&str, &str)] = match error_tag.as_deref() {
                Some(error) => {
                    error_owned = error.to_string();
                    &[
                        ("target_account", target_account.as_str()),
                        ("message", error_owned.as_str()),
                    ]
                }
                None => &[("target_account", target_account.as_str())],
            };
            telemetry::emit_safely(metrics_hook, failure_metric, tags);
        }
    }
    Ok(())
}

fn emit_wfp_setup_metric_safely<F>(
    metrics_hook: telemetry::MetricsHook<'_>,
    offline_username: &str,
    metric: &WfpSetupMetric,
    log: &mut F,
) where
    F: FnMut(&str),
{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        emit_wfp_setup_metric(metrics_hook, metric)
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => log(&format!(
            "failed to emit WFP setup metric for {offline_username}: {err}"
        )),
        Err(panic_payload) => {
            let error = panic_payload_to_string(panic_payload);
            log(&format!(
                "WFP setup metric emission panicked for {offline_username}: {error}"
            ));
        }
    }
}

pub fn install_wfp_filters<F>(
    _nano_home: &Path,
    offline_username: &str,
    metrics_hook: telemetry::MetricsHook<'_>,
    mut log: F,
) where
    F: FnMut(&str),
{
    let metric = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        install_wfp_filters_for_account(offline_username)
    })) {
        Ok(Ok(installed_filter_count)) => {
            log(&format!(
                "WFP setup succeeded for {offline_username} with {installed_filter_count} installed filters"
            ));
            WfpSetupMetric {
                outcome: WfpSetupMetricOutcome::Success,
                target_account: offline_username.to_string(),
                installed_filter_count,
                error: None,
            }
        }
        Ok(Err(err)) => {
            let error = err.to_string();
            log(&format!(
                "WFP setup failed for {offline_username}: {error}; continuing elevated setup"
            ));
            WfpSetupMetric {
                outcome: WfpSetupMetricOutcome::Failure,
                target_account: offline_username.to_string(),
                installed_filter_count: 0,
                error: Some(error),
            }
        }
        Err(panic_payload) => {
            let error = panic_payload_to_string(panic_payload);
            log(&format!(
                "WFP setup panicked for {offline_username}: {error}; continuing elevated setup"
            ));
            WfpSetupMetric {
                outcome: WfpSetupMetricOutcome::Failure,
                target_account: offline_username.to_string(),
                installed_filter_count: 0,
                error: Some(format!("panic: {error}")),
            }
        }
    };

    emit_wfp_setup_metric_safely(metrics_hook, offline_username, &metric, &mut log);
}

/// Removes the persistent Wayland Nano WFP provider, sublayer, and filters.
///
/// Track-B addition. Unlike install (whose failures are non-fatal to the rest
/// of setup), uninstall propagates failures: the caller decides whether to
/// continue tearing down users and firewall state. Panics are contained and
/// surfaced as errors.
pub fn uninstall_wfp_filters<F>(
    _nano_home: &Path,
    metrics_hook: telemetry::MetricsHook<'_>,
    mut log: F,
) -> Result<usize>
where
    F: FnMut(&str),
{
    let target = "wayland-nano-wfp";
    let (metric, outcome) =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(remove_wfp_objects)) {
            Ok(Ok(removed_filter_count)) => {
                log(&format!(
                    "WFP uninstall succeeded with {removed_filter_count} removed filters"
                ));
                (
                    WfpSetupMetric {
                        outcome: WfpSetupMetricOutcome::Success,
                        target_account: target.to_string(),
                        installed_filter_count: removed_filter_count,
                        error: None,
                    },
                    Ok(removed_filter_count),
                )
            }
            Ok(Err(err)) => {
                let error = err.to_string();
                log(&format!("WFP uninstall failed: {error}"));
                (
                    WfpSetupMetric {
                        outcome: WfpSetupMetricOutcome::Failure,
                        target_account: target.to_string(),
                        installed_filter_count: 0,
                        error: Some(error.clone()),
                    },
                    Err(err),
                )
            }
            Err(panic_payload) => {
                let error = panic_payload_to_string(panic_payload);
                log(&format!("WFP uninstall panicked: {error}"));
                (
                    WfpSetupMetric {
                        outcome: WfpSetupMetricOutcome::Failure,
                        target_account: target.to_string(),
                        installed_filter_count: 0,
                        error: Some(format!("panic: {error}")),
                    },
                    Err(anyhow::anyhow!("WFP uninstall panicked: {error}")),
                )
            }
        };

    let metric_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        emit_wfp_metric(
            metrics_hook,
            &metric,
            WFP_UNINSTALL_SUCCESS_METRIC,
            WFP_UNINSTALL_FAILURE_METRIC,
        )
    }));
    match metric_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => log(&format!("failed to emit WFP uninstall metric: {err}")),
        Err(panic_payload) => {
            let error = panic_payload_to_string(panic_payload);
            log(&format!("WFP uninstall metric emission panicked: {error}"));
        }
    }
    outcome
}
