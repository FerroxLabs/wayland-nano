//! Closed activation authority intersection. Unknown or widening requests are denied.

use crate::{ActivationCarrier, ActivationError, Capability, Control, RejectReason};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EffectiveCapability {
    #[serde(rename = "filesystem.read")]
    FilesystemRead,
    #[serde(rename = "filesystem.write")]
    FilesystemWrite,
    #[serde(rename = "shell.execute")]
    ShellExecute,
    #[serde(rename = "network.egress")]
    NetworkEgress,
    #[serde(rename = "mcp.invoke")]
    McpInvoke,
    #[serde(rename = "task.spawn")]
    TaskSpawn,
    #[serde(rename = "checkpoint.mutate")]
    CheckpointMutate,
    #[serde(rename = "computer.use")]
    ComputerUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveControl {
    Cancel,
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLimits {
    pub max_turns: u64,
    pub max_tool_calls: u64,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_microcents: u64,
    pub wall_clock_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyCeiling {
    pub capabilities: BTreeSet<EffectiveCapability>,
    pub controls: BTreeSet<EffectiveControl>,
    pub budgets: BudgetLimits,
    pub deadline_utc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    capabilities: BTreeSet<EffectiveCapability>,
    controls: BTreeSet<EffectiveControl>,
    budgets: BudgetLimits,
    deadline_utc: String,
    agent_scope: String,
    read_scope: String,
}

impl EffectivePolicy {
    pub fn capabilities(&self) -> &BTreeSet<EffectiveCapability> {
        &self.capabilities
    }

    pub fn budgets(&self) -> &BudgetLimits {
        &self.budgets
    }

    pub fn deadline_utc(&self) -> &str {
        &self.deadline_utc
    }

    pub fn agent_scope(&self) -> &str {
        &self.agent_scope
    }

    pub fn read_scope(&self) -> &str {
        &self.read_scope
    }
}

pub(crate) fn intersect(
    request: &ActivationCarrier,
    ceiling: &PolicyCeiling,
) -> Result<EffectivePolicy, ActivationError> {
    let requested_capabilities: BTreeSet<_> = request
        .capabilities
        .iter()
        .copied()
        .map(map_capability)
        .collect();
    let requested_controls: BTreeSet<_> =
        request.controls.iter().copied().map(map_control).collect();
    if !requested_capabilities.is_subset(&ceiling.capabilities)
        || !requested_controls.is_subset(&ceiling.controls)
    {
        return Err(ActivationError::new(RejectReason::AuthorityWidening));
    }
    let requested = BudgetLimits {
        max_turns: request.budgets.max_turns,
        max_tool_calls: request.budgets.max_tool_calls,
        max_input_tokens: request.budgets.max_input_tokens,
        max_output_tokens: request.budgets.max_output_tokens,
        max_cost_microcents: request.budgets.max_cost_microcents,
        wall_clock_ms: request.budgets.wall_clock_ms,
    };
    Ok(EffectivePolicy {
        capabilities: requested_capabilities,
        controls: requested_controls,
        budgets: BudgetLimits {
            max_turns: requested.max_turns.min(ceiling.budgets.max_turns),
            max_tool_calls: requested.max_tool_calls.min(ceiling.budgets.max_tool_calls),
            max_input_tokens: requested
                .max_input_tokens
                .min(ceiling.budgets.max_input_tokens),
            max_output_tokens: requested
                .max_output_tokens
                .min(ceiling.budgets.max_output_tokens),
            max_cost_microcents: requested
                .max_cost_microcents
                .min(ceiling.budgets.max_cost_microcents),
            wall_clock_ms: requested.wall_clock_ms.min(ceiling.budgets.wall_clock_ms),
        },
        deadline_utc: if request.deadline <= ceiling.deadline_utc {
            request.deadline.clone()
        } else {
            ceiling.deadline_utc.clone()
        },
        agent_scope: "own".into(),
        read_scope: "session_and_project".into(),
    })
}

fn map_capability(value: Capability) -> EffectiveCapability {
    match value {
        Capability::FilesystemRead => EffectiveCapability::FilesystemRead,
        Capability::FilesystemWrite => EffectiveCapability::FilesystemWrite,
        Capability::ShellExecute => EffectiveCapability::ShellExecute,
        Capability::NetworkEgress => EffectiveCapability::NetworkEgress,
        Capability::McpInvoke => EffectiveCapability::McpInvoke,
        Capability::TaskSpawn => EffectiveCapability::TaskSpawn,
        Capability::CheckpointMutate => EffectiveCapability::CheckpointMutate,
        Capability::ComputerUse => EffectiveCapability::ComputerUse,
    }
}

fn map_control(value: Control) -> EffectiveControl {
    match value {
        Control::Cancel => EffectiveControl::Cancel,
        Control::Pause => EffectiveControl::Pause,
    }
}
