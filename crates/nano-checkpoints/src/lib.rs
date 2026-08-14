//! Fail-closed workspace checkpoints backed by an isolated system-Git repository.

use nano_core::sensitive_path::is_sensitive_path;
use nano_session::lock::FileLock;
use nano_session::op::{
    CheckpointRestoreOutcome, Op, OpEnvelope, validate_checkpoint_created,
    validate_checkpoint_restore_begin, validate_checkpoint_restore_end,
};
use nano_session::{JournalCoordinator, NanoErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_CHECKPOINTS: usize = 32;
pub const MAX_STORE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_UNTRACKED_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_UNTRACKED_DIR_ENTRIES: usize = 200;
pub const MAX_WARNINGS: usize = 32;
pub const MAX_MANIFEST_FILES: usize = 100_000;

const STAGING_DIR: &str = ".nano-checkpoint-staging";
const DISABLED_HOOKS_PATH: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CheckpointError {
    pub kind: NanoErrorKind,
    message: &'static str,
}

impl CheckpointError {
    fn unavailable(message: &'static str) -> Self {
        Self {
            kind: NanoErrorKind::CheckpointUnavailable,
            message,
        }
    }

    fn not_found() -> Self {
        Self {
            kind: NanoErrorKind::CheckpointNotFound,
            message: "checkpoint not found",
        }
    }

    fn restore() -> Self {
        Self {
            kind: NanoErrorKind::CheckpointRestoreFailed,
            message: "checkpoint restore failed",
        }
    }
}

pub type Result<T> = std::result::Result<T, CheckpointError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub id: String,
    pub parent: Option<String>,
    pub label: Option<String>,
    pub file_count: u32,
    pub total_bytes: u64,
    pub tree_digest: String,
    created_nanos: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateResult {
    pub checkpoint: CheckpointInfo,
    pub evicted: u32,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreResult {
    pub checkpoint_id: String,
    pub safety_checkpoint_id: String,
    pub skipped_sensitive: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub checkpoint_id: String,
    pub skipped_sensitive: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoreIndex {
    checkpoints: Vec<CheckpointInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestEntry {
    path: String,
    mode: String,
    bytes: u64,
    digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    checkpoint_id: String,
    entries: Vec<ManifestEntry>,
}

#[derive(Debug)]
pub struct CheckpointStore {
    workspace: PathBuf,
    workspace_key: String,
    root: PathBuf,
    git_dir: PathBuf,
    sequence: AtomicU64,
}

impl CheckpointStore {
    pub fn open(nano_home: impl AsRef<Path>, workspace: impl AsRef<Path>) -> Result<Self> {
        let workspace = fs::canonicalize(workspace.as_ref())
            .map_err(|_| CheckpointError::unavailable("checkpoint workspace unavailable"))?;
        if !workspace.is_dir() {
            return Err(CheckpointError::unavailable(
                "checkpoint workspace unavailable",
            ));
        }
        let workspace_text = workspace.to_string_lossy();
        let workspace_key = hex_sha256(workspace_text.as_bytes())[..16].to_string();
        let root = nano_home
            .as_ref()
            .join("wayland-nano-checkpoints")
            .join(&workspace_key);
        fs::create_dir_all(root.join("manifests"))
            .map_err(|_| CheckpointError::unavailable("checkpoint store unavailable"))?;
        let store = Self {
            workspace,
            workspace_key,
            git_dir: root.join("repo.git"),
            root,
            sequence: AtomicU64::new(0),
        };
        let _lock = store.lock()?;
        store.ensure_backend()?;
        Ok(store)
    }

    pub fn workspace_key(&self) -> &str {
        &self.workspace_key
    }

    pub fn list(&self) -> Result<Vec<CheckpointInfo>> {
        let _lock = self.lock()?;
        Ok(self.load_index()?.checkpoints)
    }

    pub fn create(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        label: Option<&str>,
    ) -> Result<CreateResult> {
        let _lock = self.lock()?;
        self.create_locked(coordinator, session_id, label)
    }

    pub fn restore(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        checkpoint_id: &str,
    ) -> Result<RestoreResult> {
        let _lock = self.lock()?;
        let target = self.find_checkpoint(checkpoint_id)?;
        let safety = self.create_locked(coordinator, session_id, Some("pre-restore safety"))?;
        validate_checkpoint_restore_begin(&target.id, &safety.checkpoint.id, &target.tree_digest)
            .map_err(|_| CheckpointError::restore())?;
        self.append(
            coordinator,
            session_id,
            "restore-begin",
            Op::CheckpointRestoreBegin {
                checkpoint_id: target.id.clone(),
                safety_checkpoint_id: safety.checkpoint.id.clone(),
                file_count: target.file_count,
                tree_digest: target.tree_digest.clone(),
            },
        )?;
        let skipped_sensitive = self.apply_checkpoint(&target.id)?;
        self.append_restore_end(coordinator, session_id, &target.id, false)?;
        Ok(RestoreResult {
            checkpoint_id: target.id,
            safety_checkpoint_id: safety.checkpoint.id,
            skipped_sensitive,
        })
    }

    pub fn recover_interrupted_restore(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        journal_tail: &[OpEnvelope],
    ) -> Result<Option<Recovery>> {
        let Some(checkpoint_id) = interrupted_restore(journal_tail) else {
            return Ok(None);
        };
        let _lock = self.lock()?;
        self.find_checkpoint(&checkpoint_id)?;
        let skipped_sensitive = self.apply_checkpoint(&checkpoint_id)?;
        self.append_restore_end(coordinator, session_id, &checkpoint_id, true)?;
        Ok(Some(Recovery {
            checkpoint_id,
            skipped_sensitive,
        }))
    }

    fn create_locked(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        label: Option<&str>,
    ) -> Result<CreateResult> {
        let mut index = self.load_index()?;
        let parent = index.checkpoints.last().map(|c| c.id.clone());
        let (paths, warnings) = self.capture_paths()?;
        if paths.len() > MAX_MANIFEST_FILES {
            return Err(CheckpointError::unavailable(
                "checkpoint manifest exceeds file cap",
            ));
        }
        let index_path =
            self.root
                .join(format!("index-{}-{}.tmp", std::process::id(), self.next()));
        let _index_cleanup = RemoveFile(index_path.clone());
        self.git(&["read-tree", "--empty"], Some(&index_path), &[])?;
        for chunk in paths.chunks(256) {
            let mut args = vec![
                OsString::from("add"),
                OsString::from("-f"),
                OsString::from("--"),
            ];
            args.extend(chunk.iter().map(OsString::from));
            self.git_os(&args, Some(&index_path), &[])?;
        }
        let tree = self.git_stdout(&["write-tree"], Some(&index_path), &[])?;
        let manifest_entries = self.manifest_entries(&tree)?;
        let file_count = u32::try_from(manifest_entries.len())
            .map_err(|_| CheckpointError::unavailable("checkpoint manifest exceeds file cap"))?;
        let total_bytes = manifest_entries.iter().map(|e| e.bytes).sum();
        let tree_digest = digest_manifest(&manifest_entries)?;
        let mut commit_args = vec![OsString::from("commit-tree"), OsString::from(&tree)];
        if let Some(parent) = &parent {
            commit_args.push(OsString::from("-p"));
            commit_args.push(OsString::from(parent));
        }
        let checkpoint_id = self.git_stdout_os(
            &commit_args,
            None,
            &[
                ("GIT_AUTHOR_NAME", "Wayland Nano"),
                ("GIT_AUTHOR_EMAIL", "wayland-nano@localhost"),
                ("GIT_COMMITTER_NAME", "Wayland Nano"),
                ("GIT_COMMITTER_EMAIL", "wayland-nano@localhost"),
            ],
        )?;
        let checkpoint = CheckpointInfo {
            id: checkpoint_id.clone(),
            parent,
            label: label.map(|v| v.chars().take(128).collect()),
            file_count,
            total_bytes,
            tree_digest,
            created_nanos: now_nanos(),
        };
        let mut evicted = 0u32;
        index.checkpoints.push(checkpoint.clone());
        while index.checkpoints.len() > MAX_CHECKPOINTS
            || index.checkpoints.iter().map(|c| c.total_bytes).sum::<u64>() > MAX_STORE_BYTES
        {
            index.checkpoints.remove(0);
            evicted = evicted.saturating_add(1);
        }
        validate_checkpoint_created(
            &checkpoint.id,
            &self.workspace_key,
            checkpoint.parent.as_deref(),
            &checkpoint.tree_digest,
        )
        .map_err(|_| CheckpointError::unavailable("checkpoint metadata invalid"))?;
        self.append(
            coordinator,
            session_id,
            "created",
            Op::CheckpointCreated {
                checkpoint_id: checkpoint.id.clone(),
                workspace_key: self.workspace_key.clone(),
                parent: checkpoint.parent.clone(),
                file_count,
                total_bytes,
                tree_digest: checkpoint.tree_digest.clone(),
                evicted,
            },
        )?;
        self.write_manifest(&Manifest {
            checkpoint_id,
            entries: manifest_entries,
        })?;
        self.save_index(&index)?;
        Ok(CreateResult {
            checkpoint,
            evicted,
            warnings,
        })
    }

    fn ensure_backend(&self) -> Result<()> {
        let probe = Command::new("git")
            .arg("--version")
            .output()
            .map_err(|_| CheckpointError::unavailable("system git unavailable"))?;
        if !probe.status.success() {
            return Err(CheckpointError::unavailable("system git unavailable"));
        }
        let repo_probe = scrubbed(Command::new("git"))
            .arg("-c")
            .arg("safe.bareRepository=explicit")
            .arg("-c")
            .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
            .arg("-C")
            .arg(&self.workspace)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|_| CheckpointError::unavailable("workspace git unavailable"))?;
        if !repo_probe.status.success() {
            return Err(CheckpointError::unavailable("workspace git unavailable"));
        }
        let reported_root = PathBuf::from(String::from_utf8_lossy(&repo_probe.stdout).trim());
        let canonical_root = fs::canonicalize(reported_root)
            .map_err(|_| CheckpointError::unavailable("workspace git unavailable"))?;
        if canonical_root != self.workspace {
            return Err(CheckpointError::unavailable(
                "checkpoint workspace must be the git root",
            ));
        }
        if !self.git_dir.is_dir() {
            let output = scrubbed(Command::new("git"))
                .arg("-c")
                .arg("safe.bareRepository=explicit")
                .arg("-c")
                .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
                .arg("init")
                .arg("--bare")
                .arg(&self.git_dir)
                .output()
                .map_err(|_| {
                    CheckpointError::unavailable("checkpoint git initialization failed")
                })?;
            ensure_success(output, NanoErrorKind::CheckpointUnavailable)?;
        }
        Ok(())
    }

    fn capture_paths(&self) -> Result<(Vec<String>, Vec<String>)> {
        let tracked = self.workspace_git_paths(&["ls-files", "-c", "-z"])?;
        let untracked =
            self.workspace_git_paths(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        let mut warnings = Vec::new();
        let mut dir_counts: BTreeMap<String, usize> = BTreeMap::new();
        for path in &untracked {
            if let Some(first) = Path::new(path).components().next() {
                *dir_counts
                    .entry(first.as_os_str().to_string_lossy().into_owned())
                    .or_default() += 1;
            }
        }
        let oversized_dirs: BTreeSet<String> = dir_counts
            .into_iter()
            .filter_map(|(dir, count)| (count > MAX_UNTRACKED_DIR_ENTRIES).then_some(dir))
            .collect();
        let tracked: BTreeSet<_> = tracked.into_iter().collect();
        let mut all = tracked.clone();
        all.extend(untracked);
        let mut included = Vec::new();
        for path in all {
            let relative = Path::new(&path);
            if !safe_relative(relative) || excluded_path(relative) || sensitive(relative) {
                if sensitive(relative) {
                    bounded_warning(&mut warnings, format!("sensitive path skipped: {path}"));
                }
                continue;
            }
            let is_untracked = !tracked.contains(&path);
            if is_untracked {
                let first = relative
                    .components()
                    .next()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned());
                if first.as_ref().is_some_and(|d| oversized_dirs.contains(d)) {
                    bounded_warning(
                        &mut warnings,
                        format!(
                            "large untracked directory skipped: {}",
                            first.unwrap_or_default()
                        ),
                    );
                    continue;
                }
                let size = fs::symlink_metadata(self.workspace.join(relative))
                    .map(|m| m.len())
                    .unwrap_or(0);
                if size > MAX_UNTRACKED_FILE_BYTES {
                    bounded_warning(
                        &mut warnings,
                        format!("large untracked file skipped: {path}"),
                    );
                    continue;
                }
            }
            included.push(path);
        }
        included.sort();
        Ok((included, warnings))
    }

    fn apply_checkpoint(&self, checkpoint_id: &str) -> Result<u32> {
        let manifest = self.read_manifest(checkpoint_id)?;
        let staging = self.workspace.join(STAGING_DIR);
        let _cleanup = RemoveDir(staging.clone());
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|_| CheckpointError::restore())?;
        }
        fs::create_dir(&staging).map_err(|_| CheckpointError::restore())?;
        for entry in &manifest.entries {
            let rel = Path::new(&entry.path);
            if !safe_relative(rel) || sensitive(rel) {
                continue;
            }
            let staged = staging.join(rel);
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent).map_err(|_| CheckpointError::restore())?;
            }
            let object = format!("{checkpoint_id}:{}", entry.path);
            let bytes = self
                .git_os(
                    &[
                        OsString::from("cat-file"),
                        OsString::from("blob"),
                        OsString::from(object),
                    ],
                    None,
                    &[],
                )
                .map_err(|_| CheckpointError::restore())?
                .stdout;
            materialize_entry(&staged, &entry.mode, &bytes)
                .map_err(|_| CheckpointError::restore())?;
        }
        let target: BTreeSet<_> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
        let (current, _) = self
            .capture_paths()
            .map_err(|_| CheckpointError::restore())?;
        let mut skipped = self
            .current_sensitive_count()
            .map_err(|_| CheckpointError::restore())?;
        for path in current {
            let rel = Path::new(&path);
            if sensitive(rel) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if !target.contains(path.as_str()) {
                remove_path(&self.workspace.join(rel)).map_err(|_| CheckpointError::restore())?;
            }
        }
        for entry in &manifest.entries {
            let rel = Path::new(&entry.path);
            if sensitive(rel) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let source = staging.join(rel);
            let destination = self.workspace.join(rel);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| CheckpointError::restore())?;
            }
            remove_path(&destination).map_err(|_| CheckpointError::restore())?;
            fs::rename(&source, &destination).map_err(|_| CheckpointError::restore())?;
        }
        Ok(skipped)
    }

    fn manifest_entries(&self, tree: &str) -> Result<Vec<ManifestEntry>> {
        let bytes = self.git_bytes(&["ls-tree", "-r", "-l", "-z", tree], None, &[])?;
        let mut entries = Vec::new();
        for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let text = String::from_utf8_lossy(record);
            let (meta, path) = text
                .split_once('\t')
                .ok_or_else(|| CheckpointError::unavailable("checkpoint tree invalid"))?;
            let fields: Vec<_> = meta.split_whitespace().collect();
            if fields.len() < 4 {
                return Err(CheckpointError::unavailable("checkpoint tree invalid"));
            }
            let size = fields[3]
                .parse()
                .map_err(|_| CheckpointError::unavailable("checkpoint tree invalid"))?;
            entries.push(ManifestEntry {
                path: path.to_string(),
                mode: fields[0].to_string(),
                bytes: size,
                digest: fields[2].to_string(),
            });
        }
        Ok(entries)
    }

    fn workspace_git_paths(&self, args: &[&str]) -> Result<Vec<String>> {
        let output = scrubbed(Command::new("git"))
            .arg("-c")
            .arg("safe.bareRepository=explicit")
            .arg("-c")
            .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
            .args(args)
            .current_dir(&self.workspace)
            .output()
            .map_err(|_| CheckpointError::unavailable("workspace git scan failed"))?;
        ensure_success(output, NanoErrorKind::CheckpointUnavailable).map(|o| {
            o.stdout
                .split(|b| *b == 0)
                .filter(|v| !v.is_empty())
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .collect()
        })
    }

    fn current_sensitive_count(&self) -> Result<u32> {
        let mut paths = self.workspace_git_paths(&["ls-files", "-c", "-z"])?;
        paths.extend(self.workspace_git_paths(&[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ])?);
        paths.sort();
        paths.dedup();
        Ok(paths
            .iter()
            .filter(|path| sensitive(Path::new(path)))
            .count()
            .try_into()
            .unwrap_or(u32::MAX))
    }

    fn append(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        phase: &str,
        op: Op,
    ) -> Result<()> {
        let envelope = OpEnvelope::new(
            format!(
                "{session_id}-checkpoint-{phase}-{}-{}",
                now_nanos(),
                self.next()
            ),
            now_nanos().to_string(),
            op,
        );
        coordinator
            .append(&envelope)
            .map_err(|_| CheckpointError::unavailable("checkpoint journal unavailable"))?;
        Ok(())
    }

    fn append_restore_end(
        &self,
        coordinator: &JournalCoordinator,
        session_id: &str,
        id: &str,
        recovered: bool,
    ) -> Result<()> {
        validate_checkpoint_restore_end(id).map_err(|_| CheckpointError::restore())?;
        self.append(
            coordinator,
            session_id,
            "restore-end",
            Op::CheckpointRestoreEnd {
                checkpoint_id: id.to_string(),
                outcome: CheckpointRestoreOutcome::Applied,
                recovered,
            },
        )
    }

    fn find_checkpoint(&self, id: &str) -> Result<CheckpointInfo> {
        self.load_index()?
            .checkpoints
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(CheckpointError::not_found)
    }

    fn lock(&self) -> Result<FileLock> {
        FileLock::try_acquire(&self.root.join("store.lock"))
            .map_err(|_| CheckpointError::unavailable("checkpoint store busy"))
    }

    fn load_index(&self) -> Result<StoreIndex> {
        let path = self.root.join("index.json");
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| CheckpointError::unavailable("checkpoint index invalid")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreIndex::default()),
            Err(_) => Err(CheckpointError::unavailable("checkpoint index unavailable")),
        }
    }

    fn save_index(&self, index: &StoreIndex) -> Result<()> {
        atomic_json(&self.root.join("index.json"), index)
    }

    fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        atomic_json(
            &self
                .root
                .join("manifests")
                .join(format!("{}.json", manifest.checkpoint_id)),
            manifest,
        )
    }

    fn read_manifest(&self, id: &str) -> Result<Manifest> {
        let bytes = fs::read(self.root.join("manifests").join(format!("{id}.json")))
            .map_err(|_| CheckpointError::not_found())?;
        serde_json::from_slice(&bytes).map_err(|_| CheckpointError::restore())
    }

    fn next(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    fn git(&self, args: &[&str], index: Option<&Path>, env: &[(&str, &str)]) -> Result<Output> {
        self.git_os(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            index,
            env,
        )
    }
    fn git_stdout(
        &self,
        args: &[&str],
        index: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<String> {
        self.git_stdout_os(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            index,
            env,
        )
    }
    fn git_stdout_os(
        &self,
        args: &[OsString],
        index: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<String> {
        let output = self.git_os(args, index, env)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
    fn git_bytes(
        &self,
        args: &[&str],
        index: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>> {
        Ok(self.git(args, index, env)?.stdout)
    }
    fn git_os(
        &self,
        args: &[OsString],
        index: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<Output> {
        let mut command = scrubbed(Command::new("git"));
        command
            .arg("-c")
            .arg("safe.bareRepository=explicit")
            .arg("-c")
            .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
            .args(args)
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.workspace);
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command
            .output()
            .map_err(|_| CheckpointError::unavailable("checkpoint git command unavailable"))?;
        ensure_success(output, NanoErrorKind::CheckpointUnavailable)
    }
}

pub fn interrupted_restore(tail: &[OpEnvelope]) -> Option<String> {
    let mut open = None;
    for envelope in tail {
        match &envelope.op {
            Op::CheckpointRestoreBegin { checkpoint_id, .. } => open = Some(checkpoint_id.clone()),
            Op::CheckpointRestoreEnd { checkpoint_id, .. }
                if open.as_deref() == Some(checkpoint_id) =>
            {
                open = None
            }
            _ => {}
        }
    }
    open
}

fn scrubbed(mut command: Command) -> Command {
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
    ] {
        command.env_remove(key);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn ensure_success(output: Output, kind: NanoErrorKind) -> Result<Output> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(CheckpointError {
            kind,
            message: "checkpoint git command failed",
        })
    }
}

fn safe_relative(path: &Path) -> bool {
    !path.is_absolute() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn excluded_path(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == OsStr::new(".git") || c.as_os_str() == OsStr::new(STAGING_DIR))
}
fn sensitive(path: &Path) -> bool {
    path.ancestors()
        .take_while(|p| !p.as_os_str().is_empty())
        .any(is_sensitive_path)
}
fn bounded_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_WARNINGS && !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
fn materialize_entry(path: &Path, mode: &str, bytes: &[u8]) -> std::io::Result<()> {
    if mode == "120000" {
        let target = PathBuf::from(String::from_utf8_lossy(bytes).into_owned());
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, path)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, path)?;
        return Ok(());
    }
    fs::write(path, bytes)?;
    #[cfg(unix)]
    if mode == "100755" {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CheckpointError::unavailable("checkpoint metadata invalid"))?;
    fs::write(&tmp, bytes)
        .map_err(|_| CheckpointError::unavailable("checkpoint metadata unavailable"))?;
    if path.exists() {
        fs::remove_file(path)
            .map_err(|_| CheckpointError::unavailable("checkpoint metadata unavailable"))?;
    }
    fs::rename(tmp, path)
        .map_err(|_| CheckpointError::unavailable("checkpoint metadata unavailable"))
}
fn digest_manifest(entries: &[ManifestEntry]) -> Result<String> {
    let bytes = serde_json::to_vec(entries)
        .map_err(|_| CheckpointError::unavailable("checkpoint manifest invalid"))?;
    Ok(hex_sha256(&bytes))
}
fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

struct RemoveFile(PathBuf);
impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct RemoveDir(PathBuf);
impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_session::reader::read_journal;
    use tempfile::TempDir;

    fn run(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git command did not complete");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn fixture() -> (TempDir, PathBuf, PathBuf, JournalCoordinator) {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        let home = dir.path().join("home");
        fs::create_dir_all(&workspace).unwrap();
        run(&workspace, &["init"]);
        fs::write(workspace.join("tracked.txt"), "one").unwrap();
        run(&workspace, &["add", "tracked.txt"]);
        run(
            &workspace,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "initial",
            ],
        );
        let coordinator = JournalCoordinator::open(dir.path().join("journal.jsonl")).unwrap();
        (dir, workspace, home, coordinator)
    }

    #[test]
    fn create_modify_restore_and_git_metadata_untouched() {
        let (_dir, workspace, home, coordinator) = fixture();
        let store = CheckpointStore::open(&home, &workspace).unwrap();
        let status_before = run(&workspace, &["status", "--porcelain"]);
        let head_before = run(&workspace, &["rev-parse", "HEAD"]);
        let created = store.create(&coordinator, "s", None).unwrap();
        fs::write(workspace.join("tracked.txt"), "two").unwrap();
        fs::write(workspace.join("new.txt"), "new").unwrap();
        let restored = store
            .restore(&coordinator, "s", &created.checkpoint.id)
            .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
            "one"
        );
        assert!(!workspace.join("new.txt").exists());
        assert!(!workspace.join(STAGING_DIR).exists());
        assert_eq!(run(&workspace, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(run(&workspace, &["status", "--porcelain"]), status_before);
        assert_ne!(restored.safety_checkpoint_id, created.checkpoint.id);
    }

    #[test]
    fn ignored_and_sensitive_files_survive_restore() {
        let (_dir, workspace, home, coordinator) = fixture();
        fs::write(workspace.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(workspace.join("ignored.txt"), "ignored").unwrap();
        fs::write(workspace.join(".env"), "before").unwrap();
        let store = CheckpointStore::open(&home, &workspace).unwrap();
        let created = store.create(&coordinator, "s", None).unwrap();
        fs::write(workspace.join("ignored.txt"), "after").unwrap();
        fs::write(workspace.join(".env"), "after-secret").unwrap();
        let restored = store
            .restore(&coordinator, "s", &created.checkpoint.id)
            .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.join("ignored.txt")).unwrap(),
            "after"
        );
        assert_eq!(
            fs::read_to_string(workspace.join(".env")).unwrap(),
            "after-secret"
        );
        assert_eq!(restored.skipped_sensitive, 1);
    }

    #[test]
    fn interrupted_tail_is_recovered_and_closed() {
        let (_dir, workspace, home, coordinator) = fixture();
        let store = CheckpointStore::open(&home, &workspace).unwrap();
        let created = store.create(&coordinator, "s", None).unwrap();
        fs::write(workspace.join("tracked.txt"), "changed").unwrap();
        let begin = OpEnvelope::new(
            "begin",
            "now",
            Op::CheckpointRestoreBegin {
                checkpoint_id: created.checkpoint.id.clone(),
                safety_checkpoint_id: created.checkpoint.id.clone(),
                file_count: created.checkpoint.file_count,
                tree_digest: created.checkpoint.tree_digest.clone(),
            },
        );
        coordinator.append(&begin).unwrap();
        let recovery = store
            .recover_interrupted_restore(&coordinator, "s", &[begin])
            .unwrap()
            .unwrap();
        assert_eq!(recovery.checkpoint_id, created.checkpoint.id);
        assert_eq!(
            fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
            "one"
        );
        let journal = read_journal(&_dir.path().join("journal.jsonl")).unwrap();
        assert!(matches!(
            journal.envelopes.last().unwrap().op,
            Op::CheckpointRestoreEnd {
                recovered: true,
                ..
            }
        ));
    }
}
