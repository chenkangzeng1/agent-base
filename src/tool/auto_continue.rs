use async_trait::async_trait;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput};
use crate::types::AgentResult;

/// Pure orchestration signal tool with zero domain dependency.
///
/// When the agent encounters an error in the previous tool execution and has
/// found a solution or retry strategy, or when it needs to execute multiple
/// long tasks in sequence, it can call this tool to request the system to
/// automatically continue to the next turn without waiting for user reply.
#[derive(Clone, Debug, Default)]
pub struct AutoContinueTool;

impl AutoContinueTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AutoContinueTool {
    fn name(&self) -> &'static str {
        "request_auto_continue"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "request_auto_continue",
                "description": "When you encounter an error in the previous tool execution and have found a solution or retry strategy, or when you need to execute multiple long tasks in sequence, call this tool to request the system to automatically continue to the next turn instead of stopping and waiting for user reply. This tool is only for making the request; you still need to describe your solution in the prompt.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "reason": {
                            "type": "string",
                            "description": "A brief reason for requesting auto-continue, e.g. 'network error, trying alternative source'"
                        }
                    },
                    "required": ["reason"]
                }
            }
        })
    }

    fn metadata(&self) -> crate::tool::ToolMetadata {
        crate::tool::ToolMetadata {
            name: self.name().to_string(),
            description:
                "Request automatic continuation to the next turn without waiting for user input."
                    .to_string(),
            origin: "agent-base".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            requirements: vec![],
        }
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("no reason provided");

        Ok(ToolOutput {
            summary: format!("Auto-continue request received. Reason: {}", reason),
            raw: Some(json!({
                "action": "auto_continue",
                "reason": reason,
            })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}
