use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

use crate::types::{AgentResult, AgentError, ChatMessage, ResponseFormat, ToolCallMessage};
use super::{LlmCapabilities, LlmClient, StreamChunk, UsageInfo};

pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self {
            api_key,
            model,
            base_url: base_url
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            client: Client::new(),
        }
    }

    fn chat_message_to_json(msg: &ChatMessage) -> Value {
        match msg {
            ChatMessage::System { content } => json!({
                "role": "system",
                "content": content,
            }),
            ChatMessage::User { content } => json!({
                "role": "user",
                "content": content,
            }),
            ChatMessage::Assistant { content, reasoning_content, tool_calls } => {
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), json!("assistant"));
                obj.insert("content".to_string(), json!(content));
                if let Some(reasoning) = reasoning_content {
                    obj.insert("reasoning_content".to_string(), json!(reasoning));
                }
                if let Some(tc) = tool_calls {
                    let tool_calls_json: Vec<Value> = tc
                        .iter()
                        .map(|t| Self::tool_call_to_json(t))
                        .collect();
                    obj.insert("tool_calls".to_string(), json!(tool_calls_json));
                }
                Value::Object(obj)
            }
            ChatMessage::Tool { tool_call_id, content } => json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content,
            }),
        }
    }

    fn tool_call_to_json(tc: &ToolCallMessage) -> Value {
        json!({
            "id": tc.id,
            "type": "function",
            "function": {
                "name": tc.name,
                "arguments": tc.arguments,
            }
        })
    }

    fn messages_to_json(messages: &[ChatMessage]) -> Vec<Value> {
        messages.iter().map(Self::chat_message_to_json).collect()
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        enable_thinking: Option<bool>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let url = format!("{}/chat/completions", self.base_url);
        let raw_messages = Self::messages_to_json(messages);
        let mut request_body = json!({
            "model": self.model,
            "messages": raw_messages,
            "tools": tools,
        });

        if let Some(thinking) = enable_thinking {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("enable_thinking".to_string(), json!(thinking));
            }
        }

        if let Some(rf) = response_format {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("response_format".to_string(), rf.to_api_value());
            }
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP 请求失败: {e}")))?;

        let res_json: Value = response.json().await
            .map_err(|e| AgentError::json(format!("响应 JSON 解析失败: {e}")))?;

        if let Some(error) = res_json.get("error") {
            return Err(AgentError::LlmApi {
                message: format!("{error:#?}"),
            });
        }

        Ok(res_json)
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        enable_thinking: Option<bool>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);
        let raw_messages = Self::messages_to_json(messages);
        let mut request_body = json!({
            "model": self.model,
            "messages": raw_messages,
            "tools": tools,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        if let Some(thinking) = enable_thinking {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("enable_thinking".to_string(), json!(thinking));
            }
        }

        if let Some(rf) = response_format {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("response_format".to_string(), rf.to_api_value());
            }
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP 请求失败: {e}")))?;

        if !response.status().is_success() {
            let err_text = response.text().await
                .map_err(|e| AgentError::llm(format!("读取错误响应失败: {e}")))?;
            return Err(AgentError::LlmApi { message: err_text });
        }

        let stream = response.bytes_stream().eventsource().map(|event| match event {
            Ok(event) => {
                if event.data == "[DONE]" {
                    return Ok(StreamChunk::Stop);
                }

                let data: Value = serde_json::from_str(&event.data)
                    .map_err(|e| AgentError::json(format!("JSON Parse error: {e}")))?;

                let choices = data.get("choices").and_then(Value::as_array);

                if choices.is_none() || choices.map_or(true, |c| c.is_empty()) {
                    if let Some(usage) = data.get("usage") {
                        return Ok(StreamChunk::Usage(UsageInfo {
                            prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64).map(|v| v as u32),
                            completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64).map(|v| v as u32),
                            total_tokens: usage.get("total_tokens").and_then(Value::as_u64).map(|v| v as u32),
                        }));
                    }
                    return Ok(StreamChunk::Text(String::new()));
                }

                let choice = &choices.unwrap()[0];
                let delta = &choice["delta"];
                let finish_reason = choice["finish_reason"].as_str().unwrap_or("");

                if finish_reason == "tool_calls" || delta.get("tool_calls").is_some() {
                    return Ok(StreamChunk::ToolCall(choice.clone()));
                }

                if let Some(reasoning) = delta.get("reasoning_content") {
                    if let Some(text) = reasoning.as_str() {
                        return Ok(StreamChunk::Thought(text.to_string()));
                    }
                }

                if let Some(content) = delta.get("content") {
                    if let Some(text) = content.as_str() {
                        return Ok(StreamChunk::Text(text.to_string()));
                    }
                }

                if finish_reason == "stop" {
                    return Ok(StreamChunk::Stop);
                }

                Ok(StreamChunk::Text(String::new()))
            }
            Err(e) => Err(AgentError::LlmStream(format!("SSE Stream error: {e}"))),
        });

        Ok(Box::pin(stream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_thinking: false,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
        }
    }
}
