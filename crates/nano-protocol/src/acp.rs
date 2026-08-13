//! ACP (Agent Client Protocol) wire types — the stdio JSON-RPC protocol
//! Wayland Desktop speaks to third-party agents.
//!
//! Evidence-based subset (from Desktop's AcpConnection.ts + @agentclientprotocol/sdk 0.18.2):
//! - initialize: protocolVersion 1, clientCapabilities.fs
//! - session/new: {cwd, mcpServers[]} → {sessionId}
//! - session/load: {sessionId, cwd, mcpServers[]} → {modes} after replaying
//!   the journaled transcript as session/update notifications
//! - session/prompt: {sessionId, prompt:[{type:"text",text}]} → {stopReason}
//! - session/cancel: notification {sessionId}
//! - session/update: notification {sessionId, update:{sessionUpdate: kind, ...}}
//! - session/request_permission: agent→host request (approval UI)
//!
//! Everything else fails typed (method-not-found), never panics.

use serde::Deserialize;
use serde::Serialize;

use nano_session::NanoErrorKind;

pub use crate::error_codes::error_presentation;
use crate::error_codes::spec;
use crate::permission_mode::PermissionMode;

pub const ACP_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorBody>,
}

impl JsonRpcResponse {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// Error carrying a structured `data` payload (C7 D2 / C8: typed error
    /// kinds ride in `error.data` — no new numeric codes). Callers must keep
    /// `data` secret-free (ids, kinds, hints only).
    pub fn err_with_data(
        id: serde_json::Value,
        code: i64,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }

    /// A TYPED error response (C7): the numeric code comes from the error
    /// table (standard JSON-RPC codes only) and the typing rides in
    /// `data.nanoError` — closed typed fields only (kind enum, retryable
    /// bool, bounded numeric codes, egress-redacted host), never free-form
    /// detail strings (design §2/D2, §7).
    pub fn err_typed(
        id: serde_json::Value,
        kind: NanoErrorKind,
        message: impl Into<String>,
        extras: NanoErrorExtras,
    ) -> Self {
        let spec = spec(kind);
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code: spec.wire_code,
                message: message.into(),
                data: Some(nano_error_data(kind, &extras)),
            }),
        }
    }

    pub fn method_not_found(id: serde_json::Value, method: &str) -> Self {
        Self::err(id, -32601, format!("method not found: {method}"))
    }
}

/// The closed set of optional typed detail fields a `nanoError` payload may
/// carry (design §2/D2: booleans, bounded numeric codes, the egress-redacted
/// host — NO free-form strings, ever).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NanoErrorExtras {
    pub status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    pub host: Option<String>,
}

/// `error.data` / `_meta` payload: `{ "nanoError": { "kind", "retryable",
/// ...closed extras } }`.
pub fn nano_error_data(kind: NanoErrorKind, extras: &NanoErrorExtras) -> serde_json::Value {
    let mut nano = serde_json::json!({
        "kind": kind,
        "retryable": spec(kind).retryable,
    });
    if let Some(status) = extras.status {
        nano["status"] = serde_json::json!(status);
    }
    if let Some(retry_after_ms) = extras.retry_after_ms {
        nano["retry_after_ms"] = serde_json::json!(retry_after_ms);
    }
    if let Some(host) = &extras.host {
        nano["host"] = serde_json::json!(host);
    }
    serde_json::json!({ "nanoError": nano })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i64,
    pub message: String,
    /// C7: typed error payload (`{"nanoError": {...}}`) — absent on
    /// pre-C7-style generic errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
        }
    }
}

/// Agent capabilities advertised in the initialize response.
///
/// `mcpCapabilities` is present because session/new (and session/load)
/// genuinely consume `mcpServers`: Desktop parses the block's PRESENCE as
/// stdio support (acpTypes.ts `parseAgentCapabilitiesObject`: `stdio: mcp
/// !== null`) and reads `http`/`sse` as booleans — so the honest shape is
/// exactly `{http: false, sse: false}` (stdio-only, implied by presence).
///
/// P2a §6.4/D4 — FLAGGED SIGNATURE CHANGE (was the pure no-arg fn; single
/// call site `acp_mode.rs` initialize handler): `promptCapabilities.image`
/// advertises from the configured STARTUP leaf via the §6.3 vision catalog.
/// The advertisement is INITIALIZE-SCOPED and ADVISORY ONLY — it describes
/// the agent's startup configuration, never the session's current leaf, and
/// it goes STALE after `session/set_model`; it is never per-session truth
/// and the §6.2 rung-1/rung-3 gates NEVER trust it.
///
/// LANE-A BOUNDARY: the vision catalog type is lane A's
/// `nano-model/src/vision_catalog.rs`; consumed, never defined here.
pub fn agent_capabilities(
    startup_leaf: &str,
    vision_catalog: &nano_model::vision_catalog::VisionCatalog,
) -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {
                "text": true,
                "image": vision_catalog.image_in(startup_leaf),
                "embeddedContext": false
            },
            "mcpCapabilities": {
                "http": false,
                "sse": false
            },
            // C9: extension-method advertisement with a version marker, the
            // same discipline as session/compact / session/set_model.
            // Clients discover support HERE, never by probing; a client
            // that sends an unknown method gets the standard JSON-RPC
            // -32601 fallback.
            "nanoExtensions": {
                "session/steer": { "version": 1 }
            }
        },
        "agentInfo": {
            "name": "wayland-nano",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

// ── P2a §2.3: ACP wire intake — the typed prompt-block converter ─────────
//
// LANE-A BOUNDARY: the §4 hardened loader (`nano_tools::image`) and the §7
// image error kinds (`NanoErrorKind::{ImageInvalid, ImageUnsupportedFormat,
// ImageTooLarge, ImageTooMany, …}`) are lane-A surfaces; this module
// consumes both and defines neither. The lane-A loader
// (`load_image(bytes, claimed_mime)`, async over the spawn_blocking decode
// boundary) NEVER touches filesystem policy, so the §3.3 path threat model
// is enforced HERE, host-side.
use nano_agent::turn_input::{TurnBlock, TurnInput};
use nano_session::op::ImageRef;

/// A typed prompt-block rejection (P2a §2.3/§7): unknown block types ride
/// `InvalidParams` naming the type TAG only (closed vocabulary, no content
/// echo — the C7 `nanoError` closed-fields rule); loader failures carry the
/// §7 image kinds; path denials ride the reused `FsReadDenied` /
/// `FsSensitiveDenied`. The FIRST invalid block aborts the whole prompt — a
/// partially converted prompt is never executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRejection {
    pub kind: NanoErrorKind,
    pub message: String,
}

impl BlockRejection {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            kind: NanoErrorKind::InvalidParams,
            message: message.into(),
        }
    }

    fn fs_read_denied(message: impl Into<String>) -> Self {
        Self {
            kind: NanoErrorKind::FsReadDenied,
            message: message.into(),
        }
    }
}

/// P2a §3.3 — the TUI/path threat model (grok `placeholder_images.rs:13-37`,
/// ported to the host intake boundary): canonicalize; prefix allowlist (the
/// session workspace root + the OS user pictures dir — NEVER all of
/// `$HOME`); extension allowlist as PRE-FILTER ONLY (never trusted — §4.1
/// magic-byte sniffing is the real check); sensitive-subtree rejection (any
/// dot-prefixed component under an allowed root — `.ssh`/`.gnupg`/`.aws`-
/// style names and `.photoslibrary`-style bundles are all dot-prefixed);
/// symlink/junction escape from every allowed root fails the prefix check
/// on the CANONICAL path. The read is bounded at `MAX_IMAGE_FILE_BYTES`
/// before any decoder runs. The open is handle-verified (P2a audit H-2):
/// unix opens O_NOFOLLOW and proves the opened handle is the authorized
/// file (dev/ino identity); Windows opens reparse-safe and proves the
/// handle's final path is still the authorized canonical path — a
/// workspace-controlled swap between authorization and open fails closed.
/// Returns the bytes and the canonical display
/// path (for the placeholder/provenance — the TUI never sees bytes).
fn confined_image_read(
    raw_path: &str,
    workspace: &std::path::Path,
) -> Result<(Vec<u8>, String), BlockRejection> {
    use nano_tools::image::MAX_IMAGE_FILE_BYTES;
    // Extension pre-filter (never trusted — §4.1 sniffs magic bytes).
    let extension_ok = std::path::Path::new(raw_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
        })
        .unwrap_or(false);
    if !extension_ok {
        return Err(BlockRejection::fs_read_denied(
            "image_path: extension outside {png,jpg,jpeg,gif,webp}",
        ));
    }
    // Canonicalize (resolves symlinks/junctions): an escape from every
    // allowed root fails the prefix check below.
    let canonical = std::fs::canonicalize(raw_path)
        .map_err(|_| BlockRejection::fs_read_denied("image_path: cannot resolve the path"))?;
    // The authorized file's identity, captured NOW: the confined open below
    // must prove from the OPENED HANDLE (never the path) that it landed on
    // this same file — a symlink/junction swapped between authorization and
    // open fails closed (P2a audit H-2).
    let authorized_meta = std::fs::metadata(&canonical)
        .map_err(|_| BlockRejection::fs_read_denied("image_path: cannot resolve the path"))?;
    if !authorized_meta.is_file() {
        return Err(BlockRejection::fs_read_denied(
            "image_path: not a regular file",
        ));
    }
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(root) = std::fs::canonicalize(workspace) {
        roots.push(root);
    }
    if let Some(pictures) = dirs_next::picture_dir()
        && let Ok(root) = std::fs::canonicalize(pictures)
    {
        roots.push(root);
    }
    let Some(root) = roots.iter().find(|root| canonical.starts_with(root)) else {
        return Err(BlockRejection::fs_read_denied(
            "image_path: outside the allowed roots (workspace, OS pictures dir)",
        ));
    };
    // Sensitive-subtree rejection: a dot-prefixed component below the root.
    let relative = canonical.strip_prefix(root).unwrap_or(canonical.as_path());
    let sensitive = relative.components().any(|component| {
        matches!(component, std::path::Component::Normal(name)
            if name.to_string_lossy().starts_with('.'))
    });
    if sensitive {
        return Err(BlockRejection {
            kind: NanoErrorKind::FsSensitiveDenied,
            message: "image_path: sensitive subtree (dot-path under an allowed root)".into(),
        });
    }
    // Audit-H2 hook point: the authorize→open window a workspace-controlled
    // symlink/junction swap races. Test-only; compiled out otherwise.
    #[cfg(test)]
    PRE_OPEN_HOOK.with(|hook| {
        if let Some(fire) = hook.borrow_mut().take() {
            fire();
        }
    });
    // Handle-relative no-follow confined open (audit H-2), then the bounded
    // read: at most MAX_IMAGE_FILE_BYTES + 1 so an oversize file is a typed
    // ImageTooLarge with ZERO decode work.
    use std::io::Read as _;
    let file = open_confined(&canonical, &authorized_meta)?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BlockRejection::fs_read_denied("image_path: read failed"))?;
    if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
        return Err(BlockRejection {
            kind: NanoErrorKind::ImageTooLarge,
            message: "image file exceeds the 50 MiB intake ceiling".into(),
        });
    }
    Ok((bytes, canonical.display().to_string()))
}

// Audit-H2 regression hook: fires in the authorize→open window of
// `confined_image_read` so a test can deterministically swap in a
// symlink/junction. Test-only; never compiled into production builds.
#[cfg(test)]
thread_local! {
    static PRE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Unix (audit H-2): O_NOFOLLOW confines the final component; the OPENED
/// handle's dev/ino must equal the authorized file's identity — a swapped
/// intermediate symlink resolves to a DIFFERENT file and fails closed on
/// the identity mismatch.
#[cfg(unix)]
fn open_confined(
    canonical: &std::path::Path,
    authorized: &std::fs::Metadata,
) -> Result<std::fs::File, BlockRejection> {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(canonical)
        .map_err(|_| BlockRejection::fs_read_denied("image_path: cannot open the file"))?;
    let meta = file
        .metadata()
        .map_err(|_| BlockRejection::fs_read_denied("image_path: cannot stat the opened file"))?;
    if meta.dev() != authorized.dev() || meta.ino() != authorized.ino() {
        return Err(BlockRejection::fs_read_denied(
            "image_path: path swapped between authorization and open",
        ));
    }
    Ok(file)
}

/// Windows (audit H-2): reparse-safe CreateFileW + handle-side reparse and
/// final-path verification against the authorized canonical path (the
/// nano-session attachment-store pattern).
#[cfg(windows)]
fn open_confined(
    canonical: &std::path::Path,
    _authorized: &std::fs::Metadata,
) -> Result<std::fs::File, BlockRejection> {
    windows_confined_open::open_verified(canonical)
}

#[cfg(windows)]
mod windows_confined_open {
    use super::BlockRejection;
    use std::path::Path;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    use windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING;

    fn to_wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Open the authorized `canonical` path without following a
    /// final-component reparse point, then prove from the OPENED HANDLE
    /// (never the path) that it is a plain non-reparse file whose final
    /// path is still `canonical`. Every failure fails closed as a read
    /// denial.
    pub fn open_verified(canonical: &Path) -> Result<std::fs::File, BlockRejection> {
        let wide = to_wide(canonical);
        // Safety: `wide` is NUL-terminated and outlives the call; the
        // returned handle is either invalid (the error path) or ownership
        // moves into the `File` below (closed on drop).
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(BlockRejection::fs_read_denied(
                "image_path: cannot open the file",
            ));
        }
        if let Err(err) = verify_handle(handle, canonical) {
            // Safety: the handle is valid and not yet owned by a File.
            unsafe {
                CloseHandle(handle);
            }
            return Err(err);
        }
        // Safety: the handle is a valid, verified open file handle; the
        // File takes ownership and closes it on drop.
        use std::os::windows::io::FromRawHandle as _;
        Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut std::ffi::c_void) })
    }

    /// Handle-side verification: a reparse point swapped in after
    /// authorization was opened AS a reparse point (the open flag never
    /// follows one) and is rejected here; a swapped INTERMEDIATE component
    /// (e.g. a junctioned directory under the workspace) moves the handle's
    /// final path away from the authorized canonical path and is rejected
    /// here.
    fn verify_handle(handle: HANDLE, canonical: &Path) -> Result<(), BlockRejection> {
        // Safety: `handle` is a valid open file handle; `info` is a plain
        // out struct.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(BlockRejection::fs_read_denied(
                "image_path: cannot stat the opened file",
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(BlockRejection::fs_read_denied(
                "image_path: reparse point swapped in before open",
            ));
        }
        let final_path = final_path_for_handle(handle)?;
        if !same_path(&final_path, canonical) {
            return Err(BlockRejection::fs_read_denied(
                "image_path: path swapped between authorization and open",
            ));
        }
        Ok(())
    }

    /// The handle's final path (the nano-sandbox acl.rs:1158 pattern).
    fn final_path_for_handle(handle: HANDLE) -> Result<std::path::PathBuf, BlockRejection> {
        // Safety: `handle` is valid; the sizing call writes nothing.
        let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
        if needed == 0 {
            return Err(BlockRejection::fs_read_denied(
                "image_path: cannot query the opened file",
            ));
        }
        let mut buffer = vec![0u16; needed as usize + 1];
        // Safety: `buffer` is `needed + 1` wide chars, writable.
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if written == 0 || written as usize >= buffer.len() {
            return Err(BlockRejection::fs_read_denied(
                "image_path: cannot query the opened file",
            ));
        }
        use std::os::windows::ffi::OsStringExt as _;
        Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..written as usize],
        )))
    }

    /// Case-insensitive equality on two `\\?\`-style absolute paths (both
    /// GetFinalPathNameByHandleW and canonicalize produce them); separators
    /// are normalized first.
    fn same_path(left: &Path, right: &Path) -> bool {
        fn normalize(path: &Path) -> String {
            path.as_os_str()
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        }
        normalize(left) == normalize(right)
    }
}

/// P2a §2.3 — converts the ACP `session/prompt` `prompt` block array into
/// the ONE authoritative §5.2.1 [`TurnInput`], preserving block ORDER and
/// multiplicity exactly (the ordered manifest the journal relies on):
///
/// - `{ "type": "text", "text": … }` → `TurnBlock::Text`;
/// - `{ "type": "image", "data": <base64>, "mimeType": <mime> }` → the §4
///   loader (the claimed MIME is a HINT only — a claim-vs-sniff mismatch is
///   a typed `ImageInvalid`, never a quiet re-label) → `TurnBlock::Image`
///   carrying BOTH the durable digest reference AND the live re-encoded
///   pixels;
/// - `{ "type": "image_path", "path": … }` — Nano's TUI extension block
///   (§3.1): the §3.3 path confinement above + the same §4 loader (the TUI
///   process never reads image bytes);
/// - any other block type (`resource`, `resource_link`, `audio`, …) →
///   typed `InvalidParams` rejection naming the type TAG only. Today such
///   blocks are silently dropped; typed rejection is the deliberate,
///   flagged behavior change (§14 deviation 8).
///
/// §4.2 prompt-level caps are enforced here: >16 images or >50 MiB
/// aggregate (saturating arithmetic) → typed `ImageTooMany`. Decodes are
/// sequential (the loader's §4.2 semaphore), one block at a time. Each
/// accepted image logs its §9.4 intake receipt (host-side, one line).
pub async fn acp_blocks_to_content_blocks(
    prompt: &[serde_json::Value],
    workspace: &std::path::Path,
) -> Result<TurnInput, BlockRejection> {
    use base64::Engine;
    let mut blocks: Vec<TurnBlock> = Vec::new();
    let mut image_count: usize = 0;
    let mut aggregate_bytes: u64 = 0;
    for part in prompt {
        let tag = part.get("type").and_then(|t| t.as_str());
        match tag {
            Some("text") => {
                let text = part.get("text").and_then(|t| t.as_str()).unwrap_or("");
                blocks.push(TurnBlock::Text {
                    text: text.to_string(),
                });
            }
            Some("image") => {
                image_count += 1;
                if image_count > nano_tools::image::MAX_IMAGES_PER_PROMPT {
                    return Err(BlockRejection {
                        kind: NanoErrorKind::ImageTooMany,
                        message: "too many images in one prompt (limit: 16)".into(),
                    });
                }
                let data = part
                    .get("data")
                    .and_then(|d| d.as_str())
                    .ok_or_else(|| BlockRejection::invalid_params("image block requires data"))?;
                // Malformed base64 is a param-shape failure → InvalidParams
                // (§7: reused, not new). The string is validated, never
                // echoed.
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| {
                        BlockRejection::invalid_params("image block data is not valid base64")
                    })?;
                let claimed_mime = part.get("mimeType").and_then(|m| m.as_str());
                let mut loaded = nano_tools::image::load_image(&bytes, claimed_mime)
                    .await
                    .map_err(|err| BlockRejection {
                        kind: err.kind,
                        message: err.to_string(),
                    })?;
                loaded.placeholder = Some(format!("[Image #{image_count}: attached image]"));
                aggregate_bytes = aggregate_bytes.saturating_add(loaded.bytes.len() as u64);
                if aggregate_bytes > nano_tools::image::MAX_PROMPT_IMAGE_AGGREGATE_BYTES {
                    return Err(BlockRejection {
                        kind: NanoErrorKind::ImageTooMany,
                        message: "prompt image aggregate exceeds the 50 MiB limit".into(),
                    });
                }
                eprintln!("wayland-nano: intake {}", loaded.receipt_line());
                blocks.push(loaded_to_turn_block(loaded));
            }
            Some("image_path") => {
                image_count += 1;
                if image_count > nano_tools::image::MAX_IMAGES_PER_PROMPT {
                    return Err(BlockRejection {
                        kind: NanoErrorKind::ImageTooMany,
                        message: "too many images in one prompt (limit: 16)".into(),
                    });
                }
                let path = part.get("path").and_then(|p| p.as_str()).ok_or_else(|| {
                    BlockRejection::invalid_params("image_path block requires path")
                })?;
                let (bytes, display) = confined_image_read(path, workspace)?;
                let mut loaded =
                    nano_tools::image::load_image(&bytes, None)
                        .await
                        .map_err(|err| BlockRejection {
                            kind: err.kind,
                            message: err.to_string(),
                        })?;
                loaded.orig_path = Some(display.clone());
                loaded.placeholder = Some(format!("[Image #{image_count}: {display}]"));
                aggregate_bytes = aggregate_bytes.saturating_add(loaded.bytes.len() as u64);
                if aggregate_bytes > nano_tools::image::MAX_PROMPT_IMAGE_AGGREGATE_BYTES {
                    return Err(BlockRejection {
                        kind: NanoErrorKind::ImageTooMany,
                        message: "prompt image aggregate exceeds the 50 MiB limit".into(),
                    });
                }
                eprintln!("wayland-nano: intake {}", loaded.receipt_line());
                blocks.push(loaded_to_turn_block(loaded));
            }
            // Typed rejection naming the type TAG only (bounded: the tag is
            // truncated, never the content) — the C7 closed-fields rule.
            other => {
                let tag = other.unwrap_or("<missing>");
                let tag: String = tag.chars().take(64).collect();
                return Err(BlockRejection::invalid_params(format!(
                    "unsupported prompt block type: {tag}"
                )));
            }
        }
    }
    Ok(TurnInput { blocks })
}

/// §2.3: one loaded image → the two-half `TurnBlock::Image` — the durable
/// digest reference (journaled) AND the live re-encoded base64 pixels
/// (request-time only, NEVER journaled). The reference mime is the
/// RE-ENCODED wire mime (png/jpeg — the closed wire subset): the journaled
/// manifest must label the bytes it references, and the §5.3 rehydration
/// rebuilds the wire block from exactly those bytes.
fn loaded_to_turn_block(loaded: nano_tools::image::LoadedImage) -> TurnBlock {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&loaded.bytes);
    TurnBlock::Image {
        reference: ImageRef {
            digest: loaded.digest,
            mime: loaded.wire_mime,
            bytes: loaded.bytes.len() as u64,
            width: loaded.width,
            height: loaded.height,
            placeholder: loaded.placeholder.unwrap_or_default(),
        },
        data,
    }
}
/// A model the session can switch to (the ACP `models` block, unstable API).
/// The ACP-spec field name is `modelId` — proven by live Desktop behavior
/// (SessionLifecycle.ts maps m.modelId; rows sent as `id` render with
/// undefined ids and no-op). The Rust field stays `id`; the wire carries
/// `modelId` via serde rename.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModel {
    #[serde(rename = "modelId")]
    pub id: String,
    pub name: String,
}

/// The `models` block shared by session/new, session/load and
/// session/set_model responses. Desktop reads it top-level
/// (AcpConnection.ts `parseSessionCapabilities`).
pub fn session_models_value(
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "currentModelId": current_model_id,
        "availableModels": available,
    })
}

/// The `modes` block shared by session/new and session/load responses
/// (C2): every advertised PRIVILEGE mode comes from the single
/// [`PermissionMode`] metadata, exactly the way `session_models_value`
/// parameterizes models, so the wire can never drift from what the gate
/// enforces. C10 §3 (panel ruling Q1): `plan` is a FOURTH advertised id — a
/// PROJECTION of the orthogonal plan posture, not a privilege mode. While
/// the posture is active `currentModeId` reports `plan`; the underlying C2
/// privilege mode is preserved engine-side, never altered by entering or
/// exiting plan, and is re-advertised as `currentModeId` on exit. A client
/// must never read plan→default→plan as a privilege change.
pub fn session_modes_value(current_id: &str) -> serde_json::Value {
    let mut available: Vec<serde_json::Value> = PermissionMode::ALL
        .iter()
        .map(|mode| serde_json::json!({"id": mode.id(), "name": mode.label()}))
        .collect();
    available.push(serde_json::json!({"id": PLAN_MODE_ID, "name": "Plan"}));
    serde_json::json!({
        "availableModes": available,
        "currentModeId": current_id,
    })
}

/// The wire id of the plan-posture projection (C10 §3). Never parses as a
/// [`PermissionMode`] — the set_mode handler special-cases it.
pub const PLAN_MODE_ID: &str = "plan";

/// session/new response.
pub fn session_new_result(
    session_id: &str,
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "modes": session_modes_value(PermissionMode::default().id()),
        "models": session_models_value(current_model_id, available)
    })
}

/// session/load response. Per the ACP shape Desktop expects
/// (AcpConnection.ts `loadSession`: "session/load returns
/// modes/models/configOptions but not sessionId"), the loaded session keeps
/// the id the client sent, so no sessionId is returned here. C2: the mode
/// is NOT restored on load — a resumed session starts in `default` and
/// re-entering an elevated mode takes a fresh, explicit session/set_mode.
pub fn session_load_result(
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "modes": session_modes_value(PermissionMode::default().id()),
        "models": session_models_value(current_model_id, available)
    })
}

/// session/set_model response: the updated models state (Desktop updates its
/// cache from the requested id; echoing the state keeps the agent the source
/// of truth).
pub fn set_model_result(current_model_id: &str, available: &[AvailableModel]) -> serde_json::Value {
    serde_json::json!({
        "models": session_models_value(current_model_id, available)
    })
}

/// A replayed user message (session/load history restore). Desktop ignores
/// these (its local DB already shows the user's own messages) but real ACP
/// agents emit them, so the replay is a faithful transcript.
pub fn user_message_chunk(session_id: &str, text: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
}

/// A streamed text chunk (the assistant's reply, incrementally).
pub fn agent_message_chunk(session_id: &str, text: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
}

/// A tool call starting (shown in Desktop as a tool card).
pub fn tool_call_update(
    session_id: &str,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": call_id,
                "title": name,
                "kind": tool_kind(name),
                "status": "in_progress",
                "rawInput": args
            }
        }),
    )
}

/// A replayed tool call (session/load history restore): the same `tool_call`
/// card shape as a live call, but already carrying its final status, since
/// the work happened in a previous process lifetime. A failed call with a
/// journaled `error_kind` carries the typed presentation + `_meta.nanoError`
/// exactly like the live path (D3/D5 — one typed op, identical frames).
pub fn tool_call_replay(
    session_id: &str,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
    ok: bool,
    error_kind: Option<NanoErrorKind>,
) -> JsonRpcNotification {
    let mut update = serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": call_id,
        "title": name,
        "kind": tool_kind(name),
        "status": if ok { "completed" } else { "failed" },
        "rawInput": args
    });
    if let Some(kind) = error_kind {
        attach_typed_failure(&mut update, kind);
    }
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": update
        }),
    )
}

/// A tool call completing. On a typed failure (`error_kind`), the frame
/// gains (design §2/D3):
/// - `content`: the static table presentation — the exact shape Desktop's
///   normalizer stringifies today, so failed cards show an honest message
///   with ZERO Desktop change;
/// - `_meta.nanoError`: the closed typed payload for typed consumers.
///   `rawOutput` keeps the digest for back-compat.
pub fn tool_call_done(
    session_id: &str,
    call_id: &str,
    ok: bool,
    output: &str,
    error_kind: Option<NanoErrorKind>,
) -> JsonRpcNotification {
    let mut update = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": call_id,
        "status": if ok { "completed" } else { "failed" },
        "rawOutput": output
    });
    if let Some(kind) = error_kind {
        attach_typed_failure(&mut update, kind);
    }
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": update
        }),
    )
}

/// D3 fields on a failed tool card: ACP-spec `content` (Desktop's
/// normalizer reads it verbatim) + `_meta.nanoError` (typed consumers).
fn attach_typed_failure(update: &mut serde_json::Value, kind: NanoErrorKind) {
    update["content"] = serde_json::json!([{
        "type": "content",
        "content": { "type": "text", "text": error_presentation(kind) }
    }]);
    update["_meta"] = nano_error_data(kind, &NanoErrorExtras::default());
}

fn tool_kind(name: &str) -> &'static str {
    match name {
        n if n.starts_with("fs_read") => "read",
        n if n.starts_with("fs_edit") || n.starts_with("fs_write") => "edit",
        n if n.starts_with("shell") => "execute",
        n if n.starts_with("search") || n.starts_with("glob") => "search",
        n if n.starts_with("web_fetch") => "fetch",
        _ => "other",
    }
}

/// session/prompt response.
pub fn prompt_result(stop_reason: &str) -> serde_json::Value {
    serde_json::json!({ "stopReason": stop_reason })
}

/// A context-compaction lifecycle notice (C1 §7): emitted on
/// CompactionBegin/Complete/Cancel so UIs can render the event as a system
/// note in the transcript. `status` is "begin" | "complete" | "cancel".
/// Clients that do not know the kind tolerate it (unknown sessionUpdate
/// kinds convert to zero messages — pinned by Desktop's adapter tests).
pub fn compaction_notice(session_id: &str, status: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "compaction",
                "status": status
            }
        }),
    )
}

// ── C9 robustness-pack wire shapes ──────────────────────────────────────

/// The `session/steer` extension method (C9 Q1 RULED shape (b)): mid-turn
/// user input that the running turn drains at its next loop top. The
/// response resolves IMMEDIATELY with the enqueue ack — it is NOT the turn
/// result (the terminal result still belongs to the original prompt).
pub const SESSION_STEER_METHOD: &str = "session/steer";

/// session/steer queued ack: the submitter's proof of acceptance.
pub fn steer_queued_result(position: usize) -> serde_json::Value {
    serde_json::json!({ "queued": true, "position": position })
}

/// The dropped-steer notice (C9 §3.3): because the queued ack resolves
/// before any later cancellation, a drop-on-cancel travels as a LATER
/// session/update carrying the submitter's request id and the steer text
/// digest (never the text itself — it was never model-visible). Exactly
/// one notice per dropped steer; none is dropped silently.
pub fn steer_dropped_notice(
    session_id: &str,
    request_id: &str,
    text_digest: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "steer_dropped",
                "requestId": request_id,
                "textDigest": text_digest
            }
        }),
    )
}

/// Reconnect banner (C9 §2.2): one notice per reconnect sleep, typed
/// fields only — UIs render, never parse strings.
pub fn reconnect_notice(
    session_id: &str,
    attempt: u32,
    next_delay_ms: u64,
    deadline_remaining_ms: u64,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "reconnecting",
                "attempt": attempt,
                "nextDelayMs": next_delay_ms,
                "deadlineRemainingMs": deadline_remaining_ms
            }
        }),
    )
}

/// Loud inert-param notice (C9 §4, Q3 rung 2): a requested param was
/// omitted from the wire (or mapped-but-recorded-inert) on this surface.
pub fn param_inert_notice(
    session_id: &str,
    param: &str,
    surface: &str,
    detail: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "param_inert",
                "param": param,
                "surface": surface,
                "detail": detail
            }
        }),
    )
}

/// P2a §5.3 (Q3 RULED): the loud resume-degradation notice — a journaled
/// image manifest entry whose blob is missing/tampered/malformed became the
/// explicit placeholder in the rebuilt context. Closed fields only (the C7
/// `nanoError` discipline): the bounded cause word and the 8-char digest
/// prefix, never a path or payload.
pub fn attachment_missing_notice(
    session_id: &str,
    cause: &str,
    digest_prefix: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "attachment_missing",
                "kind": "attachment_missing",
                "cause": cause,
                "digest": digest_prefix
            }
        }),
    )
}

/// Rate-limit observation (C9 §5, Q4): the coalesced latest snapshot per
/// turn iteration. `snapshot` is the serialized RateLimitSnapshot (all
/// fields optional — UIs render "unknown" on absence, never a guess).
pub fn rate_limit_notice(session_id: &str, snapshot: serde_json::Value) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "rate_limit",
                "snapshot": snapshot
            }
        }),
    )
}

/// P1 §5: the session meter's status payload (TUI/Desktop budget surfaces):
/// totals + honesty label + the cap position when one is configured.
/// `microcents`/`priced` come from the meter (the budget authority) —
/// `priced: false` renders `unpriced`, NEVER $0.000.
pub fn budget_notice(
    session_id: &str,
    session_tokens: u64,
    microcents: u64,
    priced: bool,
    limit: Option<u64>,
    observed: Option<u64>,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "budget",
                "session_tokens": session_tokens,
                "microcents": microcents,
                "priced": priced,
                "limit": limit,
                "observed": observed
            }
        }),
    )
}

/// P1 §4.1: the typed 80% BudgetWarn notice `{limit, observed, pct_used}`
/// (C7 vocabulary; latest-wins, fires once per crossing).
pub fn budget_warn_notice(
    session_id: &str,
    limit: u64,
    observed: u64,
    pct_used: u64,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "budget_warn",
                "limit": limit,
                "observed": observed,
                "pct_used": pct_used
            }
        }),
    )
}

/// P1 §4.2: the typed clamp notice — a request's `max_tokens` was clamped
/// to the reserved output allowance. Logged, never silent.
pub fn budget_clamp_notice(session_id: &str, requested: u64, granted: u64) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "budget_clamp",
                "requested": requested,
                "granted": granted
            }
        }),
    )
}

/// session/request_permission request payload (agent → host).
pub fn request_permission_request(
    id: u64,
    session_id: &str,
    call_id: &str,
    title: &str,
    args: &serde_json::Value,
) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(id),
        method: "session/request_permission".into(),
        params: Some(serde_json::json!({
            "sessionId": session_id,
            "toolCall": {
                "toolCallId": call_id,
                "title": title,
                "rawInput": args
            },
            "options": [
                { "optionId": "allow", "kind": "allow_once", "name": "Allow once" },
                { "optionId": "deny", "kind": "reject_once", "name": "Deny" }
            ]
        })),
    }
}

/// The terminal dismiss option id minted by [`request_question_request`].
/// Deliberately `reject`-prefixed: Desktop maps the outcome by substring
/// (`optionId.includes('reject')` ⇒ rejected — AcpConnection.ts:729-730),
/// so a dismiss arrives as a `rejected` outcome and an answer as
/// `selected`+`opt_{i}`.
pub const QUESTION_DISMISS_ID: &str = "reject";

/// session/request_permission carrying a STRUCTURED QUESTION (C10 §5): the
/// same ACP method as the approval prompt (the shape already carries an
/// arbitrary options[] array and Desktop's handler maps whatever arrives —
/// AcpConnection.ts:720-744), but the options are minted FROM the tool's
/// option labels: `opt_{i}` per label plus a terminal Dismiss. The wire
/// carries only the minted ids; the agent resolves the selected id back to
/// the label through the id→label map captured at send time.
/// `toolCall.toolCallId` MUST equal the tool_call id already streamed in
/// session/update for the ask_user call, so Desktop's permission card and
/// answer channel line up (the wcore #504 failure mode is an empty or
/// mis-keyed option set).
pub fn request_question_request(
    id: u64,
    session_id: &str,
    call_id: &str,
    title: &str,
    args: &serde_json::Value,
    option_labels: &[String],
) -> JsonRpcRequest {
    let mut options: Vec<serde_json::Value> = option_labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            serde_json::json!({ "optionId": format!("opt_{i}"), "kind": "allow_once", "name": label })
        })
        .collect();
    options.push(
        serde_json::json!({ "optionId": QUESTION_DISMISS_ID, "kind": "reject_once", "name": "Dismiss" }),
    );
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(id),
        method: "session/request_permission".into(),
        params: Some(serde_json::json!({
            "sessionId": session_id,
            "toolCall": {
                "toolCallId": call_id,
                "title": title,
                "rawInput": args
            },
            "options": options
        })),
    }
}

/// A tool_call_update carrying an ACP-standard `diff` content block
/// (C10 §6): `{ "type": "diff", path, oldText, newText }`. Emitted when a
/// fs_write/fs_edit produced a diff — live-wire-only (never journaled),
/// sensitive-path-suppressed, 32k/side capped upstream. `rawOutput` flows
/// unchanged in the regular done frame; this is an ADDITIONAL update for
/// the same call id (ACP tool_call_update frames are order-tolerant).
pub fn tool_call_diff(
    session_id: &str,
    call_id: &str,
    path: &std::path::Path,
    old_text: Option<&str>,
    new_text: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": call_id,
                "content": [{
                    "type": "diff",
                    "path": path.display().to_string(),
                    "oldText": old_text,
                    "newText": new_text
                }]
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_response_shape() {
        // P2a §6.4/D4: the vendored catalog blesses NOTHING until the §13
        // leg-6 probes land, so the startup-leaf advertisement is false here.
        let catalog = nano_model::vision_catalog::VisionCatalog::vendored()
            .expect("vendored vision catalog parses");
        let caps = agent_capabilities("flux-pinned-codestral", &catalog);
        assert_eq!(caps["protocolVersion"], 1);
        assert_eq!(caps["agentInfo"]["name"], "wayland-nano");
        assert_eq!(
            caps["agentCapabilities"]["promptCapabilities"]["text"],
            true
        );
        // Advisory, initialize-scoped, startup-leaf-driven (§6.4): false for
        // an unproven leaf.
        assert_eq!(
            caps["agentCapabilities"]["promptCapabilities"]["image"],
            false
        );
        // stdio-only MCP: the block's presence advertises stdio to Desktop
        // (acpTypes.ts), http/sse stay honestly false.
        let mcp = &caps["agentCapabilities"]["mcpCapabilities"];
        assert!(
            mcp.is_object(),
            "mcpCapabilities must be advertised: {caps}"
        );
        assert_eq!(mcp["http"], false);
        assert_eq!(mcp["sse"], false);
    }

    // ── P2a §2.3: the ACP prompt-block converter ─────────────────────────

    /// Regression: text-only prompts convert byte-identically to the old
    /// extractor's join — one Text block per part, projection joins with
    /// "\n".
    #[tokio::test]
    async fn p2a_converter_text_only_matches_legacy_join() {
        let prompt = serde_json::json!([
            {"type": "text", "text": "first"},
            {"type": "text", "text": "second"}
        ]);
        let input =
            acp_blocks_to_content_blocks(prompt.as_array().unwrap(), std::path::Path::new("."))
                .await
                .expect("text-only converts");
        assert_eq!(input.projection(), "first\nsecond");
        assert!(!input.has_images());
    }

    /// Unknown block types are typed-rejected (InvalidParams) naming the
    /// type TAG only — never silently dropped (§14 deviation 8). The FIRST
    /// invalid block aborts the whole prompt.
    #[tokio::test]
    async fn p2a_converter_unknown_block_types_typed_reject() {
        for tag in ["resource", "resource_link", "audio"] {
            let prompt = serde_json::json!([{"type": tag, "data": "whatever"}]);
            let err =
                acp_blocks_to_content_blocks(prompt.as_array().unwrap(), std::path::Path::new("."))
                    .await
                    .expect_err("unknown block type must reject");
            assert_eq!(err.kind, NanoErrorKind::InvalidParams);
            assert!(
                err.message.contains(tag),
                "names the tag only: {}",
                err.message
            );
            assert!(!err.message.contains("whatever"), "no content echo");
        }
        // First-invalid-block aborts: a valid text part before a bad block
        // still rejects the whole prompt.
        let prompt = serde_json::json!([
            {"type": "text", "text": "ok"},
            {"type": "audio", "data": "x"}
        ]);
        assert!(
            acp_blocks_to_content_blocks(prompt.as_array().unwrap(), std::path::Path::new("."))
                .await
                .is_err()
        );
        // A block with no type tag is typed-rejected too.
        let prompt = serde_json::json!([{"text": "no tag"}]);
        let err =
            acp_blocks_to_content_blocks(prompt.as_array().unwrap(), std::path::Path::new("."))
                .await
                .expect_err("missing tag must reject");
        assert_eq!(err.kind, NanoErrorKind::InvalidParams);
    }

    /// Malformed base64 in an image block is a typed param-shape failure
    /// (§7: InvalidParams reused, not new) — and it happens BEFORE any
    /// loader/decode work.
    #[tokio::test]
    async fn p2a_converter_malformed_base64_is_typed() {
        let prompt = serde_json::json!([{"type": "image", "data": "!!!not-base64!!!", "mimeType": "image/png"}]);
        let err =
            acp_blocks_to_content_blocks(prompt.as_array().unwrap(), std::path::Path::new("."))
                .await
                .expect_err("malformed base64 must reject");
        assert_eq!(err.kind, NanoErrorKind::InvalidParams);
    }

    // ── P2a audit H-2: the confined image_path open ──────────────────────

    fn h2_temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nano-p2a-acp-h2-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Directory link helper (the attachment_store.rs precedent): NTFS
    /// junction on Windows (no privilege required), symlink elsewhere.
    /// Returns false when the host refuses — the caller then skips LOUDLY.
    fn make_dir_link(link: &std::path::Path, target: &std::path::Path) -> bool {
        #[cfg(windows)]
        {
            return std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .expect("spawn mklink")
                .status
                .success();
        }
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(target, link).is_ok();
        }
        #[allow(unreachable_code)]
        false
    }

    /// The legitimate workspace path still reads through the
    /// handle-verified open.
    #[test]
    fn confined_image_read_legitimate_workspace_file() {
        let root = h2_temp_dir("legit");
        let workspace = root.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let image = workspace.join("pic.png");
        std::fs::write(&image, b"workspace-image-bytes").unwrap();
        let (bytes, display) =
            confined_image_read(image.to_str().unwrap(), &workspace).expect("legitimate read");
        assert_eq!(bytes, b"workspace-image-bytes");
        assert!(display.contains("pic.png"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Audit H-2 regression: a workspace-controlled link is swapped in
    /// BETWEEN authorization and open (the test hook fires in exactly that
    /// window). The read must fail closed from the OPENED HANDLE's evidence
    /// — never return the outside bytes:
    /// - unix: the swapped intermediate symlink resolves to a different
    ///   file; the opened handle's dev/ino mismatches the authorized
    ///   identity;
    /// - Windows: the swapped intermediate junction moves the handle's
    ///   final path away from the authorized canonical path.
    #[test]
    fn confined_image_read_swap_between_authorize_and_open_fails_closed() {
        let root = h2_temp_dir("toctou");
        let workspace = root.join("ws");
        let sub = workspace.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let image = sub.join("pic.png");
        std::fs::write(&image, b"workspace-image-bytes").unwrap();
        // The outside payload a successful swap would smuggle in.
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("pic.png"), b"outside-secret-bytes").unwrap();

        // The hook fires BETWEEN authorization and open: the authorized
        // subdir is swapped for a junction/symlink to the outside dir.
        let saved = workspace.join("sub-saved");
        let linked = std::rc::Rc::new(std::cell::Cell::new(true));
        let linked_hook = linked.clone();
        let sub_hook = sub.clone();
        let saved_hook = saved.clone();
        let outside_hook = outside.clone();
        PRE_OPEN_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&sub_hook, &saved_hook).unwrap();
                if !make_dir_link(&sub_hook, &outside_hook) {
                    linked_hook.set(false);
                }
            }));
        });
        let result = confined_image_read(image.to_str().unwrap(), &workspace);
        if !linked.get() {
            eprintln!(
                "LOUD SKIP: host refused link creation (no developer mode/admin) — \
                 swap-between-authorize-and-open scenario cannot run here"
            );
            return;
        }
        let err = result.expect_err("a swapped-in link must fail closed");
        assert_eq!(err.kind, NanoErrorKind::FsReadDenied, "{err:?}");
        // Restore the layout: the legitimate path still works.
        #[cfg(windows)]
        std::fs::remove_dir(&sub).unwrap(); // junctions are removed as directories
        #[cfg(unix)]
        std::fs::remove_file(&sub).unwrap(); // a symlink is removed as a file
        std::fs::rename(&saved, &sub).unwrap();
        let (bytes, _) = confined_image_read(image.to_str().unwrap(), &workspace).unwrap();
        assert_eq!(bytes, b"workspace-image-bytes");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_update_shapes() {
        let chunk = agent_message_chunk("s1", "hello");
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["method"], "session/update");
        assert_eq!(
            json["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(json["params"]["update"]["content"]["text"], "hello");

        let tool = tool_call_update("s1", "c1", "shell", &serde_json::json!({"command":"ls"}));
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["params"]["update"]["kind"], "execute");
        assert_eq!(json["params"]["update"]["status"], "in_progress");

        let fetch = tool_call_update(
            "s1",
            "c2",
            "web_fetch",
            &serde_json::json!({"url":"https://example.com/"}),
        );
        let json = serde_json::to_value(&fetch).unwrap();
        assert_eq!(json["params"]["update"]["kind"], "fetch");
    }

    /// Cross-cutting C3/C4: a full-size fs_read page (100 KB) and a
    /// full-size web_fetch body (64 KB) ride `tool_call_done`'s rawOutput
    /// through the frame codec (serde JSON / NDJSON) without corruption.
    /// The global engine-side ceiling is tracked separately
    /// (docs/FOLLOWUPS.md F-1).
    #[test]
    fn full_size_tool_results_round_trip_through_the_codec() {
        let page = "line content\n".repeat(8 * 1024); // ~106 KB, C3 page-sized
        let body = "x".repeat(64 * 1024); // C4 body cap
        for output in [&page, &body] {
            let frame = tool_call_done("s1", "c1", true, output, None);
            let line = serde_json::to_string(&frame).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                parsed["params"]["update"]["rawOutput"].as_str().unwrap(),
                output,
            );
        }
    }

    #[test]
    fn session_new_carries_the_desktop_models_shape() {
        let available = vec![
            AvailableModel {
                id: "flux-auto".into(),
                name: "flux-auto".into(),
            },
            AvailableModel {
                id: "flux-fast".into(),
                name: "flux-fast".into(),
            },
        ];
        let result = session_new_result("s1", "flux-auto", &available);
        assert_eq!(result["sessionId"], "s1");
        assert_eq!(result["models"]["currentModelId"], "flux-auto");
        let models = result["models"]["availableModels"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1]["modelId"], "flux-fast");
        assert_eq!(models[1]["name"], "flux-fast");

        // session/load carries the same models block, without a sessionId.
        let loaded = session_load_result("flux-fast", &available);
        assert!(loaded.get("sessionId").is_none());
        assert_eq!(loaded["models"]["currentModelId"], "flux-fast");

        let switched = set_model_result("flux-fast", &available);
        assert_eq!(switched["models"]["currentModelId"], "flux-fast");
        assert_eq!(
            switched["models"]["availableModels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn prompt_result_stop_reason() {
        let result = prompt_result("end_turn");
        assert_eq!(result["stopReason"], "end_turn");
    }

    #[test]
    fn modes_block_comes_from_the_permission_mode_metadata() {
        let result = session_new_result("s1", "flux-auto", &[]);
        let modes = &result["modes"];
        assert_eq!(modes["currentModeId"], "default");
        let advertised = modes["availableModes"].as_array().unwrap();
        let ids: Vec<&str> = advertised
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        // The full C2 vocabulary, in privilege order, from PermissionMode::ALL,
        // plus the C10 plan-posture projection as the fourth advertised id.
        assert_eq!(ids, ["read_only", "default", "full_auto", "plan"]);
        for entry in advertised.iter().take(3) {
            let mode = PermissionMode::parse(entry["id"].as_str().unwrap()).unwrap();
            assert_eq!(entry["name"].as_str().unwrap(), mode.label());
        }
        // `plan` is a posture projection, never a PermissionMode.
        assert_eq!(PermissionMode::parse("plan"), None);
        assert_eq!(advertised[3]["name"].as_str().unwrap(), "Plan");
        // session/load advertises the same block and never resurrects a mode.
        let loaded = session_load_result("flux-auto", &[]);
        assert_eq!(loaded["modes"]["currentModeId"], "default");
        assert_eq!(loaded["modes"]["availableModes"], modes["availableModes"]);
    }

    #[test]
    fn method_not_found_is_typed() {
        let resp = JsonRpcResponse::method_not_found(serde_json::json!(7), "bogus/method");
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    /// C10 §5 protocol fixture (pinned BEFORE the consumers landed): the
    /// question request reuses session/request_permission with minted
    /// option ids and a terminal reject-kind Dismiss — the exact shape
    /// Desktop's generic handlePermissionRequest maps.
    #[test]
    fn question_request_shape_is_pinned() {
        let labels = vec!["Yes, proceed".to_string(), "No, stop".to_string()];
        let req = request_question_request(
            9,
            "s1",
            "call-ask_user",
            "Proceed?",
            &serde_json::json!({"question": "Proceed?", "options": [{"label": "Yes, proceed"}, {"label": "No, stop"}]}),
            &labels,
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["method"], "session/request_permission");
        assert_eq!(json["id"], 9);
        let params = &json["params"];
        assert_eq!(params["sessionId"], "s1");
        // The toolCallId MUST equal the streamed ask_user tool_call id.
        assert_eq!(params["toolCall"]["toolCallId"], "call-ask_user");
        assert_eq!(params["toolCall"]["title"], "Proceed?");
        let options = params["options"].as_array().unwrap();
        assert_eq!(options.len(), 3); // 2 minted + terminal Dismiss
        assert_eq!(
            options[0],
            serde_json::json!({"optionId": "opt_0", "kind": "allow_once", "name": "Yes, proceed"})
        );
        assert_eq!(
            options[1],
            serde_json::json!({"optionId": "opt_1", "kind": "allow_once", "name": "No, stop"})
        );
        assert_eq!(
            options[2],
            serde_json::json!({"optionId": "reject", "kind": "reject_once", "name": "Dismiss"})
        );
    }

    /// C10 §6 protocol fixture (pinned BEFORE the producers landed): the
    /// ACP-standard diff content block on tool_call_update.
    #[test]
    fn diff_content_block_shape_is_pinned() {
        let frame = tool_call_diff(
            "s1",
            "c1",
            std::path::Path::new("src/main.rs"),
            Some("old line"),
            "new line",
        );
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["method"], "session/update");
        let update = &json["params"]["update"];
        assert_eq!(update["sessionUpdate"], "tool_call_update");
        assert_eq!(update["toolCallId"], "c1");
        assert_eq!(
            update["content"][0],
            serde_json::json!({
                "type": "diff",
                "path": "src/main.rs",
                "oldText": "old line",
                "newText": "new line"
            })
        );
        // Whole-file add: oldText serializes as JSON null.
        let add = tool_call_diff("s1", "c2", std::path::Path::new("new.rs"), None, "body");
        let json = serde_json::to_value(&add).unwrap();
        assert_eq!(
            json["params"]["update"]["content"][0]["oldText"],
            serde_json::Value::Null
        );
    }

    /// C7/D3: a typed tool failure carries the ACP-spec `content`
    /// presentation (Desktop's normalizer stringifies it today — zero
    /// Desktop change), the `_meta.nanoError` typed payload, and keeps the
    /// digest in rawOutput for back-compat.
    #[test]
    fn failed_tool_card_carries_content_and_meta() {
        let frame = tool_call_done(
            "s1",
            "c1",
            false,
            "len:42",
            Some(NanoErrorKind::ApprovalDenied),
        );
        let json = serde_json::to_value(&frame).unwrap();
        let update = &json["params"]["update"];
        assert_eq!(update["status"], "failed");
        assert_eq!(update["rawOutput"], "len:42");
        assert_eq!(update["content"][0]["content"]["text"], "Denied by user");
        let nano = &update["_meta"]["nanoError"];
        assert_eq!(nano["kind"], "approval_denied");
        assert_eq!(nano["retryable"], false);

        // The replayed card carries the same typing.
        let replay = tool_call_replay(
            "s1",
            "c1",
            "fs_write",
            &serde_json::json!({"path": "a"}),
            false,
            Some(NanoErrorKind::FsWriteDenied),
        );
        let json = serde_json::to_value(&replay).unwrap();
        let update = &json["params"]["update"];
        assert_eq!(update["status"], "failed");
        assert_eq!(update["_meta"]["nanoError"]["kind"], "fs_write_denied");
        assert_eq!(
            update["content"][0]["content"]["text"],
            "Denied by policy — Path is outside the allowed set; ask the user"
        );

        // Untyped completions are byte-compatible with the pre-C7 shape.
        let plain = tool_call_done("s1", "c1", true, "len:1", None);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json["params"]["update"].get("_meta").is_none());
        assert!(json["params"]["update"].get("content").is_none());
    }

    /// C7/D2: typed error responses carry standard codes + closed data.
    #[test]
    fn typed_error_response_shape() {
        let resp = JsonRpcResponse::err_typed(
            serde_json::json!(3),
            NanoErrorKind::ModelRateLimited,
            "Rate limited — Retrying automatically; wait a moment",
            crate::acp::NanoErrorExtras {
                retry_after_ms: Some(1500),
                ..Default::default()
            },
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32603);
        let nano = &json["error"]["data"]["nanoError"];
        assert_eq!(nano["kind"], "model_rate_limited");
        assert_eq!(nano["retryable"], true);
        assert_eq!(nano["retry_after_ms"], 1500);
        assert!(nano.get("host").is_none());

        // Untyped errors never grow a data field.
        let plain = JsonRpcResponse::err(serde_json::json!(1), -32700, "parse error");
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json["error"].get("data").is_none());
    }
}
