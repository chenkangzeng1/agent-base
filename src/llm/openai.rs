use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use std::pin::Pin;

use crate::types::AgentResult;
use super::{LlmClient, StreamChunk};

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
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat(
        &self,
        messages: &[Value],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Value> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request_body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
        });
        
        if let Some(thinking) = enable_thinking {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("enable_thinking".to_string(), json!(thinking));
            }
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        let res_json: Value = response.json().await?;

        if let Some(error) = res_json.get("error") {
            anyhow::bail!("API Error: {:#?}", error);
        }

        Ok(res_json)
    }

    async fn chat_stream(
        &self,
        messages: &[Value],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request_body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "stream": true,
        });
        
        if let Some(thinking) = enable_thinking {
            if let Some(obj) = request_body.as_object_mut() {
                obj.insert("enable_thinking".to_string(), json!(thinking));
            }
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let err_text = response.text().await?;
            anyhow::bail!("API Request Failed: {}", err_text);
        }

        let stream = response.bytes_stream().eventsource().map(|event| match event {
            Ok(event) => {
                if event.data == "[DONE]" {
                    return Ok(StreamChunk::Stop);
                }

                let data: Value = serde_json::from_str(&event.data)
                    .map_err(|e| anyhow::anyhow!("JSON Parse error: {}", e))?;

                let choice = &data["choices"][0];
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

                Ok(StreamChunk::Text("".to_string()))
            }
            Err(e) => Err(anyhow::anyhow!("SSE Stream error: {}", e)),
        });

        Ok(Box::pin(stream))
    }
}
