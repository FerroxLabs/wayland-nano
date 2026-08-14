use crate::PluginError;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

pub const MARKETPLACE_MAX: usize = 512 * 1024;
pub const PLUGIN_MAX: usize = 64 * 1024;
pub const MAX_PLUGINS: usize = 256;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceManifest {
    pub name: String,
    pub version: u32,
    pub plugins: Vec<MarketplaceEntry>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceEntry {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub source: PathSource,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathSource {
    Path { path: PathBuf },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    McpServer,
    Skills,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub kind: PluginKind,
    pub mcp_server: Option<McpServer>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum McpServer {
    Stdio(StdioServer),
    Http(HttpServer),
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdioServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpServer {
    pub name: String,
    pub url: String,
}

impl MarketplaceManifest {
    pub fn load(path: &Path) -> Result<Self, PluginError> {
        let bytes = bounded_read(path, MARKETPLACE_MAX)?;
        let value: Self = serde_json::from_slice(&bytes)?;
        validate_name(&value.name)?;
        if value.plugins.len() > MAX_PLUGINS {
            return Err(PluginError::Invalid("too many marketplace entries".into()));
        }
        for entry in &value.plugins {
            validate_name(&entry.name)?;
            let PathSource::Path { path } = &entry.source;
            validate_relative(path)?;
        }
        Ok(value)
    }
}
impl PluginManifest {
    pub fn load(path: &Path) -> Result<Self, PluginError> {
        let bytes = bounded_read(path, PLUGIN_MAX)?;
        let value: Self = serde_json::from_slice(&bytes)?;
        validate_name(&value.name)?;
        match (&value.kind, &value.mcp_server) {
            (PluginKind::McpServer, Some(McpServer::Stdio(s))) => {
                validate_name(&s.name)?;
                if s.command.is_empty() || s.command.len() > 4096 {
                    return Err(PluginError::Invalid("invalid MCP command".into()));
                }
            }
            (PluginKind::McpServer, Some(McpServer::Http(_))) => {
                return Err(PluginError::HttpTransportRefused);
            }
            (PluginKind::Skills, None) => {}
            _ => {
                return Err(PluginError::Invalid(
                    "plugin kind and mcp_server disagree".into(),
                ));
            }
        }
        Ok(value)
    }
}
pub fn validate_name(name: &str) -> Result<(), PluginError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(PluginError::Invalid("invalid name".into()));
    }
    Ok(())
}
pub fn validate_relative(path: &Path) -> Result<(), PluginError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(PluginError::UnsafePath(path.into()));
    }
    for c in path.components() {
        if !matches!(c, Component::Normal(_)) {
            return Err(PluginError::UnsafePath(path.into()));
        }
    }
    let s = path.to_string_lossy();
    if s.starts_with("//") || s.starts_with("\\\\") || s.as_bytes().get(1) == Some(&b':') {
        return Err(PluginError::UnsafePath(path.into()));
    }
    Ok(())
}
pub fn join_under(root: &Path, rel: &Path) -> Result<PathBuf, PluginError> {
    validate_relative(rel)?;
    let root = fs::canonicalize(root).map_err(|e| PluginError::io(root, e))?;
    let joined =
        fs::canonicalize(root.join(rel)).map_err(|e| PluginError::io(root.join(rel), e))?;
    if !joined.starts_with(&root) {
        return Err(PluginError::UnsafePath(rel.into()));
    }
    Ok(joined)
}
fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, PluginError> {
    let meta = fs::metadata(path).map_err(|e| PluginError::io(path, e))?;
    if meta.len() > limit as u64 {
        return Err(PluginError::TooLarge { limit });
    }
    fs::read(path).map_err(|e| PluginError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_fail_closed() {
        for p in ["/x", "../x", "./x", r"C:\x", r"\\host\x"] {
            assert!(validate_relative(Path::new(p)).is_err(), "{p}");
        }
        assert!(validate_relative(Path::new("plugins/a")).is_ok());
    }
    #[test]
    fn unknown_and_kind_mismatch_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("plugin.json");
        fs::write(
            &p,
            r#"{"name":"x","kind":"skills","mcp_server":null,"extra":1}"#,
        )
        .unwrap();
        assert!(PluginManifest::load(&p).is_err());
        fs::write(&p, r#"{"name":"x","kind":"mcp_server"}"#).unwrap();
        assert!(PluginManifest::load(&p).is_err());
    }
}
