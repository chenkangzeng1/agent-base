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
    SubAgentEvent {
        subagent: String,
        event: Box<RuntimeEvent>,
    },
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
    TextDelta {
        session_id: SessionId,
        text: String,
        /// Identifies the agent that produced this event (root / sub-agent path).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        /// Distributed tracing context carried across systems (e.g. MCP caller → phi-agent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    ThoughtDelta {
        session_id: SessionId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    ToolCallStarted {
        session_id: SessionId,
        tool_name: String,
        args_json: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    ToolCallFinished {
        session_id: SessionId,
        tool_name: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    AwaitingApproval {
        session_id: SessionId,
        request: ApprovalRequest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    Checkpoint {
        session_id: SessionId,
        checkpoint: CheckpointData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    RunFinished {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    RunCancelled {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    // --- Lightweight plan update (display-only, no execution semantics) ---
    PlanUpdated {
        session_id: SessionId,
        objective: String,
        explanation: Option<String>,
        plan: Vec<PlanItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    // --- User-space events ---
    /// A user-space event produced by a tool.
    UserEvent {
        session_id: SessionId,
        event: UserEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
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

    /// Get the agent ID associated with this event, if any.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            RuntimeEvent::TextDelta { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::ThoughtDelta { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::ToolCallStarted { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::ToolCallFinished { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::AwaitingApproval { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::Checkpoint { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::RunFinished { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::RunCancelled { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::PlanUpdated { agent_id, .. } => agent_id.as_deref(),
            RuntimeEvent::UserEvent { agent_id, .. } => agent_id.as_deref(),
        }
    }

    /// Get the trace ID associated with this event, if any.
    pub fn trace_id(&self) -> Option<&str> {
        match self {
            RuntimeEvent::TextDelta { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::ThoughtDelta { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::ToolCallStarted { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::ToolCallFinished { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::AwaitingApproval { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::Checkpoint { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::RunFinished { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::RunCancelled { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::PlanUpdated { trace_id, .. } => trace_id.as_deref(),
            RuntimeEvent::UserEvent { trace_id, .. } => trace_id.as_deref(),
        }
    }
}
