//! Compaction equivalence.
//!
//! "Actionably equivalent" (shared G2 definition): replay after compaction
//! reconstructs the same pending user instructions, approval/execution state,
//! unresolved tool calls, changed-file inventory, and next legal transition.
//! Transcript wording may differ; executable decisions may not.

use crate::op::Op;
use crate::op::OpEnvelope;
use crate::replay::SessionState;

/// Returns whether two replayed states agree on every executable surface.
/// (Timestamps, op ids, and summary text are deliberately not compared.)
pub fn actionably_equivalent(left: &SessionState, right: &SessionState) -> bool {
    left.session_id == right.session_id
        && left.cwd == right.cwd
        && left.open_turn == right.open_turn
        && left.turn_interrupted == right.turn_interrupted
        && left.open_tool_calls == right.open_tool_calls
        && left.changed_files == right.changed_files
        && next_legal_transition(left) == next_legal_transition(right)
}

/// The single next legal transition out of a replayed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextTransition {
    /// No open turn: a new user instruction may begin a turn.
    AcceptUserInstruction,
    /// An interrupted turn must be resumed or explicitly abandoned first.
    ResolveInterruptedTurn,
    /// Tool calls are awaiting results; the turn continues.
    ContinueTurn,
}

pub fn next_legal_transition(state: &SessionState) -> NextTransition {
    if state.open_turn.is_some() && state.turn_interrupted {
        NextTransition::ResolveInterruptedTurn
    } else if state.open_turn.is_some() || !state.open_tool_calls.is_empty() {
        NextTransition::ContinueTurn
    } else {
        NextTransition::AcceptUserInstruction
    }
}

/// Builds the replay input for a compacted journal: the compaction ops plus
/// every envelope not covered by the latest completed compaction.
pub fn compacted_prefix<'a>(envelopes: &'a [OpEnvelope]) -> Vec<&'a OpEnvelope> {
    let covered: std::collections::HashSet<&str> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::CompactionComplete { covers_op_ids, .. } => Some(covers_op_ids),
            _ => None,
        })
        .flat_map(|ids| ids.iter().map(String::as_str))
        .collect();
    envelopes
        .iter()
        .filter(|envelope| !covered.contains(envelope.id.as_str()))
        .collect()
}
