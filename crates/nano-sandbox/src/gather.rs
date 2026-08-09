//! Root gathering: compute read/write root sets for setup payloads from
//! resolved permissions, platform defaults, and user-profile filters.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/setup.rs` (gather
//! layer: lines ~492-633 and ~1157-1278) @ 646f7c0a. Transformations:
//! codex_home -> nano_home naming; sensitive-root filter text updated to
//! Nano-owned dirs. Donor tests adapted below.

use crate::helper_materialization::helper_bin_dir;
use crate::path_normalization::canonical_path_key;
use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
use crate::setup_types::sandbox_bin_dir;
use crate::setup_types::sandbox_secrets_dir;
use crate::ssh_config_dependencies::ssh_config_dependency_paths;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

const USERPROFILE_ROOT_EXCLUSIONS: &[&str] = &[
    ".ssh",
    ".tsh",
    ".brev",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config",
    ".npm",
    ".pki",
    ".terraform.d",
];
pub const WINDOWS_PLATFORM_DEFAULT_READ_ROOTS: &[&str] = &[
    r"C:\Windows",
    r"C:\Program Files",
    r"C:\Program Files (x86)",
    r"C:\ProgramData",
];

pub fn canonical_existing(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|p| {
            if !p.exists() {
                return None;
            }
            Some(dunce::canonicalize(p).unwrap_or_else(|_| p.clone()))
        })
        .collect()
}

pub fn profile_read_roots(user_profile: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(user_profile) {
        Ok(entries) => entries,
        Err(_) => return vec![user_profile.to_path_buf()],
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| (entry.file_name(), entry.path()))
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            !USERPROFILE_ROOT_EXCLUSIONS
                .iter()
                .any(|excluded| name.eq_ignore_ascii_case(excluded))
        })
        .map(|(_, path)| path)
        .collect()
}

pub fn gather_helper_read_roots(nano_home: &Path) -> Vec<PathBuf> {
    let helper_dir = helper_bin_dir(nano_home);
    let _ = std::fs::create_dir_all(&helper_dir);
    vec![helper_dir]
}

pub fn gather_full_read_roots_for_permissions(
    command_cwd: &Path,
    permissions: &ResolvedWindowsSandboxPermissions,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
) -> Vec<PathBuf> {
    let mut roots = gather_helper_read_roots(nano_home);
    roots.extend(
        WINDOWS_PLATFORM_DEFAULT_READ_ROOTS
            .iter()
            .map(PathBuf::from),
    );
    if let Ok(up) = std::env::var("USERPROFILE") {
        roots.extend(profile_read_roots(Path::new(&up)));
    }
    roots.push(command_cwd.to_path_buf());
    roots.extend(
        permissions
            .writable_roots_for_cwd(command_cwd, env_map)
            .into_iter()
            .map(|root| root.root),
    );
    canonical_existing(&roots)
}

pub fn gather_read_roots(
    command_cwd: &Path,
    permissions: &ResolvedWindowsSandboxPermissions,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
) -> Vec<PathBuf> {
    if permissions.has_full_disk_read_access() {
        return gather_full_read_roots_for_permissions(
            command_cwd,
            permissions,
            env_map,
            nano_home,
        );
    }

    let mut roots = gather_helper_read_roots(nano_home);
    if permissions.include_platform_defaults() {
        roots.extend(
            WINDOWS_PLATFORM_DEFAULT_READ_ROOTS
                .iter()
                .map(PathBuf::from),
        );
    }
    roots.extend(permissions.readable_roots_for_cwd(command_cwd));
    canonical_existing(&roots)
}

pub fn gather_write_roots_for_permissions(
    permissions: &ResolvedWindowsSandboxPermissions,
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
) -> Vec<PathBuf> {
    let roots = permissions
        .writable_roots_for_cwd(command_cwd, env_map)
        .into_iter()
        .map(|root| root.root)
        .collect::<Vec<_>>();
    let mut dedup: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for r in canonical_existing(&roots) {
        if dedup.insert(r.clone()) {
            out.push(r);
        }
    }
    out
}

pub fn effective_write_roots_for_setup(
    permissions: &ResolvedWindowsSandboxPermissions,
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
    write_roots_override: Option<&[PathBuf]>,
) -> Vec<PathBuf> {
    effective_write_roots_for_permissions(
        permissions,
        command_cwd,
        env_map,
        nano_home,
        write_roots_override,
    )
}

pub fn effective_write_roots_for_permissions(
    permissions: &ResolvedWindowsSandboxPermissions,
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
    write_roots_override: Option<&[PathBuf]>,
) -> Vec<PathBuf> {
    let write_roots = if let Some(roots) = write_roots_override {
        canonical_existing(roots)
    } else {
        gather_write_roots_for_permissions(permissions, command_cwd, env_map)
    };
    let write_roots = expand_user_profile_root(write_roots);
    let write_roots = filter_user_profile_root(write_roots);
    let write_roots = filter_user_profile_root_exclusions(write_roots);
    let write_roots = filter_ssh_config_dependency_roots(write_roots);
    filter_sensitive_write_roots(write_roots, nano_home)
}

pub fn expand_user_profile_root(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return roots;
    };
    expand_user_profile_root_for(roots, Path::new(&user_profile))
}

fn expand_user_profile_root_for(roots: Vec<PathBuf>, user_profile: &Path) -> Vec<PathBuf> {
    let user_profile_key = canonical_path_key(user_profile);
    let mut expanded = Vec::new();
    for root in roots {
        if canonical_path_key(&root) == user_profile_key {
            expanded.extend(profile_read_roots(user_profile));
        } else {
            expanded.push(root);
        }
    }

    expanded.sort_by_key(|root| canonical_path_key(root));
    expanded.dedup_by(|a, b| canonical_path_key(a.as_path()) == canonical_path_key(b.as_path()));
    expanded
}

pub fn filter_user_profile_root(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return roots;
    };
    let user_profile_key = canonical_path_key(Path::new(&user_profile));
    roots.retain(|root| canonical_path_key(root) != user_profile_key);
    roots
}

pub fn filter_user_profile_root_exclusions(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return roots;
    };
    let user_profile = Path::new(&user_profile);
    roots.retain(|root| !is_user_profile_root_exclusion(root, user_profile));
    roots
}

fn is_user_profile_root_exclusion(root: &Path, user_profile: &Path) -> bool {
    let root_key = canonical_path_key(root);
    let profile_key = canonical_path_key(user_profile);
    let profile_prefix = format!("{}/", profile_key.trim_end_matches('/'));
    let Some(relative_key) = root_key.strip_prefix(&profile_prefix) else {
        return false;
    };
    let Some(child_name) = relative_key
        .split('/')
        .next()
        .filter(|name| !name.is_empty())
    else {
        return false;
    };

    USERPROFILE_ROOT_EXCLUSIONS
        .iter()
        .any(|excluded| child_name.eq_ignore_ascii_case(excluded))
}

pub fn filter_ssh_config_dependency_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let Ok(user_profile) = std::env::var("USERPROFILE") else {
        return roots;
    };
    let user_profile = Path::new(&user_profile);
    let dependency_paths = ssh_config_dependency_paths(user_profile);
    roots.retain(|root| !is_ssh_config_dependency_root(root, user_profile, &dependency_paths));
    roots
}

fn is_ssh_config_dependency_root(
    root: &Path,
    user_profile: &Path,
    dependency_paths: &[PathBuf],
) -> bool {
    let Some(child_name) = user_profile_child_name(root, user_profile) else {
        return false;
    };

    dependency_paths.iter().any(|path| {
        user_profile_child_name(path, user_profile)
            .is_some_and(|dependency_child| child_name.eq_ignore_ascii_case(&dependency_child))
    })
}

fn user_profile_child_name(path: &Path, user_profile: &Path) -> Option<String> {
    let root_key = canonical_path_key(path);
    let profile_key = canonical_path_key(user_profile);
    let profile_prefix = format!("{}/", profile_key.trim_end_matches('/'));
    let relative_key = root_key.strip_prefix(&profile_prefix)?;
    relative_key
        .split('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn filter_sensitive_write_roots(mut roots: Vec<PathBuf>, nano_home: &Path) -> Vec<PathBuf> {
    // Never grant capability write access to NANO_HOME or anything under
    // NANO_HOME/.sandbox, NANO_HOME/.sandbox-bin, or NANO_HOME/.sandbox-secrets.
    // These locations contain sandbox control/state and helper binaries and
    // must remain tamper-resistant.
    let nano_home_key = canonical_path_key(nano_home);
    let sbx_dir_key = canonical_path_key(&crate::sandbox_dir(nano_home));
    let sbx_dir_prefix = format!("{}/", sbx_dir_key.trim_end_matches('/'));
    let sbx_bin_dir_key = canonical_path_key(&sandbox_bin_dir(nano_home));
    let sbx_bin_dir_prefix = format!("{}/", sbx_bin_dir_key.trim_end_matches('/'));
    let secrets_dir_key = canonical_path_key(&sandbox_secrets_dir(nano_home));
    let secrets_dir_prefix = format!("{}/", secrets_dir_key.trim_end_matches('/'));

    roots.retain(|root| {
        let key = canonical_path_key(root);
        key != nano_home_key
            && key != sbx_dir_key
            && !key.starts_with(&sbx_dir_prefix)
            && key != sbx_bin_dir_key
            && !key.starts_with(&sbx_bin_dir_prefix)
            && key != secrets_dir_key
            && !key.starts_with(&secrets_dir_prefix)
    });
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn profile_read_roots_excludes_configured_top_level_entries() {
        let tmp = TempDir::new().expect("tempdir");
        for name in [".ssh", ".aws", "projects", ".config"] {
            std::fs::create_dir_all(tmp.path().join(name)).expect("create profile child");
        }
        let roots: HashSet<PathBuf> = profile_read_roots(tmp.path()).into_iter().collect();
        assert!(roots.contains(&tmp.path().join("projects")));
        assert!(!roots.contains(&tmp.path().join(".ssh")));
        assert!(!roots.contains(&tmp.path().join(".aws")));
        assert!(!roots.contains(&tmp.path().join(".config")));
    }

    #[test]
    fn profile_read_roots_falls_back_to_profile_root_when_enumeration_fails() {
        let missing = Path::new(r"C:\nanok3-definitely-missing-profile-dir");
        assert_eq!(profile_read_roots(missing), vec![missing.to_path_buf()]);
    }

    #[test]
    fn expand_user_profile_root_for_replaces_profile_root_with_children() {
        let tmp = TempDir::new().expect("tempdir");
        let profile = tmp.path().join("profile");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(profile.join(".ssh")).expect("create .ssh");
        std::fs::create_dir_all(profile.join("work")).expect("create work");
        std::fs::create_dir_all(&other).expect("create other");

        let expanded =
            expand_user_profile_root_for(vec![profile.clone(), other.clone()], profile.as_path());

        assert!(expanded.contains(&profile.join("work")));
        assert!(!expanded.contains(&profile.join(".ssh")));
        assert!(expanded.contains(&other));
        assert!(!expanded.contains(&profile));
    }

    #[test]
    fn sensitive_filter_strips_nano_home_and_sandbox_dirs() {
        let tmp = TempDir::new().expect("tempdir");
        let nano_home = tmp.path().join("nano-home");
        let keep = tmp.path().join("workspace");
        for dir in [
            nano_home.clone(),
            crate::sandbox_dir(&nano_home),
            sandbox_bin_dir(&nano_home),
            sandbox_secrets_dir(&nano_home),
            keep.clone(),
        ] {
            std::fs::create_dir_all(&dir).expect("create dir");
        }

        let filtered = filter_sensitive_write_roots(
            vec![
                nano_home.clone(),
                crate::sandbox_dir(&nano_home),
                sandbox_bin_dir(&nano_home).join("helper"),
                sandbox_secrets_dir(&nano_home).join("creds"),
                keep.clone(),
            ],
            &nano_home,
        );

        assert_eq!(filtered, vec![keep]);
    }

    #[test]
    fn user_profile_exclusion_matches_top_level_only() {
        let profile = Path::new(r"C:\Users\dev");
        assert!(is_user_profile_root_exclusion(
            Path::new(r"C:\Users\dev\.ssh"),
            profile
        ));
        assert!(is_user_profile_root_exclusion(
            Path::new(r"C:\Users\dev\.aws\credentials"),
            profile
        ));
        assert!(!is_user_profile_root_exclusion(
            Path::new(r"C:\Users\dev\projects\.ssh"),
            profile
        ));
        assert!(!is_user_profile_root_exclusion(
            Path::new(r"D:\other\.ssh"),
            profile
        ));
    }
}
