use crate::{
    PluginError,
    manifest::{McpServer, PluginKind, PluginManifest},
};
use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub struct SpawnPreview {
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub source: String,
    pub resolved_ref: String,
    pub archive_sha256: String,
    pub namespace: String,
    pub kind: PluginKind,
    pub skills: Vec<String>,
    pub spawn: Option<SpawnPreview>,
    pub ignored: Vec<String>,
}
impl InstallPlan {
    pub fn build(
        source: String,
        resolved_ref: String,
        archive_sha256: String,
        registry: &str,
        manifest: &PluginManifest,
        skills: Vec<String>,
        installed_skills: &[String],
    ) -> Result<Self, PluginError> {
        for skill in &skills {
            if installed_skills.contains(skill) {
                return Err(PluginError::Invalid(format!("skill collision: {skill}")));
            }
        }
        let spawn = match &manifest.mcp_server {
            Some(McpServer::Stdio(s)) => Some(SpawnPreview {
                command: s.command.clone(),
                args: s.args.clone(),
                env_keys: s.env.keys().cloned().collect(),
            }),
            _ => None,
        };
        Ok(Self {
            source,
            resolved_ref,
            archive_sha256,
            namespace: format!("{registry}/{}", manifest.name),
            kind: manifest.kind,
            skills,
            spawn,
            ignored: Vec::new(),
        })
    }
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "Plugin install plan\n  source: {}\n  resolved ref: {}\n  sha256: {}\n  plugin: {}\n  trust: UNSIGNED — integrity is pinned, not verified",
            self.source, self.resolved_ref, self.archive_sha256, self.namespace
        );
        for x in &self.skills {
            let _ = writeln!(s, "  skill: {}:{x}", self.namespace);
        }
        if let Some(p) = &self.spawn {
            let _ = writeln!(s, "  process: {} {}", p.command, p.args.join(" "));
            if !p.env_keys.is_empty() {
                let _ = writeln!(s, "  env keys: {}", p.env_keys.join(", "));
            }
        }
        for i in &self.ignored {
            let _ = writeln!(s, "  ignored: {i}");
        }
        s
    }
}
