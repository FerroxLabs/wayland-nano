use crate::{
    PluginError,
    manifest::{self, McpServer, PathSource, PluginKind, PluginManifest},
    plan::InstallPlan,
    source::RegistrySource,
};
use nano_agent::mcp::{McpServerSpec, SpecSource, Transport};
use nano_session::lock::{FileLock, LockError};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryRecord {
    pub name: String,
    pub source: RegistrySource,
    pub reg_key: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPlugin {
    pub plugin: String,
    pub registry: String,
    pub kind: PluginKind,
    pub source: String,
    pub resolved_ref: String,
    pub archive_sha256: String,
    pub manifest_sha256: String,
    pub installed_at: u64,
    pub instance_id: Option<String>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    registries: Vec<RegistryRecord>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LockFile {
    plugins: BTreeMap<String, Pin>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    archive_sha256: String,
    manifest_sha256: String,
    installed: InstalledPlugin,
}
#[derive(serde::Serialize)]
struct Receipt<'a> {
    receipt_id: String,
    ts_unix: u64,
    op: &'a str,
    plugin: Option<&'a str>,
    registry: Option<&'a str>,
    source_kind: &'a str,
    r#ref: Option<&'a str>,
    archive_sha256: Option<&'a str>,
    manifest_sha256: Option<&'a str>,
    instance_id: Option<&'a str>,
    outcome: &'a str,
}
struct JournalEvent<'a> {
    op: &'a str,
    plugin: Option<&'a str>,
    registry: Option<&'a str>,
    source_kind: &'a str,
    resolved_ref: Option<&'a str>,
    archive_sha256: Option<&'a str>,
    manifest_sha256: Option<&'a str>,
    instance_id: Option<&'a str>,
}

pub struct PluginStore {
    root: PathBuf,
}
impl PluginStore {
    pub fn new(nano_home: &Path) -> Self {
        Self {
            root: nano_home.join("wayland-nano-plugins"),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn ensure(&self) -> Result<(), PluginError> {
        for p in [
            self.root.clone(),
            self.root.join("registries"),
            self.root.join("installed"),
        ] {
            fs::create_dir_all(&p).map_err(|e| PluginError::io(p, e))?;
        }
        Ok(())
    }
    fn lock(&self) -> Result<FileLock, PluginError> {
        self.ensure()?;
        FileLock::try_acquire(&self.root.join(".lock")).map_err(|e| match e {
            LockError::Busy => PluginError::PluginStoreBusy,
            LockError::Io(x) => PluginError::io(self.root.join(".lock"), x),
        })
    }
    pub fn registries(&self) -> Result<Vec<RegistryRecord>, PluginError> {
        Ok(self.load_registries()?.registries)
    }
    pub fn add_registry(&self, name: &str, source: RegistrySource) -> Result<(), PluginError> {
        manifest::validate_name(name)?;
        let _g = self.lock()?;
        let mut f = self.load_registries()?;
        if f.registries.iter().any(|r| r.name == name) {
            return Err(PluginError::Invalid(format!("registry exists: {name}")));
        }
        let rec = RegistryRecord {
            name: name.into(),
            reg_key: source.reg_key(),
            source,
        };
        self.journal(JournalEvent {
            op: "registry_add",
            plugin: None,
            registry: Some(name),
            source_kind: source_kind(&rec.source),
            resolved_ref: None,
            archive_sha256: None,
            manifest_sha256: None,
            instance_id: None,
        })?;
        f.registries.push(rec);
        self.atomic_json(&self.root.join("registries.json"), &f)
    }
    pub fn remove_registry(&self, name: &str) -> Result<(), PluginError> {
        let _g = self.lock()?;
        let mut f = self.load_registries()?;
        let before = f.registries.len();
        let kind = f
            .registries
            .iter()
            .find(|r| r.name == name)
            .map(|r| source_kind(&r.source))
            .ok_or_else(|| PluginError::NotFound(name.into()))?;
        if self.installed()?.iter().any(|p| p.registry == name) {
            return Err(PluginError::Invalid(
                "registry has installed plugins".into(),
            ));
        }
        self.journal(JournalEvent {
            op: "registry_remove",
            plugin: None,
            registry: Some(name),
            source_kind: kind,
            resolved_ref: None,
            archive_sha256: None,
            manifest_sha256: None,
            instance_id: None,
        })?;
        f.registries.retain(|r| r.name != name);
        debug_assert_ne!(before, f.registries.len());
        self.atomic_json(&self.root.join("registries.json"), &f)
    }
    pub fn installed(&self) -> Result<Vec<InstalledPlugin>, PluginError> {
        Ok(self
            .load_lock()?
            .plugins
            .into_values()
            .map(|p| p.installed)
            .collect())
    }
    pub fn preview_install(
        &self,
        registry: &RegistryRecord,
        registry_root: &Path,
        resolved_ref: &str,
        archive_sha: &str,
        plugin_name: &str,
    ) -> Result<InstallPlan, PluginError> {
        let market = manifest::MarketplaceManifest::load(&registry_root.join("marketplace.json"))?;
        let entry = market
            .plugins
            .iter()
            .find(|p| p.name == plugin_name)
            .ok_or_else(|| PluginError::NotFound(plugin_name.into()))?;
        let PathSource::Path { path } = &entry.source;
        let plugin_root = manifest::join_under(registry_root, path)?;
        let pm = PluginManifest::load(&plugin_root.join("plugin.json"))?;
        if pm.name != entry.name {
            return Err(PluginError::Invalid(
                "marketplace and plugin names disagree".into(),
            ));
        }
        let skills = discover_skills(&plugin_root, &pm)?;
        let lock = self.load_lock()?;
        InstallPlan::build(
            registry.source.display(),
            resolved_ref.into(),
            archive_sha.into(),
            &registry.name,
            &pm,
            skills,
            &self.installed_skill_names(&lock)?,
        )
    }
    pub fn install(
        &self,
        registry: &RegistryRecord,
        registry_root: &Path,
        resolved_ref: &str,
        archive_sha: &str,
        plugin_name: &str,
    ) -> Result<(InstallPlan, InstalledPlugin), PluginError> {
        let _g = self.lock()?;
        let market = manifest::MarketplaceManifest::load(&registry_root.join("marketplace.json"))?;
        let entry = market
            .plugins
            .iter()
            .find(|p| p.name == plugin_name)
            .ok_or_else(|| PluginError::NotFound(plugin_name.into()))?;
        let PathSource::Path { path } = &entry.source;
        let plugin_root = manifest::join_under(registry_root, path)?;
        let manifest_path = plugin_root.join("plugin.json");
        let pm = PluginManifest::load(&manifest_path)?;
        if pm.name != entry.name {
            return Err(PluginError::Invalid(
                "marketplace and plugin names disagree".into(),
            ));
        }
        let manifest_bytes =
            fs::read(&manifest_path).map_err(|e| PluginError::io(&manifest_path, e))?;
        let manifest_sha = format!("{:x}", Sha256::digest(&manifest_bytes));
        let key = format!("{}/{}", registry.name, pm.name);
        let mut lock = self.load_lock()?;
        if let Some(pin) = lock.plugins.get(&key) {
            if pin.archive_sha256 != archive_sha || pin.manifest_sha256 != manifest_sha {
                return Err(PluginError::PinMismatch {
                    plugin: key,
                    expected: format!("{}/{}", pin.archive_sha256, pin.manifest_sha256),
                    observed: format!("{archive_sha}/{manifest_sha}"),
                });
            }
            return Err(PluginError::AlreadyInstalled(key));
        }
        let skills = discover_skills(&plugin_root, &pm)?;
        let installed_skills = self.installed_skill_names(&lock)?;
        let plan = InstallPlan::build(
            registry.source.display(),
            resolved_ref.into(),
            archive_sha.into(),
            &registry.name,
            &pm,
            skills.clone(),
            &installed_skills,
        )?;
        let dest = self
            .root
            .join("installed")
            .join(format!("{}@{}", pm.name, registry.name));
        let staging =
            self.root
                .join("installed")
                .join(format!(".staging-{}-{}", std::process::id(), now()));
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(|e| PluginError::io(&staging, e))?
        }
        fs::create_dir_all(&staging).map_err(|e| PluginError::io(&staging, e))?;
        let stage_result = (|| {
            match &pm.kind {
                PluginKind::Skills => {
                    copy_tree(&plugin_root.join("skills"), &staging.join("skills"))?
                }
                PluginKind::McpServer => {
                    let server = pm
                        .mcp_server
                        .as_ref()
                        .ok_or_else(|| PluginError::Invalid("missing server".into()))?;
                    self.atomic_json(&staging.join("server.json"), server)?
                }
            }
            let installed = InstalledPlugin {
                plugin: pm.name.clone(),
                registry: registry.name.clone(),
                kind: pm.kind,
                source: registry.source.display(),
                resolved_ref: resolved_ref.into(),
                archive_sha256: archive_sha.into(),
                manifest_sha256: manifest_sha.clone(),
                installed_at: now(),
                instance_id: None,
            };
            self.atomic_json(&staging.join("provenance.json"), &installed)?;
            Ok::<_, PluginError>(installed)
        })();
        let installed = match stage_result {
            Ok(x) => x,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        self.journal(JournalEvent {
            op: "install",
            plugin: Some(&installed.plugin),
            registry: Some(&installed.registry),
            source_kind: source_kind(&registry.source),
            resolved_ref: Some(resolved_ref),
            archive_sha256: Some(archive_sha),
            manifest_sha256: Some(&manifest_sha),
            instance_id: None,
        })?;
        if dest.exists() {
            let _ = fs::remove_dir_all(&staging);
            return Err(PluginError::AlreadyInstalled(key));
        }
        fs::rename(&staging, &dest).map_err(|e| PluginError::io(&dest, e))?;
        lock.plugins.insert(
            key,
            Pin {
                archive_sha256: archive_sha.into(),
                manifest_sha256: manifest_sha,
                installed: installed.clone(),
            },
        );
        if let Err(e) = self.atomic_json(&self.root.join("plugins.lock.json"), &lock) {
            let _ = fs::remove_dir_all(&dest);
            return Err(e);
        };
        let verify = self.load_lock()?;
        if !verify
            .plugins
            .contains_key(&format!("{}/{}", registry.name, pm.name))
        {
            return Err(PluginError::Invalid(
                "lockfile reload verification failed".into(),
            ));
        }
        Ok((plan, installed))
    }
    pub fn remove_plugin(&self, plugin: &str, registry: &str) -> Result<(), PluginError> {
        let _g = self.lock()?;
        let key = format!("{registry}/{plugin}");
        let mut lock = self.load_lock()?;
        let pin = lock
            .plugins
            .get(&key)
            .ok_or_else(|| PluginError::NotFound(key.clone()))?
            .clone();
        self.journal(JournalEvent {
            op: "remove",
            plugin: Some(plugin),
            registry: Some(registry),
            source_kind: "installed",
            resolved_ref: Some(&pin.installed.resolved_ref),
            archive_sha256: Some(&pin.archive_sha256),
            manifest_sha256: Some(&pin.manifest_sha256),
            instance_id: pin.installed.instance_id.as_deref(),
        })?;
        let dest = self
            .root
            .join("installed")
            .join(format!("{plugin}@{registry}"));
        if dest.exists() {
            fs::remove_dir_all(&dest).map_err(|e| PluginError::io(&dest, e))?
        }
        lock.plugins.remove(&key);
        self.atomic_json(&self.root.join("plugins.lock.json"), &lock)
    }
    pub fn plugin_mcp_specs(&self) -> Result<Vec<McpServerSpec>, PluginError> {
        let lock = self.load_lock()?;
        let mut out = Vec::new();
        for pin in lock.plugins.values() {
            if pin.installed.kind != PluginKind::McpServer {
                continue;
            }
            let p = self
                .root
                .join("installed")
                .join(format!(
                    "{}@{}",
                    pin.installed.plugin, pin.installed.registry
                ))
                .join("server.json");
            let bytes = fs::read(&p).map_err(|e| PluginError::io(&p, e))?;
            let server: McpServer = serde_json::from_slice(&bytes)?;
            match server {
                McpServer::Stdio(s) => out.push(McpServerSpec {
                    name: s.name,
                    transport: Transport::Stdio {
                        command: s.command,
                        args: s.args,
                        env: s.env.into_iter().collect(),
                    },
                    source: SpecSource::Marketplace(format!(
                        "{}/{}",
                        pin.installed.registry, pin.installed.plugin
                    )),
                }),
                McpServer::Http(_) => return Err(PluginError::HttpTransportRefused),
            }
        }
        Ok(out)
    }
    pub fn plugin_skill_roots(&self) -> Result<Vec<PathBuf>, PluginError> {
        let lock = self.load_lock()?;
        let mut roots = Vec::new();
        for pin in lock.plugins.values() {
            if pin.installed.kind == PluginKind::Skills {
                let p = self
                    .root
                    .join("installed")
                    .join(format!(
                        "{}@{}",
                        pin.installed.plugin, pin.installed.registry
                    ))
                    .join("skills");
                if !p.is_dir() {
                    return Err(PluginError::Invalid("installed skill root missing".into()));
                }
                roots.push(p)
            }
        }
        Ok(roots)
    }
    fn installed_skill_names(&self, lock: &LockFile) -> Result<Vec<String>, PluginError> {
        let mut names = Vec::new();
        for p in lock.plugins.values() {
            if p.installed.kind == PluginKind::Skills {
                let root = self
                    .root
                    .join("installed")
                    .join(format!("{}@{}", p.installed.plugin, p.installed.registry))
                    .join("skills");
                if root.exists() {
                    for e in fs::read_dir(root).map_err(|e| PluginError::io(&self.root, e))? {
                        let e = e.map_err(|x| PluginError::io(&self.root, x))?;
                        names.push(e.file_name().to_string_lossy().into())
                    }
                }
            }
        }
        Ok(names)
    }
    fn load_registries(&self) -> Result<RegistryFile, PluginError> {
        load_json_or_default(&self.root.join("registries.json"))
    }
    fn load_lock(&self) -> Result<LockFile, PluginError> {
        load_json_or_default(&self.root.join("plugins.lock.json"))
    }
    fn atomic_json<T: serde::Serialize>(&self, path: &Path, value: &T) -> Result<(), PluginError> {
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(value)?;
        {
            let mut f = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp)
                .map_err(|e| PluginError::io(&tmp, e))?;
            f.write_all(&bytes).map_err(|e| PluginError::io(&tmp, e))?;
            f.sync_all().map_err(|e| PluginError::io(&tmp, e))?;
        }
        fs::rename(&tmp, path).map_err(|e| PluginError::io(path, e))
    }
    fn journal(&self, event: JournalEvent<'_>) -> Result<(), PluginError> {
        let ts = now();
        let id_seed = format!(
            "{ts}:{op}:{}:{}",
            event.plugin.unwrap_or(""),
            event.registry.unwrap_or(""),
            op = event.op,
        );
        let id = format!("rcpt_{}", &format!("{:x}", Sha256::digest(id_seed))[..16]);
        let receipt = Receipt {
            receipt_id: id,
            ts_unix: ts,
            op: event.op,
            plugin: event.plugin,
            registry: event.registry,
            source_kind: event.source_kind,
            r#ref: event.resolved_ref,
            archive_sha256: event.archive_sha256,
            manifest_sha256: event.manifest_sha256,
            instance_id: event.instance_id,
            outcome: "planned",
        };
        let path = self.root.join("journal.jsonl");
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| PluginError::io(&path, e))?;
        serde_json::to_writer(&mut f, &receipt)?;
        f.write_all(b"\n").map_err(|e| PluginError::io(&path, e))?;
        f.sync_all().map_err(|e| PluginError::io(&path, e))
    }
}
fn load_json_or_default<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> Result<T, PluginError> {
    match fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(PluginError::io(path, e)),
    }
}
fn discover_skills(root: &Path, pm: &PluginManifest) -> Result<Vec<String>, PluginError> {
    if pm.kind != PluginKind::Skills {
        return Ok(Vec::new());
    }
    let skills = root.join("skills");
    let mut out = Vec::new();
    for e in fs::read_dir(&skills).map_err(|e| PluginError::io(&skills, e))? {
        let e = e.map_err(|x| PluginError::io(&skills, x))?;
        let ft = e.file_type().map_err(|x| PluginError::io(e.path(), x))?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() && e.path().join("SKILL.md").is_file() {
            let n = e.file_name().to_string_lossy().into_owned();
            manifest::validate_name(&n)?;
            out.push(n)
        }
    }
    if out.is_empty() {
        return Err(PluginError::Invalid(
            "skills plugin contains no skills/*/SKILL.md".into(),
        ));
    }
    Ok(out)
}
fn copy_tree(src: &Path, dst: &Path) -> Result<(), PluginError> {
    fs::create_dir_all(dst).map_err(|e| PluginError::io(dst, e))?;
    for e in fs::read_dir(src).map_err(|e| PluginError::io(src, e))? {
        let e = e.map_err(|x| PluginError::io(src, x))?;
        let ft = e.file_type().map_err(|x| PluginError::io(e.path(), x))?;
        let to = dst.join(e.file_name());
        if ft.is_symlink() {
            continue;
        } else if ft.is_dir() {
            copy_tree(&e.path(), &to)?
        } else if ft.is_file() {
            fs::copy(e.path(), &to).map_err(|x| PluginError::io(&to, x))?;
        }
    }
    Ok(())
}
fn source_kind(s: &RegistrySource) -> &'static str {
    match s {
        RegistrySource::LocalDir { .. } => "local_dir",
        RegistrySource::Github { .. } => "github",
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The activation adapters consumed by the host bootstrap paths. An ABSENT
/// store (a fresh nano_home that never installed a plugin) resolves empty;
/// a CORRUPT installed-state document (unparseable lockfile, a pinned
/// plugin whose installed payload is gone) is a typed error — the caller
/// fails startup closed, never a silent downgrade to zero capabilities.
pub fn plugin_mcp_specs(nano_home: &Path) -> Result<Vec<McpServerSpec>, PluginError> {
    PluginStore::new(nano_home).plugin_mcp_specs()
}
pub fn plugin_skill_roots(nano_home: &Path) -> Result<Vec<PathBuf>, PluginError> {
    PluginStore::new(nano_home).plugin_skill_roots()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_store_resolves_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(plugin_mcp_specs(d.path()).unwrap().is_empty());
        assert!(plugin_skill_roots(d.path()).unwrap().is_empty());
    }

    #[test]
    fn corrupt_lockfile_is_a_typed_error_never_a_silent_zero() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("wayland-nano-plugins");
        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("plugins.lock.json"), b"{ not json").unwrap();
        assert!(matches!(
            plugin_mcp_specs(d.path()),
            Err(PluginError::Json(_))
        ));
        assert!(matches!(
            plugin_skill_roots(d.path()),
            Err(PluginError::Json(_))
        ));
    }

    #[test]
    fn pinned_plugin_with_missing_payload_is_a_typed_error() {
        // The lockfile pins an installed MCP plugin but the installed
        // payload (server.json) is gone: corrupt, not absent — a typed
        // error, never a silent zero-spec startup.
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("wayland-nano-plugins");
        fs::create_dir_all(&store).unwrap();
        fs::write(
            store.join("plugins.lock.json"),
            r#"{"plugins":{"r/p":{"archive_sha256":"a","manifest_sha256":"m","installed":{"plugin":"p","registry":"r","kind":"mcp_server","source":"local","resolved_ref":"x","archive_sha256":"a","manifest_sha256":"m","installed_at":1,"instance_id":null}}}}"#,
        )
        .unwrap();
        assert!(matches!(
            plugin_mcp_specs(d.path()),
            Err(PluginError::Io { .. })
        ));
    }
}
