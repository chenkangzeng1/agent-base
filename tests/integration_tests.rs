use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_core::{
    AgentBuilder, AgentEvent, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, ChatMessage, LlmCapabilities, LlmClient, RiskLevel, StreamChunk, Tool,
    ToolContext, ToolControlFlow, ToolOutput, ToolPolicy,
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};

type ChunkStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

struct MockLlmClient {
    responses: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
    call_count: Mutex<usize>,
}

impl MockLlmClient {
    fn new(scripted_responses: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            responses: Mutex::new(scripted_responses.into_iter()),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
    ) -> AgentResult<ChunkStream> {
        *self.call_count.lock().unwrap() += 1;

        let chunks: Vec<AgentResult<StreamChunk>> = self
            .responses
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();

        let stream = futures_util::stream::iter(chunks);
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "echo",
                "description": "echo back the message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let msg = args["message"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: format!("echo: {msg}"),
            raw: Some(json!({ "echo": msg })),
            control_flow: ToolControlFlow::Continue,
        })
    }
}

// ---------------------------------------------------------------------------
// 测试用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_simple_text_reply() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("Hello, ".to_string()),
        StreamChunk::Text("world!".to_string()),
        StreamChunk::Stop,
    ]]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .system_prompt("You are a helpful assistant")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "Hi").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");

    let session = runtime.session(session_id).unwrap();
    let messages = session.chat_messages();
    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[0], ChatMessage::System { .. }));
    assert!(matches!(messages[1], ChatMessage::User { .. }));
    assert!(matches!(messages[2], ChatMessage::Assistant { .. }));

    assert_eq!(llm.call_count(), 1);
}

#[tokio::test]
async fn test_multiple_turns_with_tool() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\": \"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("Done!".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "Echo hello").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");

    assert_eq!(llm.call_count(), 2);
}

#[tokio::test]
async fn test_tool_not_found() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::ToolCall(json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_1",
                    "function": {
                        "name": "nonexistent_tool",
                        "arguments": "{}"
                    }
                }]
            }
        })),
        StreamChunk::Stop,
    ]]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .system_prompt("system prompt")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    assert!(result.is_ok(), "Tool not found should not crash: {result:?}");

    let events = result.unwrap();
    let has_tool_error = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallFinished { summary, .. } if summary.contains("not found"))
    });
    assert!(has_tool_error, "Should have tool not found in finished events");
}

#[tokio::test]
async fn test_approval_deny_stops_execution() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\": \"test\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("I cannot proceed without approval".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    struct DenyHandler;

    #[async_trait]
    impl ApprovalHandler for DenyHandler {
        async fn approve(&self, _request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
            Ok(ApprovalDecision::Deny)
        }
    }

    struct RequireApprovalPolicy;

    impl ToolPolicy for RequireApprovalPolicy {
        fn evaluate_approval(
            &self,
            _tool_name: &str,
            _args: &Value,
            _args_json: &str,
        ) -> Option<ApprovalRequest> {
            Some(ApprovalRequest {
                title: "Test".to_string(),
                message: "Require approval".to_string(),
                action_key: None,
                risk_level: RiskLevel::Sensitive,
                raw: None,
            })
        }

        fn on_pre_call(&self, _: &str, _: &Value, _: &ToolContext) {}
        fn on_post_call(&self, _: &str, _: &Value, _: &ToolOutput, _: &ToolContext) {}
    }

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .approval_handler(Arc::new(DenyHandler))
        .tool_policy(Arc::new(RequireApprovalPolicy))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    let events = result.expect("Approval denial should be handled gracefully");

    let has_awaiting_approval = events
        .iter()
        .any(|e| matches!(e, AgentEvent::AwaitingApproval { .. }));
    assert!(has_awaiting_approval, "Should emit AwaitingApproval event");

    let has_denial_finished = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallFinished { summary, .. } if summary.contains("审批拒绝"))
    });
    assert!(has_denial_finished, "Should emit ToolCallFinished with denial summary");

    assert_eq!(llm.call_count(), 2, "Should make 2 LLM calls (tool call then recovery)");
}

#[tokio::test]
async fn test_approval_allow_once_executes_tool() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\": \"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("done".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    struct AllowOnceHandler;

    #[async_trait]
    impl ApprovalHandler for AllowOnceHandler {
        async fn approve(&self, _request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
            Ok(ApprovalDecision::AllowOnce)
        }
    }

    struct RequireApprovalPolicy;

    impl ToolPolicy for RequireApprovalPolicy {
        fn evaluate_approval(
            &self,
            _tool_name: &str,
            _args: &Value,
            _args_json: &str,
        ) -> Option<ApprovalRequest> {
            Some(ApprovalRequest {
                title: "Test".to_string(),
                message: "Require approval".to_string(),
                action_key: None,
                risk_level: RiskLevel::Sensitive,
                raw: None,
            })
        }

        fn on_pre_call(&self, _: &str, _: &Value, _: &ToolContext) {}
        fn on_post_call(&self, _: &str, _: &Value, _: &ToolOutput, _: &ToolContext) {}
    }

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .approval_handler(Arc::new(AllowOnceHandler))
        .tool_policy(Arc::new(RequireApprovalPolicy))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");
    assert_eq!(llm.call_count(), 2);
}

#[tokio::test]
async fn test_empty_text_and_no_tool_call_continues() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![StreamChunk::Text(String::new()), StreamChunk::Stop],
        vec![
            StreamChunk::Text("final reply".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");
    assert_eq!(llm.call_count(), 2);
}

#[tokio::test]
async fn test_tool_parse_error_recovers() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::ToolCall(json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_1",
                    "function": {
                        "name": "echo",
                        "arguments": "invalid json {{{"
                    }
                }]
            }
        })),
        StreamChunk::Stop,
    ]]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    assert!(result.is_ok(), "Should recover from tool parse error: {result:?}");
}

#[tokio::test]
async fn test_event_collection() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        StreamChunk::Text("reply".to_string()),
        StreamChunk::Stop,
    ]]));

    let mut runtime = AgentBuilder::new(llm.clone()).build();

    let session_id = runtime.create_session();
    let events = runtime.run_turn_stream(session_id, "test").await.unwrap();

    let has_text_delta = events.iter().any(|e| matches!(e, AgentEvent::TextDelta { .. }));
    let has_run_completed =
        events.iter().any(|e| matches!(e, AgentEvent::RunCompleted { .. }));

    assert!(has_text_delta, "Should have TextDelta event");
    assert!(has_run_completed, "Should have RunCompleted event");
}
