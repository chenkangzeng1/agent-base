//! Integration tests for the bridge protocol (ProtocolServer).
//!
//! Tests protocol flow end-to-end using a mock LLM client — no real API key needed.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use agent_base::{AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ResponseFormat, StreamChunk};
use async_trait::async_trait;
use futures_core::Stream;
use phi_agent::bridge::messages::PROTOCOL_VERSION;
use phi_agent::bridge::server::ProtocolServer;
use phi_agent::{
    ApprovalMode, AutoApprovalHandler, SafetyConfig, TurnFactMiddleware, TurnToolLimitMiddleware, base_agent_builder,
    build_system_prompt,
};
use serde_json::{Value, json};

// ── Mock LLM ──────────────────────────────────────────────────────────

struct EmptyStream;

impl Stream for EmptyStream {
    type Item = AgentResult<StreamChunk>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

/// A mock LLM client that can be programmed to return tool calls or text.
struct MockLlmClient {
    /// If set, the next `chat()` call returns a tool-call response.
    tool_call_response: tokio::sync::Mutex<Option<(String, String)>>,
    text_response: String,
}

impl MockLlmClient {
    fn new() -> Self {
        Self { tool_call_response: tokio::sync::Mutex::new(None), text_response: "mock response".to_string() }
    }

    #[allow(dead_code)]
    async fn set_tool_call(&self, name: &str, args: &Value) {
        *self.tool_call_response.lock().await = Some((name.to_string(), args.to_string()));
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let tc = self.tool_call_response.lock().await.take();
        if let Some((name, args)) = tc {
            Ok(json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": "call-test-1",
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }]
                    }
                }]
            }))
        } else {
            Ok(json!({
                "choices": [{"message": {"content": self.text_response.clone()}}]
            }))
        }
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        Ok(Box::pin(EmptyStream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: true,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn build_server(mock: Arc<MockLlmClient>) -> ProtocolServer {
    let builder = base_agent_builder(mock)
        .system_prompt(build_system_prompt())
        .approval_handler(Arc::new(AutoApprovalHandler::new(ApprovalMode::Auto)))
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&SafetyConfig::default()));
    ProtocolServer::from_builder(builder).expect("build server")
}

fn event_type(event: &phi_agent::RuntimeEvent) -> &'static str {
    match event {
        phi_agent::RuntimeEvent::TextDelta { .. } => "text_delta",
        phi_agent::RuntimeEvent::ThoughtDelta { .. } => "thought_delta",
        phi_agent::RuntimeEvent::ToolCallStarted { .. } => "tool_call_started",
        phi_agent::RuntimeEvent::ToolCallFinished { .. } => "tool_call_finished",
        phi_agent::RuntimeEvent::RunFinished { .. } => "run_finished",
        phi_agent::RuntimeEvent::RunCancelled { .. } => "run_cancelled",
        _ => "other",
    }
}

async fn collect_events(event_rx: &mut tokio::sync::broadcast::Receiver<phi_agent::RuntimeEvent>) -> Vec<String> {
    let mut events = Vec::new();
    loop {
        tokio::select! {
            event_result = event_rx.recv() => {
                match event_result {
                    Ok(event) => {
                        let typ = event_type(&event);
                        events.push(typ.to_string());
                        if typ == "run_finished" || typ == "run_cancelled" {
                            return events;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return events,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                return events;
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_run_returns_events() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let sid = server.create_session(None).await.0;
    let mut event_rx = server.subscribe_events();

    // Spawn the turn
    let server_clone = server.clone();
    tokio::spawn(async move {
        let _ = server_clone.run_turn(&sid, "hello", |_event| Ok(())).await;
    });

    let events = collect_events(&mut event_rx).await;
    assert!(!events.is_empty(), "should receive at least one event");
    assert!(events.contains(&"run_finished".to_string()), "should finish: {events:?}");
}

#[tokio::test]
async fn test_create_session_and_subscribe() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let (sid, ext) = server.create_session(Some("my-session".to_string())).await;
    assert_eq!(ext.as_deref(), Some("my-session"));
    assert!(sid.id > 0);
}

#[test]
fn test_protocol_version_is_1() {
    assert_eq!(PROTOCOL_VERSION, 1);
}

// ── BR-04, BR-05, BR-06 ─────────────────────────────────────────────

/// BR-04: ProxyTool called without a prepared slot returns error, not panic.
///
/// When the LLM requests a tool call but no slot has been prepared
/// (single-slot is None), ProxyTool should return a clean error
/// instead of panicking.
#[tokio::test]
async fn test_br_04_empty_slot_returns_error() {
    let mock = Arc::new(MockLlmClient::new());
    // Pre-configure mock to return a tool call
    mock.set_tool_call("test_tool", &json!({"arg": "value"})).await;

    let server = build_server(mock.clone());
    server
        .register_tool("test_tool".to_string(), "A test tool".to_string(), json!({}))
        .await;

    let (sid, _) = server.create_session(None).await;
    let mut event_rx = server.subscribe_events();

    // Deliberately do NOT prepare a slot — tool call should fail gracefully

    let server_clone = server.clone();
    let sid_clone = sid.clone();
    tokio::spawn(async move {
        let _ = server_clone.run_turn(&sid_clone, "call the tool", |_event| Ok(())).await;
    });

    let events = collect_events(&mut event_rx).await;
    // Should still finish (run_finished) — the tool error is handled,
    // not a panic
    assert!(
        events.contains(&"run_finished".to_string()),
        "should finish even with empty slot: {events:?}"
    );
}

/// BR-05: Session ID reuse — same external_id returns same session.
///
/// ``get_or_create_session`` with the same external_id should return
/// the same underlying SessionId, preserving conversation context.
#[tokio::test]
async fn test_br_05_session_id_reuse() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    // First call creates a new session
    let sid1 = server
        .get_or_create_session(Some("shared-session".to_string()))
        .await;

    // Second call with same external_id should return the SAME session
    let sid2 = server
        .get_or_create_session(Some("shared-session".to_string()))
        .await;

    assert_eq!(sid1.id, sid2.id, "same external_id should reuse session");

    // Different external_id should create a NEW session
    let sid3 = server
        .get_or_create_session(Some("other-session".to_string()))
        .await;

    assert_ne!(sid3.id, sid1.id, "different external_id should create new session");

    // None (no external_id) should always create new sessions
    let sid4 = server.get_or_create_session(None).await;
    let sid5 = server.get_or_create_session(None).await;
    assert_ne!(sid4.id, sid5.id, "None external_id should always create new");
}

/// BR-06: Sequential tool calls don't interfere.
///
/// Multiple sequential tool calls (prepare → call → prepare → call)
/// should work correctly without cross-talk.  The single-slot pattern
/// handles one at a time; this test verifies the slot is properly
/// reset between calls.
#[tokio::test]
async fn test_br_06_sequential_tool_calls() {
    let mock = Arc::new(MockLlmClient::new());

    // First tool call: test_tool
    mock.set_tool_call("test_tool", &json!({"step": 1})).await;

    let server = build_server(mock.clone());
    server
        .register_tool("test_tool".to_string(), "Tool 1".to_string(), json!({}))
        .await;

    let sid = server.get_or_create_session(None).await;
    let mut event_rx = server.subscribe_events();

    // Prepare slot for the first tool call
    let _tx1 = server.prepare_tool_call().await;

    let server_clone = server.clone();
    let sid_clone = sid.clone();
    tokio::spawn(async move {
        let _ = server_clone.run_turn(&sid_clone, "call tool 1", |_event| Ok(())).await;
    });

    let events1 = collect_events(&mut event_rx).await;
    assert!(
        events1.contains(&"run_finished".to_string()),
        "first turn should finish: {events1:?}"
    );

    // Second tool call — should work even after the first consumed the slot
    mock.set_tool_call("test_tool", &json!({"step": 2})).await;

    let mut event_rx2 = server.subscribe_events();
    let _tx2 = server.prepare_tool_call().await;

    let sid_clone2 = sid.clone();
    let server_clone2 = server.clone();
    tokio::spawn(async move {
        let _ = server_clone2.run_turn(&sid_clone2, "call tool again", |_event| Ok(())).await;
    });

    let events2 = collect_events(&mut event_rx2).await;
    assert!(
        events2.contains(&"run_finished".to_string()),
        "second turn should finish: {events2:?}"
    );

    // Verify both turns completed — sequential calls don't interfere
    let finished1 = events1.iter().any(|t| t == "run_finished");
    let finished2 = events2.iter().any(|t| t == "run_finished");
    assert!(finished1, "first turn should finish: {events1:?}");
    assert!(finished2, "second turn should finish: {events2:?}");
}

// ── ProtocolServer unit tests ──

#[tokio::test]
async fn test_register_tool_appears_in_list() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    server.register_tool("my_tool".into(), "A test tool".into(), json!({})).await;

    let tools = server.list_tools().await;
    assert!(tools.iter().any(|t| t.name == "my_tool"));
}

#[tokio::test]
async fn test_register_multiple_tools() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    server.register_tool("zzz_tool".into(), "Z".into(), json!({})).await;
    server.register_tool("aaa_tool".into(), "A".into(), json!({})).await;
    server.register_tool("mmm_tool".into(), "M".into(), json!({})).await;

    let tools = server.list_tools().await;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    // Tools should be sorted by name
    assert!(tools.len() >= 3);
    assert_eq!(names[0], "aaa_tool");
    assert_eq!(names[1], "mmm_tool");
    assert_eq!(names[2], "zzz_tool");
}

#[tokio::test]
async fn test_prepare_tool_call_sender_usable() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let tx = server.prepare_tool_call().await;
    // Sender should be usable
    let result = tx.send(Ok(agent_base::ToolOutput {
        summary: "done".into(),
        raw: None,
        control_flow: agent_base::ToolControlFlow::Continue,
        truncation: None,
    }));
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_subscribe_events_receiver_open() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let rx = server.subscribe_events();
    // Receiver should not be closed initially
    assert_eq!(rx.len(), 0);
}

#[tokio::test]
async fn test_get_or_create_different_external_ids() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let sid1 = server.get_or_create_session(Some("ext-1".into())).await;
    let sid2 = server.get_or_create_session(Some("ext-2".into())).await;

    assert_ne!(sid1.id, sid2.id, "different external_ids should create different sessions");
}

#[tokio::test]
async fn test_create_session_without_external_id() {
    let mock = Arc::new(MockLlmClient::new());
    let server = build_server(mock);

    let sid = server.get_or_create_session(None).await;
    assert!(sid.id > 0);
    assert!(sid.external_id.is_none());
}
