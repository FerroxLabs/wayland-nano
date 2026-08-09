//! Workspace metadata dir protection (deny-write ACEs for agent tooling dirs).
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/workspace_acl.rs` @
//! 646f7c0a. Transformation: `.codex` -> `.nano` (recorded branding);
//! protect_workspace_codex_dir -> protect_workspace_nano_dir.

use crate::acl::add_deny_write_ace;
use crate::path_normalization::canonicalize_path;
use anyhow::Result;
use std::ffi::c_void;
use std::path::Path;

pub fn is_command_cwd_root(root: &Path, canonical_command_cwd: &Path) -> bool {
    canonicalize_path(root) == canonical_command_cwd
}

/// # Safety
/// Caller must ensure `psid` is a valid SID pointer.
pub unsafe fn protect_workspace_nano_dir(cwd: &Path, psid: *mut c_void) -> Result<bool> {
    protect_workspace_subdir(cwd, psid, ".nano")
}

/// # Safety
/// Caller must ensure `psid` is a valid SID pointer.
pub unsafe fn protect_workspace_agents_dir(cwd: &Path, psid: *mut c_void) -> Result<bool> {
    protect_workspace_subdir(cwd, psid, ".agents")
}

unsafe fn protect_workspace_subdir(cwd: &Path, psid: *mut c_void, subdir: &str) -> Result<bool> {
    let path = cwd.join(subdir);
    if path.is_dir() {
        add_deny_write_ace(&path, psid)
    } else {
        Ok(false)
    }
}
