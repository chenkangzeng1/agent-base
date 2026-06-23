use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::approval::ApprovalRequest;
use super::checkpoint::CheckpointData;
use super::plan::ExecutionPlan;
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
    // --- Framework system events (mirrored from AgentEvent) ---
    TextDelta { session_id: SessionId, text: String },
    ThoughtDelta { session_id: SessionId, text: String },
    ToolCallStarted { session_id: SessionId, tool_name: String, args_json: String },
    ToolCallFinished { session_id: SessionId, tool_name: String, summary: String },
    AwaitingApproval { session_id: SessionId, request: ApprovalRequest },
    Checkpoint { session_id: SessionId, checkpoint: CheckpointData },
    RunFinished { session_id: SessionId },
    RunCancelled { session_id: SessionId },
    PlanGenerating { session_id: SessionId, plan_id: String },
    PlanGenerated { session_id: SessionId, plan: ExecutionPlan },
    PlanStepParsed { session_id: SessionId, plan_id: String, step_index: usize, step_id: String, step_description: String },
    PlanStepStarted { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanStepCompleted { session_id: SessionId, step_id: String, success: bool, result: Option<String>, payload: Option<Value> },
    PlanStepWaitingConfirmation { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanCompleted { session_id: SessionId, plan_id: String, success: bool },
    PlanFailed { session_id: SessionId, plan_id: String, error: String },
    // --- Adaptive recovery events ---
    StepRetry { session_id: SessionId, step_id: String, retry_count: usize, backoff_ms: u64 },
    StepAlternativeTrying { session_id: SessionId, original_step_id: String, alternative_step_id: String, alternative_count: usize },
    PlanReplanning { session_id: SessionId, plan_id: String, replan_count: usize },
    PlanReplanned { session_id: SessionId, plan_id: String, new_steps: usize },
    PlanRecoveryExhausted { session_id: SessionId, step_id: String, retries: usize, alternatives: usize, replans: usize },
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
            RuntimeEvent::PlanGenerating { session_id, .. } => session_id,
            RuntimeEvent::PlanGenerated { session_id, .. } => session_id,
            RuntimeEvent::PlanStepParsed { session_id, .. } => session_id,
            RuntimeEvent::PlanStepStarted { session_id, .. } => session_id,
            RuntimeEvent::PlanStepCompleted { session_id, .. } => session_id,
            RuntimeEvent::PlanStepWaitingConfirmation { session_id, .. } => session_id,
            RuntimeEvent::PlanCompleted { session_id, .. } => session_id,
            RuntimeEvent::PlanFailed { session_id, .. } => session_id,
            RuntimeEvent::StepRetry { session_id, .. } => session_id,
            RuntimeEvent::StepAlternativeTrying { session_id, .. } => session_id,
            RuntimeEvent::PlanReplanning { session_id, .. } => session_id,
            RuntimeEvent::PlanReplanned { session_id, .. } => session_id,
            RuntimeEvent::PlanRecoveryExhausted { session_id, .. } => session_id,
            RuntimeEvent::UserEvent { session_id, .. } => session_id,
        }
    }

    /// Extract this event as a [`PlanEvent`] if it is plan-related.
    /// Returns `None` for non-plan events (TextDelta, ToolCallStarted, etc.).
    pub fn as_plan_event(&self) -> Option<PlanEvent> {
        PlanEvent::try_from_event(self)
    }
}

// ---------------------------------------------------------------------------
// PlanEvent — plan lifecycle aggregate enum
// ---------------------------------------------------------------------------

/// Aggregate enum grouping all plan-related [`RuntimeEvent`] variants.
///
/// Consumers can match exhaustively on `PlanEvent` instead of filtering
/// through the full `RuntimeEvent` enum.  If a new plan event variant is
/// added to `RuntimeEvent`, the consumer gets a compile error instead of
/// silently ignoring it through a wildcard arm.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "planEventType", rename_all = "camelCase")]
pub enum PlanEvent {
    PlanGenerating { session_id: SessionId, plan_id: String },
    PlanGenerated { session_id: SessionId, plan: ExecutionPlan },
    PlanStepParsed { session_id: SessionId, plan_id: String, step_index: usize, step_id: String, step_description: String },
    PlanStepStarted { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanStepCompleted { session_id: SessionId, step_id: String, success: bool, result: Option<String>, payload: Option<Value> },
    PlanStepWaitingConfirmation { session_id: SessionId, step_id: String, step_description: String, payload: Option<Value> },
    PlanCompleted { session_id: SessionId, plan_id: String, success: bool },
    PlanFailed { session_id: SessionId, plan_id: String, error: String },
    // --- Adaptive recovery events ---
    StepRetry { session_id: SessionId, step_id: String, retry_count: usize, backoff_ms: u64 },
    StepAlternativeTrying { session_id: SessionId, original_step_id: String, alternative_step_id: String, alternative_count: usize },
    PlanReplanning { session_id: SessionId, plan_id: String, replan_count: usize },
    PlanReplanned { session_id: SessionId, plan_id: String, new_steps: usize },
    PlanRecoveryExhausted { session_id: SessionId, step_id: String, retries: usize, alternatives: usize, replans: usize },
}

impl PlanEvent {
    /// Try to convert a [`RuntimeEvent`] reference into a [`PlanEvent`].
    /// Returns `None` for non-plan events.
    pub fn try_from_event(event: &RuntimeEvent) -> Option<Self> {
        match event {
            RuntimeEvent::PlanGenerating { session_id, plan_id } => {
                Some(PlanEvent::PlanGenerating { session_id: session_id.clone(), plan_id: plan_id.clone() })
            }
            RuntimeEvent::PlanGenerated { session_id, plan } => {
                Some(PlanEvent::PlanGenerated { session_id: session_id.clone(), plan: plan.clone() })
            }
            RuntimeEvent::PlanStepParsed { session_id, plan_id, step_index, step_id, step_description } => {
                Some(PlanEvent::PlanStepParsed { session_id: session_id.clone(), plan_id: plan_id.clone(), step_index: *step_index, step_id: step_id.clone(), step_description: step_description.clone() })
            }
            RuntimeEvent::PlanStepStarted { session_id, step_id, step_description, payload } => {
                Some(PlanEvent::PlanStepStarted { session_id: session_id.clone(), step_id: step_id.clone(), step_description: step_description.clone(), payload: payload.clone() })
            }
            RuntimeEvent::PlanStepCompleted { session_id, step_id, success, result, payload } => {
                Some(PlanEvent::PlanStepCompleted { session_id: session_id.clone(), step_id: step_id.clone(), success: *success, result: result.clone(), payload: payload.clone() })
            }
            RuntimeEvent::PlanStepWaitingConfirmation { session_id, step_id, step_description, payload } => {
                Some(PlanEvent::PlanStepWaitingConfirmation { session_id: session_id.clone(), step_id: step_id.clone(), step_description: step_description.clone(), payload: payload.clone() })
            }
            RuntimeEvent::PlanCompleted { session_id, plan_id, success } => {
                Some(PlanEvent::PlanCompleted { session_id: session_id.clone(), plan_id: plan_id.clone(), success: *success })
            }
            RuntimeEvent::PlanFailed { session_id, plan_id, error } => {
                Some(PlanEvent::PlanFailed { session_id: session_id.clone(), plan_id: plan_id.clone(), error: error.clone() })
            }
            RuntimeEvent::StepRetry { session_id, step_id, retry_count, backoff_ms } => {
                Some(PlanEvent::StepRetry { session_id: session_id.clone(), step_id: step_id.clone(), retry_count: *retry_count, backoff_ms: *backoff_ms })
            }
            RuntimeEvent::StepAlternativeTrying { session_id, original_step_id, alternative_step_id, alternative_count } => {
                Some(PlanEvent::StepAlternativeTrying { session_id: session_id.clone(), original_step_id: original_step_id.clone(), alternative_step_id: alternative_step_id.clone(), alternative_count: *alternative_count })
            }
            RuntimeEvent::PlanReplanning { session_id, plan_id, replan_count } => {
                Some(PlanEvent::PlanReplanning { session_id: session_id.clone(), plan_id: plan_id.clone(), replan_count: *replan_count })
            }
            RuntimeEvent::PlanReplanned { session_id, plan_id, new_steps } => {
                Some(PlanEvent::PlanReplanned { session_id: session_id.clone(), plan_id: plan_id.clone(), new_steps: *new_steps })
            }
            RuntimeEvent::PlanRecoveryExhausted { session_id, step_id, retries, alternatives, replans } => {
                Some(PlanEvent::PlanRecoveryExhausted { session_id: session_id.clone(), step_id: step_id.clone(), retries: *retries, alternatives: *alternatives, replans: *replans })
            }
            // Non-plan events
            RuntimeEvent::TextDelta { .. }
            | RuntimeEvent::ThoughtDelta { .. }
            | RuntimeEvent::ToolCallStarted { .. }
            | RuntimeEvent::ToolCallFinished { .. }
            | RuntimeEvent::AwaitingApproval { .. }
            | RuntimeEvent::Checkpoint { .. }
            | RuntimeEvent::RunFinished { .. }
            | RuntimeEvent::RunCancelled { .. }
            | RuntimeEvent::UserEvent { .. } => None,
        }
    }
}
