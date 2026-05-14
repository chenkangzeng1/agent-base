use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

use crate::types::{AgentResult, AgentError, ChatMessage, ResponseFormat};
use super::{LlmCapabilities, LlmClient, StreamChunk, UsageInfo};

pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self {
            api_key,
            model,
            base_url: base_url
                .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            client: Client::new(),
        }
    }

    fn convert_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
        let mut system_prompt: Option<String> = None;
        let mut result: Vec<Value> = Vec::new();

        for msg in messages {
            match msg {
                ChatMessage::System { content } => {
                    system_prompt = Some(content.clone());
                }
                ChatMessage::User { content } => {
                    result.push(json!({
                        "role": "user",
                        "content": [{ "type": "text", "text": content }],
                    }));
                }
                ChatMessage::Assistant { content, reasoning_content: _, tool_calls } => {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(text) = content {
                        if !text.is_empty() {
                            parts.push(json!({"type": "text", "text": text}));
                        }
                    }
                    if let Some(tc) = tool_calls {
                        for t in tc {
                            let input: Value = serde_json::from_str(&t.arguments)
                                .unwrap_or(Value::Null);
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
                ChatMessage::Tool { tool_call_id, content } => {
                    result.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
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
                let description = func.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let input_schema = func.get("parameters")
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
    ) -> Value {
        let (system_prompt, anthropic_messages) = Self::convert_messages(messages);
        let anthropic_tools = Self::convert_tools(tools);

        let mut body = json!({
            "model": model,
            "max_tokens": 8192,
            "messages": anthropic_messages,
        });

        if !anthropic_tools.is_empty() {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("tools".to_string(), json!(anthropic_tools));
            }
        }

        if let Some(system) = system_prompt {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("system".to_string(), json!(system));
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
                if let Some(cb) = cb {
                    if cb.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let id = cb.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = cb.get("name").and_then(Value::as_str).unwrap_or("").to_string();
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
                }
                Ok(StreamChunk::Text(String::new()))
            }
            "content_block_delta" => {
                let delta = data.get("delta");
                let idx = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(d) = delta {
                    match d.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = d.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                            Ok(StreamChunk::Text(text))
                        }
                        Some("input_json_delta") => {
                            let partial = d.get("partial_json").and_then(Value::as_str).unwrap_or("").to_string();
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
                            let thinking = d.get("thinking").and_then(Value::as_str).unwrap_or("").to_string();
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
            "message_stop" => Ok(StreamChunk::Stop),
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
        _enable_thinking: Option<bool>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = Self::build_body(messages, tools, &self.model);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP 请求失败: {e}")))?;

        let status = response.status();
        let res_json: Value = response.json().await
            .map_err(|e| AgentError::json(format!("响应 JSON 解析失败: {e}")))?;

        if !status.is_success() {
            let err_msg = res_json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(AgentError::LlmApi {
                message: err_msg.to_string(),
            });
        }

        Ok(res_json)
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        _enable_thinking: Option<bool>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut body = Self::build_body(messages, tools, &self.model);

        if let Some(obj) = body.as_object_mut() {
            obj.insert("stream".to_string(), json!(true));
        }

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP 请求失败: {e}")))?;

        if !response.status().is_success() {
            let err_text = response.text().await
                .map_err(|e| AgentError::llm(format!("读取错误响应失败: {e}")))?;
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
                        let event_type = if ev.event.is_empty() { "message_stop" } else { ev.event.as_str() };
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
}
