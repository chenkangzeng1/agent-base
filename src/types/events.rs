use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::approval::ApprovalRequest;
use super::checkpoint::CheckpointData;
use super::plan::ExecutionPlan;
use super::session::SessionId;

// ---------------------------------------------------------------------------
// AgentEvent — framework-internal events (pub(crate))
// ---------------------------------------------------------------------------

/// Framework-internal events produced by the runtime kernel.
///
/// These are emitted on the internal broadcast channel and mapped to
/// [`RuntimeEvent`] before reaching external consumers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum AgentEvent {
    TextDelta {
        session_id: SessionId,
        text: String,
    },
    ThoughtDelta {
        session_id: SessionId,
        text: String,
    },
    ToolCallStarted {
        session_id: SessionId,
        tool_name: String,
        args_json: String,
    },
    ToolCallFinished {
        session_id: SessionId,
        tool_name: String,
        summary: String,
    },
    AwaitingApproval {
        session_id: SessionId,
        request: ApprovalRequest,
    },
    Checkpoint {
        session_id: SessionId,
        checkpoint: CheckpointData,
    },
    RunFinished {
        session_id: SessionId,
    },
    PlanGenerated {
        session_id: SessionId,
        plan: ExecutionPlan,
    },
    PlanStepStarted {
        session_id: SessionId,
        step_id: String,
        step_description: String,
        payload: Option<Value>,
    },
    PlanStepCompleted {
        session_id: SessionId,
        step_id: String,
        success: bool,
        result: Option<String>,
        payload: Option<Value>,
    },
    PlanCompleted {
        session_id: SessionId,
        plan_id: String,
        success: bool,
    },
    PlanGenerating {
        session_id: SessionId,
        plan_id: String,
    },
    PlanStepParsed {
        session_id: SessionId,
        plan_id: String,
        step_index: usize,
        step_id: String,
        step_description: String,
    },
    PlanFailed {
        session_id: SessionId,
        plan_id: String,
        error: String,
    },
    PlanStepWaitingConfirmation {
        session_id: SessionId,
        step_id: String,
        step_description: String,
        payload: Option<Value>,
    },
}

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
// RuntimeEvent — unified event stream for external consumers
// ---------------------------------------------------------------------------

/// Unified runtime event — the single event type exposed to external consumers
/// (frontends, CLIs, tests).
///
/// All framework-internal [`AgentEvent`]s and user-space [`UserEvent`]s are
/// mapped to this enum before delivery.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "runtimeEventType", rename_all = "camelCase")]
pub enum RuntimeEvent {
    // --- Framework system events (mirrored from AgentEvent) ---
    TextDelta { session_id: SessionId, text: String },
    ThoughtDelta { session_id: SessionId, text: String },
    ToolCallStarted { session_id: SessionId, tool_name: String, args_json: String },
    ToolCallFinished { session_id: SessionId, tool_name: String, summary: String },
    AwaitingApproval { session_id: SessionId, request: ApprovalRequest },
    Checkpoint { session_id: SessionId, checkpoint: CheckpointData },
    RunFinished { session_id: SessionId },
    PlanGenerating { session_id: SessionId, plan_id: String },
    PlanGenerated { session_id: SessionId, plan: ExecutionPlan },
    PlanStepParsed { session_id: SessionId, plan_id: String, step_index: usize, step_id: String, step_description: String },
    PlanStepStarted { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanStepCompleted { session_id: SessionId, step_id: String, success: bool, result: Option<String>, payload: Option<Value> },
    PlanStepWaitingConfirmation { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanCompleted { session_id: SessionId, plan_id: String, success: bool },
    PlanFailed { session_id: SessionId, plan_id: String, error: String },
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
            RuntimeEvent::PlanGenerating { session_id, .. } => session_id,
            RuntimeEvent::PlanGenerated { session_id, .. } => session_id,
            RuntimeEvent::PlanStepParsed { session_id, .. } => session_id,
            RuntimeEvent::PlanStepStarted { session_id, .. } => session_id,
            RuntimeEvent::PlanStepCompleted { session_id, .. } => session_id,
            RuntimeEvent::PlanStepWaitingConfirmation { session_id, .. } => session_id,
            RuntimeEvent::PlanCompleted { session_id, .. } => session_id,
            RuntimeEvent::PlanFailed { session_id, .. } => session_id,
            RuntimeEvent::UserEvent { session_id, .. } => session_id,
        }
    }
}

/// Convert an internal `AgentEvent` into a public `RuntimeEvent`.
impl From<AgentEvent> for RuntimeEvent {
    fn from(event: AgentEvent) -> Self {
        match event {
            AgentEvent::TextDelta { session_id, text } => RuntimeEvent::TextDelta { session_id, text },
            AgentEvent::ThoughtDelta { session_id, text } => RuntimeEvent::ThoughtDelta { session_id, text },
            AgentEvent::ToolCallStarted { session_id, tool_name, args_json } => RuntimeEvent::ToolCallStarted { session_id, tool_name, args_json },
            AgentEvent::ToolCallFinished { session_id, tool_name, summary } => RuntimeEvent::ToolCallFinished { session_id, tool_name, summary },
            AgentEvent::AwaitingApproval { session_id, request } => RuntimeEvent::AwaitingApproval { session_id, request },
            AgentEvent::Checkpoint { session_id, checkpoint } => RuntimeEvent::Checkpoint { session_id, checkpoint },
            AgentEvent::RunFinished { session_id } => RuntimeEvent::RunFinished { session_id },
            AgentEvent::PlanGenerated { session_id, plan } => RuntimeEvent::PlanGenerated { session_id, plan },
            AgentEvent::PlanStepStarted { session_id, step_id, step_description, payload } => RuntimeEvent::PlanStepStarted { session_id, step_id, step_description, payload },
            AgentEvent::PlanStepCompleted { session_id, step_id, success, result, payload } => RuntimeEvent::PlanStepCompleted { session_id, step_id, success, result, payload },
            AgentEvent::PlanCompleted { session_id, plan_id, success } => RuntimeEvent::PlanCompleted { session_id, plan_id, success },
            AgentEvent::PlanGenerating { session_id, plan_id } => RuntimeEvent::PlanGenerating { session_id, plan_id },
            AgentEvent::PlanStepParsed { session_id, plan_id, step_index, step_id, step_description } => RuntimeEvent::PlanStepParsed { session_id, plan_id, step_index, step_id, step_description },
            AgentEvent::PlanFailed { session_id, plan_id, error } => RuntimeEvent::PlanFailed { session_id, plan_id, error },
            AgentEvent::PlanStepWaitingConfirmation { session_id, step_id, step_description, payload } => RuntimeEvent::PlanStepWaitingConfirmation { session_id, step_id, step_description, payload },
        }
    }
}
