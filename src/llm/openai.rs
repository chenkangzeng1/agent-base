use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::pin::Pin;
use std::time::Duration;

use super::{LlmCapabilities, LlmClient, ReasoningConfig, ReasoningEffort, StreamChunk, UsageInfo};
use crate::types::{
    AgentError, AgentResult, ChatMessage, ImageAttachment, ImageDetail, ResponseFormat,
    ToolCallMessage,
};

#[derive(Clone, Debug)]
pub struct LlmClientConfig {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout: Duration,
}

impl Default for LlmClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(120),
            pool_max_idle_per_host: 10,
            pool_idle_timeout: Duration::from_secs(90),
        }
    }
}

pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self::new_with_config(api_key, model, base_url, LlmClientConfig::default())
    }

    pub fn new_with_config(
        api_key: String,
        model: String,
        base_url: Option<String>,
        config: LlmClientConfig,
    ) -> Self {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
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
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            client,
        }
    }

    /// 使用不同模型的变体。共享底层 HTTP 连接池，零额外成本。
    ///
    /// 类似 Claude Code 的 opus/sonnet/haiku — 同一 client，不同 model。
    pub fn with_model(&self, model: impl Into<String>) -> Self {
        Self {
            api_key: self.api_key.clone(),
            model: model.into(),
            base_url: self.base_url.clone(),
            client: self.client.clone(), // reqwest::Client 内部是 Arc
        }
    }

    fn is_qwen_model(&self) -> bool {
        self.model.starts_with("qwen")
    }

    fn is_deepseek_model(&self) -> bool {
        self.model.starts_with("deepseek")
    }

    fn apply_reasoning_config(
        &self,
        request_body: &mut Value,
        reasoning: Option<&ReasoningConfig>,
    ) {
        let Some(config) = reasoning else { return };

        if self.is_qwen_model() {
            // qwen 模型使用 enable_thinking 和 thinking_budget
            // 对于 OpenAI 兼容接口，直接放在请求体顶层
            if let Some(enabled) = config.enabled
                && let Some(obj) = request_body.as_object_mut()
            {
                obj.insert("enable_thinking".to_string(), json!(enabled));
            }
            if let Some(budget) = config.budget_tokens
                && let Some(obj) = request_body.as_object_mut()
            {
                obj.insert("thinking_budget".to_string(), json!(budget));
            }
            // 将 effort 转换为 thinking_budget
            if let Some(effort) = &config.effort {
                let budget = match effort {
                    ReasoningEffort::None => 0,
                    ReasoningEffort::Low => 500,
                    ReasoningEffort::Medium => 2000,
                    ReasoningEffort::High => 5000,
                    ReasoningEffort::XHigh => 10000,
                };
                if let Some(obj) = request_body.as_object_mut() {
                    obj.insert("thinking_budget".to_string(), json!(budget));
                    // 对于 low 和 none，禁用 thinking
                    if matches!(effort, ReasoningEffort::None | ReasoningEffort::Low) {
                        obj.insert("enable_thinking".to_string(), json!(false));
                    } else {
                        obj.insert("enable_thinking".to_string(), json!(true));
                    }
                }
            }
        } else if self.is_deepseek_model() {
            if let Some(effort) = &config.effort {
                let effort_str = match effort {
                    ReasoningEffort::None => "none",
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::XHigh => "high",
                };
                if let Some(obj) = request_body.as_object_mut() {
                    obj.insert("reasoning_effort".to_string(), json!(effort_str));
                }
            }
            if config.enabled == Some(true) || config.budget_tokens.is_some() {
                let mut extra_body = serde_json::Map::new();
                if let Some(enabled) = config.enabled {
                    extra_body.insert(
                        "thinking".to_string(),
                        json!({"type": if enabled { "enabled" } else { "disabled" }}),
                    );
                }
                if let Some(budget) = config.budget_tokens {
                    extra_body.insert("thinking_budget".to_string(), json!(budget));
                }
                if !extra_body.is_empty()
                    && let Some(obj) = request_body.as_object_mut()
                {
                    obj.insert("extra_body".to_string(), Value::Object(extra_body));
                }
            }
        } else {
            if let Some(effort) = &config.effort {
                let effort_str = match effort {
                    ReasoningEffort::None => "none",
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::Medium => "medium",
                    ReasoningEffort::High => "high",
                    ReasoningEffort::XHigh => "high",
                };
                if let Some(obj) = request_body.as_object_mut() {
                    obj.insert("reasoning_effort".to_string(), json!(effort_str));
                }
            }
        }
    }

    fn chat_message_to_json(msg: &ChatMessage) -> Value {
        match msg {
            ChatMessage::System { content, .. } => json!({
                "role": "system",
                "content": content,
            }),
            ChatMessage::User {
                content, images, ..
            } => {
                if images.is_empty() {
                    json!({
                        "role": "user",
                        "content": content,
                    })
                } else {
                    let mut content_parts: Vec<Value> = Vec::new();
                    content_parts.push(json!({"type": "text", "text": content}));
                    for img in images {
                        content_parts.push(Self::image_to_json(img));
                    }
                    json!({
                        "role": "user",
                        "content": content_parts,
                    })
                }
            }
            ChatMessage::Assistant {
                content,
                reasoning_content,
                tool_calls,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), json!("assistant"));
                obj.insert("content".to_string(), json!(content));
                if let Some(reasoning) = reasoning_content {
                    obj.insert("reasoning_content".to_string(), json!(reasoning));
                }
                if let Some(tc) = tool_calls {
                    let tool_calls_json: Vec<Value> =
                        tc.iter().map(Self::tool_call_to_json).collect();
                    obj.insert("tool_calls".to_string(), json!(tool_calls_json));
                }
                Value::Object(obj)
            }
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content,
            }),
            ChatMessage::Custom { role, data } => json!({
                "role": role,
                "content": data.to_string(),
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

    fn image_to_json(img: &ImageAttachment) -> Value {
        match img {
            ImageAttachment::Url { url, detail } => {
                let mut obj = serde_json::Map::new();
                obj.insert("url".to_string(), json!(url));
                if let Some(d) = detail {
                    let detail_str = match d {
                        ImageDetail::Low => "low",
                        ImageDetail::High => "high",
                        ImageDetail::Auto => "auto",
                    };
                    obj.insert("detail".to_string(), json!(detail_str));
                }
                json!({
                    "type": "image_url",
                    "image_url": Value::Object(obj),
                })
            }
            ImageAttachment::Base64 {
                data,
                media_type,
                detail,
            } => {
                let mime = media_type.as_deref().unwrap_or("image/jpeg");
                let data_url = format!("data:{mime};base64,{data}");
                let mut obj = serde_json::Map::new();
                obj.insert("url".to_string(), json!(data_url));
                if let Some(d) = detail {
                    let detail_str = match d {
                        ImageDetail::Low => "low",
                        ImageDetail::High => "high",
                        ImageDetail::Auto => "auto",
                    };
                    obj.insert("detail".to_string(), json!(detail_str));
                }
                json!({
                    "type": "image_url",
                    "image_url": Value::Object(obj),
                })
            }
        }
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
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let url = format!("{}/chat/completions", self.base_url);
        let raw_messages = Self::messages_to_json(messages);
        let mut request_body = json!({
            "model": self.model,
            "messages": raw_messages,
            "tools": tools,
            "max_tokens": 8192,
        });

        self.apply_reasoning_config(&mut request_body, reasoning);

        if let Some(rf) = response_format
            && let Some(obj) = request_body.as_object_mut()
        {
            obj.insert("response_format".to_string(), rf.to_api_value());
        }

        tracing::info!(model = %self.model, msg_count = messages.len(), "llm chat request");
        tracing::debug!(request_body = %serde_json::to_string_pretty(&request_body).unwrap_or_default(), "llm request body");

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP request failed: {e}")))?;

        let status = response.status();
        let res_json: Value = response
            .json()
            .await
            .map_err(|e| AgentError::json(format!("Response JSON parse failed: {e}")))?;

        if !status.is_success() {
            tracing::warn!(%status, "OpenAI API non-success");
        }

        if let Some(error) = res_json.get("error") {
            tracing::warn!(?error, "OpenAI API returned error");
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
        reasoning: Option<&ReasoningConfig>,
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
            "max_tokens": 8192,
        });

        self.apply_reasoning_config(&mut request_body, reasoning);

        if let Some(rf) = response_format
            && let Some(obj) = request_body.as_object_mut()
        {
            obj.insert("response_format".to_string(), rf.to_api_value());
        }

        tracing::debug!(request_body = %serde_json::to_string_pretty(&request_body).unwrap_or_default(), "llm stream request body");

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AgentError::llm(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let err_text = response
                .text()
                .await
                .map_err(|e| AgentError::llm(format!("Failed to read error response: {e}")))?;
            tracing::warn!(%status, error = %err_text, "OpenAI API stream non-success");
            return Err(AgentError::LlmApi { message: err_text });
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .map(|event| match event {
                Ok(event) => {
                    if event.data == "[DONE]" {
                        return Ok(StreamChunk::Stop {
                            finish_reason: None,
                        });
                    }

                    let data: Value = serde_json::from_str(&event.data)
                        .map_err(|e| AgentError::json(format!("JSON Parse error: {e}")))?;

                    let choices = data.get("choices").and_then(Value::as_array);

                    if choices.is_none() || choices.is_none_or(|c| c.is_empty()) {
                        if let Some(usage) = data.get("usage") {
                            return Ok(StreamChunk::Usage(UsageInfo {
                                prompt_tokens: usage
                                    .get("prompt_tokens")
                                    .and_then(Value::as_u64)
                                    .map(|v| v as u32),
                                completion_tokens: usage
                                    .get("completion_tokens")
                                    .and_then(Value::as_u64)
                                    .map(|v| v as u32),
                                total_tokens: usage
                                    .get("total_tokens")
                                    .and_then(Value::as_u64)
                                    .map(|v| v as u32),
                            }));
                        }
                        return Ok(StreamChunk::Text(String::new()));
                    }

                    let choice = &choices.unwrap()[0];
                    let delta = &choice["delta"];
                    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");

                    if finish_reason == "tool_calls"
                        || delta.get("tool_calls").is_some_and(|v| !v.is_null())
                    {
                        return Ok(StreamChunk::ToolCall(choice.clone()));
                    }

                    if let Some(reasoning) = delta.get("reasoning_content")
                        && let Some(text) = reasoning.as_str()
                    {
                        return Ok(StreamChunk::Thought(text.to_string()));
                    }

                    if let Some(content) = delta.get("content")
                        && let Some(text) = content.as_str()
                    {
                        return Ok(StreamChunk::Text(text.to_string()));
                    }

                    if finish_reason == "stop" || finish_reason == "length" {
                        return Ok(StreamChunk::Stop {
                            finish_reason: Some(finish_reason.to_string()),
                        });
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
            supports_thinking: true,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[async_trait]
impl super::StreamClient for OpenAiClient {
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
    use crate::types::{ChatMessage, ImageAttachment, ImageDetail, ToolCallMessage};
    use futures_util::TryStreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(model: &str) -> OpenAiClient {
        OpenAiClient::new("test-key".into(), model.into(), None)
    }

    // ---- pure helpers ----

    #[test]
    fn chat_message_to_json_all_variants() {
        let v = OpenAiClient::chat_message_to_json(&ChatMessage::system("be helpful"));
        assert_eq!(
            v,
            serde_json::json!({"role": "system", "content": "be helpful"})
        );

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::user("hi"));
        assert_eq!(v, serde_json::json!({"role": "user", "content": "hi"}));

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::user_with_images(
            "look",
            vec![ImageAttachment::Url {
                url: "http://x/a.png".into(),
                detail: None,
            }],
        ));
        assert_eq!(v["role"], "user");
        assert_eq!(
            v["content"][0],
            serde_json::json!({"type": "text", "text": "look"})
        );
        assert_eq!(v["content"][1]["type"], "image_url");

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::assistant("hello"));
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["content"], "hello");

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::assistant_with_reasoning(
            "answer",
            "let me think",
        ));
        assert_eq!(v["reasoning_content"], "let me think");

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::assistant_tool_call(
            "call_1", "echo", "{}",
        ));
        assert_eq!(v["tool_calls"][0]["function"]["name"], "echo");
        assert_eq!(v["tool_calls"][0]["type"], "function");

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::tool("tid", "result"));
        assert_eq!(
            v,
            serde_json::json!({"role": "tool", "tool_call_id": "tid", "content": "result"})
        );

        let v = OpenAiClient::chat_message_to_json(&ChatMessage::Custom {
            role: "artifact".into(),
            data: serde_json::json!({"x": 1}),
        });
        assert_eq!(v["role"], "artifact");
        assert_eq!(v["content"], "{\"x\":1}");
    }

    #[test]
    fn image_to_json_url_and_base64() {
        let v = OpenAiClient::image_to_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: None,
        });
        assert_eq!(
            v,
            serde_json::json!({"type": "image_url", "image_url": {"url": "http://x/a.png"}})
        );

        let v = OpenAiClient::image_to_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: Some(ImageDetail::High),
        });
        assert_eq!(v["image_url"]["detail"], "high");

        let v = OpenAiClient::image_to_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: Some("image/png".into()),
            detail: None,
        });
        assert_eq!(v["image_url"]["url"], "data:image/png;base64,abc");

        let v = OpenAiClient::image_to_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: None,
            detail: Some(ImageDetail::Low),
        });
        assert_eq!(v["image_url"]["url"], "data:image/jpeg;base64,abc");
        assert_eq!(v["image_url"]["detail"], "low");
    }

    #[test]
    fn tool_call_to_json_shape() {
        let tc = ToolCallMessage {
            id: "call_1".into(),
            name: "echo".into(),
            arguments: "{\"x\":1}".into(),
        };
        let v = OpenAiClient::tool_call_to_json(&tc);
        assert_eq!(v["id"], "call_1");
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "echo");
        assert_eq!(v["function"]["arguments"], "{\"x\":1}");
    }

    #[test]
    fn apply_reasoning_config_none_is_noop() {
        let c = client("gpt-4o");
        let mut body = serde_json::json!({"model": "gpt-4o"});
        c.apply_reasoning_config(&mut body, None);
        assert_eq!(body, serde_json::json!({"model": "gpt-4o"}));
    }

    #[test]
    fn apply_reasoning_config_qwen_flags() {
        let c = client("qwen-max");
        let mut body = serde_json::json!({"model": "qwen-max"});
        let rc = ReasoningConfig {
            enabled: Some(true),
            budget_tokens: Some(1000),
            effort: None,
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(body["enable_thinking"], serde_json::json!(true));
        assert_eq!(body["thinking_budget"], serde_json::json!(1000));
    }

    #[test]
    fn apply_reasoning_config_qwen_effort_maps_budget() {
        let c = client("qwen-max");
        let mut body = serde_json::json!({"model": "qwen-max"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(body["thinking_budget"], serde_json::json!(5000));
        assert_eq!(body["enable_thinking"], serde_json::json!(true));

        let mut body = serde_json::json!({"model": "qwen-max"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::Low),
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(body["enable_thinking"], serde_json::json!(false));
    }

    #[test]
    fn apply_reasoning_config_deepseek_effort() {
        let c = client("deepseek-chat");
        let mut body = serde_json::json!({"model": "deepseek-chat"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::Medium),
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(body["reasoning_effort"], serde_json::json!("medium"));
    }

    #[test]
    fn apply_reasoning_config_deepseek_thinking_extra_body() {
        let c = client("deepseek-chat");
        let mut body = serde_json::json!({"model": "deepseek-chat"});
        let rc = ReasoningConfig {
            enabled: Some(true),
            budget_tokens: Some(2000),
            effort: None,
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(
            body["extra_body"]["thinking"]["type"],
            serde_json::json!("enabled")
        );
        assert_eq!(
            body["extra_body"]["thinking_budget"],
            serde_json::json!(2000)
        );
    }

    #[test]
    fn apply_reasoning_config_other_effort() {
        let c = client("gpt-4o");
        let mut body = serde_json::json!({"model": "gpt-4o"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::XHigh),
        };
        c.apply_reasoning_config(&mut body, Some(&rc));
        assert_eq!(body["reasoning_effort"], serde_json::json!("high"));
    }

    #[test]
    fn with_model_and_capabilities() {
        let c = OpenAiClient::new("k".into(), "gpt-4o".into(), Some("http://x".into()));
        let c2 = c.with_model("gpt-4o-mini");
        assert_eq!(c2.model_name(), "gpt-4o-mini");
        assert_eq!(c.model_name(), "gpt-4o");

        let caps = c.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_thinking);
        assert_eq!(caps.max_context_tokens, Some(128_000));
        assert_eq!(caps.max_output_tokens, Some(16_384));
    }

    // ---- mock HTTP ----

    #[tokio::test]
    async fn chat_posts_and_parses_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "cmpl-1",
                "choices": [{"message": {"role": "assistant", "content": "hi there"}}],
            })))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("test-key".into(), "gpt-4o".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hello")], &[], None, None)
            .await
            .unwrap();
        assert_eq!(resp["choices"][0]["message"]["content"], "hi there");
    }

    #[tokio::test]
    async fn chat_returns_llm_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": {"message": "invalid api key", "type": "invalid_request_error"},
            })))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("bad-key".into(), "gpt-4o".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hello")], &[], None, None)
            .await;
        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn chat_stream_parses_text_thought_toolcall_and_stop() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hmm\"},\"finish_reason\":\"\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"echo\",\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "Hello"));
        assert!(matches!(&chunks[1], StreamChunk::Thought(t) if t == "hmm"));
        assert!(matches!(&chunks[2], StreamChunk::ToolCall(_)));
        assert!(matches!(&chunks[3], StreamChunk::Stop { finish_reason: Some(r) } if r == "stop"));
        assert!(matches!(
            &chunks[4],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn chat_stream_parses_usage_chunk() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"total_tokens\":30}}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(
            matches!(&chunks[0], StreamChunk::Usage(u) if u.prompt_tokens == Some(10) && u.completion_tokens == Some(20) && u.total_tokens == Some(30))
        );
        assert!(matches!(
            &chunks[1],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn chat_stream_returns_error_on_non_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("bad".into(), "gpt-4o".into(), Some(server.uri()));
        let resp = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await;
        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn chat_stream_errors_on_invalid_json() {
        let server = MockServer::start().await;
        let sse = "data: not-json\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }

    // ── B2: remaining enum arms + adapter delegation + HTTP edges ────────

    #[test]
    fn image_to_json_detail_variants() {
        // Url + Low / Auto
        let v = OpenAiClient::image_to_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: Some(ImageDetail::Low),
        });
        assert_eq!(v["image_url"]["detail"], "low");

        let v = OpenAiClient::image_to_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: Some(ImageDetail::Auto),
        });
        assert_eq!(v["image_url"]["detail"], "auto");

        // Base64 + High / Auto
        let v = OpenAiClient::image_to_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: None,
            detail: Some(ImageDetail::High),
        });
        assert_eq!(v["image_url"]["detail"], "high");

        let v = OpenAiClient::image_to_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: None,
            detail: Some(ImageDetail::Auto),
        });
        assert_eq!(v["image_url"]["detail"], "auto");
    }

    #[test]
    fn apply_reasoning_config_qwen_effort_all_levels() {
        // None/Low already covered; exercise None→0, Medium→2000, XHigh→10000.
        for (effort, budget, enabled) in [
            (ReasoningEffort::None, 0, false),
            (ReasoningEffort::Medium, 2000, true),
            (ReasoningEffort::XHigh, 10000, true),
        ] {
            let c = client("qwen-max");
            let mut body = serde_json::json!({"model": "qwen-max"});
            let rc = ReasoningConfig {
                enabled: None,
                budget_tokens: None,
                effort: Some(effort),
            };
            c.apply_reasoning_config(&mut body, Some(&rc));
            assert_eq!(body["thinking_budget"], serde_json::json!(budget));
            assert_eq!(body["enable_thinking"], serde_json::json!(enabled));
        }
    }

    #[test]
    fn apply_reasoning_config_deepseek_effort_all_levels() {
        // Medium already covered; exercise None/Low/High/XHigh.
        for (effort, expected) in [
            (ReasoningEffort::None, "none"),
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::High, "high"),
            (ReasoningEffort::XHigh, "high"),
        ] {
            let c = client("deepseek-chat");
            let mut body = serde_json::json!({"model": "deepseek-chat"});
            let rc = ReasoningConfig {
                enabled: None,
                budget_tokens: None,
                effort: Some(effort),
            };
            c.apply_reasoning_config(&mut body, Some(&rc));
            assert_eq!(body["reasoning_effort"], serde_json::json!(expected));
        }
    }

    #[test]
    fn apply_reasoning_config_other_effort_all_levels() {
        // XHigh already covered; exercise None/Low/Medium/High.
        for (effort, expected) in [
            (ReasoningEffort::None, "none"),
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
        ] {
            let c = client("gpt-4o");
            let mut body = serde_json::json!({"model": "gpt-4o"});
            let rc = ReasoningConfig {
                enabled: None,
                budget_tokens: None,
                effort: Some(effort),
            };
            c.apply_reasoning_config(&mut body, Some(&rc));
            assert_eq!(body["reasoning_effort"], serde_json::json!(expected));
        }
    }

    #[test]
    fn stream_client_delegates_capabilities_and_model_name() {
        let c = OpenAiClient::new("k".into(), "gpt-4o".into(), None);
        let caps = <OpenAiClient as crate::llm::StreamClient>::capabilities(&c);
        assert!(caps.supports_streaming);
        assert_eq!(
            <OpenAiClient as crate::llm::StreamClient>::model_name(&c),
            "gpt-4o"
        );
    }

    #[tokio::test]
    async fn stream_client_stream_delegates_to_chat_stream() {
        let server = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"\"}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = <OpenAiClient as crate::llm::StreamClient>::stream(
            &client,
            &[ChatMessage::user("hi")],
            &[],
            None,
            None,
        )
        .await
        .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "Hi"));
    }

    #[tokio::test]
    async fn chat_errors_on_non_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let resp = client
            .chat(&[ChatMessage::user("hi")], &[], None, None)
            .await;
        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn chat_stream_emits_empty_text_for_no_choices_or_usage() {
        let server = MockServer::start().await;
        let sse = "data: {\"id\":\"x\"}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t.is_empty()));
    }

    #[tokio::test]
    async fn chat_stream_emits_empty_text_for_empty_delta() {
        let server = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"\"}]}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let client = OpenAiClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = client
            .chat_stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t.is_empty()));
    }
}
