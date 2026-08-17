use crate::engine::middleware::{Middleware, PostLlmCtx, PreLlmCtx, UserMessageCtx};
use crate::engine::{AgentBuilder, DenyAllApprovalHandler, RetryOnError};
use crate::llm::{LlmCapabilities, LlmClient, StreamChunk};
use crate::tool::{Content, Tool, ToolContext, ToolPolicy};
use crate::types::{
    AgentError, AgentResult, ApprovalRequest, ChatMessage, CheckpointData, CheckpointStep,
    ResponseFormat, RiskLevel, RunOutcome, RuntimeEvent, SessionId, TurnContext,
};
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

/// Minimal LLM client for tests that don't need LLM calls.
struct DummyClient;

#[async_trait]
impl LlmClient for DummyClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        Ok(Value::Null)
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        unimplemented!("not used")
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::default()
    }
}

#[tokio::test]
async fn run_turn_emits_run_finished_on_session_not_found() {
    let client = crate::llm::adapt(Arc::new(DummyClient));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    // Use a SessionId that was never created — session lookup will fail.
    let nonexistent = SessionId::new(99999);

    let event_fired = Arc::new(AtomicBool::new(false));
    let event_fired_clone = event_fired.clone();

    let result = runtime
        .run_turn(nonexistent.clone(), "test input", move |event| {
            if let RuntimeEvent::RunFinished { .. } = &event {
                event_fired_clone.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
        .await;

    // Must return an error
    assert!(
        result.is_err(),
        "run_turn should return Err for nonexistent session"
    );
    // Must have emitted RunFinished before returning
    assert!(
        event_fired.load(Ordering::SeqCst),
        "run_turn must emit RunFinished before returning Err on session not found"
    );
}

/// Middleware that always fails — used to test the middleware error path.
struct FailingMiddleware;

#[async_trait]
impl Middleware for FailingMiddleware {
    async fn on_user_message(&self, _ctx: &mut UserMessageCtx) -> AgentResult<()> {
        Err(AgentError::internal("middleware intentionally fails"))
    }
}

#[tokio::test]
async fn run_turn_emits_run_finished_on_middleware_failure() {
    let client = crate::llm::adapt(Arc::new(DummyClient));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .middleware(FailingMiddleware)
        .build()
        .expect("build runtime");

    // Create a valid session — middleware failure happens AFTER session lookup.
    let sid = runtime.create_session().await;

    let event_fired = Arc::new(AtomicBool::new(false));
    let event_fired_clone = event_fired.clone();

    let result = runtime
        .run_turn(sid, "test input", move |event| {
            if let RuntimeEvent::RunFinished { .. } = &event {
                event_fired_clone.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
        .await;

    assert!(
        result.is_err(),
        "run_turn should return Err when middleware fails"
    );
    assert!(
        event_fired.load(Ordering::SeqCst),
        "run_turn must emit RunFinished before returning Err on middleware failure"
    );
}

/// LLM client whose stream immediately yields an error.
struct ErrorStreamClient;

#[async_trait]
impl LlmClient for ErrorStreamClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        Ok(Value::Null)
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        // Return a stream that immediately yields an error then ends.
        struct ErrorStream;
        impl Stream for ErrorStream {
            type Item = AgentResult<StreamChunk>;
            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Poll::Ready(Some(Err(AgentError::internal("simulated LLM error"))))
            }
        }
        Ok(Box::pin(ErrorStream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::default()
    }
}

/// LLM client whose stream immediately yields a cancellation error, exercising
/// the `e.is_cancelled()` branch of the LLM-stream error path.
struct CancelledStreamClient;

#[async_trait]
impl LlmClient for CancelledStreamClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        Ok(Value::Null)
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        struct CancelledStream;
        impl Stream for CancelledStream {
            type Item = AgentResult<StreamChunk>;
            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Poll::Ready(Some(Err(AgentError::Cancelled)))
            }
        }
        Ok(Box::pin(CancelledStream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::default()
    }
}

#[tokio::test]
async fn run_turn_emits_run_finished_on_llm_error() {
    let client = crate::llm::adapt(Arc::new(ErrorStreamClient));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    let event_fired = Arc::new(AtomicBool::new(false));
    let event_fired_clone = event_fired.clone();

    let result = runtime
        .run_turn(sid, "test input", move |event| {
            if let RuntimeEvent::RunFinished { .. } = &event {
                event_fired_clone.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
        .await;

    // LLM errors should still emit RunFinished so event listeners don't hang.
    assert!(
        event_fired.load(Ordering::SeqCst),
        "run_turn must emit RunFinished when LLM returns an error"
    );
    // Note: the react loop may retry LLM errors, so the result might be Ok (retry succeeded
    // via retry logic) or Err.  Either is fine — the key assertion is that RunFinished fires.
    let _ = result;
}

/// Mock LLM that returns scripted responses — one Vec<StreamChunk> per call.
struct ScriptedClient {
    script: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
}

impl ScriptedClient {
    fn new(script: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            script: Mutex::new(script.into_iter()),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        Ok(Value::Null)
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&crate::ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let chunks: Vec<AgentResult<StreamChunk>> = self
            .script
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();
        Ok(Box::pin(futures_util::stream::iter(chunks)))
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

#[tokio::test]
async fn truncation_guard_blocks_tool_calls_on_length_finish_reason() {
    // First call: tool call with finish_reason="length" — should be blocked by guard.
    // Second call: model retries with corrected approach (text response).
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![
        // Turn 1: truncated tool call
        vec![
            StreamChunk::ToolCall(serde_json::json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_trunc",
                        "function": {
                            "name": "shell",
                            "arguments": "{\"cmd\": \"rm -rf /inco"
                        }
                    }]
                }
            })),
            StreamChunk::Stop {
                finish_reason: Some("length".to_string()),
            },
        ],
        // Turn 2: model sees the error and retries
        vec![
            StreamChunk::Text(
                "I see the previous call was truncated. Let me re-issue it.".to_string(),
            ),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])));

    let runtime = AgentBuilder::new(client)
        .system_prompt("You are a careful assistant.")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "run a command", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        result.is_ok(),
        "run_turn should complete: {:?}",
        result.err()
    );

    // Verify the session messages contain the truncation error, not the
    // partial argument that would have been executed.
    let session = runtime.session(&sid).await.expect("session exists");
    let messages = session.chat_messages().to_vec();

    let has_truncation_error = messages.iter().any(|m| {
        if let ChatMessage::Tool { content, .. } = m {
            content.contains("Tool call was not executed") && content.contains("output token limit")
        } else {
            false
        }
    });
    assert!(
        has_truncation_error,
        "session should contain truncation error tool result. Messages: {:#?}",
        messages
            .iter()
            .map(|m| format!("{:?}", m))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn run_managed_processes_follow_up_messages() {
    // ScriptedClient: first call returns text "done", second call (triggered
    // by follow-up) returns "follow-up done". run_managed should process both.
    let scripted = Arc::new(ScriptedClient::new(vec![
        // Turn 1: initial user input → text response
        vec![
            StreamChunk::Text("done".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
        // Turn 2: follow-up → another text response
        vec![
            StreamChunk::Text("follow-up done".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ]));
    let client = crate::llm::adapt(scripted);
    let runtime = AgentBuilder::new(client.clone())
        .system_prompt("You are a helpful assistant.")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    // Pre-seed a follow-up message — it will be drained after the first
    // inner loop completes and trigger a second inner loop.
    runtime.follow_up("continue working on the task".to_string());

    let mut events = Vec::new();
    let result = runtime
        .run_managed(sid.clone(), "do something", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        result.is_ok(),
        "run_managed should complete: {:?}",
        result.err()
    );

    // Verify session contains both turns
    let session = runtime.session(&sid).await.expect("session exists");
    let messages = session.chat_messages().to_vec();

    let text_count = messages
        .iter()
        .filter(|m| matches!(m, ChatMessage::Assistant { content: Some(c), .. } if c == "done" || c == "follow-up done"))
        .count();
    assert_eq!(
        text_count, 2,
        "both initial and follow-up turns should produce assistant responses"
    );
}

/// Minimal tool that echoes its `text` argument back as `echo: <text>`.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo back the provided text"
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        let text = args.get("text").and_then(Value::as_str).unwrap_or("");
        Ok(vec![Content::text(format!("echo: {text}"))])
    }
}

#[tokio::test]
async fn run_turn_executes_tool_call_and_returns_text() {
    // Turn 1: model requests the echo tool; Turn 2: model emits a final answer.
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![
        vec![
            StreamChunk::ToolCall(serde_json::json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"text\":\"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamChunk::Text("final answer".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])));

    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "echo hello", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(result.is_ok(), "run_turn failed: {:?}", result.err());

    let session = runtime.session(&sid).await.expect("session exists");
    let messages = session.chat_messages().to_vec();

    assert!(
        messages.iter().any(
            |m| matches!(m, ChatMessage::Tool { content, .. } if content.contains("echo: hello"))
        ),
        "session should contain the echo tool result: {:#?}",
        messages
            .iter()
            .map(|m| format!("{:?}", m))
            .collect::<Vec<_>>()
    );
    assert!(
        messages.iter().any(
            |m| matches!(m, ChatMessage::Assistant { content: Some(c), .. } if c == "final answer")
        ),
        "session should contain the final assistant text"
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ToolCallStarted { .. })),
        "run_turn should emit ToolCallStarted"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ToolCallFinished { .. })),
        "run_turn should emit ToolCallFinished"
    );
}

// ── B1: core error / edge paths ──────────────────────────────────────

/// Tool that always fails — exercises the error-recovery (Stop / Retry) paths.
struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &'static str {
        "fail"
    }

    fn description(&self) -> &'static str {
        "Always fails"
    }

    fn schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        Err(AgentError::internal("simulated tool failure"))
    }
}

/// Tool that cancels the current run's token — exercises the cancellation path.
struct CancelTool;

#[async_trait]
impl Tool for CancelTool {
    fn name(&self) -> &'static str {
        "cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel the current run"
    }

    fn schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        ctx.cancel_token.cancel();
        Ok(vec![Content::text("cancelled")])
    }
}

/// Tool that cancels the run's token and then blocks forever, so
/// `execute_tool`'s `cancel_token.cancelled()` branch fires and propagates a raw
/// `AgentError::Cancelled` (not wrapped in `ToolExecution`) to `handle_tool_error`.
struct CancelPendingTool;

#[async_trait]
impl Tool for CancelPendingTool {
    fn name(&self) -> &'static str {
        "cancel_pending"
    }

    fn description(&self) -> &'static str {
        "Cancel the current run mid-execution"
    }

    fn schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        ctx.cancel_token.cancel();
        // Never resolve — let execute_tool's cancellation branch fire instead.
        std::future::pending::<()>().await;
        unreachable!("cancelled tool should never resolve")
    }
}

/// Policy that always requests approval, so `process_approval` reaches the handler.
struct RequireApproval;

#[async_trait]
impl ToolPolicy for RequireApproval {
    async fn evaluate_approval(&self, tool_name: &str, _args: &Value) -> Option<ApprovalRequest> {
        Some(ApprovalRequest {
            title: format!("Approve {tool_name}"),
            message: "Approve this tool call?".to_string(),
            action_key: Some(format!("approve:{tool_name}")),
            risk_level: RiskLevel::Sensitive,
            raw: None,
        })
    }
}

/// Middleware recording the `on_pre_llm` / `on_post_llm` hooks, and nudging
/// `nudge_count` so the write-back branch is exercised.
struct HookMiddleware {
    pre_called: Arc<AtomicBool>,
    post_called: Arc<AtomicBool>,
}

#[async_trait]
impl Middleware for HookMiddleware {
    async fn on_pre_llm(&self, _ctx: &mut PreLlmCtx) -> AgentResult<()> {
        self.pre_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        self.post_called.store(true, Ordering::SeqCst);
        ctx.nudge_count += 1;
        Ok(())
    }
}

#[tokio::test]
async fn run_emits_run_finished_on_session_not_found() {
    let client = crate::llm::adapt(Arc::new(DummyClient));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let nonexistent = SessionId::new(99999);
    let event_fired = Arc::new(AtomicBool::new(false));
    let event_fired_clone = event_fired.clone();

    let result = runtime
        .run(nonexistent.clone(), move |event| {
            if matches!(event, RuntimeEvent::RunFinished { .. }) {
                event_fired_clone.store(true, Ordering::SeqCst);
            }
            Ok(())
        })
        .await;

    assert!(
        result.is_err(),
        "run should return Err for a nonexistent session"
    );
    assert!(
        event_fired.load(Ordering::SeqCst),
        "run must emit RunFinished before returning Err on session not found"
    );
}

#[tokio::test]
async fn run_completes_with_text_response() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::Text("answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let mut events = Vec::new();
    let result = runtime
        .run(sid.clone(), |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "run failed: {:?}",
        result.err()
    );

    let session = runtime.session(&sid).await.expect("session exists");
    assert!(
        session
            .chat_messages()
            .iter()
            .any(|m| matches!(m, ChatMessage::Assistant { content: Some(c), .. } if c == "answer")),
        "run should push the assistant text response into the session"
    );
}

#[tokio::test]
async fn run_turn_emits_run_cancelled_when_cancelled_mid_run() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_cancel",
                    "function": { "name": "cancel", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(CancelTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Cancelled)),
        "run_turn should return Cancelled outcome: {:?}",
        result
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RunCancelled { .. })),
        "run_turn should emit RunCancelled when cancelled mid-run"
    );
}

#[tokio::test]
async fn run_turn_failing_tool_stops_with_failed_outcome() {
    // Default recovery is StopOnError — a failing tool should stop the run.
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_fail",
                    "function": { "name": "fail", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(FailingTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "do something", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "failing tool with StopOnError should yield Failed outcome: {:?}",
        result
    );
}

#[tokio::test]
async fn run_turn_failing_tool_retries_then_completes() {
    // RetryOnError recovery: first turn fails, second turn (after retry prompt) completes.
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![
        vec![
            StreamChunk::ToolCall(serde_json::json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_fail",
                        "function": { "name": "fail", "arguments": "{}" }
                    }]
                }
            })),
            StreamChunk::Stop {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamChunk::Text("recovered".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(FailingTool)
        .error_recovery(Arc::new(RetryOnError))
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "do something", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "failing tool with RetryOnError should eventually complete: {:?}",
        result
    );
}

#[tokio::test]
async fn run_turn_approval_denied_completes_without_executing() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_denied",
                    "function": { "name": "echo", "arguments": "{\"text\":\"hello\"}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .approval_handler(Arc::new(DenyAllApprovalHandler))
        .tool_policy(Arc::new(RequireApproval))
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "echo hello", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "approval denial should stop the run cleanly (Completed): {:?}",
        result
    );

    // The denied tool must NOT have executed.
    let session = runtime.session(&sid).await.expect("session exists");
    assert!(
        !session.chat_messages().iter().any(
            |m| matches!(m, ChatMessage::Tool { content, .. } if content.contains("echo: hello"))
        ),
        "denied tool call must not execute"
    );
}

#[tokio::test]
async fn resume_from_checkpoint_executes_pending_tool_calls() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::Text("resumed answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    let checkpoint = CheckpointData {
        session_id: sid.clone(),
        user_input: "resume".to_string(),
        step: CheckpointStep::BeforeToolCalls {
            tool_calls: vec![(
                "call_resume".to_string(),
                "echo".to_string(),
                r#"{"text":"resume"}"#.to_string(),
            )],
        },
        turn_count: 0,
    };

    let mut events = Vec::new();
    let result = runtime
        .runner
        .resume_from_checkpoint(checkpoint, |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "resume_from_checkpoint should complete: {:?}",
        result.err()
    );

    let session = runtime.session(&sid).await.expect("session exists");
    assert!(
        session.chat_messages().iter().any(
            |m| matches!(m, ChatMessage::Tool { content, .. } if content.contains("echo: resume"))
        ),
        "resumed tool call should have executed"
    );
}

#[tokio::test]
async fn middleware_on_pre_and_post_llm_hooks_fire() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::Text("hello".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])));

    let pre_called = Arc::new(AtomicBool::new(false));
    let post_called = Arc::new(AtomicBool::new(false));
    let mw = HookMiddleware {
        pre_called: pre_called.clone(),
        post_called: post_called.clone(),
    };

    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .middleware(mw)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime.run_turn(sid.clone(), "hi", |_| Ok(())).await;

    assert!(result.is_ok(), "run_turn failed: {:?}", result.err());
    assert!(
        pre_called.load(Ordering::SeqCst),
        "on_pre_llm hook should fire"
    );
    assert!(
        post_called.load(Ordering::SeqCst),
        "on_post_llm hook should fire"
    );

    // The middleware incremented nudge_count, which should be written back.
    let session = runtime.session(&sid).await.expect("session exists");
    assert_eq!(session.nudge_count, 1, "nudge_count should be written back");
}

// ── Bug ① regression: exactly one RunFinished per entry point ──────

#[tokio::test]
async fn run_emits_runfinished_exactly_once() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::Text("answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let mut events = Vec::new();
    let outcome = runtime
        .run(sid.clone(), |event| {
            events.push(event);
            Ok(())
        })
        .await
        .expect("run");

    assert!(matches!(outcome, RunOutcome::Completed));
    let finished = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::RunFinished { .. }))
        .count();
    assert_eq!(finished, 1, "run() must emit exactly one RunFinished");
}

#[tokio::test]
async fn run_managed_emits_runfinished_exactly_once() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::Text("done".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let mut events = Vec::new();
    let outcome = runtime
        .run_managed(sid.clone(), "do something", |event| {
            events.push(event);
            Ok(())
        })
        .await
        .expect("run_managed");

    assert!(matches!(outcome, RunOutcome::Completed));
    let finished = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::RunFinished { .. }))
        .count();
    assert_eq!(
        finished, 1,
        "run_managed() must emit exactly one RunFinished"
    );
}

// ── Bug ② regression: run_managed cancel cleans up ephemeral msgs ───

#[tokio::test]
async fn run_managed_cancel_cleans_up_ephemeral_messages() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_cancel",
                    "function": { "name": "cancel", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(CancelTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    // Inject an ephemeral message that must be cleaned up when the run ends.
    runtime
        .set_messages(&sid, vec![ChatMessage::user_ephemeral("temp nudge")])
        .await
        .unwrap();

    let result = runtime
        .run_managed(sid.clone(), "cancel now", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Cancelled)),
        "run_managed should return Cancelled: {:?}",
        result
    );

    let session = runtime.session(&sid).await.expect("session exists");
    assert!(
        session.chat_messages().iter().all(|m| !m.is_ephemeral()),
        "run_managed cancel must clean up ephemeral messages, got: {:?}",
        session.chat_messages()
    );
}

// ── Characterization: exact ordered event sequences ──────────────────
//
// These pin the observable ordering (checkpoint steps + terminal events)
// and the turn-end TurnContext values, so the TurnEndCtx / handle_llm_turn
// refactor cannot silently change behavior.

/// Map a captured event stream to a compact ordered label sequence,
/// keeping only checkpoint steps and terminal (RunFinished/RunCancelled)
/// events — the observable ordering the refactor must preserve.
fn terminal_seq(events: &[RuntimeEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|e| match e {
            RuntimeEvent::Checkpoint { checkpoint, .. } => Some(match &checkpoint.step {
                CheckpointStep::AfterUserInput => "AfterUserInput",
                CheckpointStep::BeforeLlm { .. } => "BeforeLlm",
                CheckpointStep::BeforeToolCalls { .. } => "BeforeToolCalls",
                CheckpointStep::AfterToolCalls { .. } => "AfterToolCalls",
            }),
            RuntimeEvent::RunFinished { .. } => Some("RunFinished"),
            RuntimeEvent::RunCancelled { .. } => Some("RunCancelled"),
            _ => None,
        })
        .collect()
}

/// Single-turn text-only script.
fn text_script(text: &str) -> Vec<Vec<StreamChunk>> {
    vec![vec![
        StreamChunk::Text(text.to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]]
}

#[tokio::test]
async fn turn_end_callback_receives_correct_context() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("answer"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let captured = Arc::new(Mutex::new(Vec::<TurnContext>::new()));
    let captured_clone = captured.clone();
    runtime.on_turn_end(move |ctx| {
        captured_clone.lock().unwrap().push(ctx.clone());
    });

    let sid = runtime.create_session().await;
    let result = runtime.run_turn(sid.clone(), "question", |_| Ok(())).await;

    assert!(matches!(result, Ok(RunOutcome::Completed)));
    let contexts = captured.lock().unwrap();
    assert_eq!(contexts.len(), 1, "one turn → one turn-end callback");
    let ctx = &contexts[0];
    assert!(matches!(&ctx.outcome, RunOutcome::Completed));
    assert_eq!(ctx.turn_number, 1);
    assert_eq!(ctx.tool_call_count, 0);
    assert_eq!(ctx.tool_success, 0);
    assert_eq!(ctx.tool_failed, 0);
    assert!(ctx.error_message.is_none());
    assert_eq!(ctx.full_text_len, "answer".len() as u64);
    assert!(!ctx.has_thinking);
    assert_eq!(ctx.llm_calls, 1);
    assert_eq!(ctx.user_input.as_str(), "question");
}

#[tokio::test]
async fn run_turn_text_only_event_order() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("answer"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let (events, outcome) = runtime
        .run_turn_collect(sid, "question")
        .await
        .expect("run_turn_collect");

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(
        terminal_seq(&events),
        vec!["AfterUserInput", "BeforeLlm", "RunFinished"]
    );
}

/// Reasoning-only turn: the model emits reasoning_content but no text and no
/// tool call. Mirrors the deepseek-v4-pro "thinking runaway" on long
/// multi-agent contexts.
fn reasoning_script() -> Vec<Vec<StreamChunk>> {
    (0..10)
        .map(|_| {
            vec![
                StreamChunk::Thought("thinking but never committing".to_string()),
                StreamChunk::Stop {
                    finish_reason: Some("stop".to_string()),
                },
            ]
        })
        .collect()
}

/// When a tool is registered but the model keeps producing reasoning-only
/// responses, the react loop must NOT report a fake completion — it nudges,
/// then fails after `REASONING_ONLY_MAX_STRIKES` consecutive strikes.
#[tokio::test]
async fn reasoning_only_with_tools_fails_instead_of_fake_completing() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(reasoning_script())));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "do the thing", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "reasoning-only with tools must fail, not complete: {:?}",
        result
    );
}

/// Reasoning-only responses must never be promoted into the answer — even when
/// no tools are registered, a model that keeps "thinking" without producing a
/// final answer must be nudged and eventually fail, not silently completed.
#[tokio::test]
async fn reasoning_only_without_tools_fails_instead_of_promoting() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(reasoning_script())));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "do the thing", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "reasoning-only without tools must fail, not promote reasoning to the answer: {:?}",
        result
    );
}

/// A completely empty response (no text, no reasoning, no tool call) must be
/// retried a bounded number of times, then fail — not loop forever.
#[tokio::test]
async fn empty_response_fails_after_bounded_retries() {
    let script = (0..5)
        .map(|_| {
            vec![StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            }]
        })
        .collect();
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(script)));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid.clone(), "do the thing", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "empty response must fail after bounded retries, not loop forever: {:?}",
        result
    );
}

#[tokio::test]
async fn run_turn_tool_call_then_text_event_order() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![
        vec![
            StreamChunk::ToolCall(serde_json::json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"text\":\"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamChunk::Text("final answer".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let (events, outcome) = runtime
        .run_turn_collect(sid, "echo hello")
        .await
        .expect("run_turn_collect");

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(
        terminal_seq(&events),
        vec![
            "AfterUserInput",
            "BeforeLlm",
            "BeforeToolCalls",
            "AfterToolCalls",
            "BeforeLlm",
            "RunFinished"
        ]
    );
}

#[tokio::test]
async fn run_turn_cancel_does_not_emit_runfinished() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_cancel",
                    "function": { "name": "cancel", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(CancelTool)
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(matches!(result, Ok(RunOutcome::Cancelled)));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RunCancelled { .. })),
        "cancel must emit RunCancelled"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::RunFinished { .. }))
            .count(),
        0,
        "cancelled run_turn must not emit RunFinished"
    );
}

#[tokio::test]
async fn run_text_only_event_order() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("answer"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let mut events = Vec::new();
    let outcome = runtime
        .run(sid.clone(), |event| {
            events.push(event);
            Ok(())
        })
        .await
        .expect("run");

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(terminal_seq(&events), vec!["BeforeLlm", "RunFinished"]);
}

#[tokio::test]
async fn run_managed_text_only_event_order() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("done"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let mut events = Vec::new();
    let outcome = runtime
        .run_managed(sid.clone(), "do something", |event| {
            events.push(event);
            Ok(())
        })
        .await
        .expect("run_managed");

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(terminal_seq(&events), vec!["BeforeLlm", "RunFinished"]);
}

#[tokio::test]
async fn max_turns_exceeded_event_order() {
    // Turn 1 requests a tool; with max_turns = 1, turn 2 hits the cap
    // before any LLM call, so the second script entry is never consumed.
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![
        vec![
            StreamChunk::ToolCall(serde_json::json!({
                "delta": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"text\":\"hello\"}"
                        }
                    }]
                }
            })),
            StreamChunk::Stop {
                finish_reason: Some("tool_calls".to_string()),
            },
        ],
        vec![
            StreamChunk::Text("unused".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(EchoTool)
        .execution_max_turns(1)
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let (events, outcome) = runtime
        .run_turn_collect(sid, "go")
        .await
        .expect("run_turn_collect");

    assert!(matches!(outcome, RunOutcome::MaxTurnsExceeded { .. }));
    assert_eq!(
        terminal_seq(&events),
        vec![
            "AfterUserInput",
            "BeforeLlm",
            "BeforeToolCalls",
            "AfterToolCalls",
            "RunFinished"
        ]
    );
}

// ── Cancel / error-recovery branch coverage (react_loop coverage trough) ──
//
// The following tests target the cancel and error sub-paths that the happy-path
// suite never reached: mid-execution tool cancellation, cancelled LLM stream
// errors, the optional llm_retry branch, and run_managed's cancel / turn-budget
// branches.

#[tokio::test]
async fn run_turn_tool_cancelled_mid_execution_returns_cancelled() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_cancel",
                    "function": { "name": "cancel_pending", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(CancelPendingTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        matches!(&result, Err(e) if e.is_cancelled()),
        "run_turn should surface a Cancelled error for mid-execution tool cancel, got: {result:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RunCancelled { .. })),
        "cancelled tool execution must emit RunCancelled"
    );
}

#[tokio::test]
async fn run_turn_llm_stream_cancelled_returns_cancelled() {
    let client = crate::llm::adapt(Arc::new(CancelledStreamClient));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let mut events = Vec::new();
    let result = runtime
        .run_turn(sid.clone(), "test input", |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(
        matches!(&result, Err(e) if e.is_cancelled()),
        "run_turn should surface the cancelled LLM stream error, got: {result:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RunCancelled { .. })),
        "cancelled LLM stream must emit RunCancelled"
    );
}

#[tokio::test]
async fn run_turn_with_llm_retry_completes() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("answer"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .llm_retry(crate::types::RetryConfig::default().max_retries(1))
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime.run_turn(sid.clone(), "hi", |_| Ok(())).await;

    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "run_turn with llm_retry should complete: {:?}",
        result
    );
}

#[tokio::test]
async fn run_managed_tool_cancelled_mid_execution_returns_cancelled() {
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(vec![vec![
        StreamChunk::ToolCall(serde_json::json!({
            "delta": {
                "tool_calls": [{
                    "id": "call_cancel",
                    "function": { "name": "cancel_pending", "arguments": "{}" }
                }]
            }
        })),
        StreamChunk::Stop {
            finish_reason: Some("tool_calls".to_string()),
        },
    ]])));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .register_tool(CancelPendingTool)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_managed(sid.clone(), "cancel now", |_| Ok(()))
        .await;

    assert!(
        matches!(&result, Err(e) if e.is_cancelled()),
        "run_managed should surface a Cancelled error for mid-execution tool cancel, got: {result:?}"
    );
}

#[tokio::test]
async fn run_managed_max_turns_exceeded() {
    // max_turns = 1 caps the *cumulative* turn budget across follow-ups. The
    // first inner loop consumes the single turn; the seeded follow-up message
    // triggers a second iteration, which hits the cap before any further LLM call.
    let client = crate::llm::adapt(Arc::new(ScriptedClient::new(text_script("done"))));
    let runtime = AgentBuilder::new(client)
        .system_prompt("test")
        .execution_max_turns(1)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    runtime.follow_up("continue".to_string());

    let result = runtime
        .run_managed(sid.clone(), "do something", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::MaxTurnsExceeded { .. })),
        "run_managed should exceed its cumulative turn budget: {:?}",
        result
    );
}
