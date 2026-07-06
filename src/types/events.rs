use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::approval::ApprovalRequest;
use super::checkpoint::CheckpointData;
use super::plan_update::PlanItem;
use super::session::SessionId;

// ---------------------------------------------------------------------------
// UserEvent — user-space events produced by tools
// ---------------------------------------------------------------------------

/// User-space events produced by tools during execution.
///
/// Tools send these through `ToolContext::emit_user_event()`. The framework
/// wraps them in [`RuntimeEvent::UserEvent`] before delivering to external
/// consumers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "userEventType", rename_all = "camelCase")]
pub enum UserEvent {
    /// Tool progress notification.
    Progress { text: String },
    /// Sub-agent event forwarding (used by `SubAgentTool`).
    SubAgentEvent { subagent: String, event: Box<RuntimeEvent> },
    /// User-defined structured event for custom business semantics.
    Structured { event_type: String, data: Value },
}

// ---------------------------------------------------------------------------
// RuntimeEvent — unified event stream for all consumers
// ---------------------------------------------------------------------------

/// Unified runtime event — the single event type for both internal and external
/// consumers (frontends, CLIs, tests).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "runtimeEventType", rename_all = "camelCase")]
pub enum RuntimeEvent {
    // --- Framework system events ---
    TextDelta { session_id: SessionId, text: String },
    ThoughtDelta { session_id: SessionId, text: String },
    ToolCallStarted { session_id: SessionId, tool_name: String, args_json: String },
    ToolCallFinished { session_id: SessionId, tool_name: String, summary: String },
    AwaitingApproval { session_id: SessionId, request: ApprovalRequest },
    Checkpoint { session_id: SessionId, checkpoint: CheckpointData },
    RunFinished { session_id: SessionId },
    RunCancelled { session_id: SessionId },
    // --- Lightweight plan update (display-only, no execution semantics) ---
    PlanUpdated { session_id: SessionId, objective: String, explanation: Option<String>, plan: Vec<PlanItem> },
    // --- User-space events ---
    /// A user-space event produced by a tool.
    UserEvent { session_id: SessionId, event: UserEvent },
}

impl RuntimeEvent {
    /// Get the session ID associated with this event.
    pub fn session_id(&self) -> &SessionId {
        match self {
            RuntimeEvent::TextDelta { session_id, .. } => session_id,
            RuntimeEvent::ThoughtDelta { session_id, .. } => session_id,
            RuntimeEvent::ToolCallStarted { session_id, .. } => session_id,
            RuntimeEvent::ToolCallFinished { session_id, .. } => session_id,
            RuntimeEvent::AwaitingApproval { session_id, .. } => session_id,
            RuntimeEvent::Checkpoint { session_id, .. } => session_id,
            RuntimeEvent::RunFinished { session_id, .. } => session_id,
            RuntimeEvent::RunCancelled { session_id, .. } => session_id,
            RuntimeEvent::PlanUpdated { session_id, .. } => session_id,
            RuntimeEvent::UserEvent { session_id, .. } => session_id,
        }
    }
}
