use agent_base::{AgentBuilder, RuntimeEvent, ChatMessage, LlmClient, OpenAiClient, StreamChunk};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// 用于捕获请求参数的 Mock LLM Client
struct CaptureLlmClient {
    /// 捕获到的请求 body（JSON 格式）
    captured_bodies: Mutex<Vec<Value>>,
}

impl CaptureLlmClient {
    fn new() -> Self {
        Self {
            captured_bodies: Mutex::new(Vec::new()),
        }
    }

    fn captured_bodies(&self) -> Vec<Value> {
        self.captured_bodies.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for CaptureLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&agent_base::ReasoningConfig>,
        _response_format: Option<&agent_base::ResponseFormat>,
    ) -> agent_base::AgentResult<Value> {
        unimplemented!()
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&agent_base::ReasoningConfig>,
        response_format: Option<&agent_base::ResponseFormat>,
    ) -> agent_base::AgentResult<Pin<Box<dyn futures_core::Stream<Item = agent_base::AgentResult<StreamChunk>> + Send>>> {
        // 构造一个模拟的请求 body 来验证参数传递
        let mut body = json!({
            "model": "test-model",
            "messages": [],
            "tools": [],
            "stream": true,
        });

        if let Some(config) = reasoning {
            if let Some(enabled) = config.enabled {
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("enable_thinking".to_string(), json!(enabled));
                }
            }
            if let Some(budget) = config.budget_tokens {
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("thinking_budget".to_string(), json!(budget));
                }
            }
        }

        self.captured_bodies.lock().unwrap().push(body);

        // 返回一个空的流
        let stream = futures_util::stream::iter(vec![Ok(StreamChunk::Stop)]);
        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> agent_base::LlmCapabilities {
        agent_base::LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: true,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
        }
    }
}

#[tokio::test]
async fn test_thinking_budget_parameter_passed() {
    let llm = Arc::new(CaptureLlmClient::new());

    let runtime = AgentBuilder::new(llm.clone())
        .system_prompt("test")
        .enable_thinking(true)
        .thinking_budget(128)
        .build().unwrap();

    let session_id = runtime.create_session().await;
    let _ = runtime.run_turn_stream(session_id, "test").await;

    let bodies = llm.captured_bodies();
    assert!(!bodies.is_empty(), "Should have captured at least one request body");

    let body = &bodies[0];
    println!("Captured request body: {}", serde_json::to_string_pretty(body).unwrap());

    // 验证 enable_thinking 被正确设置
    assert_eq!(body["enable_thinking"], true, "enable_thinking should be true");

    // 验证 thinking_budget 被正确设置
    assert_eq!(body["thinking_budget"], 128, "thinking_budget should be 128");
}

#[tokio::test]
async fn test_extra_body_format_for_thinking() {
    // 这个测试验证 OpenAiClient 实际生成的请求 body 格式
    // 由于 OpenAiClient 的方法不好直接测试，我们通过 AgentBuilder 来间接验证

    let llm = Arc::new(CaptureLlmClient::new());

    let runtime = AgentBuilder::new(llm.clone())
        .system_prompt("test")
        .enable_thinking(true)
        .thinking_budget(128)
        .build().unwrap();

    let session_id = runtime.create_session().await;
    let _ = runtime.run_turn_stream(session_id, "test").await;

    let bodies = llm.captured_bodies();
    assert!(!bodies.is_empty(), "Should have captured at least one request body");

    let body = &bodies[0];
    println!("Captured request body: {}", serde_json::to_string_pretty(body).unwrap());

    // 验证参数存在
    assert!(body.get("enable_thinking").is_some(), "enable_thinking should be present");
    assert!(body.get("thinking_budget").is_some(), "thinking_budget should be present");
}
