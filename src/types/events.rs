use serde_json::Value;

use super::approval::ApprovalRequest;
use super::checkpoint::CheckpointData;
use super::plan::ExecutionPlan;
use super::session::SessionId;

#[derive(Clone, Debug)]
pub enum AgentEvent {
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
    Custom {
        session_id: SessionId,
        payload: Value,
    },
    PlanGenerated {
        session_id: SessionId,
        plan: ExecutionPlan,
    },
    PlanStepStarted {
        session_id: SessionId,
        step_id: String,
        step_description: String,
    },
    PlanStepCompleted {
        session_id: SessionId,
        step_id: String,
        success: bool,
        result: Option<String>,
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
}
