//! Production wiring: Flux-backed ModelDriver and tool-backed ToolExecutor.

use crate::loop_protection::ProgressSignals;
use crate::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall, ToolDefinition};
use nano_tools::fs::{FsTools, ReadBounds};
use nano_tools::shell::{ShellKind, ShellTool};

/// ModelDriver over the real Flux Completions client.
#[derive(Debug)]
pub struct FluxDriver {
    client: FluxCompletionsClient,
    api_key: String,
}

impl FluxDriver {
    pub fn new(client: FluxCompletionsClient, api_key: impl Into<String>) -> Self {
        Self {
            client,
            api_key: api_key.into(),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for FluxDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.client.complete(request, &self.api_key).await
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
            description: "Run a cmd.exe command inside the workspace sandbox. Args: command."
                .into(),
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
