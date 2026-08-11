//! Setup data layer: versioned markers, sandbox user records, offline-proxy
//! settings, and network-identity selection.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/setup.rs` (data
//! structures and env parsing only — the provisioning *execution* lands
//! separately) @ 646f7c0a. Deliberate transformations, per dual-track
//! isolation (scorecard §6: test artifacts namespaced per track):
//! - usernames `CodexSandbox{Offline,Online}` → `NanoSandbox{Offline,Online}`
//!   (both tracks' setups must not collide on one host);
//! - env keys `CODEX_NETWORK_ALLOW_LOCAL_BINDING` → `NANO_NETWORK_ALLOW_LOCAL_BINDING`,
//!   `CODEX_WINDOWS_SANDBOX_PROXY_PORTS` → `NANO_WINDOWS_SANDBOX_PROXY_PORTS`
//!   (wire format otherwise unchanged).

use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

pub const SETUP_VERSION: u32 = 5;
pub const OFFLINE_USERNAME: &str = "NanoSandboxOffline";
pub const ONLINE_USERNAME: &str = "NanoSandboxOnline";

pub fn sandbox_bin_dir(nano_home: &Path) -> PathBuf {
    nano_home.join(".sandbox-bin")
}

pub fn sandbox_secrets_dir(nano_home: &Path) -> PathBuf {
    nano_home.join(".sandbox-secrets")
}

pub fn setup_marker_path(nano_home: &Path) -> PathBuf {
    crate::sandbox_dir(nano_home).join("setup_marker.json")
}

pub fn sandbox_users_path(nano_home: &Path) -> PathBuf {
    crate::sandbox_dir(nano_home).join("sandbox_users.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetupMarker {
    pub version: u32,
    pub offline_username: String,
    pub online_username: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub proxy_ports: Vec<u16>,
    #[serde(default)]
    pub allow_local_binding: bool,
}

impl SetupMarker {
    pub fn version_matches(&self) -> bool {
        self.version == SETUP_VERSION
    }

    pub fn offline_proxy_settings(&self) -> OfflineProxySettings {
        OfflineProxySettings {
            proxy_ports: self.proxy_ports.clone(),
            allow_local_binding: self.allow_local_binding,
        }
    }

    pub fn request_mismatch_reason(
        &self,
        network_identity: SandboxNetworkIdentity,
        offline_proxy_settings: &OfflineProxySettings,
    ) -> Option<String> {
        if !network_identity.uses_offline_identity() {
            return None;
        }
        if self.proxy_ports == offline_proxy_settings.proxy_ports
            && self.allow_local_binding == offline_proxy_settings.allow_local_binding
        {
            return None;
        }
        Some(format!(
            "offline firewall settings changed (stored_ports={:?}, desired_ports={:?}, stored_allow_local_binding={}, desired_allow_local_binding={})",
            self.proxy_ports,
            offline_proxy_settings.proxy_ports,
            self.allow_local_binding,
            offline_proxy_settings.allow_local_binding
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxUserRecord {
    pub username: String,
    /// DPAPI-encrypted password blob, base64 encoded.
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxUsersFile {
    pub version: u32,
    pub offline: SandboxUserRecord,
    pub online: SandboxUserRecord,
}

impl SandboxUsersFile {
    pub fn version_matches(&self) -> bool {
        self.version == SETUP_VERSION
    }
}

#[derive(Debug, Clone, Default)]
pub struct SetupRootOverrides {
    pub read_roots: Option<Vec<PathBuf>>,
    pub read_roots_include_platform_defaults: bool,
    pub write_roots: Option<Vec<PathBuf>>,
    pub deny_read_paths: Option<Vec<PathBuf>>,
    pub deny_write_paths: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineProxySettings {
    pub proxy_ports: Vec<u16>,
    pub allow_local_binding: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxNetworkIdentity {
    Offline,
    Online,
}

impl SandboxNetworkIdentity {
    pub fn from_permissions(
        permissions: &ResolvedWindowsSandboxPermissions,
        proxy_enforced: bool,
    ) -> Self {
        if proxy_enforced || !permissions.network_policy().is_enabled() {
            Self::Offline
        } else {
            Self::Online
        }
    }

    pub fn uses_offline_identity(self) -> bool {
        matches!(self, Self::Offline)
    }
}

const PROXY_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "WS_PROXY",
    "WSS_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "ws_proxy",
    "wss_proxy",
];
const ALLOW_LOCAL_BINDING_ENV_KEY: &str = "NANO_NETWORK_ALLOW_LOCAL_BINDING";
// Internal wire format shared with the network proxy: a comma-separated,
// sorted list of non-zero loopback proxy ports used only when computing the
// Windows offline sandbox setup marker.
const WINDOWS_SANDBOX_PROXY_PORTS_ENV_KEY: &str = "NANO_WINDOWS_SANDBOX_PROXY_PORTS";

pub fn offline_proxy_settings_from_env(
    env_map: &HashMap<String, String>,
    network_identity: SandboxNetworkIdentity,
) -> OfflineProxySettings {
    if !network_identity.uses_offline_identity() {
        return OfflineProxySettings {
            proxy_ports: vec![],
            allow_local_binding: false,
        };
    }
    OfflineProxySettings {
        proxy_ports: proxy_ports_from_env(env_map),
        allow_local_binding: env_map
            .get(ALLOW_LOCAL_BINDING_ENV_KEY)
            .is_some_and(|value| value == "1"),
    }
}

pub fn proxy_ports_from_env(env_map: &HashMap<String, String>) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    for key in PROXY_ENV_KEYS {
        if let Some(value) = env_map.get(*key)
            && let Some(port) = loopback_proxy_port_from_url(value)
        {
            ports.insert(port);
        }
    }
    if let Some(value) = env_map.get(WINDOWS_SANDBOX_PROXY_PORTS_ENV_KEY) {
        ports.extend(
            value
                .split(',')
                .filter_map(|port| port.trim().parse::<u16>().ok())
                .filter(|port| *port != 0),
        );
    }
    ports.into_iter().collect()
}

fn loopback_proxy_port_from_url(url: &str) -> Option<u16> {
    let authority = url.trim().split_once("://")?.1.split('/').next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);

    if let Some(host) = host_port.strip_prefix('[') {
        let (host, rest) = host.split_once(']')?;
        if host != "::1" {
            return None;
        }
        let port = rest.strip_prefix(':')?.parse::<u16>().ok()?;
        return (port != 0).then_some(port);
    }

    let (host, port) = host_port.rsplit_once(':')?;
    if !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1") {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    (port != 0).then_some(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn marker_round_trip_and_version() {
        let marker = SetupMarker {
            version: SETUP_VERSION,
            offline_username: OFFLINE_USERNAME.into(),
            online_username: ONLINE_USERNAME.into(),
            created_at: Some("2026-08-09".into()),
            proxy_ports: vec![8080, 9000],
            allow_local_binding: true,
        };
        let json = serde_json::to_string(&marker).unwrap();
        let back: SetupMarker = serde_json::from_str(&json).unwrap();
        assert_eq!(back, marker);
        assert!(back.version_matches());
        assert_eq!(back.offline_proxy_settings().proxy_ports, vec![8080, 9000]);
    }

    #[test]
    fn mismatch_reason_only_for_offline_drift() {
        let marker = SetupMarker {
            version: SETUP_VERSION,
            offline_username: OFFLINE_USERNAME.into(),
            online_username: ONLINE_USERNAME.into(),
            created_at: None,
            proxy_ports: vec![8080],
            allow_local_binding: false,
        };
        let desired = OfflineProxySettings {
            proxy_ports: vec![9090],
            allow_local_binding: true,
        };
        assert!(
            marker
                .request_mismatch_reason(SandboxNetworkIdentity::Offline, &desired)
                .is_some()
        );
        assert!(
            marker
                .request_mismatch_reason(SandboxNetworkIdentity::Online, &desired)
                .is_none()
        );
        let same = marker.offline_proxy_settings();
        assert!(
            marker
                .request_mismatch_reason(SandboxNetworkIdentity::Offline, &same)
                .is_none()
        );
    }

    #[test]
    fn proxy_ports_parse_loopback_only() {
        let map = env_map(&[
            ("HTTP_PROXY", "http://127.0.0.1:8080"),
            ("HTTPS_PROXY", "http://localhost:9000/"),
            ("ALL_PROXY", "http://user@127.0.0.1:7000"),
            ("WS_PROXY", "http://[::1]:6000"),
            ("WSS_PROXY", "http://192.168.1.5:5000"), // not loopback: rejected
            ("NANO_WINDOWS_SANDBOX_PROXY_PORTS", "0,4444, 5555"),
        ]);
        assert_eq!(
            proxy_ports_from_env(&map),
            vec![4444, 5555, 6000, 7000, 8080, 9000]
        );
    }

    #[test]
    fn offline_settings_respect_identity_and_env() {
        let map = env_map(&[
            ("HTTP_PROXY", "http://127.0.0.1:8080"),
            ("NANO_NETWORK_ALLOW_LOCAL_BINDING", "1"),
        ]);
        let offline = offline_proxy_settings_from_env(&map, SandboxNetworkIdentity::Offline);
        assert_eq!(offline.proxy_ports, vec![8080]);
        assert!(offline.allow_local_binding);
        let online = offline_proxy_settings_from_env(&map, SandboxNetworkIdentity::Online);
        assert!(online.proxy_ports.is_empty());
        assert!(!online.allow_local_binding);
    }

    #[test]
    fn users_file_round_trip() {
        let users = SandboxUsersFile {
            version: SETUP_VERSION,
            offline: SandboxUserRecord {
                username: OFFLINE_USERNAME.into(),
                password: "blob-off".into(),
            },
            online: SandboxUserRecord {
                username: ONLINE_USERNAME.into(),
                password: "blob-on".into(),
            },
        };
        let json = serde_json::to_string_pretty(&users).unwrap();
        let back: SandboxUsersFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, users);
        assert!(back.version_matches());
    }
}
