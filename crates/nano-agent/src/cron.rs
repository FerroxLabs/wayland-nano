//! Cron / scheduling (C11 §5): session-alive-only, journal-first, fail-closed.
//!
//! - 5-field cron only (`min hour dom mon dow`); interval sugar is REJECTED
//!   with a typed error naming the cron equivalent (Q5 RULED).
//! - The store (`nano_home/cron/jobs.json`) is a CACHE: session journals are
//!   authoritative for what fired AND for job existence — `cronjob` tool
//!   create/delete journal `CronCreated`/`CronDeleted` BEFORE touching the
//!   cache (F-6 closure), and reconciliation rebuilds missing jobs / removes
//!   tombstoned ones whenever the two disagree. No MAC (Q6 RULED);
//!   a corrupt store disables the whole scheduler with a typed error.
//! - A fire = a journal-first `CronFired` reservation keyed by a stable
//!   occurrence id (`{job_id}:{scheduled_fire_time}`, RFC3339 UTC minute
//!   resolution), flushed BEFORE any prompt injection or model call.
//!   Claim-before-fire (F-8 data-integrity): the occurrence idempotency
//!   check is REPEATED under the session guard (in-process mutex + OS
//!   file lock, or the lifetime ownership lock it stands in for), so two
//!   hosts sharing one NANO_HOME cannot both reserve the same occurrence
//!   — the check-and-reserve is atomic across processes.
//! - Missed-while-dead fires coalesce to ONE with `coalesced: n` journaled
//!   (Q4 RULED) — never catch-up storms.
//! - `mode_at_fire = min(session_mode_at_fire, default)` — ONE authoritative
//!   derivation, recorded in every `CronFired` for audit (§5.5).
//! - The injection scan (ported from wcore `_scan_cron_prompt`) is
//!   DEFENSE-IN-DEPTH ONLY; the real boundary is the capped fire-time mode
//!   plus the ordinary sandbox/policy gate.

use crate::bootstrap::SessionGuardRegistry;
use crate::clock::Clock;
use chrono::Datelike;
use chrono::Timelike;
use nano_session::JournalWriter;
use nano_session::Op;
use nano_session::OpEnvelope;
use nano_session::SessionState;
use nano_session::read_journal;
use std::io;
use std::path::Path;
use std::path::PathBuf;

// ── Schedule (5-field cron) ────────────────────────────────────────────────

/// A parsed 5-field cron schedule. Bitmasks: bits index the field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: u64, // 0..=59
    hours: u32,   // 0..=23
    dom: u32,     // 1..=31
    months: u16,  // 1..=12
    dow: u8,      // 0..=6 (Sunday = 0; input 7 normalizes to 0)
    dom_any: bool,
    dow_any: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// Interval sugar ("every 10m") is rejected in v1 (Q5 RULED) — the
    /// message names the cron equivalent.
    #[error("interval schedules are not supported in v1; use the cron equivalent: {0}")]
    IntervalSugar(String),
    #[error("invalid 5-field cron schedule {expr:?}: {detail}")]
    Invalid { expr: String, detail: String },
}

/// Parses a standard 5-field crontab expression. Anything else — including
/// interval sugar like "every 10m" or "@every 1h" — is a typed error naming
/// the cron equivalent when one is computable.
pub fn parse_schedule(expr: &str) -> Result<CronSchedule, ScheduleError> {
    let trimmed = expr.trim();
    // Interval sugar detection BEFORE field splitting: "every 10m",
    // "every 2h", "@every 90s".
    let sugar = trimmed
        .strip_prefix("@every ")
        .or_else(|| trimmed.strip_prefix("every "));
    if let Some(span) = sugar {
        return Err(ScheduleError::IntervalSugar(cron_equivalent(span)));
    }
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(ScheduleError::Invalid {
            expr: expr.to_string(),
            detail: format!(
                "expected 5 fields (min hour dom mon dow), got {}",
                fields.len()
            ),
        });
    }
    let invalid = |detail: &str| ScheduleError::Invalid {
        expr: expr.to_string(),
        detail: detail.to_string(),
    };
    let minutes = parse_field(fields[0], 0, 59).map_err(|e| invalid(&format!("minute: {e}")))?;
    let hours = parse_field(fields[1], 0, 23).map_err(|e| invalid(&format!("hour: {e}")))?;
    let dom = parse_field(fields[2], 1, 31).map_err(|e| invalid(&format!("day-of-month: {e}")))?;
    let months = parse_field(fields[3], 1, 12).map_err(|e| invalid(&format!("month: {e}")))?;
    let dow_raw =
        parse_field(fields[4], 0, 7).map_err(|e| invalid(&format!("day-of-week: {e}")))?;
    // 7 == Sunday == 0.
    let mut dow = dow_raw & !0b1000_0000u128;
    if dow_raw & (1 << 7) != 0 {
        dow |= 1;
    }
    Ok(CronSchedule {
        minutes: minutes as u64,
        hours: hours as u32,
        dom: dom as u32,
        months: months as u16,
        dow: dow as u8,
        dom_any: fields[2] == "*",
        dow_any: fields[4] == "*",
    })
}

/// Best-effort cron equivalent for rejected interval sugar.
fn cron_equivalent(span: &str) -> String {
    let span = span.trim();
    let (digits, unit) =
        span.split_at(span.find(|c: char| c.is_alphabetic()).unwrap_or(span.len()));
    let Ok(n) = digits.parse::<u64>() else {
        return "*/N * * * *".to_string();
    };
    if n == 0 {
        return "*/N * * * *".to_string();
    }
    match unit.chars().next() {
        Some('m') if n < 60 => format!("*/{n} * * * *"),
        Some('h') if n < 24 => format!("0 */{n} * * *"),
        Some('s') => "*/1 * * * * (minute resolution is the floor)".to_string(),
        Some('d') => "0 0 */N * *".to_string(),
        _ => "*/N * * * *".to_string(),
    }
}

/// One cron field: `*`, `*/n`, `a`, `a-b`, `a-b/n`, comma lists. Returns a
/// 128-bit mask (bit i = value i matches).
fn parse_field(field: &str, lo: u32, hi: u32) -> Result<u128, String> {
    let mut mask: u128 = 0;
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty list item".into());
        }
        let (range_part, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().map_err(|_| format!("bad step {step:?}"))?;
                if step == 0 {
                    return Err("step must be > 0".into());
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range_part == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let a: u32 = a.parse().map_err(|_| format!("bad value {a:?}"))?;
            let b: u32 = b.parse().map_err(|_| format!("bad value {b:?}"))?;
            if a > b {
                return Err(format!("range {a}-{b} is inverted"));
            }
            (a, b)
        } else {
            let a: u32 = range_part
                .parse()
                .map_err(|_| format!("bad value {range_part:?}"))?;
            (a, a)
        };
        if start < lo || end > hi {
            return Err(format!("value out of range {lo}-{hi}"));
        }
        let mut value = start;
        while value <= end {
            mask |= 1u128 << value;
            value += step;
        }
    }
    if mask == 0 {
        return Err("field matches nothing".into());
    }
    Ok(mask)
}

impl CronSchedule {
    /// True when the given epoch second (UTC) falls on this schedule.
    /// Vixie rule: when BOTH dom and dow are restricted, either matching is
    /// a hit; otherwise both must match.
    pub fn matches(&self, epoch_secs: u64) -> bool {
        let Some(dt) = chrono::DateTime::from_timestamp(epoch_secs as i64, 0) else {
            return false;
        };
        let minute_hit = self.minutes & (1u64 << dt.minute()) != 0;
        let hour_hit = self.hours & (1u32 << dt.hour()) != 0;
        let month_hit = self.months & (1u16 << dt.month()) != 0;
        let dom_hit = self.dom & (1u32 << dt.day()) != 0;
        let dow_hit = self.dow & (1u8 << dt.weekday().num_days_from_sunday()) != 0;
        let day_hit = match (self.dom_any, self.dow_any) {
            (true, true) => true,
            (true, false) => dow_hit,
            (false, true) => dom_hit,
            (false, false) => dom_hit || dow_hit,
        };
        minute_hit && hour_hit && month_hit && day_hit
    }

    /// The next scheduled instant STRICTLY AFTER `after_secs`, at minute
    /// resolution. `None` when nothing matches within a 4-year horizon
    /// (e.g. Feb 29 schedules beyond the horizon).
    pub fn next_after(&self, after_secs: u64) -> Option<u64> {
        let mut candidate = after_secs - (after_secs % 60) + 60;
        let horizon = after_secs + 4 * 366 * 24 * 3600;
        while candidate <= horizon {
            if self.matches(candidate) {
                return Some(candidate);
            }
            // Day-level skip: if month/dom/dow all miss, jump to the next
            // midnight instead of walking 1440 minutes.
            let dt = chrono::DateTime::from_timestamp(candidate as i64, 0)?;
            let month_hit = self.months & (1u16 << dt.month()) != 0;
            let dom_hit = self.dom & (1u32 << dt.day()) != 0;
            let dow_hit = self.dow & (1u8 << dt.weekday().num_days_from_sunday()) != 0;
            let day_hit = match (self.dom_any, self.dow_any) {
                (true, true) => true,
                (true, false) => dow_hit,
                (false, true) => dom_hit,
                (false, false) => dom_hit || dow_hit,
            };
            if month_hit && day_hit {
                candidate += 60;
            } else {
                candidate += 24 * 3600 - (candidate % (24 * 3600));
            }
        }
        None
    }
}

/// RFC3339 UTC at minute resolution — the scheduled-fire-time encoding used
/// in occurrence ids (`{job_id}:{scheduled}`).
pub fn rfc3339_minute(epoch_secs: u64) -> String {
    chrono::DateTime::from_timestamp(epoch_secs as i64, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:00Z").to_string())
        .unwrap_or_default()
}

/// Strict parse of the occurrence-id timestamp shape. Minute resolution is
/// enforced (seconds must be 00); anything else is REJECTED — malformed
/// occurrence identities must never influence last_fired.
pub fn parse_rfc3339_minute(value: &str) -> Option<u64> {
    let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ").ok()?;
    if naive.second() != 0 {
        return None;
    }
    u64::try_from(naive.and_utc().timestamp()).ok()
}

/// The idempotency key for one scheduled occurrence: stable across restarts,
/// clock jitter, and coalescing.
pub fn occurrence_id(job_id: &str, scheduled_epoch_secs: u64) -> String {
    format!("{job_id}:{}", rfc3339_minute(scheduled_epoch_secs))
}

// ── Prompt injection scan (wcore `_scan_cron_prompt` port) ─────────────────
// DEFENSE-IN-DEPTH ONLY: this scan WILL miss adversarial text and the design
// does not rely on it. The real boundary is the capped fire-time mode plus
// the ordinary sandbox/policy gate every fired turn runs under.

const CRON_INVISIBLE_CHARS: &[char] = &[
    '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}', '\u{202a}', '\u{202b}', '\u{202c}',
    '\u{202d}', '\u{202e}',
];

const CRON_THREAT_PATTERNS: &[(&str, &str)] = &[
    ("ignore previous instructions", "prompt_injection"),
    ("ignore all previous instructions", "prompt_injection"),
    ("ignore prior instructions", "prompt_injection"),
    ("ignore above instructions", "prompt_injection"),
    ("disregard your instructions", "disregard_rules"),
    ("disregard all instructions", "disregard_rules"),
    ("disregard any instructions", "disregard_rules"),
    ("disregard your rules", "disregard_rules"),
    ("disregard your guidelines", "disregard_rules"),
    ("do not tell the user", "deception_hide"),
    ("system prompt override", "sys_prompt_override"),
    ("authorized_keys", "ssh_backdoor"),
    ("/etc/sudoers", "sudoers_mod"),
    ("visudo", "sudoers_mod"),
    ("rm -rf /", "destructive_root_rm"),
];

/// Scan a cron prompt for critical threats. `Some(reason)` = reject
/// fail-closed. Verbatim port of the wcore scan (same patterns, same
/// compound checks).
pub fn scan_cron_prompt(prompt: &str) -> Option<String> {
    for ch in CRON_INVISIBLE_CHARS {
        if prompt.contains(*ch) {
            return Some(format!(
                "prompt contains invisible unicode U+{:04X} (possible injection)",
                *ch as u32
            ));
        }
    }
    let lower = prompt.to_lowercase();
    for (needle, pid) in CRON_THREAT_PATTERNS {
        if lower.contains(needle) {
            return Some(format!("prompt matches threat pattern '{pid}'"));
        }
    }
    if (lower.contains("cat ") || lower.contains("less ") || lower.contains("more "))
        && (lower.contains(".env")
            || lower.contains("credentials")
            || lower.contains(".netrc")
            || lower.contains(".pgpass"))
    {
        return Some("prompt matches threat pattern 'read_secrets'".to_string());
    }
    let secret_hints = [
        "$key",
        "$token",
        "$secret",
        "$password",
        "$credential",
        "$api",
    ];
    if (lower.contains("curl ") || lower.contains("wget "))
        && secret_hints.iter().any(|h| lower.contains(h))
    {
        return Some("prompt matches threat pattern 'exfil_curl_wget'".to_string());
    }
    None
}

// ── Store (jobs.json — a CACHE; journals are authoritative) ────────────────

/// Job payload: a prompt + target session + schedule ONLY. No shell command
/// field, no env overrides, no mode field — nothing that could carry
/// privilege.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CronJob {
    pub job_id: String,
    pub session_id: String,
    /// Canonical 5-field crontab.
    pub schedule: String,
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Creation instant (RFC3339 minute): the coalescing anchor for a
    /// never-fired job (overdue math counts occurrences AFTER creation).
    #[serde(default)]
    pub created_at: Option<String>,
    /// Cache of the latest fired scheduled instant (RFC3339 minute). The
    /// journal is authoritative; reconciliation repairs this forward.
    #[serde(default)]
    pub last_fired: Option<String>,
    /// Derived: next scheduled instant after last_fired (recomputed on
    /// every save).
    #[serde(default)]
    pub next_fire: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum CronStoreError {
    /// A corrupt or unparsable jobs file disables the ENTIRE scheduler with
    /// a typed error — never guesses, and the session itself is unaffected.
    #[error("cron job store is corrupt; scheduler disabled: {0}")]
    Corrupt(String),
    #[error("cron job store io error: {0}")]
    Io(#[from] io::Error),
}

/// Persistence boundary for the job cache, so tests can inject failures
/// into the crash windows (e.g. save fails after the journal reservation).
pub trait CronStoreLike: std::fmt::Debug + Send + Sync {
    fn load(&self) -> Result<Vec<CronJob>, CronStoreError>;
    fn save(&self, jobs: &[CronJob]) -> Result<(), CronStoreError>;
}

#[derive(Debug)]
pub struct JsonCronStore {
    path: PathBuf,
}

impl JsonCronStore {
    pub fn new(nano_home: &Path) -> Self {
        Self {
            path: nano_home.join("cron").join("jobs.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CronStoreLike for JsonCronStore {
    fn load(&self) -> Result<Vec<CronJob>, CronStoreError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&self.path)?;
        serde_json::from_slice(&bytes)
            .map_err(|err| CronStoreError::Corrupt(format!("{}: {err}", self.path.display())))
    }

    /// Atomic write: tempfile in the same directory, flush + close, then a
    /// replace-existing rename (Windows-safe: `std::fs::rename` maps to
    /// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`; POSIX rename replaces
    /// by definition). Mode 0600 on unix.
    fn save(&self, jobs: &[CronJob]) -> Result<(), CronStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut jobs = jobs.to_vec();
        // Refresh the derived next_fire cache on every persist.
        for job in &mut jobs {
            job.next_fire = next_fire_for(job);
        }
        let bytes = serde_json::to_vec_pretty(&jobs)
            .map_err(|err| CronStoreError::Corrupt(err.to_string()))?;
        // Per-process tmp name: two hosts sharing one NANO_HOME write
        // through this same path, and a fixed tmp name lets one host's
        // rename fail under the other's truncate (F-8 two-process proof:
        // the collision aborted a fire AFTER its reservation was durable).
        let tmp = self
            .path
            .with_extension(format!("jsonl.{}.tmp", std::process::id()));
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            use std::io::Write;
            file.write_all(&bytes)?;
            file.sync_data()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Derived `next_fire`: the next scheduled instant after `last_fired` (or
/// after creation when never fired).
fn next_fire_for(job: &CronJob) -> Option<String> {
    let schedule = parse_schedule(&job.schedule).ok()?;
    let base = job
        .last_fired
        .as_deref()
        .or(job.created_at.as_deref())
        .and_then(parse_rfc3339_minute)
        .unwrap_or(0);
    schedule.next_after(base).map(rfc3339_minute)
}

// ── Runner ─────────────────────────────────────────────────────────────────

/// What one tick did with one job (audit surface for tests and `cronjob
/// list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTickOutcome {
    /// Fired (reservation journaled, prompt injected).
    Fired {
        job_id: String,
        occurrence_id: String,
        coalesced: u32,
        mode_at_fire: String,
    },
    /// The occurrence was already journaled — a prior crash interrupted the
    /// injection, or a concurrent host claimed the occurrence first (F-8)
    /// — reconciled forward, NOT re-fired.
    AlreadyReserved {
        job_id: String,
        occurrence_id: String,
    },
    /// Session guard held (interactive turn / fork / another fire): the fire
    /// defers to the next tick — missed-tick Skip, never stacking.
    Busy { job_id: String },
    /// Nothing due.
    Idle { job_id: String },
    /// Fail-closed: typed error recorded, `last_fired` NOT advanced, nothing
    /// injected; retried at the next scheduled fire.
    Error { job_id: String, error: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CronFireError {
    #[error("cron fire failed: {0}")]
    Failed(String),
}

/// The host-side effect of a fire: inject the stored prompt as user input
/// (with provenance metadata) and run the turn at `mode_at_fire`. Called
/// ONLY after the journal-first reservation is durable.
#[async_trait::async_trait]
pub trait CronFireExecutor: std::fmt::Debug {
    async fn fire(
        &self,
        job: &CronJob,
        turn_id: &str,
        occurrence_id: &str,
        mode_at_fire: &str,
    ) -> Result<(), CronFireError>;
}

/// The session-alive-only runner (Q3 RULED): one tick computes due jobs and
/// dispatches them through the §5.4 fire transaction. Hosts call `tick` on
/// a 30s interval (wcore TICK_INTERVAL parity); all trigger logic drives a
/// [`Clock`], never the wall clock.
pub struct CronRunner<'a> {
    pub sessions_dir: PathBuf,
    pub clock: &'a dyn Clock,
    pub guards: &'a SessionGuardRegistry,
    /// The live session's current mode, when the session is attached to
    /// this host (`None` = the runner loaded it itself → `default`). The
    /// ONE authoritative derivation is `min(session_mode, default)`.
    pub live_mode: &'a dyn Fn(&str) -> Option<&'static str>,
}

impl std::fmt::Debug for CronRunner<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronRunner")
            .field("sessions_dir", &self.sessions_dir)
            .finish_non_exhaustive()
    }
}

/// The pinned fire-time mode derivation (§5.5): `min(session_mode_at_fire,
/// default)` under the C2 ordering `read_only < default < full_auto`. A
/// fire into a live `full_auto` session runs at `default`; a fire into a
/// `read_only` session enforces the lower mode.
pub fn mode_at_fire(live_mode: Option<&'static str>) -> String {
    match live_mode {
        Some("read_only") => "read_only".to_string(),
        _ => "default".to_string(),
    }
}

impl CronRunner<'_> {
    /// One scheduler tick: reconcile → coalesce → reserve (journal-first) →
    /// fire. The store is reloaded and repersisted through the injected
    /// store each tick; a corrupt store disables the whole tick with a
    /// typed error.
    ///
    /// EXISTENCE RECONCILIATION (C11 §5.5, F-6 closure): `CronCreated`/
    /// `CronDeleted` are the journal-first durable acts of the `cronjob`
    /// tool, so the cache can lag (a kill between the journal append and
    /// the cache persist). Before the due-loop, sessions WITHOUT any cached
    /// job are discovered by a directory scan and their journals folded:
    /// live journaled jobs missing from the cache are rebuilt (and fire
    /// this very tick when due). Sessions WITH a cached job get the same
    /// repair (plus tombstone removal) inside `tick_one`, which already
    /// folds their journal. Legacy cache jobs with no cron ops in their
    /// session journal are never touched.
    pub async fn tick<S: CronStoreLike>(
        &self,
        store: &S,
        executor: &dyn CronFireExecutor,
    ) -> Result<Vec<JobTickOutcome>, CronStoreError> {
        let mut jobs = store.load()?;
        let now = self.clock.now_ms() / 1000;
        let mut outcomes = Vec::new();
        let mut dirty = false;

        // Discovery: fold the journals of sessions no cached job covers.
        // Unreadable/absent journals are skipped (nothing actionable without
        // a job; a cached job's unreadable journal is tick_one's typed
        // Error). A repaired job enters THIS tick's due-loop.
        {
            let covered: std::collections::BTreeSet<&str> =
                jobs.iter().map(|job| job.session_id.as_str()).collect();
            if let Ok(entries) = std::fs::read_dir(&self.sessions_dir) {
                let journal_paths: Vec<PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                            && path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|stem| !covered.contains(stem))
                    })
                    .collect();
                for path in journal_paths {
                    let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let Ok(report) = read_journal(&path) else {
                        continue;
                    };
                    let state = SessionState::fold(&report.envelopes);
                    for (job_id, record) in &state.cron_jobs {
                        if jobs.iter().any(|job| &job.job_id == job_id) {
                            continue;
                        }
                        jobs.push(CronJob {
                            job_id: job_id.clone(),
                            session_id: session_id.to_string(),
                            schedule: record.schedule.clone(),
                            prompt: record.prompt.clone(),
                            enabled: true,
                            created_at: Some(record.created_at.clone()),
                            last_fired: state.cron_last_fired.get(job_id).cloned(),
                            next_fire: None,
                        });
                        dirty = true;
                    }
                }
            }
        }

        // Job ids tombstoned in their session journal but still cached
        // (torn delete): removed after the loop, NEVER fired.
        let mut tombstoned: Vec<String> = Vec::new();
        // Live journaled jobs of covered sessions missing from the cache
        // (torn create on a covered session): rebuilt after the loop —
        // they enter the due-loop on the NEXT tick.
        let mut discovered: Vec<CronJob> = Vec::new();

        for job in jobs.iter_mut().filter(|job| job.enabled) {
            // Persist-one-job helper for the fire transaction: the §5.4
            // ordering requires the CACHE persist (full job set, never a
            // single-job overwrite) after the journal reservation and before
            // the injection.
            let persist = |updated: &CronJob| -> Result<(), CronStoreError> {
                let mut all = store.load()?;
                match all.iter_mut().find(|job| job.job_id == updated.job_id) {
                    Some(slot) => *slot = updated.clone(),
                    None => all.push(updated.clone()),
                }
                store.save(&all)
            };
            let outcome = self
                .tick_one(
                    job,
                    now,
                    &persist,
                    &mut dirty,
                    &mut tombstoned,
                    &mut discovered,
                    executor,
                )
                .await;
            outcomes.push(outcome);
        }
        if !tombstoned.is_empty() {
            jobs.retain(|job| !tombstoned.contains(&job.job_id));
            dirty = true;
        }
        for repaired in discovered {
            if !jobs.iter().any(|job| job.job_id == repaired.job_id) {
                jobs.push(repaired);
                dirty = true;
            }
        }
        if dirty {
            store.save(&jobs)?;
        }
        Ok(outcomes)
    }

    #[allow(clippy::too_many_arguments)] // the reconciliation out-params travel with the tick
    async fn tick_one(
        &self,
        job: &mut CronJob,
        now: u64,
        persist: &dyn Fn(&CronJob) -> Result<(), CronStoreError>,
        dirty: &mut bool,
        tombstoned: &mut Vec<String>,
        discovered: &mut Vec<CronJob>,
        executor: &dyn CronFireExecutor,
    ) -> JobTickOutcome {
        let fail = |error: String| JobTickOutcome::Error {
            job_id: job.job_id.clone(),
            error,
        };
        let Ok(schedule) = parse_schedule(&job.schedule) else {
            return fail(format!("unparseable schedule {:?}", job.schedule));
        };

        // ── Step 1: resolve the target session (replay path) ──
        let journal_path = self.sessions_dir.join(format!("{}.jsonl", job.session_id));
        let report = match read_journal(&journal_path) {
            Ok(report) if journal_path.exists() => report,
            Ok(_) => return fail(format!("session not found: {}", job.session_id)),
            Err(err) => return fail(format!("session journal unreadable: {err}")),
        };
        let state = SessionState::fold(&report.envelopes);

        // ── Journal-authoritative existence (C11 §5.5, F-6 closure) ──
        // A torn DELETE (CronDeleted durable, cache persist lost) leaves the
        // job cached: remove it WITHOUT firing. A torn CREATE on a session
        // another cached job covers (CronCreated durable, cache persist
        // lost) is rebuilt here — tick()'s pre-loop discovery covers
        // sessions with NO cached job; both feed the same doctrine: the
        // journal is authoritative for existence, the cache is a cache.
        if state.cron_tombstones.contains(&job.job_id) && !state.cron_jobs.contains_key(&job.job_id)
        {
            tombstoned.push(job.job_id.clone());
            return JobTickOutcome::Idle {
                job_id: job.job_id.clone(),
            };
        }
        for (job_id, record) in &state.cron_jobs {
            if *job_id != job.job_id {
                discovered.push(CronJob {
                    job_id: job_id.clone(),
                    session_id: job.session_id.clone(),
                    schedule: record.schedule.clone(),
                    prompt: record.prompt.clone(),
                    enabled: true,
                    created_at: Some(record.created_at.clone()),
                    last_fired: state.cron_last_fired.get(job_id).cloned(),
                    next_fire: None,
                });
            }
        }

        // ── Replay reconciliation: the journal is authoritative ──
        let journaled_last = state.cron_last_fired.get(&job.job_id).cloned();
        let reconciled_last = match (&job.last_fired, &journaled_last) {
            (Some(cached), Some(journaled)) => Some(cached.clone().max(journaled.clone())),
            (None, Some(journaled)) => Some(journaled.clone()),
            (cached, None) => cached.clone(),
        };
        if reconciled_last != job.last_fired {
            // The journal is ahead of the cache: rewrite the cache forward.
            job.last_fired = reconciled_last.clone();
            *dirty = true;
        }
        let base = reconciled_last
            .as_deref()
            .or(job.created_at.as_deref())
            .and_then(parse_rfc3339_minute)
            .unwrap_or_else(|| {
                // Never fired and no creation anchor: the current minute —
                // a fresh job fires at its next scheduled instant.
                now.saturating_sub(now % 60)
            });

        // ── Coalesce (Q4 RULED): count overdue occurrences, fire ONE ──
        let mut due: Vec<u64> = Vec::new();
        let mut next = schedule.next_after(base);
        while let Some(instant) = next {
            if instant > now {
                break;
            }
            due.push(instant);
            next = schedule.next_after(instant);
            if due.len() > 8192 {
                break; // pathological schedule: bounded coalesce count
            }
        }
        let Some(&latest_due) = due.last() else {
            let new_next = schedule.next_after(base).map(rfc3339_minute);
            if job.next_fire != new_next {
                job.next_fire = new_next;
                *dirty = true;
            }
            return JobTickOutcome::Idle {
                job_id: job.job_id.clone(),
            };
        };
        let coalesced = due.len().saturating_sub(1) as u32;
        let occurrence_key = occurrence_id(&job.job_id, latest_due);

        // ── Step 2: idempotency — the reservation is the durable act ──
        if state.cron_fired_occurrences.contains(&occurrence_key) {
            if job.last_fired.as_deref() != Some(rfc3339_minute(latest_due).as_str()) {
                job.last_fired = Some(rfc3339_minute(latest_due));
                job.next_fire = schedule.next_after(latest_due).map(rfc3339_minute);
                *dirty = true;
            }
            return JobTickOutcome::AlreadyReserved {
                job_id: job.job_id.clone(),
                occurrence_id: occurrence_key,
            };
        }

        // ── Guard: same exclusion as interactive turns and forks ──
        let Ok(_guard) = self.guards.try_acquire(&journal_path) else {
            return JobTickOutcome::Busy {
                job_id: job.job_id.clone(),
            };
        };

        // ── Claim-before-fire (F-8 data-integrity) ──
        // The idempotency fold above ran BEFORE any exclusion was held: a
        // concurrent host sharing this NANO_HOME could have journaled this
        // very occurrence — or a delete — in between, and a stale pass here
        // would double-fire. Every journal writer holds this same exclusion
        // (the guard's OS layer, or the lifetime ownership lock it stands
        // in for on sessions this process owns), so a re-fold UNDER the
        // guard observes every durable reservation: check-and-reserve is
        // atomic across processes and the double-fire window is closed.
        let locked_state = match read_journal(&journal_path) {
            Ok(report) => SessionState::fold(&report.envelopes),
            Err(err) => return fail(format!("session journal unreadable under guard: {err}")),
        };
        if locked_state.cron_tombstones.contains(&job.job_id)
            && !locked_state.cron_jobs.contains_key(&job.job_id)
        {
            tombstoned.push(job.job_id.clone());
            return JobTickOutcome::Idle {
                job_id: job.job_id.clone(),
            };
        }
        if locked_state
            .cron_fired_occurrences
            .contains(&occurrence_key)
        {
            if job.last_fired.as_deref() != Some(rfc3339_minute(latest_due).as_str()) {
                job.last_fired = Some(rfc3339_minute(latest_due));
                job.next_fire = schedule.next_after(latest_due).map(rfc3339_minute);
                *dirty = true;
            }
            return JobTickOutcome::AlreadyReserved {
                job_id: job.job_id.clone(),
                occurrence_id: occurrence_key,
            };
        }

        // ── Step 3: journal-first reservation, THEN the cache ──
        let mode = mode_at_fire((self.live_mode)(&job.session_id));
        let turn_id = format!("{}-cron-{}", job.session_id, latest_due);
        let mut writer = match JournalWriter::open(&journal_path) {
            Ok(writer) => writer,
            Err(err) => return fail(format!("cannot open session journal: {err}")),
        };
        let fired = OpEnvelope::new(
            format!("{}-cronfired-{}", job.session_id, latest_due),
            "now",
            Op::CronFired {
                job_id: job.job_id.clone(),
                session_id: job.session_id.clone(),
                turn_id: turn_id.clone(),
                occurrence_id: occurrence_key.clone(),
                mode_at_fire: mode.clone(),
                coalesced,
            },
        );
        if let Err(err) = writer.append(&fired).and_then(|_| writer.sync()) {
            // Journal append failed: abort BEFORE the cache is touched — the
            // reverse window (cache advanced, reservation missing) is
            // impossible by this ordering. The occurrence fires next tick.
            return fail(format!("cannot journal CronFired: {err}"));
        }
        job.last_fired = Some(rfc3339_minute(latest_due));
        job.next_fire = schedule.next_after(latest_due).map(rfc3339_minute);
        if persist(job).is_err() {
            // The reservation IS durable; the cache persist failed. The fire
            // still aborts (no prompt injected) and reconciliation repairs
            // the stale cache — the occurrence-id check blocks any refire.
            return JobTickOutcome::AlreadyReserved {
                job_id: job.job_id.clone(),
                occurrence_id: occurrence_key,
            };
        }

        // ── Step 4: only now inject the prompt and run the turn ──
        let fire_result = executor.fire(job, &turn_id, &occurrence_key, &mode).await;
        match fire_result {
            Ok(()) => JobTickOutcome::Fired {
                job_id: job.job_id.clone(),
                occurrence_id: occurrence_key,
                coalesced,
                mode_at_fire: mode,
            },
            // The reservation stays durable: a failed injection is an
            // aborted fire (audit only), never a refire.
            Err(err) => fail(format!("fire injection failed: {err}")),
        }
    }
}

// ── cronjob tool (create/list/delete) ──────────────────────────────────────

/// The `cronjob` tool definition. A MUTATING tool: the C2 gate denies it in
/// `read_only`, prompts in `default`, and — being neither a contained write
/// nor a sandboxed shell — PROMPTS even in `full_auto` (an unattended
/// full_auto session cannot mint new scheduled work).
pub fn cronjob_tool_definition() -> nano_model::types::ToolDefinition {
    nano_model::types::ToolDefinition {
        name: "cronjob".into(),
        description: "Manage scheduled cron jobs for this session. Args: action \
             (create|list|delete); create takes schedule (5-field crontab ONLY — \
             interval sugar like \"every 10m\" is rejected) and prompt; delete \
             takes job_id. Job payload is prompt + session + schedule only."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "list", "delete"]},
                "schedule": {"type": "string"},
                "prompt": {"type": "string", "maxLength": 4000},
                "job_id": {"type": "string"}
            },
            "required": ["action"]
        }),
    }
}

/// ToolExecutor decorator adding the `cronjob` tool against a job store.
/// Every other tool delegates to the inner executor unchanged.
///
/// JOURNAL-FIRST (C11 §5.5, F-6 closure): create/delete append
/// `Op::CronCreated`/`Op::CronDeleted` through the session's coordinator —
/// the durable act — BEFORE touching the `jobs.json` cache. An append
/// failure leaves the cache untouched and the schedule unchanged (typed
/// error). A cache persist failure AFTER the durable append reports success
/// with a reconciliation note: the runner rebuilds existence from the
/// journal on its next tick (the §5.4 fire-transaction doctrine applied to
/// the lifecycle ops).
#[derive(Debug)]
pub struct CronjobExecutor<'a, T: crate::turn::ToolExecutor, S: CronStoreLike> {
    inner: &'a T,
    store: &'a S,
    session_id: String,
    coordinator: &'a nano_session::JournalCoordinator,
}

impl<'a, T: crate::turn::ToolExecutor, S: CronStoreLike> CronjobExecutor<'a, T, S> {
    pub fn new(
        inner: &'a T,
        store: &'a S,
        session_id: String,
        coordinator: &'a nano_session::JournalCoordinator,
    ) -> Self {
        Self {
            inner,
            store,
            session_id,
            coordinator,
        }
    }

    /// The op id for a lifecycle append (`{session}-cron{kind}-{nanos}` —
    /// nanosecond uniqueness, the C11 goal-op-id pattern).
    fn op_id(&self, kind: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{}-cron{}-{nanos}", self.session_id, kind)
    }
}

#[async_trait::async_trait]
impl<T: crate::turn::ToolExecutor, S: CronStoreLike> crate::turn::ToolExecutor
    for CronjobExecutor<'_, T, S>
{
    async fn execute(&self, call: &nano_model::types::ToolCall) -> crate::turn::ToolOutcome {
        if call.name != "cronjob" {
            return self.inner.execute(call).await;
        }
        let outcome = |ok: bool, output: String| crate::turn::ToolOutcome {
            ok,
            output,
            progress: crate::loop_protection::ProgressSignals::default(),
            error_kind: None,
        };
        let arg_str = |key: &str| call.arguments.get(key).and_then(|v| v.as_str());
        match arg_str("action") {
            Some("create") => {
                let (Some(schedule), Some(prompt)) = (arg_str("schedule"), arg_str("prompt"))
                else {
                    return outcome(false, "cronjob create requires schedule and prompt".into());
                };
                // 5-field cron only; interval sugar is a typed error naming
                // the cron equivalent (Q5 RULED).
                if let Err(err) = parse_schedule(schedule) {
                    return outcome(false, err.to_string());
                }
                // Defense-in-depth injection scan (§5.5): reject hits
                // fail-closed.
                if let Some(reason) = scan_cron_prompt(prompt) {
                    return outcome(false, format!("cronjob create rejected: {reason}"));
                }
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let now_secs = (nanos / 1_000_000_000) as u64;
                let created_at = rfc3339_minute(now_secs);
                let job = CronJob {
                    job_id: format!("cron-{nanos}"),
                    session_id: self.session_id.clone(),
                    schedule: schedule.to_string(),
                    prompt: prompt.to_string(),
                    enabled: true,
                    created_at: Some(created_at.clone()),
                    last_fired: None,
                    next_fire: None,
                };
                let job_id = job.job_id.clone();
                // Journal-first: the CronCreated op is the durable act of
                // creation. An append failure leaves the cache UNTOUCHED —
                // no unjournaled job ever reaches the scheduler.
                let envelope = OpEnvelope::new(
                    self.op_id("create"),
                    "now",
                    Op::CronCreated {
                        job_id: job_id.clone(),
                        session_id: self.session_id.clone(),
                        schedule: job.schedule.clone(),
                        prompt: job.prompt.clone(),
                        created_at,
                    },
                );
                if let Err(err) = self.coordinator.append(&envelope) {
                    return outcome(
                        false,
                        format!(
                            "cronjob create failed (nothing scheduled): cannot journal CronCreated: {err}"
                        ),
                    );
                }
                let mut jobs = match self.store.load() {
                    Ok(jobs) => jobs,
                    Err(err) => {
                        return outcome(
                            true,
                            format!(
                                "created cron job {job_id} (journaled; cache unreadable: {err} — the scheduler rebuilds it from the journal next tick)"
                            ),
                        );
                    }
                };
                jobs.push(job);
                match self.store.save(&jobs) {
                    Ok(()) => outcome(true, format!("created cron job {job_id}")),
                    // The journal holds the creation durably; the runner's
                    // existence reconciliation repairs the cache next tick.
                    Err(err) => outcome(
                        true,
                        format!(
                            "created cron job {job_id} (journaled; cache persist failed: {err} — the scheduler rebuilds it from the journal next tick)"
                        ),
                    ),
                }
            }
            Some("list") => match self.store.load() {
                Ok(jobs) => outcome(
                    true,
                    serde_json::to_string_pretty(&jobs).unwrap_or_default(),
                ),
                Err(err) => outcome(false, err.to_string()),
            },
            Some("delete") => {
                let Some(job_id) = arg_str("job_id") else {
                    return outcome(false, "cronjob delete requires job_id".into());
                };
                let mut jobs = match self.store.load() {
                    Ok(jobs) => jobs,
                    Err(err) => return outcome(false, err.to_string()),
                };
                let before = jobs.len();
                jobs.retain(|job| job.job_id != job_id);
                if jobs.len() == before {
                    return outcome(false, format!("no such cron job: {job_id}"));
                }
                // Journal-first: the CronDeleted tombstone lands durably
                // BEFORE the cache removal — a kill between the two leaves
                // the job cached but tombstoned, and the runner removes it
                // WITHOUT firing on the next tick.
                let envelope = OpEnvelope::new(
                    self.op_id("delete"),
                    "now",
                    Op::CronDeleted {
                        job_id: job_id.to_string(),
                        session_id: self.session_id.clone(),
                    },
                );
                if let Err(err) = self.coordinator.append(&envelope) {
                    return outcome(
                        false,
                        format!(
                            "cronjob delete failed (job still scheduled): cannot journal CronDeleted: {err}"
                        ),
                    );
                }
                match self.store.save(&jobs) {
                    Ok(()) => outcome(true, format!("deleted cron job {job_id}")),
                    // The tombstone is durable; the runner removes the stale
                    // cache entry (without firing it) next tick.
                    Err(err) => outcome(
                        true,
                        format!(
                            "deleted cron job {job_id} (journaled; cache persist failed: {err} — the scheduler removes the stale entry next tick)"
                        ),
                    ),
                }
            }
            other => outcome(
                false,
                format!("unknown cronjob action: {}", other.unwrap_or("<missing>")),
            ),
        }
    }
}
