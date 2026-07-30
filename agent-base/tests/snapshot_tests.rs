//! Snapshot tests for tool definitions, message serialization, and prompt structure.
//!
//! These catch unintended changes to LLM-visible output formats.

use agent_base::tool::{Tool, ToolContext, ToolOutput, ToolRegistry};
use agent_base::types::ChatMessage;
use async_trait::async_trait;
use serde_json::json;

/// A minimal test tool for snapshotting tool definitions.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> serde_json::Value {
        json!({
            "type": "function",
            "function": {
                "name": "echo",
                "description": "Echo back the input message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "The message to echo"
                        }
                    },
                    "required": ["message"]
                }
            }
        })
    }

    async fn call(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> agent_base::types::AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "echo".to_string(),
            ..Default::default()
        })
    }
}

#[test]
fn tool_definitions_snapshot() {
    let mut registry = ToolRegistry::default();
    registry.register(EchoTool);
    let definitions = registry.definitions();
    insta::assert_json_snapshot!(definitions);
}

#[test]
fn chat_message_system_snapshot() {
    let msg = ChatMessage::system("You are a helpful assistant.");
    let json = serde_json::to_value(&msg).unwrap();
    insta::assert_json_snapshot!(json);
}

#[test]
fn chat_message_user_snapshot() {
    let msg = ChatMessage::user("Hello, world!");
    let json = serde_json::to_value(&msg).unwrap();
    insta::assert_json_snapshot!(json);
}

#[test]
fn chat_message_assistant_with_tool_calls_snapshot() {
    let msg = ChatMessage::assistant_tool_call("call_1", "echo", r#"{"message":"hi"}"#);
    let json = serde_json::to_value(&msg).unwrap();
    insta::assert_json_snapshot!(json);
}

#[test]
fn chat_message_tool_result_snapshot() {
    let msg = ChatMessage::tool("call_1", "echo: hi");
    let json = serde_json::to_value(&msg).unwrap();
    insta::assert_json_snapshot!(json);
}
