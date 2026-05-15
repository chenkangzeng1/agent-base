use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use agent_base::{
    AgentBuilder, AgentEvent, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, ChatMessage, LlmCapabilities, LlmClient, ResponseFormat, RiskLevel, RunOutcome, StreamChunk, Tool,
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
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _enable_thinking: Option<bool>,
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
// 6.2 多模态消息测试
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
// 6.4 Checkpoint / Resume 测试
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
// 6.3 子 Agent 测试
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
