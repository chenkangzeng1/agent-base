use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::pin::Pin;

use super::{LlmCapabilities, LlmClient, ReasoningConfig, StreamChunk, UsageInfo};
use crate::types::{AgentError, AgentResult, ChatMessage, ImageAttachment, ResponseFormat};

pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self::new_with_config(
            api_key,
            model,
            base_url,
            crate::llm::LlmClientConfig::default(),
        )
    }

    pub fn new_with_config(
        api_key: String,
        model: String,
        base_url: Option<String>,
        config: crate::llm::LlmClientConfig,
    ) -> Self {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.request_timeout)
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(config.pool_idle_timeout)
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to build reqwest client with custom config, falling back to default");
                Client::new()
            });
        Self {
            api_key,
            model,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            client,
        }
    }

    fn convert_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
        let mut system_prompt: Option<String> = None;
        let mut result: Vec<Value> = Vec::new();

        for msg in messages {
            match msg {
                ChatMessage::System { content, .. } => {
                    system_prompt = Some(content.clone());
                }
                ChatMessage::User {
                    content, images, ..
                } => {
                    let mut content_parts: Vec<Value> = Vec::new();
                    content_parts.push(json!({"type": "text", "text": content}));
                    for img in images {
                        match img {
                            ImageAttachment::Url { url, detail: _ } => {
                                content_parts.push(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "url",
                                        "url": url,
                                    }
                                }));
                            }
                            ImageAttachment::Base64 {
                                data,
                                media_type,
                                detail: _,
                            } => {
                                let mime = media_type.as_deref().unwrap_or("image/jpeg");
                                content_parts.push(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": mime,
                                        "data": data,
                                    }
                                }));
                            }
                        }
                    }
                    result.push(json!({
                        "role": "user",
                        "content": content_parts,
                    }));
                }
                ChatMessage::Assistant {
                    content,
                    reasoning_content: _,
                    tool_calls,
                } => {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(text) = content
                        && !text.is_empty()
                    {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    if let Some(tc) = tool_calls {
                        for t in tc {
                            let input: Value =
                                serde_json::from_str(&t.arguments).unwrap_or(Value::Null);
                            parts.push(json!({
                                "type": "tool_use",
                                "id": t.id,
                                "name": t.name,
                                "input": input,
                            }));
                        }
                    }
                    if !parts.is_empty() {
                        result.push(json!({"role": "assistant", "content": parts}));
                    }
                }
                ChatMessage::Tool {
                    tool_call_id,
                    content,
                } => {
                    result.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }]
                    }));
                }
                ChatMessage::Custom { role: _, data } => {
                    // Custom messages are passed as user-role with their data serialized.
                    result.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "text",
                            "text": data.to_string(),
                        }]
                    }));
                }
            }
        }

        (system_prompt, result)
    }

    fn convert_tools(tools: &[Value]) -> Vec<Value> {
        tools
            .iter()
            .filter_map(|tool| {
                let func = tool.get("function")?;
                let name = func.get("name")?.as_str()?;
                let description = func
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let input_schema = func
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"}));
                Some(json!({
                    "name": name,
                    "description": description,
                    "input_schema": input_schema,
                }))
            })
            .collect()
    }

    fn build_body(
        messages: &[ChatMessage],
        tools: &[Value],
        model: &str,
        reasoning: Option<&ReasoningConfig>,
    ) -> Value {
        let (system_prompt, anthropic_messages) = Self::convert_messages(messages);
        let anthropic_tools = Self::convert_tools(tools);

        let mut body = json!({
            "model": model,
            "max_tokens": 8192,
            "messages": anthropic_messages,
        });

        if !anthropic_tools.is_empty()
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("tools".to_string(), json!(anthropic_tools));
        }

        if let Some(system) = system_prompt
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("system".to_string(), json!(system));
        }

        if let Some(config) = reasoning {
            if config.enabled == Some(true) || config.budget_tokens.is_some() {
                let mut thinking = serde_json::Map::new();
                thinking.insert("type".to_string(), json!("enabled"));
                if let Some(budget) = config.budget_tokens {
                    thinking.insert("budget_tokens".to_string(), json!(budget));
                }
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("thinking".to_string(), Value::Object(thinking));
                }
            } else if config.enabled == Some(false) {
                let mut thinking = serde_json::Map::new();
                thinking.insert("type".to_string(), json!("disabled"));
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("thinking".to_string(), Value::Object(thinking));
                }
            }
        }

        body
    }

    fn parse_sse(data_str: &str, event_type: &str) -> AgentResult<StreamChunk> {
        if data_str.is_empty() {
            return Ok(StreamChunk::Text(String::new()));
        }

        let data: Value = serde_json::from_str(data_str)
            .map_err(|e| AgentError::json(format!("Anthropic SSE JSON: {e}")))?;

        match event_type {
            "message_start" => {
                let input_tokens = data
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let output_tokens = data
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                Ok(StreamChunk::Usage(UsageInfo {
                    prompt_tokens: input_tokens,
                    completion_tokens: output_tokens,
                    total_tokens: None,
                }))
            }
            "content_block_start" => {
                let cb = data.get("content_block");
                let idx = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(cb) = cb
                    && cb.get("type").and_then(Value::as_str) == Some("tool_use")
                {
                    let id = cb
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = cb
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok(StreamChunk::ToolCall(json!({
                        "delta": {
                            "tool_calls": [{
                                "index": idx,
                                "id": if id.is_empty() { Value::Null } else { json!(id) },
                                "function": {
                                    "name": name,
                                    "arguments": "",
                                }
                            }]
                        }
                    })));
                }
                Ok(StreamChunk::Text(String::new()))
            }
            "content_block_delta" => {
                let delta = data.get("delta");
                let idx = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(d) = delta {
                    match d.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = d
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            Ok(StreamChunk::Text(text))
                        }
                        Some("input_json_delta") => {
                            let partial = d
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            Ok(StreamChunk::ToolCall(json!({
                                "delta": {
                                    "tool_calls": [{
                                        "index": idx,
                                        "function": {
                                            "arguments": partial,
                                        }
                                    }]
                                }
                            })))
                        }
                        Some("thinking_delta") => {
                            let thinking = d
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            Ok(StreamChunk::Thought(thinking))
                        }
                        _ => Ok(StreamChunk::Text(String::new())),
                    }
                } else {
                    Ok(StreamChunk::Text(String::new()))
                }
            }
            "content_block_stop" => Ok(StreamChunk::Text(String::new())),
            "message_delta" => {
                let output_tokens = data
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                Ok(StreamChunk::Usage(UsageInfo {
                    prompt_tokens: None,
                    completion_tokens: output_tokens,
                    total_tokens: None,
                }))
            }
            "message_stop" => {
                let finish_reason = data
                    .get("message")
                    .and_then(|m| m.get("stop_reason"))
                    .and_then(Value::as_str)
                    .map(String::from);
                Ok(StreamChunk::Stop { finish_reason })
            }
            "ping" => Ok(StreamChunk::Text(String::new())),
            _ => Ok(StreamChunk::Text(String::new())),
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = Self::build_body(messages, tools, &self.model, reasoning);
        tracing::debug!(model = %self.model, url = %url, body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "Anthropic chat request");

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP request failed: {e}")))?;

        let status = response.status();
        let res_json: Value = response
            .json()
            .await
            .map_err(|e| AgentError::json(format!("Response JSON parse failed: {e}")))?;

        if !status.is_success() {
            let err_msg = res_json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            tracing::warn!(status = %status, error = %err_msg, "Anthropic API non-success");
            return Err(AgentError::LlmApi {
                message: err_msg.to_string(),
            });
        }

        tracing::debug!(status = %status, "Anthropic chat response received");
        Ok(res_json)
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut body = Self::build_body(messages, tools, &self.model, reasoning);

        if let Some(obj) = body.as_object_mut() {
            obj.insert("stream".to_string(), json!(true));
        }
        tracing::debug!(model = %self.model, url = %url, body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "Anthropic chat_stream request");

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let err_text = response
                .text()
                .await
                .map_err(|e| AgentError::llm(format!("Failed to read error response: {e}")))?;
            tracing::warn!(%status, error = %err_text, "Anthropic API stream non-success");
            return Err(AgentError::LlmApi { message: err_text });
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(|event| async move {
                match event {
                    Ok(ref ev) if ev.event == "error" => {
                        let err_msg = ev.data.clone();
                        Some(Err(AgentError::LlmApi { message: err_msg }))
                    }
                    Ok(ev) => {
                        // eventsource-stream normalizes an empty SSE `event:`
                        // field to "message", so `ev.event` is never empty here.
                        let event_type = ev.event.as_str();
                        match Self::parse_sse(&ev.data, event_type) {
                            Ok(chunk) => Some(Ok(chunk)),
                            Err(e) => Some(Err(e)),
                        }
                    }
                    Err(e) => Some(Err(AgentError::LlmStream(format!("SSE Stream error: {e}")))),
                }
            });

        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_thinking: true,
            max_context_tokens: Some(200_000),
            max_output_tokens: Some(8_192),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl super::StreamClient for AnthropicClient {
    async fn stream(
        &self,
        messages: &[crate::types::ChatMessage],
        tools: &[serde_json::Value],
        reasoning: Option<&super::ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
    ) -> crate::types::AgentResult<
        std::pin::Pin<
            Box<
                dyn futures_core::Stream<Item = crate::types::AgentResult<super::StreamChunk>>
                    + Send,
            >,
        >,
    > {
        <Self as super::LlmClient>::chat_stream(self, messages, tools, reasoning, response_format)
            .await
    }

    fn capabilities(&self) -> super::LlmCapabilities {
        <Self as super::LlmClient>::capabilities(self)
    }

    fn model_name(&self) -> &str {
        <Self as super::LlmClient>::model_name(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ImageAttachment};
    use futures_util::TryStreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- pure helpers ----

    #[test]
    fn convert_messages_handles_all_variants() {
        let msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::user_with_images(
                "pic",
                vec![
                    ImageAttachment::Url {
                        url: "http://x/a.png".into(),
                        detail: None,
                    },
                    ImageAttachment::Base64 {
                        data: "abc".into(),
                        media_type: Some("image/png".into()),
                        detail: None,
                    },
                ],
            ),
            ChatMessage::assistant("hi back"),
            ChatMessage::assistant_tool_call("tc1", "echo", "{\"x\":1}"),
            ChatMessage::tool("tc1", "done"),
            ChatMessage::Custom {
                role: "artifact".into(),
                data: serde_json::json!({"x": 1}),
            },
        ];

        let (sys, out) = AnthropicClient::convert_messages(&msgs);
        assert_eq!(sys.as_deref(), Some("sys"));
        assert_eq!(out.len(), 6);

        assert_eq!(
            out[0],
            serde_json::json!({"role": "user", "content": [{"type": "text", "text": "hi"}]})
        );

        assert_eq!(out[1]["role"], "user");
        assert_eq!(
            out[1]["content"][0],
            serde_json::json!({"type": "text", "text": "pic"})
        );
        assert_eq!(out[1]["content"][1]["type"], "image");
        assert_eq!(out[1]["content"][1]["source"]["type"], "url");
        assert_eq!(out[1]["content"][2]["source"]["type"], "base64");
        assert_eq!(out[1]["content"][2]["source"]["media_type"], "image/png");

        assert_eq!(
            out[2],
            serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "hi back"}]})
        );

        assert_eq!(out[3]["role"], "assistant");
        assert_eq!(out[3]["content"][0]["type"], "tool_use");
        assert_eq!(out[3]["content"][0]["id"], "tc1");
        assert_eq!(out[3]["content"][0]["name"], "echo");
        assert_eq!(out[3]["content"][0]["input"], serde_json::json!({"x": 1}));

        assert_eq!(out[4]["role"], "user");
        assert_eq!(out[4]["content"][0]["type"], "tool_result");
        assert_eq!(out[4]["content"][0]["tool_use_id"], "tc1");
        assert_eq!(out[4]["content"][0]["content"], "done");

        assert_eq!(out[5]["role"], "user");
        assert_eq!(out[5]["content"][0]["type"], "text");
        assert_eq!(out[5]["content"][0]["text"], "{\"x\":1}");
    }

    #[test]
    fn convert_tools_maps_openai_to_anthropic() {
        let tools = vec![
            serde_json::json!({"type": "function", "function": {"name": "echo", "description": "echo back", "parameters": {"type": "object", "properties": {}}}}),
            serde_json::json!({"function": {"name": "bare"}}),
            serde_json::json!({"type": "function"}),
            serde_json::json!({"function": {"description": "no name"}}),
        ];
        let out = AnthropicClient::convert_tools(&tools);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], "echo");
        assert_eq!(out[0]["description"], "echo back");
        assert_eq!(out[0]["input_schema"]["type"], "object");
        assert_eq!(out[1]["name"], "bare");
        assert_eq!(out[1]["description"], "");
        assert_eq!(
            out[1]["input_schema"],
            serde_json::json!({"type": "object"})
        );
    }

    #[test]
    fn build_body_basic() {
        let body =
            AnthropicClient::build_body(&[ChatMessage::user("hi")], &[], "claude-sonnet", None);
        assert_eq!(body["model"], "claude-sonnet");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_body_includes_system_and_tools() {
        let msgs = vec![ChatMessage::system("sys"), ChatMessage::user("hi")];
        let tools = vec![serde_json::json!({"function": {"name": "echo"}})];
        let body = AnthropicClient::build_body(&msgs, &tools, "m", None);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["tools"][0]["name"], "echo");
    }

    #[test]
    fn build_body_reasoning_variants() {
        let msgs = vec![ChatMessage::user("hi")];

        let rc = ReasoningConfig {
            enabled: Some(true),
            budget_tokens: None,
            effort: None,
        };
        let body = AnthropicClient::build_body(&msgs, &[], "m", Some(&rc));
        assert_eq!(body["thinking"]["type"], "enabled");

        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: Some(500),
            effort: None,
        };
        let body = AnthropicClient::build_body(&msgs, &[], "m", Some(&rc));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 500);

        let rc = ReasoningConfig {
            enabled: Some(false),
            budget_tokens: None,
            effort: None,
        };
        let body = AnthropicClient::build_body(&msgs, &[], "m", Some(&rc));
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn parse_sse_empty_data() {
        let c = AnthropicClient::parse_sse("", "message_start").unwrap();
        assert!(matches!(c, StreamChunk::Text(t) if t.is_empty()));
    }

    #[test]
    fn parse_sse_message_start_usage() {
        let c = AnthropicClient::parse_sse(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "message_start",
        )
        .unwrap();
        assert!(
            matches!(c, StreamChunk::Usage(u) if u.prompt_tokens == Some(10) && u.completion_tokens == Some(5))
        );
    }

    #[test]
    fn parse_sse_content_block_start_tool_use() {
        let c = AnthropicClient::parse_sse(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"echo"}}"#,
            "content_block_start",
        )
        .unwrap();
        match c {
            StreamChunk::ToolCall(v) => {
                assert_eq!(v["delta"]["tool_calls"][0]["index"], 0);
                assert_eq!(v["delta"]["tool_calls"][0]["id"], "toolu_1");
                assert_eq!(v["delta"]["tool_calls"][0]["function"]["name"], "echo");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_content_block_start_text() {
        let c = AnthropicClient::parse_sse(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "content_block_start",
        )
        .unwrap();
        assert!(matches!(c, StreamChunk::Text(t) if t.is_empty()));
    }

    #[test]
    fn parse_sse_content_block_delta_variants() {
        let text = AnthropicClient::parse_sse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            "content_block_delta",
        )
        .unwrap();
        assert!(matches!(text, StreamChunk::Text(t) if t == "hello"));

        let tj = AnthropicClient::parse_sse(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"x=1"}}"#,
            "content_block_delta",
        )
        .unwrap();
        match tj {
            StreamChunk::ToolCall(v) => {
                assert_eq!(v["delta"]["tool_calls"][0]["index"], 1);
                assert_eq!(v["delta"]["tool_calls"][0]["function"]["arguments"], "x=1");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }

        let th = AnthropicClient::parse_sse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            "content_block_delta",
        )
        .unwrap();
        assert!(matches!(th, StreamChunk::Thought(t) if t == "hmm"));

        let unk = AnthropicClient::parse_sse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"whatever"}}"#,
            "content_block_delta",
        )
        .unwrap();
        assert!(matches!(unk, StreamChunk::Text(t) if t.is_empty()));

        let nd = AnthropicClient::parse_sse(
            r#"{"type":"content_block_delta","index":0}"#,
            "content_block_delta",
        )
        .unwrap();
        assert!(matches!(nd, StreamChunk::Text(t) if t.is_empty()));
    }

    #[test]
    fn parse_sse_message_delta_usage() {
        let c = AnthropicClient::parse_sse(
            r#"{"type":"message_delta","usage":{"output_tokens":12}}"#,
            "message_delta",
        )
        .unwrap();
        assert!(
            matches!(c, StreamChunk::Usage(u) if u.completion_tokens == Some(12) && u.prompt_tokens.is_none())
        );
    }

    #[test]
    fn parse_sse_message_stop() {
        let c = AnthropicClient::parse_sse(
            r#"{"type":"message_stop","message":{"stop_reason":"end_turn"}}"#,
            "message_stop",
        )
        .unwrap();
        assert!(matches!(c, StreamChunk::Stop { finish_reason: Some(r) } if r == "end_turn"));

        let c = AnthropicClient::parse_sse(r#"{"type":"message_stop"}"#, "message_stop").unwrap();
        assert!(matches!(
            c,
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[test]
    fn parse_sse_ping_unknown_and_invalid() {
        assert!(matches!(
            AnthropicClient::parse_sse(r#"{"type":"ping"}"#, "ping").unwrap(),
            StreamChunk::Text(_)
        ));
        assert!(matches!(
            AnthropicClient::parse_sse(r#"{}"#, "something_else").unwrap(),
            StreamChunk::Text(_)
        ));
        assert!(AnthropicClient::parse_sse("not-json", "message_stop").is_err());
    }

    #[test]
    fn capabilities_and_model_name() {
        let c = AnthropicClient::new("k".into(), "claude-sonnet".into(), None);
        let caps = c.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_thinking);
        assert_eq!(caps.max_context_tokens, Some(200_000));
        assert_eq!(caps.max_output_tokens, Some(8_192));
        assert_eq!(c.model_name(), "claude-sonnet");
    }

    // ---- mock HTTP ----

    #[tokio::test]
    async fn chat_posts_and_parses_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "content": [{"type": "text", "text": "hello"}],
            })))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        assert_eq!(resp["id"], "msg_1");
    }

    #[tokio::test]
    async fn chat_returns_llm_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": "bad request"},
            })))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("bad".into(), "claude-sonnet".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hi")], &[], None, None)
            .await;
        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn chat_stream_parses_full_event_sequence() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"echo\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"abc\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":12}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\",\"message\":{\"stop_reason\":\"end_turn\"}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(
            matches!(&chunks[0], StreamChunk::Usage(u) if u.prompt_tokens == Some(10) && u.completion_tokens == Some(5))
        );
        assert!(matches!(&chunks[1], StreamChunk::ToolCall(_)));
        assert!(matches!(&chunks[2], StreamChunk::ToolCall(_)));
        assert!(matches!(&chunks[3], StreamChunk::Text(t) if t == "hello"));
        assert!(matches!(&chunks[4], StreamChunk::Thought(t) if t == "hmm"));
        assert!(matches!(&chunks[5], StreamChunk::Usage(u) if u.completion_tokens == Some(12)));
        assert!(
            matches!(&chunks[6], StreamChunk::Stop { finish_reason: Some(r) } if r == "end_turn")
        );
    }

    #[tokio::test]
    async fn chat_stream_returns_error_on_error_event() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }

    // ── B2: adapter delegation + remaining HTTP/SSE edges ────────────────

    #[test]
    fn stream_client_delegates_capabilities_and_model_name() {
        let c = AnthropicClient::new("k".into(), "claude-sonnet".into(), None);
        let caps = <AnthropicClient as crate::llm::StreamClient>::capabilities(&c);
        assert!(caps.supports_streaming);
        assert_eq!(
            <AnthropicClient as crate::llm::StreamClient>::model_name(&c),
            "claude-sonnet"
        );
    }

    #[tokio::test]
    async fn stream_client_stream_delegates_to_chat_stream() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let stream = <AnthropicClient as crate::llm::StreamClient>::stream(
            &client,
            &[ChatMessage::user("hi")],
            &[],
            None,
            None,
        )
        .await
        .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "hi"));
    }

    #[tokio::test]
    async fn chat_errors_on_non_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hi")], &[], None, None)
            .await;
        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn chat_stream_returns_error_on_non_success_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let result = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn chat_stream_wraps_parse_error_as_err() {
        let server = MockServer::start().await;
        let sse = concat!("event: message_stop\n", "data: not-json\n\n");
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = AnthropicClient::new("k".into(), "claude-sonnet".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }
}
