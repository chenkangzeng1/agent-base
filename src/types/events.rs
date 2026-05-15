use serde_json::Value;

use super::approval::ApprovalRequest;
use super::checkpoint::CheckpointData;
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
    RunCompleted {
        session_id: SessionId,
    },
    RunFailed {
        session_id: SessionId,
        error: String,
    },
    Custom {
        session_id: SessionId,
        payload: Value,
    },
}
