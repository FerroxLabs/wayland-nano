//! P1 §3.3 blocker regression suite (tasks.rs submodule): durable
//! parent-journal rollup (journal-first), append-failure hold, and the
//! crash-recovery orphan fold (exactly-once at the reconciliation boundary).

use super::*;
use nano_model::pricing::PricingCatalog;
use nano_model::types::{ModelEvent, ModelRequest, ModelResponse};

fn catalog() -> Arc<PricingCatalog> {
    Arc::new(
        PricingCatalog::from_toml_str(
            "[metered.mock]\ninput_per_mtok_usd = 1.0\noutput_per_mtok_usd = 2.0\n",
        )
        .unwrap(),
    )
}

fn usage_response(text: &str, input: u64, output: u64) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "stop".into(),
            },
        ],
        usage: nano_model::types::Usage {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        },
        stop_reason: "stop".into(),
    }
}

/// A driver answering every prompt with one usage-bearing response.
#[derive(Debug)]
struct UsageDriver {
    input: u64,
    output: u64,
    pub requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait::async_trait]
impl ModelDriver for UsageDriver {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> Result<ModelResponse, nano_model::types::ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(usage_response("child report", self.input, self.output))
    }
}

struct Dirs {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("note.txt"), "data").unwrap();
    Dirs {
        _tmp: tmp,
        home,
        workspace,
    }
}

fn metered_registry(
    dirs: &Dirs,
    driver: Arc<UsageDriver>,
    rollup_gate: Arc<std::sync::atomic::AtomicBool>,
    captured: Arc<Mutex<Vec<OpEnvelope>>>,
) -> (TaskRegistry, crate::cost::CostMeter) {
    let meter = crate::cost::CostMeter::new("metered", catalog(), None);
    let factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync> =
        Arc::new(move || Ok(driver.clone()));
    let sink: RollupSink = Arc::new(move |envelope: &OpEnvelope| {
        let durable = rollup_gate.load(Ordering::SeqCst);
        if durable {
            captured.lock().unwrap().push(envelope.clone());
        }
        durable
    });
    let registry = TaskRegistry::new(&dirs.home, &dirs.workspace, "mock".into(), factory)
        .with_meter(meter.clone(), sink, "s1");
    (registry, meter)
}

/// Durable path: the child terminal state appends Op::ChildUsageRollup to
/// the PARENT journal with stable ids, and the live path lands the same
/// usage in the ONE session meter.
#[test]
fn child_usage_rolls_up_to_parent_journal_and_meter() {
    let dirs = dirs();
    let driver = Arc::new(UsageDriver {
        input: 120,
        output: 60,
        requests: Mutex::new(Vec::new()),
    });
    let captured = Arc::new(Mutex::new(Vec::new()));
    let (registry, meter) = metered_registry(
        &dirs,
        driver,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
        captured.clone(),
    );
    let id = registry.spawn("do the thing", None).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let status = registry.status(&id).unwrap();
        if status.starts_with("done") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "child never settled");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The parent-journal rollup op, with stable ids and the child's sum.
    let envelopes = captured.lock().unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].id, format!("{id}-rollup-1"));
    match &envelopes[0].op {
        Op::ChildUsageRollup {
            task_id,
            child_turn_id,
            outcome,
            usage,
        } => {
            assert_eq!(task_id, &id);
            assert_eq!(child_turn_id, &format!("{id}-turn-1"));
            assert_eq!(*outcome, TurnOutcome::Completed);
            assert_eq!(usage.input_tokens, 120);
            assert_eq!(usage.output_tokens, 60);
        }
        other => panic!("expected ChildUsageRollup, got {other:?}"),
    }
    drop(envelopes);
    // Live path: the ONE session meter carries the child's usage.
    let usage = meter.session_usage();
    assert_eq!(usage.input_tokens, 120);
    assert_eq!(usage.output_tokens, 60);
    // And the child result is visible only after the rollup landed.
    assert!(registry.result(&id).unwrap().contains("child report"));
}

/// Journal-first, fail-closed: while the rollup append fails, the
/// completion is HELD (never reported); when the append recovers, the held
/// outcome rolls up and becomes visible — exactly once.
#[test]
fn rollup_append_failure_holds_the_completion() {
    let dirs = dirs();
    let driver = Arc::new(UsageDriver {
        input: 10,
        output: 5,
        requests: Mutex::new(Vec::new()),
    });
    let captured = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (registry, _meter) = metered_registry(&dirs, driver, gate.clone(), captured.clone());
    let id = registry.spawn("do the thing", None).unwrap();
    // The child finishes, but the rollup cannot land: the completion stays
    // held — status keeps reporting running.
    std::thread::sleep(std::time::Duration::from_millis(500));
    for _ in 0..10 {
        let status = registry.status(&id).unwrap();
        assert!(
            status.starts_with("running"),
            "unjournaled completion must never be reported: {status}"
        );
        assert!(captured.lock().unwrap().is_empty());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The journal recovers: the held outcome rolls up and turns visible.
    gate.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let status = registry.status(&id).unwrap();
        if status.starts_with("done") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "held completion never rolled up"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(captured.lock().unwrap().len(), 1, "rolled up exactly once");
}

/// Crash test at the reconciliation boundary (§3.3): a child journal whose
/// terminal TurnEnd is durable but whose rollup never landed folds EXACTLY
/// ONCE at resume; a journaled rollup marker suppresses the fold (no
/// double-count); foreign/pre-P1 dirs are never folded.
#[test]
fn orphan_fold_counts_exactly_once_and_respects_the_marker() {
    let dirs = dirs();
    let task_dir = dirs.home.join("tasks").join("task-orphan-1");
    std::fs::create_dir_all(&task_dir).unwrap();
    let mut usage = TurnUsage::default();
    usage.add_provider_reported(300, 150, 0, 0, 0, false);
    let mut writer = JournalWriter::open(&task_dir.join("journal.jsonl")).unwrap();
    writer
        .append(&OpEnvelope::new(
            "task-orphan-1-begin-1",
            "now",
            Op::SessionBegin {
                session_id: "task-orphan-1".into(),
                cwd: dirs.workspace.display().to_string(),
            },
        ))
        .unwrap();
    writer
        .append(&OpEnvelope::new(
            "task-orphan-1-turn-1-end",
            "now",
            Op::TurnEnd {
                turn_id: "task-orphan-1-turn-1".into(),
                outcome: TurnOutcome::Completed,
                usage: Some(usage.clone()),
            },
        ))
        .unwrap();
    writer.sync().unwrap();
    std::fs::write(task_dir.join(SESSION_MARKER_FILE), "s1").unwrap();

    // No rollup marker in the parent journal: folded once.
    let rolled_up = std::collections::BTreeSet::new();
    let folded = fold_orphan_child_usage(&dirs.home, "s1", &rolled_up);
    assert_eq!(folded.input_tokens, 300);
    assert_eq!(folded.output_tokens, 150);
    // Folding again after the rollup landed (marker present): nothing.
    let rolled_up: std::collections::BTreeSet<String> =
        ["task-orphan-1".to_string()].into_iter().collect();
    let skipped = fold_orphan_child_usage(&dirs.home, "s1", &rolled_up);
    assert!(skipped.is_zero(), "a journaled rollup never re-folds");
    // A foreign session never folds this dir.
    let foreign = fold_orphan_child_usage(&dirs.home, "other-session", &rolled_up);
    assert!(foreign.is_zero());
    // A pre-P1 dir (no marker) is never folded.
    std::fs::remove_file(task_dir.join(SESSION_MARKER_FILE)).unwrap();
    let rolled_up = std::collections::BTreeSet::new();
    let unmarked = fold_orphan_child_usage(&dirs.home, "s1", &rolled_up);
    assert!(unmarked.is_zero());
}
