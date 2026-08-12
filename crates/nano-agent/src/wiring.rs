//! Production wiring: Flux-backed ModelDriver and tool-backed ToolExecutor.

use crate::loop_protection::ProgressSignals;
use crate::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::anthropic_messages::AnthropicMessagesClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::flux_responses::FluxResponsesClient;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall, ToolDefinition};
use nano_tools::fs::{FsTools, ReadBounds};
use nano_tools::shell::{ShellKind, ShellTool};

/// One of the three Flux wire surfaces. Completions is the production wire;
/// Responses and Anthropic Messages are selectable compat surfaces (per
/// FINDINGS batch-2 WIRE-2, never the default).
#[derive(Debug)]
enum FluxClient {
    Completions(FluxCompletionsClient),
    Responses(FluxResponsesClient),
    Anthropic(AnthropicMessagesClient),
}

/// ModelDriver over a Flux wire client (default: Completions).
#[derive(Debug)]
pub struct FluxDriver {
    client: FluxClient,
    api_key: String,
}

impl FluxDriver {
    /// Default construction: the production Chat Completions wire.
    pub fn new(client: FluxCompletionsClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Completions(client),
            api_key: api_key.into(),
        }
    }

    /// Explicit opt-in: the Responses surface.
    pub fn responses(client: FluxResponsesClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Responses(client),
            api_key: api_key.into(),
        }
    }

    /// Explicit opt-in: the Anthropic Messages COMPAT surface (thinking/cache
    /// are inert on live Flux — FINDINGS batch-2 WIRE-2).
    pub fn anthropic_compat(client: AnthropicMessagesClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Anthropic(client),
            api_key: api_key.into(),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for FluxDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        match &self.client {
            FluxClient::Completions(client) => client.complete(request, &self.api_key).await,
            FluxClient::Responses(client) => client.complete(request, &self.api_key).await,
            FluxClient::Anthropic(client) => client.complete(request, &self.api_key).await,
        }
    }
}

/// The v1 tool surface advertised to the model.
pub fn v1_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "fs_read".into(),
            description: "Read a file (bounded). Args: path, optional line_offset, max_lines."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "line_offset": {"type": "integer"},
                    "max_lines": {"type": "integer"}
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "fs_write".into(),
            description: "Write a file (creating parents). Args: path, content.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "fs_edit".into(),
            description:
                "Exact-replacement edit. Args: path, old_string, new_string, optional replace_all."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDefinition {
            name: "shell".into(),
            description: "Run a shell command inside the workspace sandbox. Args: command.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        },
    ]
}

/// ToolExecutor over the real fs/shell tools, policy-checked and sandboxed.
#[derive(Debug)]
pub struct RealToolExecutor {
    fs: FsTools,
    shell: ShellTool,
    workspace: std::path::PathBuf,
    /// Dedup state for honest progress signals: re-reading identical content
    /// or re-running an identical command with identical output is NOT new
    /// information (the no-progress detector depends on this truth).
    seen: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl RealToolExecutor {
    pub fn new(fs: FsTools, shell: ShellTool, workspace: &std::path::Path) -> Self {
        Self {
            fs,
            shell,
            workspace: workspace.to_path_buf(),
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Returns true when this (kind:key) content digest was NOT seen before.
    fn mark_and_check_novelty(&self, kind: &str, key: &str, digest_input: &str) -> bool {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        digest_input.hash(&mut hasher);
        let digest = hasher.finish();
        let mut seen = self.seen.lock().unwrap();
        seen.insert(format!("{kind}:{key}"), digest) != Some(digest)
    }

    fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
        call.arguments.get(key).and_then(|v| v.as_str())
    }

    fn resolve(&self, raw: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(raw);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }
}

impl ToolExecutor for RealToolExecutor {
    fn execute(&self, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "fs_read" => {
                let Some(path) = Self::arg_str(call, "path") else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing path".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                let bounds = ReadBounds {
                    line_offset: call
                        .arguments
                        .get("line_offset")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize),
                    max_lines: call
                        .arguments
                        .get("max_lines")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(1000),
                    ..Default::default()
                };
                match self.fs.read_file(&self.resolve(path), &bounds) {
                    Ok((content, truncated)) => {
                        let novel = self.mark_and_check_novelty("read", path, &content);
                        ToolOutcome {
                            ok: true,
                            output: if truncated {
                                format!("{content}\n…[truncated]")
                            } else {
                                content
                            },
                            progress: ProgressSignals {
                                new_information: novel,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "fs_write" => {
                let (Some(path), Some(content)) =
                    (Self::arg_str(call, "path"), Self::arg_str(call, "content"))
                else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing path or content".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                match self.fs.write_file(&self.resolve(path), content) {
                    Ok(()) => ToolOutcome {
                        ok: true,
                        output: "written".into(),
                        progress: ProgressSignals {
                            files_changed: true,
                            ..Default::default()
                        },
                    },
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "fs_edit" => {
                let (Some(path), Some(old), Some(new)) = (
                    Self::arg_str(call, "path"),
                    Self::arg_str(call, "old_string"),
                    Self::arg_str(call, "new_string"),
                ) else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing path/old_string/new_string".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                let replace_all = call
                    .arguments
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match self
                    .fs
                    .edit_file(&self.resolve(path), old, new, replace_all)
                {
                    Ok(n) => ToolOutcome {
                        ok: true,
                        output: format!("{n} replacement(s)"),
                        progress: ProgressSignals {
                            files_changed: true,
                            ..Default::default()
                        },
                    },
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "shell" => {
                let Some(command) = Self::arg_str(call, "command") else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing command".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                match self.shell.run(
                    ShellKind::platform_default(),
                    command,
                    Some(std::time::Duration::from_secs(120)),
                ) {
                    Ok(out) => {
                        let digest = format!("{}|{}|{}", out.exit_code, out.stdout, out.stderr);
                        let novel = self.mark_and_check_novelty("shell", command, &digest);
                        ToolOutcome {
                            ok: out.exit_code == 0,
                            output: format!(
                                "exit={}\nstdout:\n{}\nstderr:\n{}",
                                out.exit_code, out.stdout, out.stderr
                            ),
                            progress: ProgressSignals {
                                process_outcome_changed: novel,
                                new_information: novel,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            other => ToolOutcome {
                ok: false,
                output: format!("unknown tool: {other}"),
                progress: ProgressSignals::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C2 (claude concern 3, prospective pin): the full_auto gate approves
    /// `shell` on sandbox-backend availability ALONE, blind to arguments.
    /// That is only sound while the shell tool's schema carries no
    /// sandbox-relaxing argument surface (no escalation/unsandboxed-request
    /// analogue). This test pins the schema: ANY change to shell's argument
    /// surface must update this test AND re-audit the gate's
    /// argument-blindness (design §4 invariant — a relaxing argument must
    /// be inspected-and-rejected by the gate or ignored by the tool under
    /// ALL modes).
    #[test]
    fn shell_schema_has_no_sandbox_relaxing_arguments() {
        let defs = v1_tool_definitions();
        let shell = defs
            .iter()
            .find(|d| d.name == "shell")
            .expect("shell tool advertised");
        let properties = shell.input_schema["properties"]
            .as_object()
            .expect("shell properties");
        let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["command"],
            "shell gained an argument — re-audit the full_auto gate's argument-blindness"
        );
        assert_eq!(
            shell.input_schema["required"],
            serde_json::json!(["command"])
        );
    }

    /// C2 (claude concern 2): the full_auto gate extracts `path` from
    /// fs_write/fs_edit arguments for its containment check. If either tool
    /// renames that argument, full_auto silently degrades to default (every
    /// write prompts) — the per-mode matrix in nano-cli asserts a contained
    /// fs_edit auto-approves, and THIS pin catches the rename at the source.
    #[test]
    fn fs_write_and_fs_edit_take_a_path_argument() {
        let defs = v1_tool_definitions();
        for name in ["fs_write", "fs_edit"] {
            let def = defs.iter().find(|d| d.name == name).expect(name);
            assert!(
                def.input_schema["properties"].get("path").is_some(),
                "{name} lost its `path` argument"
            );
            assert!(
                def.input_schema["required"]
                    .as_array()
                    .expect("required")
                    .iter()
                    .any(|r| r == "path"),
                "{name} no longer requires `path`"
            );
        }
    }
}
