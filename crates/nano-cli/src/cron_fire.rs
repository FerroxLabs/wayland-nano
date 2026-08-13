//! The host-side cron fire executor (C11 §5.4 step 4): injects the stored
//! prompt as a provenance-marked user turn into the job's bound session and
//! runs it at the capped `mode_at_fire`. Called ONLY after the journal-first
//! reservation is durable. Reuses the exec discipline end to end: the ONE
//! bootstrap path, the non-interactive auto-deny gate (a cron fire can
//! never prompt), and the journal-first op sink.

use nano_agent::bootstrap::{SessionSeed, bootstrap_session};
use nano_agent::cron::{CronFireError, CronFireExecutor, CronJob};
use nano_agent::turn::ModelDriver;
use nano_agent::turn::ToolExecutor;
use nano_protocol::permission_mode::PermissionMode;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Generic over the host's driver/tool factories — acp-host (and any other
/// host) wires its production factories; tests wire scripted doubles.
pub struct HostCronFire<'a, FD, FT> {
    pub nano_home: PathBuf,
    pub sessions_dir: PathBuf,
    pub model_name: String,
    pub make_driver: &'a FD,
    pub make_tools: &'a FT,
    /// C8: fire-time binding resolution (credential re-resolution per fire,
    /// same discipline as the prompt path — fail-closed on resolution).
    pub router: &'a crate::provider_router::ProviderRouter,
    pub sandbox_available: bool,
}

impl<FD, FT> std::fmt::Debug for HostCronFire<'_, FD, FT> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostCronFire")
            .field("model_name", &self.model_name)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl<FD, FT, D, T> CronFireExecutor for HostCronFire<'_, FD, FT>
where
    FD: Fn(&crate::provider_router::ProviderBinding) -> D + Send + Sync,
    FT: Fn(
            &std::path::Path,
            PermissionMode,
            &std::path::Path,
            Option<crate::acp_mode::DiffHook>,
            Option<std::sync::Arc<dyn nano_model::metering::UsageSink>>,
        ) -> (T, nano_core::permissions::FileSystemSandboxPolicy)
        + Send
        + Sync,
    D: ModelDriver,
    T: ToolExecutor,
{
    async fn fire(
        &self,
        job: &CronJob,
        turn_id: &str,
        occurrence_id: &str,
        mode_at_fire: &str,
    ) -> Result<(), CronFireError> {
        let fail = |message: String| CronFireError::Failed(message);
        // The fire-time mode derivation is already capped by the runner
        // (min(session_mode, default)); parse is total over the vocabulary.
        let mode = PermissionMode::parse(mode_at_fire).unwrap_or(PermissionMode::Default);
        // Resolve the session through the SAME bootstrap as any resume.
        let workspace = std::env::current_dir().unwrap_or_default();
        let session = bootstrap_session(
            &self.sessions_dir,
            &workspace,
            SessionSeed::Resume(job.session_id.clone()),
        )
        .map_err(|err| fail(format!("session resolve failed: {err}")))?;
        // The session's recorded cwd anchors the tools (a resumed session
        // continues where it started).
        let session_cwd = session
            .state
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or(workspace);
        let context = crate::acp_mode::messages_from_envelopes(&session.envelopes);
        // The session's deterministic plan file (C10): the tool-layer
        // policy root for the plan-file write exception. A cron fire has no
        // live posture cell — the path derivation is the same function of
        // (sessions_dir, session_id) the posture cell uses.
        let plan_file = self
            .sessions_dir
            .join(format!("{}.plan.md", job.session_id));
        // P1: cron fires run the unmetered search posture (None — the
        // make_tools fallback handle); session CostMeter wiring is the ACP
        // prompt path's (P1 economy scope).
        let (tools, policy) = (self.make_tools)(&session_cwd, mode, &plan_file, None, None);
        let cron_store = nano_agent::cron::JsonCronStore::new(&self.nano_home);
        let executor =
            nano_agent::cron::CronjobExecutor::new(&tools, &cron_store, job.session_id.clone());
        // C8: resolve the provider binding at fire time (credential
        // re-resolution, fail-closed — a vanished key fails the fire with a
        // typed error, never a silent fallback onto another provider).
        let env_reader = |name: &str| std::env::var(name).ok();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let binding = self
            .router
            .resolve_binding(&self.model_name, &env_reader, now)
            .map_err(|err| fail(format!("provider binding unavailable: {err:?}")))?;
        let driver = (self.make_driver)(&binding);
        // P3 §3.3: the fired session's appends route through a coordinator.
        let journal = Arc::new(
            nano_session::JournalCoordinator::open(&session.journal_path)
                .map_err(|err| fail(format!("cannot open session journal: {err}")))?,
        );
        let events = Arc::new(Mutex::new(crate::exec_mode::ExecEvents::new(
            Vec::new(), // a cron fire has no stdout stream; events sink to void
            session.session_id.clone(),
        )));
        let gate = crate::exec_mode::ExecApproval {
            mode,
            policy,
            cwd: session_cwd,
            sandbox_available: self.sandbox_available,
            events: events.clone(),
            // P2a §9.1: same sticky-OR fold as exec — an image-influenced
            // session's cron fires deny protected trust mutations (no human
            // is present to approve them).
            image_influenced: crate::acp_mode::image_influenced_from_envelopes(&session.envelopes),
        };
        // Provenance: the transcript and journal show the input as
        // scheduled, never as the interactive user.
        let input = format!(
            "[scheduled by cron job {} — occurrence {}] {}",
            job.job_id, occurrence_id, job.prompt
        );
        let view = nano_agent::bootstrap::BootstrappedSession {
            session_id: session.session_id.clone(),
            journal_path: session.journal_path.clone(),
            envelopes: Vec::new(),
            state: nano_session::SessionState::new(),
            turn_counter: 0,
        };
        // The runner minted this turn id into the CronFired reservation —
        // the fire turn MUST use it so the audit linkage holds.
        let outcome = crate::exec_mode::run_exec_turn(
            &driver,
            &executor,
            &gate,
            &self.model_name,
            &view,
            turn_id,
            &input,
            context,
            journal,
            events,
            &[],
            // P1: cron executors carry no search wiring — the scheduled
            // surface stays the pre-P1 set (fail-closed registration).
            false,
        )
        .await;
        match outcome.state {
            nano_agent::turn::TurnState::Complete => Ok(()),
            other => Err(fail(format!(
                "cron-fired turn did not complete: {}",
                other.label()
            ))),
        }
    }
}

/// The runner tick hosts call on their 30s interval. A corrupt job store
/// disables the scheduler for the process lifetime (fail-closed, Q6) — the
/// caller flips its own `disabled` flag when this returns `Err`.
#[allow(clippy::too_many_arguments)] // the host factory bundle travels together (exec_run.rs precedent)
pub async fn tick_once<FD, FT, D, T>(
    nano_home: &std::path::Path,
    sessions_dir: &std::path::Path,
    model_name: &str,
    make_driver: &FD,
    make_tools: &FT,
    router: &crate::provider_router::ProviderRouter,
    sandbox_available: bool,
    live_mode: &dyn Fn(&str) -> Option<&'static str>,
) -> Result<Vec<nano_agent::cron::JobTickOutcome>, nano_agent::cron::CronStoreError>
where
    FD: Fn(&crate::provider_router::ProviderBinding) -> D + Send + Sync,
    FT: Fn(
            &std::path::Path,
            PermissionMode,
            &std::path::Path,
            Option<crate::acp_mode::DiffHook>,
            Option<std::sync::Arc<dyn nano_model::metering::UsageSink>>,
        ) -> (T, nano_core::permissions::FileSystemSandboxPolicy)
        + Send
        + Sync,
    D: ModelDriver,
    T: ToolExecutor,
{
    let clock = nano_agent::clock::SystemClock;
    let runner = nano_agent::cron::CronRunner {
        sessions_dir: sessions_dir.to_path_buf(),
        clock: &clock,
        guards: nano_agent::bootstrap::session_guard_registry(),
        live_mode,
    };
    let store = nano_agent::cron::JsonCronStore::new(nano_home);
    let executor = HostCronFire {
        nano_home: nano_home.to_path_buf(),
        sessions_dir: sessions_dir.to_path_buf(),
        model_name: model_name.to_string(),
        make_driver,
        make_tools,
        router,
        sandbox_available,
    };
    runner.tick(&store, &executor).await
}
