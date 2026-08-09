//! Sandbox identity readiness: marker/user loading, credential decode,
//! identity selection, and offline-proxy reconciliation.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/identity.rs` @
//! 646f7c0a. Transformations: `crate::setup::*` -> `crate::setup_types::*`;
//! `WindowsSandboxProxySettingsMode` from nano_core::permissions;
//! codex_home -> nano_home naming. Scope note: this file currently carries the
//! PURE readiness layer (load/select/reconcile + tests). The execution-bound
//! entry points (`require_logon_sandbox_creds`, `refresh_logon_sandbox_creds`)
//! land with the setup-execution port (B-SBX-08c), which provides `gather_*`
//! and `run_*`.

use crate::dpapi;
use crate::logging::debug_log;
use crate::setup_types::SandboxNetworkIdentity;
use crate::setup_types::SandboxUserRecord;
use crate::setup_types::SandboxUsersFile;
use crate::setup_types::SetupMarker;
use crate::setup_types::sandbox_users_path;
use crate::setup_types::setup_marker_path;
use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use nano_core::permissions::WindowsSandboxProxySettingsMode;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SandboxIdentity {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct SandboxCreds {
    pub username: String,
    pub password: String,
}

/// Returns true when the on-disk setup artifacts exist and match the current
/// setup version.
///
/// This is a coarse readiness check; the execution-bound entry points perform
/// the additional runtime validation for offline firewall settings.
pub fn sandbox_setup_is_complete(nano_home: &Path) -> bool {
    let marker_ok = matches!(load_marker(nano_home), Ok(Some(marker)) if marker.version_matches());
    if !marker_ok {
        return false;
    }
    matches!(load_users(nano_home), Ok(Some(users)) if users.version_matches())
}

pub fn load_marker(nano_home: &Path) -> Result<Option<SetupMarker>> {
    let path = setup_marker_path(nano_home);
    let marker = match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<SetupMarker>(&contents) {
            Ok(m) => Some(m),
            Err(err) => {
                debug_log(
                    &format!("sandbox setup marker parse failed: {err}"),
                    Some(nano_home),
                );
                None
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            debug_log(
                &format!("sandbox setup marker read failed: {err}"),
                Some(nano_home),
            );
            None
        }
    };
    Ok(marker)
}

pub fn load_users(nano_home: &Path) -> Result<Option<SandboxUsersFile>> {
    let path = sandbox_users_path(nano_home);
    let file = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            debug_log(
                &format!("sandbox users read failed: {err}"),
                Some(nano_home),
            );
            return Ok(None);
        }
    };
    match serde_json::from_str::<SandboxUsersFile>(&file) {
        Ok(users) => Ok(Some(users)),
        Err(err) => {
            debug_log(
                &format!("sandbox users parse failed: {err}"),
                Some(nano_home),
            );
            Ok(None)
        }
    }
}

pub(crate) fn remove_sandbox_users_file(nano_home: &Path, reason: &str) -> Result<()> {
    let path = sandbox_users_path(nano_home);
    debug_log(
        &format!("{reason}; deleting {}", path.display()),
        Some(nano_home),
    );
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("delete {}", path.display())),
    }
}

fn decode_password(record: &SandboxUserRecord) -> Result<String> {
    let blob = BASE64_STANDARD
        .decode(record.password.as_bytes())
        .context("base64 decode password")?;
    let decrypted = dpapi::unprotect(&blob)?;
    let pwd = String::from_utf8(decrypted).context("sandbox password not utf-8")?;
    Ok(pwd)
}

pub(crate) fn select_identity(
    network_identity: SandboxNetworkIdentity,
    nano_home: &Path,
) -> Result<Option<SandboxIdentity>> {
    let _marker = match load_marker(nano_home)? {
        Some(m) if m.version_matches() => m,
        _ => return Ok(None),
    };
    let users = match load_users(nano_home)? {
        Some(u) if u.version_matches() => u,
        _ => return Ok(None),
    };
    let chosen = match network_identity {
        SandboxNetworkIdentity::Offline => users.offline,
        SandboxNetworkIdentity::Online => users.online,
    };
    let password = decode_password(&chosen)?;
    Ok(Some(SandboxIdentity {
        username: chosen.username,
        password,
    }))
}

/// Chooses the desired offline proxy settings: Preserve mode keeps the
/// existing marker's settings; anything else reconciles from the environment.
pub(crate) fn desired_offline_proxy_settings(
    marker: Option<&SetupMarker>,
    proxy_settings_mode: WindowsSandboxProxySettingsMode,
    env_map: &HashMap<String, String>,
    network_identity: SandboxNetworkIdentity,
) -> crate::setup_types::OfflineProxySettings {
    match (marker, proxy_settings_mode) {
        (Some(marker), WindowsSandboxProxySettingsMode::Preserve) if marker.version_matches() => {
            marker.offline_proxy_settings()
        }
        _ => crate::setup_types::offline_proxy_settings_from_env(env_map, network_identity),
    }
}

#[cfg(test)]
mod tests {
    use super::desired_offline_proxy_settings;
    use super::remove_sandbox_users_file;
    use crate::setup_types::OFFLINE_USERNAME;
    use crate::setup_types::SETUP_VERSION;
    use crate::setup_types::SandboxNetworkIdentity;
    use crate::setup_types::SetupMarker;
    use crate::setup_types::sandbox_users_path;
    use nano_core::permissions::WindowsSandboxProxySettingsMode;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    fn marker_with(proxy_ports: Vec<u16>, allow_local_binding: bool) -> SetupMarker {
        SetupMarker {
            version: SETUP_VERSION,
            offline_username: OFFLINE_USERNAME.to_string(),
            online_username: "online".to_string(),
            created_at: None,
            proxy_ports,
            allow_local_binding,
        }
    }

    #[test]
    fn remove_sandbox_users_file_deletes_existing_file() {
        let nano_home = TempDir::new().expect("tempdir");
        let users_path = sandbox_users_path(nano_home.path());
        fs::create_dir_all(users_path.parent().expect("sandbox secrets dir"))
            .expect("create sandbox secrets dir");
        fs::write(&users_path, "users").expect("write users");

        remove_sandbox_users_file(nano_home.path(), "stale creds").expect("remove users");
        assert!(!users_path.exists());
    }

    #[test]
    fn remove_sandbox_users_file_ignores_missing_file() {
        let nano_home = TempDir::new().expect("tempdir");
        let users_path = sandbox_users_path(nano_home.path());

        remove_sandbox_users_file(nano_home.path(), "stale creds").expect("remove users");
        assert!(!users_path.exists());
    }

    #[test]
    fn preserving_proxy_settings_uses_the_existing_marker() {
        let marker = marker_with(vec![7890], true);
        let env_map = HashMap::from([(
            "HTTP_PROXY".to_string(),
            "http://127.0.0.1:8080".to_string(),
        )]);

        assert_eq!(
            desired_offline_proxy_settings(
                Some(&marker),
                WindowsSandboxProxySettingsMode::Preserve,
                &env_map,
                SandboxNetworkIdentity::Offline,
            ),
            marker.offline_proxy_settings()
        );
        assert_eq!(
            desired_offline_proxy_settings(
                Some(&marker),
                WindowsSandboxProxySettingsMode::Reconcile,
                &env_map,
                SandboxNetworkIdentity::Offline,
            )
            .proxy_ports,
            vec![8080]
        );
    }

    #[test]
    fn guardian_preserve_mode_does_not_churn_marker_with_empty_proxy_ports() {
        let marker = marker_with(vec![3128, 8081], true);
        let env_map = HashMap::new();
        let reconciled = desired_offline_proxy_settings(
            Some(&marker),
            WindowsSandboxProxySettingsMode::Reconcile,
            &env_map,
            SandboxNetworkIdentity::Offline,
        );
        assert_eq!(reconciled.proxy_ports, Vec::<u16>::new());

        let desired = desired_offline_proxy_settings(
            Some(&marker),
            WindowsSandboxProxySettingsMode::Preserve,
            &env_map,
            SandboxNetworkIdentity::Offline,
        );
        assert_eq!(desired, marker.offline_proxy_settings());
        assert_eq!(
            marker.request_mismatch_reason(SandboxNetworkIdentity::Offline, &desired),
            None
        );
    }
}
