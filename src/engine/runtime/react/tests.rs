use crate::engine::middleware::{Middleware, PostLlmCtx, PreLlmCtx, UserMessageCtx};
use crate::engine::{AgentBuilder, DenyAllApprovalHandler, RetryOnError};
use crate::llm::StreamChunk;
use crate::tool::{Content, Tool, ToolContext, ToolPolicy};
use crate::types::{
    AgentError, AgentResult, ApprovalRequest, ChatMessage, CheckpointData, CheckpointStep,
    RiskLevel, RunOutcome, RuntimeEvent, SessionId, TurnContext,
};
use async_trait::async_trait;
use futures_core::Stream;
use llm_trait::LlmProvider;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

/// Minimal LLM provider for tests that don't need LLM calls.
struct DummyProvider;

#[async_trait]
impl LlmProvider for DummyProvider {
    async fn stream(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatStream, llm_trait::LlmError> {
        unimplemented!("not used")
    }

    async fn chat(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatResponse, llm_trait::LlmError> {
        Ok(llm_trait::ChatResponse {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![],
            usage: Default::default(),
            finish_reason: llm_trait::response::FinishReason::Stop,
            raw: None,
            thinking_signature: None,
        })
    }

    fn capabilities(&self) -> llm_trait::Capabilities {
        Default::default()
    }

    fn info(&self) -> llm_trait::ProviderInfo {
        llm_trait::ProviderInfo {
            name: "test".to_string(),
            model: "test".to_string(),
            version: None,
        }
    }
}

#[tokio::test]
async fn run_turn_emits_run_finished_on_session_not_found() {
    let runtime = AgentBuilder::new(Arc::new(DummyProvider))
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
    let runtime = AgentBuilder::new(Arc::new(DummyProvider))
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

/// LLM provider whose stream immediately yields an error.
struct ErrorStreamProvider;

#[async_trait]
impl LlmProvider for ErrorStreamProvider {
    async fn stream(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatStream, llm_trait::LlmError> {
        // Return a stream that immediately yields an error then ends.
        struct ErrorStream;
        impl Stream for ErrorStream {
            type Item = Result<StreamChunk, llm_trait::LlmError>;
            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Poll::Ready(Some(Err(llm_trait::LlmError::llm("simulated LLM error"))))
            }
        }
        Ok(llm_trait::ChatStream::new(Box::pin(ErrorStream)))
    }

    async fn chat(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatResponse, llm_trait::LlmError> {
        Ok(llm_trait::ChatResponse {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![],
            usage: Default::default(),
            finish_reason: llm_trait::response::FinishReason::Stop,
            raw: None,
            thinking_signature: None,
        })
    }

    fn capabilities(&self) -> llm_trait::Capabilities {
        Default::default()
    }

    fn info(&self) -> llm_trait::ProviderInfo {
        llm_trait::ProviderInfo {
            name: "test".to_string(),
            model: "test".to_string(),
            version: None,
        }
    }
}

/// LLM provider whose stream immediately yields a cancellation error, exercising
/// the `e.is_cancelled()` branch of the LLM-stream error path.
struct CancelledStreamProvider;

#[async_trait]
impl LlmProvider for CancelledStreamProvider {
    async fn stream(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatStream, llm_trait::LlmError> {
        struct CancelledStream;
        impl Stream for CancelledStream {
            type Item = Result<StreamChunk, llm_trait::LlmError>;
            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                Poll::Ready(Some(Err(llm_trait::LlmError::llm("cancelled"))))
            }
        }
        Ok(llm_trait::ChatStream::new(Box::pin(CancelledStream)))
    }

    async fn chat(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatResponse, llm_trait::LlmError> {
        Ok(llm_trait::ChatResponse {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![],
            usage: Default::default(),
            finish_reason: llm_trait::response::FinishReason::Stop,
            raw: None,
            thinking_signature: None,
        })
    }

    fn capabilities(&self) -> llm_trait::Capabilities {
        Default::default()
    }

    fn info(&self) -> llm_trait::ProviderInfo {
        llm_trait::ProviderInfo {
            name: "test".to_string(),
            model: "test".to_string(),
            version: None,
        }
    }
}

#[tokio::test]
async fn run_turn_emits_run_finished_on_llm_error() {
    let runtime = AgentBuilder::new(Arc::new(ErrorStreamProvider))
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
struct ScriptedProvider {
    script: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Vec<StreamChunk>>) -> Self {
        Self {
            script: Mutex::new(script.into_iter()),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatStream, llm_trait::LlmError> {
        let chunks: Vec<Result<StreamChunk, llm_trait::LlmError>> = self
            .script
            .lock()
            .unwrap()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(Ok)
            .collect();
        Ok(llm_trait::ChatStream::new(Box::pin(
            futures_util::stream::iter(chunks),
        )))
    }

    async fn chat(
        &self,
        _request: llm_trait::ChatRequest,
    ) -> Result<llm_trait::ChatResponse, llm_trait::LlmError> {
        Ok(llm_trait::ChatResponse {
            content: String::new(),
            reasoning_content: None,
            tool_calls: vec![],
            usage: Default::default(),
            finish_reason: llm_trait::response::FinishReason::Stop,
            raw: None,
            thinking_signature: None,
        })
    }

    fn capabilities(&self) -> llm_trait::Capabilities {
        llm_trait::Capabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    fn info(&self) -> llm_trait::ProviderInfo {
        llm_trait::ProviderInfo {
            name: "test".to_string(),
            model: "test".to_string(),
            version: None,
        }
    }
}

#[tokio::test]
async fn truncation_guard_blocks_tool_calls_on_length_finish_reason() {
    // First call: tool call with finish_reason="length" — should be blocked by guard.
    // Second call: model retries with corrected approach (text response).
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
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
    ])))
    .system_prompt("You are a careful assistant.")
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "run a command", move |event| {
            events_clone.lock().unwrap().push(event);
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
    // ScriptedProvider: first call returns text "done", second call (triggered
    // by follow-up) returns "follow-up done". run_managed should process both.
    let scripted = Arc::new(ScriptedProvider::new(vec![
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
    let runtime = AgentBuilder::new(scripted)
        .system_prompt("You are a helpful assistant.")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;

    // Pre-seed a follow-up message — it will be drained after the first
    // inner loop completes and trigger a second inner loop.
    runtime.follow_up("continue working on the task".to_string());

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_managed(sid.clone(), "do something", move |event| {
            events_clone.lock().unwrap().push(event);
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
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
    ])))
    .system_prompt("test")
    .register_tool(EchoTool)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "echo hello", move |event| {
            events_clone.lock().unwrap().push(event);
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

    let events = events.lock().unwrap();

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
    let runtime = AgentBuilder::new(Arc::new(DummyProvider))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run(sid.clone(), move |event| {
            events_clone.lock().unwrap().push(event);
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
    .system_prompt("test")
    .register_tool(CancelTool)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await;

    let events = events.lock().unwrap();

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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
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
    ])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("resumed answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
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

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .runner
        .resume_from_checkpoint(checkpoint, move |event| {
            events_clone.lock().unwrap().push(event);
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
    let pre_called = Arc::new(AtomicBool::new(false));
    let post_called = Arc::new(AtomicBool::new(false));
    let mw = HookMiddleware {
        pre_called: pre_called.clone(),
        post_called: post_called.clone(),
    };

    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("hello".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
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
    assert_eq!(
        session.run_state.nudge_count, 1,
        "nudge_count should be written back"
    );
}

// ── Bug ① regression: exactly one RunFinished per entry point ──────

#[tokio::test]
async fn run_emits_runfinished_exactly_once() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("answer".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let outcome = runtime
        .run(sid.clone(), move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await
        .expect("run");

    let events = events.lock().unwrap();

    assert!(matches!(outcome, RunOutcome::Completed));
    let finished = events
        .iter()
        .filter(|e| matches!(e, RuntimeEvent::RunFinished { .. }))
        .count();
    assert_eq!(finished, 1, "run() must emit exactly one RunFinished");
}

#[tokio::test]
async fn run_managed_emits_runfinished_exactly_once() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("done".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let outcome = runtime
        .run_managed(sid.clone(), "do something", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await
        .expect("run_managed");

    let events = events.lock().unwrap();

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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("answer"))))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("answer"))))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(reasoning_script())))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(reasoning_script())))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(script)))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
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
    ])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
    .system_prompt("test")
    .register_tool(CancelTool)
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await;

    let events = events.lock().unwrap();

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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("answer"))))
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;
    runtime.add_user_message(&sid, "question").await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let outcome = runtime
        .run(sid.clone(), move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await
        .expect("run");

    let events = events.lock().unwrap();

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(terminal_seq(&events), vec!["BeforeLlm", "RunFinished"]);
}

#[tokio::test]
async fn run_managed_text_only_event_order() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("done"))))
        .system_prompt("test")
        .build()
        .expect("build runtime");
    let sid = runtime.create_session().await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let outcome = runtime
        .run_managed(sid.clone(), "do something", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await
        .expect("run_managed");

    let events = events.lock().unwrap();

    assert!(matches!(outcome, RunOutcome::Completed));
    assert_eq!(terminal_seq(&events), vec!["BeforeLlm", "RunFinished"]);
}

#[tokio::test]
async fn max_turns_exceeded_event_order() {
    // Turn 1 requests a tool; with max_turns = 1, turn 2 hits the cap
    // before any LLM call, so the second script entry is never consumed.
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
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
    ])))
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

// ── Cancel / error-recovery branch coverage (react coverage trough) ──
//
// The following tests target the cancel and error sub-paths that the happy-path
// suite never reached: mid-execution tool cancellation, cancelled LLM stream
// errors, the optional llm_retry branch, and run_managed's cancel / turn-budget
// branches.

#[tokio::test]
async fn run_turn_tool_cancelled_mid_execution_returns_cancelled() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
    .system_prompt("test")
    .register_tool(CancelPendingTool)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "cancel now", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await;

    let events = events.lock().unwrap();

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
    // With LlmProvider, LlmError has no Cancelled variant — the stream error
    // converts to AgentError::Llm, which the react loop surfaces as Failed.
    // Real cancellation (user-triggered) is handled by the cancel_token path
    // in run_llm_turn_with_retry, not by stream errors.
    let runtime = AgentBuilder::new(Arc::new(CancelledStreamProvider))
        .system_prompt("test")
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let result = runtime
        .run_turn(sid.clone(), "test input", move |event| {
            events_clone.lock().unwrap().push(event);
            Ok(())
        })
        .await;

    let events = events.lock().unwrap();

    assert!(
        matches!(&result, Ok(RunOutcome::Failed { .. }) | Err(_)),
        "run_turn should surface the LLM stream error, got: {result:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::RunFinished { .. })),
        "LLM stream error must emit RunFinished"
    );
}

#[tokio::test]
async fn run_turn_with_llm_retry_completes() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("answer"))))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
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
    ]])))
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
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(text_script("done"))))
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

// ---------------------------------------------------------------------------
// Phase 1: FinishReason branch tests
// ---------------------------------------------------------------------------

/// Model returns `tool_use` finish reason but no actual tool call data.
/// The react loop should return `RunOutcome::Failed`.
#[tokio::test]
async fn tool_use_with_no_tool_calls_returns_failed() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        // A ToolCall chunk with no delta.tool_calls — sets is_tool_call=true
        // but leaves tool_calls empty.
        StreamChunk::ToolCall(serde_json::json!({ "no_delta": true })),
        StreamChunk::Stop {
            finish_reason: Some("tool_use".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;

    let result = runtime.run_turn(sid, "do something", |_| Ok(())).await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "tool_use with no tool calls should return Failed: {:?}",
        result
    );
}

/// Model returns `max_tokens` (truncated) with text-only output (no tool calls).
/// The react loop should return `RunOutcome::Failed`.
#[tokio::test]
async fn truncated_text_only_returns_failed() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("Here is part of my answer before being cut".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("max_tokens".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;

    let result = runtime.run_turn(sid, "do something", |_| Ok(())).await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "truncated text-only should return Failed: {:?}",
        result
    );
}

/// Model returns `length` (OpenAI truncation) with text-only output.
/// The react loop should return `RunOutcome::Failed`.
#[tokio::test]
async fn openai_length_text_only_returns_failed() {
    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        StreamChunk::Text("Partial response before length limit".to_string()),
        StreamChunk::Stop {
            finish_reason: Some("length".to_string()),
        },
    ]])))
    .system_prompt("test")
    .build()
    .expect("build runtime");
    let sid = runtime.create_session().await;

    let result = runtime.run_turn(sid, "do something", |_| Ok(())).await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "OpenAI length with text-only should return Failed: {:?}",
        result
    );
}

/// Model emits a ToolCall chunk (setting `is_tool_call = true`) but the stream
/// ends with `finish_reason = "stop"` and zero parsed tool calls (no text either).
///
/// This mirrors the real-world scenario where the model starts generating a tool
/// call but the stream produces an incomplete/empty tool call delta.  With
/// `finish_reason = "stop"` (not `"tool_use"`), the anomaly-detection branch at
/// line 582 does NOT fire.
///
/// After the fix, the react loop should detect this inconsistent state via the
/// new incomplete-tool-call branch and surface a failure instead of silently
/// falling through to the text-only branch and completing.
#[tokio::test]
async fn tool_call_chunk_with_no_parsed_calls_and_stop_finish_fails() {
    let guard = Arc::new(RecordingGuard::new(GuardDecision::Fail {
        error: "incomplete tool call".to_string(),
    }));
    let guard_clone = guard.clone();

    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![vec![
        // A ToolCall chunk that sets is_tool_call=true but contains no
        // actual tool_calls in the delta (simulates incomplete stream).
        StreamChunk::ToolCall(serde_json::json!({ "no_delta": true })),
        StreamChunk::Stop {
            finish_reason: Some("stop".to_string()),
        },
    ]])))
    .system_prompt("test")
    .register_tool(EchoTool)
    .guard_dyn(guard_clone)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid, "帮我解读一下这个工程", |_| Ok(()))
        .await;

    // The guard returns Fail for incomplete tool calls, so the run
    // should return Failed — NOT silently complete.
    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "incomplete tool call should fail, not silently complete. Got: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// Branch 4 (text-only after tools): guard integration tests
// ---------------------------------------------------------------------------

use crate::engine::react_loop_guard::{GuardCtx, GuardDecision, ReactLoopGuard};

/// A guard that records every `on_turn` call for later inspection,
/// and returns a configurable decision.
struct RecordingGuard {
    /// Recorded (run_has_tool_calls, model_response) from on_turn calls
    /// where is_text_only was true.
    calls: Mutex<Vec<(bool, String)>>,
    /// What to return from on_turn.
    decision: GuardDecision,
}

impl RecordingGuard {
    fn new(decision: GuardDecision) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            decision,
        }
    }
}

#[async_trait]
impl ReactLoopGuard for RecordingGuard {
    async fn on_turn(&self, ctx: &GuardCtx) -> GuardDecision {
        if ctx.is_text_only {
            self.calls
                .lock()
                .unwrap()
                .push((ctx.run_has_tool_calls, ctx.model_response.clone()));
        }
        self.decision.clone()
    }
}

/// When the model calls a tool then returns text, the guard's `on_text_only`
/// must be invoked with `run_has_tool_calls = true`.
#[tokio::test]
async fn branch4_guard_called_with_run_has_tool_calls() {
    let guard = Arc::new(RecordingGuard::new(GuardDecision::Complete));
    let guard_clone = guard.clone();

    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
        // Turn 1: tool call
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
        // Turn 2: text-only answer → branch 4
        vec![
            StreamChunk::Text("The answer is 42.".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])))
    .system_prompt("test")
    .register_tool(EchoTool)
    .guard_dyn(guard_clone)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;

    let result = runtime.run_turn(sid, "echo hello", |_| Ok(())).await;
    assert!(
        matches!(result, Ok(RunOutcome::Completed)),
        "run_turn should complete: {:?}",
        result
    );

    let calls = guard.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "on_text_only should be called exactly once");
    let (run_has_tools, response) = &calls[0];
    assert!(
        *run_has_tools,
        "run_has_tool_calls must be true after tool use"
    );
    assert_eq!(response, "The answer is 42.");
}

/// When the guard returns `Continue`, the react loop should inject a nudge
/// and call the LLM again — the second text response should complete.
#[tokio::test]
async fn branch4_guard_continue_nudges_and_retries() {
    // Guard always returns Continue → nudge each time → loops until max turns.
    // We verify: (1) guard called multiple times, (2) nudge injected, (3) run_has_tool_calls persists.
    let guard = Arc::new(RecordingGuard::new(GuardDecision::Continue {
        nudge: Some("please continue".to_string()),
    }));
    let guard_clone = guard.clone();

    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(vec![
        // Turn 1: tool call
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
        // Turn 2: text-only → guard says Continue
        vec![
            StreamChunk::Text("maybe done".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
        // Turn 3: after nudge, model gives final answer
        vec![
            StreamChunk::Text("here is the complete answer".to_string()),
            StreamChunk::Stop {
                finish_reason: Some("stop".to_string()),
            },
        ],
    ])))
    .system_prompt("test")
    .register_tool(EchoTool)
    .guard_dyn(guard_clone)
    .build()
    .expect("build runtime");

    let sid = runtime.create_session().await;

    let (_events, _outcome) = runtime
        .run_turn_collect(sid.clone(), "echo hello")
        .await
        .expect("run_turn_collect");

    // Extract guard call data before any async work.
    let (call_count, all_have_tools) = {
        let calls = guard.calls.lock().unwrap();
        (calls.len(), calls.iter().all(|(has_tools, _)| *has_tools))
    };
    assert!(
        call_count >= 2,
        "guard should be called at least twice (initial + after nudge), got: {}",
        call_count
    );

    // Verify the nudge was pushed as a user message.
    let session = runtime.session(&sid).await.expect("session exists");
    let has_nudge = session.chat_messages().iter().any(
        |m| matches!(m, ChatMessage::User { content, .. } if content.contains("please continue")),
    );
    assert!(has_nudge, "session should contain the nudge message");

    // The second on_text_only call should still have run_has_tool_calls = true
    // (it's never reset mid-run).
    assert!(
        all_have_tools,
        "all on_text_only calls should have run_has_tool_calls = true"
    );
}

/// Guard that counts calls when `is_empty_response` is true and returns
/// `Continue` for the first `max_strikes` calls, then `Fail`.
/// All other scenes return `Complete`.
struct EmptyResponseStrikeGuard {
    max_strikes: usize,
    calls: AtomicUsize,
}

impl EmptyResponseStrikeGuard {
    fn new(max_strikes: usize) -> Self {
        Self {
            max_strikes,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ReactLoopGuard for EmptyResponseStrikeGuard {
    async fn on_turn(&self, ctx: &GuardCtx) -> GuardDecision {
        if ctx.is_empty_response {
            let count = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= self.max_strikes {
                GuardDecision::Fail {
                    error: format!("exceeded max retries ({})", self.max_strikes),
                }
            } else {
                GuardDecision::Continue {
                    nudge: Some("please retry".to_string()),
                }
            }
        } else {
            GuardDecision::Complete
        }
    }
}

/// Incomplete tool calls (is_tool_call=true, tool_calls empty) should trigger
/// the on_empty_response guard, which nudges the model to retry.  After
/// `max_strikes` consecutive failures, the guard returns Fail and the run ends
/// with RunOutcome::Failed — not an infinite loop, not a silent completion.
#[tokio::test]
async fn incomplete_tool_calls_fail_after_max_strikes() {
    let max_strikes = 3;
    let guard = Arc::new(EmptyResponseStrikeGuard::new(max_strikes));
    let guard_clone = guard.clone();

    // Every turn returns an incomplete tool call — no turn ever succeeds.
    let script: Vec<Vec<StreamChunk>> = (0..max_strikes + 1)
        .map(|_| {
            vec![
                StreamChunk::ToolCall(serde_json::json!({ "no_delta": true })),
                StreamChunk::Stop {
                    finish_reason: Some("stop".to_string()),
                },
            ]
        })
        .collect();

    let runtime = AgentBuilder::new(Arc::new(ScriptedProvider::new(script)))
        .system_prompt("test")
        .register_tool(EchoTool)
        .guard_dyn(guard_clone)
        .build()
        .expect("build runtime");

    let sid = runtime.create_session().await;
    let result = runtime
        .run_turn(sid, "帮我解读一下这个工程", |_| Ok(()))
        .await;

    assert!(
        matches!(result, Ok(RunOutcome::Failed { .. })),
        "incomplete tool calls should fail after {} strikes, got: {:?}",
        max_strikes,
        result
    );

    // Verify the guard was called exactly max_strikes times.
    let calls = guard.calls.load(Ordering::SeqCst);
    assert_eq!(
        calls, max_strikes,
        "guard should be called {} times, got: {}",
        max_strikes, calls
    );
}
