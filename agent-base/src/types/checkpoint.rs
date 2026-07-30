use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message::ChatMessage;
use super::session::SessionId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointData {
    pub session_id: SessionId,
    pub user_input: String,
    pub step: CheckpointStep,
    pub turn_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckpointStep {
    AfterUserInput,
    BeforeLlm {
        messages: Vec<ChatMessage>,
        tools: Vec<Value>,
    },
    BeforeToolCalls {
        tool_calls: Vec<(String, String, String)>,
    },
    AfterToolCalls {
        tool_calls: Vec<(String, String, String)>,
        results: Vec<ToolResultData>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultData {
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
}
