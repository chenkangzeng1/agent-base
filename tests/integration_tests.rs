use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AbortOnFailure, AgentBuilder, AgentError, AgentEvent, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, ChatMessage, ExecutionPlan, InMemoryPlanStore, LlmCapabilities, LlmClient,
    PlanGenerator, PlanStatus, PlanStep, PlanStore, RecoveryAction, ResponseFormat, RetryOnError,
    RiskLevel, RunOutcome, StepExecutor, StepResult, StepStatus, StreamChunk, Tool,
    ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, AlwaysContinue, StepContinuePolicy,
    RecoveryStrategy,
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
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
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
            truncated: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Test suites
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
    let result = runtime.run_turn_stream(session_id.clone(), "Hi").await;
    assert!(result.is_ok(), "Expected ok, got: {result:?}");
    let (_events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);

    let session = runtime.session(&session_id).unwrap();
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

    let (events, _outcome) = result.unwrap();
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
        .error_recovery(Arc::new(agent_base::RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "test").await;
    let (events, _outcome) = result.expect("Approval denial should be handled gracefully");

    let has_awaiting_approval = events
        .iter()
        .any(|e| matches!(e, AgentEvent::AwaitingApproval { .. }));
    assert!(has_awaiting_approval, "Should emit AwaitingApproval event");

    let has_denial_finished = events.iter().any(|e| {
        matches!(e, AgentEvent::ToolCallFinished { summary, .. } if summary.contains("rejected by approval"))
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
    let (events, _outcome) = runtime.run_turn_stream(session_id, "test").await.unwrap();

    let has_text_delta = events.iter().any(|e| matches!(e, AgentEvent::TextDelta { .. }));
    let has_run_finished =
        events.iter().any(|e| matches!(e, AgentEvent::RunFinished { .. }));

    assert!(has_text_delta, "Should have TextDelta event");
    assert!(has_run_finished, "Should have RunFinished event");
}

// ---------------------------------------------------------------------------
// 6.2 multi-modal message tests
// ---------------------------------------------------------------------------

#[test]
fn test_chat_message_user_with_images() {
    use agent_base::{ChatMessage, ImageAttachment, ImageDetail};

    let msg = ChatMessage::user("hello");
    match &msg {
        ChatMessage::User { images, .. } => {
            assert!(images.is_empty());
        }
        _ => panic!("expected User variant"),
    }

    let images = vec![
        ImageAttachment::Url {
            url: "https://example.com/img.jpg".to_string(),
            detail: Some(ImageDetail::High),
        },
        ImageAttachment::Base64 {
            data: "abc123".to_string(),
            media_type: Some("image/png".to_string()),
            detail: None,
        },
    ];
    let msg_with_images = ChatMessage::user_with_images("describe this", images);
    match &msg_with_images {
        ChatMessage::User { content, images } => {
            assert_eq!(content, "describe this");
            assert_eq!(images.len(), 2);
            assert!(matches!(images[0], ImageAttachment::Url { .. }));
            assert!(matches!(images[1], ImageAttachment::Base64 { .. }));
        }
        _ => panic!("expected User variant"),
    }
}

#[test]
fn test_image_attachment_serialization() {
    use agent_base::ImageAttachment;
    use serde_json;

    let img = ImageAttachment::Url {
        url: "https://example.com/img.jpg".to_string(),
        detail: None,
    };
    let json_str = serde_json::to_string(&img).unwrap();
    let parsed: ImageAttachment = serde_json::from_str(&json_str).unwrap();
    match parsed {
        ImageAttachment::Url { url, .. } => {
            assert_eq!(url, "https://example.com/img.jpg");
        }
        _ => panic!("expected Url variant"),
    }

    let img_base64 = ImageAttachment::Base64 {
        data: "abc123".to_string(),
        media_type: Some("image/jpeg".to_string()),
        detail: None,
    };
    let json_str = serde_json::to_string(&img_base64).unwrap();
    let parsed: ImageAttachment = serde_json::from_str(&json_str).unwrap();
    match parsed {
        ImageAttachment::Base64 { data, .. } => {
            assert_eq!(data, "abc123");
        }
        _ => panic!("expected Base64 variant"),
    }
}

#[test]
fn test_session_push_user_with_images() {
    use agent_base::types::SessionId;
    use agent_base::{AgentSession, ChatMessage, ImageAttachment, MessageRole};

    let session_id = SessionId {
        id: 1,
        external_id: None,
    };
    let mut session = AgentSession::new(session_id);

    let images = vec![ImageAttachment::Url {
        url: "https://example.com/img.jpg".to_string(),
        detail: None,
    }];
    session.push_user_message_with_images("describe this image", images);

    let chat_msgs = session.chat_messages();
    assert_eq!(chat_msgs.len(), 1);
    match &chat_msgs[0] {
        ChatMessage::User { content, images } => {
            assert_eq!(content, "describe this image");
            assert_eq!(images.len(), 1);
        }
        _ => panic!("expected User variant"),
    }

    let msgs = session.messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, MessageRole::User);
    assert_eq!(msgs[0].content, "describe this image");
}

// ---------------------------------------------------------------------------
// 6.4 Checkpoint / Resume tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_checkpoint_events_emitted() {
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

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();
    let (events, _outcome) = runtime.run_turn_stream(session_id, "test checkpoint").await.unwrap();

    let checkpoint_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Checkpoint { .. }))
        .count();
    assert!(
        checkpoint_count >= 2,
        "Should have at least AfterUserInput and BeforeLlm checkpoints, got {checkpoint_count}"
    );

    let has_after_user_input = events.iter().any(|e| {
        matches!(e, AgentEvent::Checkpoint { checkpoint, .. } if matches!(checkpoint.step, agent_base::CheckpointStep::AfterUserInput))
    });
    assert!(has_after_user_input, "Should have AfterUserInput checkpoint");

    let has_before_llm = events.iter().any(|e| {
        matches!(e, AgentEvent::Checkpoint { checkpoint, .. } if matches!(checkpoint.step, agent_base::CheckpointStep::BeforeLlm { .. }))
    });
    assert!(has_before_llm, "Should have BeforeLlm checkpoint");

    let has_before_tool_calls = events.iter().any(|e| {
        matches!(e, AgentEvent::Checkpoint { checkpoint, .. } if matches!(checkpoint.step, agent_base::CheckpointStep::BeforeToolCalls { .. }))
    });
    assert!(has_before_tool_calls, "Should have BeforeToolCalls checkpoint");
}

#[tokio::test]
async fn test_resume_from_after_user_input_checkpoint() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::Text("resumed reply".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();

    let mut checkpoint_opt: Option<agent_base::CheckpointData> = None;
    let _ = runtime
        .run_turn_with_handler(session_id.clone(), "resume test", |event| {
            if let AgentEvent::Checkpoint { checkpoint, .. } = &event {
                if matches!(checkpoint.step, agent_base::CheckpointStep::AfterUserInput) {
                    checkpoint_opt = Some(checkpoint.clone());
                    return Err(agent_base::AgentError::Cancelled);
                }
            }
            Ok(())
        })
        .await;

    let checkpoint = checkpoint_opt.expect("Should have captured AfterUserInput checkpoint");

    let result = runtime.resume_from_checkpoint(checkpoint, |_| Ok(())).await;
    assert!(result.is_ok(), "Resume should succeed: {result:?}");

    let session = runtime.session(&session_id).unwrap();
    let chat_msgs = session.chat_messages();
    let has_assistant_reply = chat_msgs
        .iter()
        .any(|m| matches!(m, ChatMessage::Assistant { content, .. } if content.as_deref() == Some("resumed reply")));
    assert!(has_assistant_reply, "Should have resumed reply in session");
}

#[tokio::test]
async fn test_resume_from_before_tool_calls_checkpoint() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"message\":\"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("tool results processed".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();

    let mut checkpoint_opt: Option<agent_base::CheckpointData> = None;
    let _ = runtime
        .run_turn_with_handler(session_id.clone(), "echo hello", |event| {
            if let AgentEvent::Checkpoint { checkpoint, .. } = &event {
                if matches!(checkpoint.step, agent_base::CheckpointStep::BeforeToolCalls { .. }) {
                    checkpoint_opt = Some(checkpoint.clone());
                    return Err(agent_base::AgentError::Cancelled);
                }
            }
            Ok(())
        })
        .await;

    let checkpoint =
        checkpoint_opt.expect("Should have captured BeforeToolCalls checkpoint");

    let result = runtime.resume_from_checkpoint(checkpoint, |_| Ok(())).await;
    assert!(result.is_ok(), "Resume from BeforeToolCalls should succeed: {result:?}");

    let session = runtime.session(&session_id).unwrap();
    let chat_msgs = session.chat_messages();
    let has_tool_result = chat_msgs.iter().any(|m| {
        matches!(m, ChatMessage::Tool { content, .. } if content.contains("echo: hello"))
    });
    assert!(has_tool_result, "Should have echo tool result in session");
}

// ---------------------------------------------------------------------------
// 6.3 sub-agent tool tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sub_agent_tool() {
    use agent_base::SubAgentTool;

    let sub_llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::Text("sub-agent processed: ".to_string()),
            StreamChunk::Text("task completed".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let sub_runtime = AgentBuilder::new(sub_llm.clone())
        .system_prompt("you are a sub-agent")
        .build();

    let sub_agent_tool = SubAgentTool::new(
        "delegate_task",
        "delegate a task to a sub-agent",
        sub_runtime,
    );

    let parent_llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "delegate_task",
                            "arguments": "{\"task\": \"analyze the data\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("parent final reply".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut parent_runtime = AgentBuilder::new(parent_llm.clone())
        .register_tool(sub_agent_tool)
        .system_prompt("you are the main agent")
        .build();

    let session_id = parent_runtime.create_session();
    let result = parent_runtime
        .run_turn_stream(session_id.clone(), "delegate this task")
        .await;
    assert!(result.is_ok(), "Sub-agent delegation should succeed: {result:?}");
    assert_eq!(parent_llm.call_count(), 2, "Parent should make 2 LLM calls");

    let session = parent_runtime.session(&session_id).unwrap();
    let chat_msgs = session.chat_messages();
    let has_parent_final = chat_msgs.iter().any(|m| {
        matches!(m, ChatMessage::Assistant { content, .. } if content.as_deref() == Some("parent final reply"))
    });
    assert!(has_parent_final, "Should have parent final reply");
}

// =========================================================================
// 7. handle_tool_error 全面测试
// =========================================================================

fn tool_call_chunk(id: &str, name: &str, args: Value) -> StreamChunk {
    StreamChunk::ToolCall(serde_json::json!({
        "delta": {
            "tool_calls": [{
                "id": id,
                "function": {
                    "name": name,
                    "arguments": args.to_string()
                }
            }]
        }
    }))
}

struct FailingTool {
    name: &'static str,
    error: String,
    call_count: Mutex<usize>,
}

impl FailingTool {
    fn new(name: &'static str, error: impl Into<String>) -> Self {
        Self { name, error: error.into(), call_count: Mutex::new(0) }
    }
}

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": "a tool that always fails",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    }
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        *self.call_count.lock().unwrap() += 1;
        Err(AgentError::internal(&self.error))
    }
}

fn assert_valid_message_history(messages: &[ChatMessage]) {
    for (i, msg) in messages.iter().enumerate() {
        if let ChatMessage::Assistant { tool_calls: Some(tc), .. } = msg {
            assert!(!tc.is_empty(), "message[{i}]: Assistant tool_calls must not be empty");
            let mut found = std::collections::HashSet::new();
            for j in i + 1..messages.len() {
                if let ChatMessage::Tool { tool_call_id, .. } = &messages[j] {
                    found.insert(tool_call_id.clone());
                } else if matches!(&messages[j], ChatMessage::User { .. }) {
                    break;
                } else if matches!(&messages[j], ChatMessage::Assistant { .. }) {
                    break;
                }
            }
            for tc_msg in tc {
                assert!(
                    found.contains(&tc_msg.id),
                    "message[{i}]: Assistant tool_calls references id={} but no Tool result follows before next User/Assistant",
                    tc_msg.id
                );
            }
        }
    }
}

#[tokio::test]
async fn tool_execution_error_retry_llm_receives_error_and_recovers() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "failing", json!({"input": "test"})),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("I saw the error and will try a different approach.".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let failing_tool = FailingTool::new("failing", "connection refused: ssh timeout");

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(failing_tool)
        .system_prompt("sys")
        .error_recovery(Arc::new(RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "do something").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);

    let retry_user_msg = messages.iter().any(|m| {
        matches!(m, ChatMessage::User { content, .. }
            if content.contains("failing") && content.contains("connection refused"))
    });
    assert!(retry_user_msg, "LLM should receive error details in retry message");

    let recovery_msg = messages.iter().any(|m| {
        matches!(m, ChatMessage::Assistant { content, .. }
            if content.as_deref() == Some("I saw the error and will try a different approach."))
    });
    assert!(recovery_msg, "LLM should respond after seeing error");

    assert_eq!(llm.call_count(), 2, "Should make 2 LLM calls (tool call then recovery)");

    let has_run_finished = events.iter().any(|e| matches!(e, AgentEvent::RunFinished { .. }));
    assert!(has_run_finished, "Should emit RunFinished on completion");
}

#[tokio::test]
async fn tool_execution_error_stop_on_error_default() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        tool_call_chunk("call_1", "failing", json!({"input": "test"})),
        StreamChunk::Stop,
    ]]));

    let failing_tool = FailingTool::new("failing", "ssh connection lost");

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(failing_tool)
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id, "do something").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (events, outcome) = result.unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { .. }), "Should be Failed, got: {outcome:?}");

    let has_run_finished = events.iter().any(|e| matches!(e, AgentEvent::RunFinished { .. }));
    assert!(has_run_finished, "Should emit RunFinished even on failure");

    assert_eq!(llm.call_count(), 1, "Should only make 1 LLM call before stopping");
}

#[tokio::test]
async fn tool_args_parse_error_fed_back_to_llm_on_retry() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            StreamChunk::ToolCall(json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "not valid json {{{"
                        }
                    }]
                }
            })),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("I'll fix the JSON".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .system_prompt("sys")
        .error_recovery(Arc::new(RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (_events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);

    let has_parse_error = messages.iter().any(|m| {
        matches!(m, ChatMessage::User { content, .. }
            if content.contains("echo") && content.contains("argument parsing failed"))
    });
    assert!(has_parse_error, "LLM should receive parse error details");

    assert_eq!(llm.call_count(), 2);
}

#[tokio::test]
async fn consecutive_tool_failures_message_integrity() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "failing", json!({"input": "a"})),
            StreamChunk::Stop,
        ],
        vec![
            tool_call_chunk("call_2", "failing", json!({"input": "b"})),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("I give up.".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let failing_tool = FailingTool::new("failing", "persistent failure");

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(failing_tool)
        .system_prompt("sys")
        .error_recovery(Arc::new(RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);

    let failure_count = messages.iter().filter(|m| {
        matches!(m, ChatMessage::User { content, .. } if content.contains("persistent failure"))
    }).count();
    assert!(failure_count >= 2, "Should have at least 2 failure messages: got {failure_count}");
}

#[tokio::test]
async fn approval_deny_with_stop_messages_remain_valid() {
    let llm = Arc::new(MockLlmClient::new(vec![vec![
        tool_call_chunk("call_1", "echo", json!({"message": "test"})),
        StreamChunk::Stop,
    ]]));

    struct DenyHandler;
    #[async_trait]
    impl ApprovalHandler for DenyHandler {
        async fn approve(&self, _request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
            Ok(ApprovalDecision::Deny)
        }
    }

    struct RequireApprovalPolicy;
    impl ToolPolicy for RequireApprovalPolicy {
        fn evaluate_approval(&self, _: &str, _: &Value, _: &str) -> Option<ApprovalRequest> {
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
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (_events, outcome) = result.unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { .. }), "StopOnError should produce Failed, got: {outcome:?}");

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);
}

#[tokio::test]
async fn approval_deny_with_retry_tool_result_still_present() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "echo", json!({"message": "test"})),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("I understand you denied that.".to_string()),
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
        fn evaluate_approval(&self, _: &str, _: &Value, _: &str) -> Option<ApprovalRequest> {
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
        .error_recovery(Arc::new(RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (_events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);

    let has_denial = messages.iter().any(|m| {
        matches!(m, ChatMessage::Tool { content, .. } if content.contains("[Action Denied]"))
    });
    assert!(has_denial, "Denial tool result should be visible to LLM");

    let has_llm_recovery = messages.iter().any(|m| {
        matches!(m, ChatMessage::Assistant { content, .. } if content.as_deref() == Some("I understand you denied that."))
    });
    assert!(has_llm_recovery, "LLM should respond to denial");
}

#[tokio::test]
async fn custom_retry_prompt_template_replacements() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "failing", json!({"input": "x"})),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("got it".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let failing_tool = FailingTool::new("failing", "disk full");

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(failing_tool)
        .system_prompt("sys")
        .error_recovery(Arc::new(RetryOnError))
        .tool_error_retry_prompt("[自定义] 工具 {tool_names} 失败：{error}，请重试")
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();

    let custom_msg = messages.iter().find(|m| {
        matches!(m, ChatMessage::User { content, .. }
            if content.contains("[自定义]") && content.contains("failing") && content.contains("disk full"))
    });
    assert!(custom_msg.is_some(), "Custom template should have placeholders replaced");
    let msg_content = match custom_msg.unwrap() {
        ChatMessage::User { content, .. } => content,
        _ => unreachable!(),
    };
    assert!(msg_content.contains("[自定义] 工具 failing 失败：")
        && msg_content.contains("disk full")
        && msg_content.contains("请重试"),
        "Template replacement mismatch: {msg_content}");
}

#[tokio::test]
async fn run_with_handler_cancelled_emits_run_finished_and_saves() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "echo", json!({"message": "test"})),
            StreamChunk::Stop,
        ],
        vec![
            StreamChunk::Text("should not be reached".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .system_prompt("sys")
        .build();

    let session_id = runtime.create_session();

    let result = runtime
        .run_turn_with_handler(session_id.clone(), "test", |event| {
            if matches!(event, AgentEvent::ToolCallStarted { .. }) {
                return Err(AgentError::Cancelled);
            }
            Ok(())
        })
        .await;

    assert!(result.is_err(), "Should be cancelled: {result:?}");
    let err = result.unwrap_err();
    assert!(err.is_cancelled(), "Should be Cancelled error, got: {err}");

    let stored_ok = runtime.session_store().load(&session_id).await.is_ok_and(|s| s.is_some());
    assert!(stored_ok, "Session should be saved even on cancellation");
}

#[tokio::test]
async fn retry_then_empty_llm_response_continues() {
    let llm = Arc::new(MockLlmClient::new(vec![
        vec![
            tool_call_chunk("call_1", "failing", json!({"input": "test"})),
            StreamChunk::Stop,
        ],
        vec![StreamChunk::Text(String::new()), StreamChunk::Stop],
        vec![
            StreamChunk::Text("recovered after empty".to_string()),
            StreamChunk::Stop,
        ],
    ]));

    let failing_tool = FailingTool::new("failing", "transient error");

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(failing_tool)
        .system_prompt("sys")
        .error_recovery(Arc::new(RetryOnError))
        .build();

    let session_id = runtime.create_session();
    let result = runtime.run_turn_stream(session_id.clone(), "test").await;
    assert!(result.is_ok(), "Expected ok: {result:?}");

    let (_events, outcome) = result.unwrap();
    assert_eq!(outcome, RunOutcome::Completed);

    let session = runtime.session(&session_id).unwrap();
    let messages = session.chat_messages();
    assert_valid_message_history(messages);

    assert_eq!(llm.call_count(), 3, "Should make 3 LLM calls (tool fail + empty + recovery)");
}

// =========================================================================
// 8. Plan-and-Execute tests
// =========================================================================

struct MockPlanGenerator {
    steps: Vec<PlanStep>,
}

impl MockPlanGenerator {
    fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps }
    }
}

struct MockStepExecutor {
    execution_results: Mutex<Vec<StepResult>>,
}

impl MockStepExecutor {
    fn new() -> Self {
        Self {
            execution_results: Mutex::new(Vec::new()),
        }
    }

    fn with_results(results: Vec<StepResult>) -> Self {
        Self {
            execution_results: Mutex::new(results),
        }
    }
}

#[async_trait]
impl PlanGenerator for MockPlanGenerator {
    async fn generate_plan(
        &self,
        objective: &str,
        _context: &str,
        _tools: &[Value],
    ) -> AgentResult<ExecutionPlan> {
        let mut plan = ExecutionPlan::new("test-plan-1", objective);
        plan.steps = self.steps.clone();
        Ok(plan)
    }
}

#[async_trait]
impl StepExecutor for MockStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _plan_context: &Value,
    ) -> AgentResult<StepResult> {
        let mut results = self.execution_results.lock().unwrap();
        if results.is_empty() {
            Ok(StepResult::success(format!("Step {} completed", step.id), 100))
        } else {
            Ok(results.remove(0))
        }
    }
}

struct MockRecoveryStrategy;

#[async_trait]
impl RecoveryStrategy for MockRecoveryStrategy {
    async fn handle_step_failure(
        &self,
        _step: &PlanStep,
        _error: &str,
        _retry_count: usize,
    ) -> AgentResult<RecoveryAction> {
        Ok(RecoveryAction::Abort)
    }
}

#[tokio::test]
async fn test_plan_execution_success() {
    let steps = vec![
        PlanStep::new("step-1", "First step", json!({"type": "tool_call", "tool_name": "echo", "args": {"message": "hello"}})),
        PlanStep::new("step-2", "Second step", json!({"type": "tool_call", "tool_name": "echo", "args": {"message": "world"}})),
    ];

    let generator = Arc::new(MockPlanGenerator::new(steps));
    let executor = Arc::new(MockStepExecutor::new());
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let llm = Arc::new(MockLlmClient::new(vec![]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .build();

    let session_id = runtime.create_session();
    let result = runtime
        .run_plan_deterministic(
            session_id,
            "Test objective",
            generator,
            executor,
            Some(Arc::new(AlwaysContinue)),
            Some(Arc::new(MockRecoveryStrategy)),
            Some(plan_store.clone()),
            |_| Ok(()),
        )
        .await;

    assert!(result.is_ok(), "Plan execution should succeed: {result:?}");
    assert_eq!(result.unwrap(), RunOutcome::Completed);

    let stored_plan = plan_store.load_plan("test-plan-1").await.unwrap();
    assert!(stored_plan.is_some(), "Plan should be stored");
    assert_eq!(stored_plan.unwrap().plan.status, PlanStatus::Completed);
}

#[tokio::test]
async fn test_plan_execution_failure_aborts() {
    let steps = vec![
        PlanStep::new("step-1", "First step", json!({"type": "tool_call", "tool_name": "echo", "args": {"message": "hello"}})),
        PlanStep::new("step-2", "Second step", json!({"type": "tool_call", "tool_name": "echo", "args": {"message": "world"}})),
    ];

    let results = vec![
        StepResult::success("Step 1 done", 100),
        StepResult::failure("Step 2 failed", 100),
    ];

    let generator = Arc::new(MockPlanGenerator::new(steps));
    let executor = Arc::new(MockStepExecutor::with_results(results));
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let llm = Arc::new(MockLlmClient::new(vec![]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .build();

    let session_id = runtime.create_session();
    let result = runtime
        .run_plan_deterministic(
            session_id,
            "Test objective",
            generator,
            executor,
            Some(Arc::new(AlwaysContinue)),
            Some(Arc::new(MockRecoveryStrategy)),
            Some(plan_store.clone()),
            |_| Ok(()),
        )
        .await;

    assert!(result.is_ok(), "Plan execution should return ok: {result:?}");
    assert!(matches!(result.unwrap(), RunOutcome::Failed { .. }));

    let stored_plan = plan_store.load_plan("test-plan-1").await.unwrap();
    assert!(stored_plan.is_some(), "Plan should be stored");
    assert_eq!(stored_plan.unwrap().plan.status, PlanStatus::Failed);
}

#[tokio::test]
async fn test_plan_events_emitted() {
    let steps = vec![
        PlanStep::new("step-1", "First step", json!({"type": "tool_call", "tool_name": "echo", "args": {"message": "hello"}})),
    ];

    let generator = Arc::new(MockPlanGenerator::new(steps));
    let executor = Arc::new(MockStepExecutor::new());
    let plan_store = Arc::new(InMemoryPlanStore::new());

    let llm = Arc::new(MockLlmClient::new(vec![]));

    let mut runtime = AgentBuilder::new(llm.clone())
        .register_tool(EchoTool)
        .build();

    let session_id = runtime.create_session();
    let mut events = Vec::new();
    let result = runtime
        .run_plan_deterministic(
            session_id,
            "Test objective",
            generator,
            executor,
            Some(Arc::new(AlwaysContinue)),
            Some(Arc::new(MockRecoveryStrategy)),
            Some(plan_store),
            |event| {
                events.push(event);
                Ok(())
            },
        )
        .await;

    assert!(result.is_ok());

    let has_plan_generated = events.iter().any(|e| matches!(e, AgentEvent::PlanGenerated { .. }));
    assert!(has_plan_generated, "Should emit PlanGenerated event");

    let has_step_started = events.iter().any(|e| matches!(e, AgentEvent::PlanStepStarted { .. }));
    assert!(has_step_started, "Should emit PlanStepStarted event");

    let has_step_completed = events.iter().any(|e| matches!(e, AgentEvent::PlanStepCompleted { .. }));
    assert!(has_step_completed, "Should emit PlanStepCompleted event");

    let has_plan_completed = events.iter().any(|e| matches!(e, AgentEvent::PlanCompleted { .. }));
    assert!(has_plan_completed, "Should emit PlanCompleted event");
}

#[tokio::test]
async fn test_plan_store_operations() {
    let store = InMemoryPlanStore::new();
    let plan = ExecutionPlan::new("plan-1", "Test plan");

    store.save_plan(&plan, json!({})).await.unwrap();

    let loaded = store.load_plan("plan-1").await.unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().plan.objective, "Test plan");

    let plans = store.list_plans().await.unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0], "plan-1");

    store.delete_plan("plan-1").await.unwrap();
    let loaded = store.load_plan("plan-1").await.unwrap();
    assert!(loaded.is_none());
}

#[test]
fn test_plan_data_structures() {
    let mut plan = ExecutionPlan::new("test", "objective");
    assert_eq!(plan.status, PlanStatus::Created);
    assert!(plan.steps.is_empty());

    let step = PlanStep::new("step-1", "description", json!({"type": "tool_call"}));
    plan.steps.push(step);

    assert_eq!(plan.progress(), (0, 1));
    assert!(!plan.is_completed());
    assert!(!plan.has_failed());

    plan.steps[0].status = StepStatus::Completed;
    assert_eq!(plan.progress(), (1, 1));
    assert!(plan.is_completed());
}

#[test]
fn test_step_result_convenience_methods() {
    let success = StepResult::success("output", 100);
    assert!(success.success);
    assert_eq!(success.output, Some("output".to_string()));
    assert!(success.error.is_none());
    assert_eq!(success.duration_ms, 100);

    let failure = StepResult::failure("error", 200);
    assert!(!failure.success);
    assert!(failure.output.is_none());
    assert_eq!(failure.error, Some("error".to_string()));
    assert_eq!(failure.duration_ms, 200);
}

#[test]
fn test_plan_serialization() {
    let plan = ExecutionPlan::new("test-id", "test objective");
    let json = serde_json::to_string(&plan).unwrap();
    let deserialized: ExecutionPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, "test-id");
    assert_eq!(deserialized.objective, "test objective");
}
