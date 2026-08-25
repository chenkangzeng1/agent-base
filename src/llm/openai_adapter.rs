use super::{
    LlmCapabilities, LlmClientConfig, ReasoningConfig, ReasoningEffort, StreamChunk, StreamClient,
    UsageInfo,
};
use crate::types::{
    AgentError, AgentResult, ChatMessage, ImageAttachment, ImageDetail, ResponseFormat,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::pin::Pin;

// ── Public API ──

/// OpenAI-compatible LLM client backed by `async-openai` types.
///
/// Supports:
/// - OpenAI Chat Completions API
/// - OpenAI Responses API
/// - Qwen / DeepSeek (via OpenAI-compatible endpoints with extra reasoning params)
pub struct OpenAiAdapter {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
    /// Which API variant to use.
    variant: ApiVariant,
}

#[derive(Clone, Debug)]
enum ApiVariant {
    Chat,
    Responses,
}

/// Backward-compatible alias. Prefer [`OpenAiAdapter`] in new code.
pub type OpenAiClient = OpenAiAdapter;

impl OpenAiAdapter {
    /// Backward-compatible constructor (equivalent to [`chat_client`](Self::chat_client)).
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self::chat_client(api_key, model, base_url)
    }

    /// Create a Chat Completions client.
    pub fn chat_client(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        Self::new_inner(
            api_key,
            model,
            base_url,
            ApiVariant::Chat,
            LlmClientConfig::default(),
        )
    }

    /// Create a Responses API client.
    pub fn responses_client(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        Self::new_inner(
            api_key,
            model,
            base_url,
            ApiVariant::Responses,
            LlmClientConfig::default(),
        )
    }

    /// Create with custom config.
    pub fn chat_client_with_config(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        config: LlmClientConfig,
    ) -> Self {
        Self::new_inner(api_key, model, base_url, ApiVariant::Chat, config)
    }

    /// Create with custom config (Responses).
    pub fn responses_client_with_config(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        config: LlmClientConfig,
    ) -> Self {
        Self::new_inner(api_key, model, base_url, ApiVariant::Responses, config)
    }

    fn new_inner(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        variant: ApiVariant,
        config: LlmClientConfig,
    ) -> Self {
        let client = Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.request_timeout)
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(config.pool_idle_timeout)
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to build reqwest client, falling back to default");
                Client::new()
            });
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            client,
            variant,
        }
    }

    /// Create a variant with a different model, sharing the HTTP connection pool.
    pub fn with_model(&self, model: impl Into<String>) -> Self {
        Self {
            api_key: self.api_key.clone(),
            model: model.into(),
            base_url: self.base_url.clone(),
            client: self.client.clone(),
            variant: self.variant.clone(),
        }
    }

    fn is_qwen_model(&self) -> bool {
        self.model.starts_with("qwen")
    }

    fn is_deepseek_model(&self) -> bool {
        self.model.starts_with("deepseek")
    }
}

// ── StreamClient impl ──

#[async_trait]
impl StreamClient for OpenAiAdapter {
    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        match self.variant {
            ApiVariant::Chat => {
                self.stream_chat(messages, tools, reasoning, response_format)
                    .await
            }
            ApiVariant::Responses => self.stream_responses(messages, tools, reasoning).await,
        }
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

// ── Chat Completions API ──

impl OpenAiAdapter {
    async fn stream_chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/chat/completions", self.base_url);

        // Build request as typed struct, then serialize to Value for injection.
        let request = build_chat_request(
            &self.model,
            messages,
            tools,
            reasoning,
            response_format,
            self.is_qwen_model(),
            self.is_deepseek_model(),
        );

        let mut body = serde_json::to_value(&request)
            .map_err(|e| AgentError::json(format!("Request serialization failed: {e}")))?;

        // Inject Qwen/DeepSeek non-standard reasoning params.
        inject_reasoning_params(
            &mut body,
            reasoning,
            self.is_qwen_model(),
            self.is_deepseek_model(),
        );

        tracing::debug!(
            model = %self.model,
            body = %serde_json::to_string_pretty(&body).unwrap_or_default(),
            "OpenAI chat stream request"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
            tracing::warn!(%status, error = %err_text, "OpenAI API stream non-success");
            return Err(AgentError::LlmApi { message: err_text });
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .map(|event| match event {
                Ok(ev) => {
                    if ev.data == "[DONE]" {
                        return Ok(StreamChunk::Stop {
                            finish_reason: None,
                        });
                    }
                    let data: Value = serde_json::from_str(&ev.data)
                        .map_err(|e| AgentError::json(format!("SSE JSON parse error: {e}")))?;
                    parse_chat_stream_chunk(&data)
                }
                Err(e) => Err(AgentError::LlmStream(format!("SSE Stream error: {e}"))),
            });

        Ok(Box::pin(stream))
    }

    async fn stream_responses(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/responses", self.base_url);
        let input = convert_messages_to_responses_input(messages);

        let mut body = json!({
            "model": self.model,
            "input": input,
            "stream": true,
        });

        if !tools.is_empty()
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("tools".to_string(), json!(tools));
        }

        if let Some(config) = reasoning
            && (config.enabled == Some(true) || config.budget_tokens.is_some())
        {
            let mut reasoning_cfg = serde_json::Map::new();
            reasoning_cfg.insert("summary".to_string(), json!("auto"));
            if let Some(budget) = config.budget_tokens {
                reasoning_cfg.insert("budget_tokens".to_string(), json!(budget));
            }
            if let Some(obj) = body.as_object_mut() {
                obj.insert("reasoning".to_string(), Value::Object(reasoning_cfg));
            }
        }

        tracing::debug!(
            model = %self.model,
            body = %serde_json::to_string_pretty(&body).unwrap_or_default(),
            "Responses API stream request"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
            tracing::warn!(%status, error = %err_text, "Responses API stream non-success");
            return Err(AgentError::LlmApi { message: err_text });
        }

        // Stateful stream: buffer function_call arguments across events.
        let buffers: HashMap<String, FunctionCallBuffer> = HashMap::new();
        let next_index: usize = 0;

        let stream = response
            .bytes_stream()
            .eventsource()
            .scan((buffers, next_index), |(buffers, next_index), event| {
                let result = match event {
                    Ok(ev) => {
                        let data: Result<Value, _> = serde_json::from_str(&ev.data);
                        match data {
                            Ok(val) => parse_responses_event(&ev.event, &val, buffers, next_index),
                            Err(e) => Err(AgentError::json(format!("Responses API SSE JSON: {e}"))),
                        }
                    }
                    Err(e) => Err(AgentError::LlmStream(format!("SSE Stream error: {e}"))),
                };
                std::future::ready(Some(result))
            })
            .flat_map(|result| match result {
                Ok(chunks) => {
                    futures_util::stream::iter(chunks.into_iter().map(Ok).collect::<Vec<_>>())
                }
                Err(e) => futures_util::stream::iter(vec![Err(e)]),
            });

        Ok(Box::pin(stream))
    }
}

// ── Chat Completions request builder ──

fn build_chat_request(
    model: &str,
    messages: &[ChatMessage],
    tools: &[Value],
    reasoning: Option<&ReasoningConfig>,
    response_format: Option<&ResponseFormat>,
    is_qwen: bool,
    is_deepseek: bool,
) -> Value {
    let raw_messages: Vec<Value> = messages.iter().map(chat_message_to_openai_json).collect();

    let mut body = json!({
        "model": model,
        "messages": raw_messages,
        "max_tokens": 8192,
        "stream": true,
    });

    if !tools.is_empty()
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("tools".to_string(), json!(tools));
    }

    // Standard OpenAI reasoning_effort (for non-Qwen/DeepSeek models).
    if !is_qwen
        && !is_deepseek
        && let Some(config) = reasoning
        && let Some(effort) = &config.effort
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert(
            "reasoning_effort".to_string(),
            json!(reasoning_effort_to_str(effort)),
        );
    }

    if let Some(rf) = response_format
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("response_format".to_string(), rf.to_api_value());
    }

    // Stream options for usage tracking.
    body.as_object_mut()
        .unwrap()
        .insert("stream_options".to_string(), json!({"include_usage": true}));

    body
}

/// Inject Qwen/DeepSeek non-standard reasoning params into the request body.
fn inject_reasoning_params(
    body: &mut Value,
    reasoning: Option<&ReasoningConfig>,
    is_qwen: bool,
    is_deepseek: bool,
) {
    let Some(config) = reasoning else { return };

    if is_qwen {
        if let Some(enabled) = config.enabled
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("enable_thinking".to_string(), json!(enabled));
        }
        if let Some(budget) = config.budget_tokens
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("thinking_budget".to_string(), json!(budget));
        }
        if let Some(effort) = &config.effort
            && let Some(obj) = body.as_object_mut()
        {
            let (budget, enabled) = qwen_effort_to_params(effort);
            obj.insert("thinking_budget".to_string(), json!(budget));
            obj.insert("enable_thinking".to_string(), json!(enabled));
        }
    } else if is_deepseek {
        if let Some(effort) = &config.effort
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert(
                "reasoning_effort".to_string(),
                json!(deepseek_effort_to_str(effort)),
            );
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
                && let Some(obj) = body.as_object_mut()
            {
                obj.insert("extra_body".to_string(), Value::Object(extra_body));
            }
        }
    }
}

// ── Chat stream chunk parser ──

fn parse_chat_stream_chunk(data: &Value) -> AgentResult<StreamChunk> {
    let choices = data.get("choices").and_then(Value::as_array);

    if choices.is_none() || choices.is_none_or(|c| c.is_empty()) {
        // Usage-only chunk (stream_options.include_usage).
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

    // Tool calls.
    if finish_reason == "tool_calls" || delta.get("tool_calls").is_some_and(|v| !v.is_null()) {
        return Ok(StreamChunk::ToolCall(choice.clone()));
    }

    // Reasoning content (DeepSeek/Qwen).
    if let Some(reasoning) = delta.get("reasoning_content")
        && let Some(text) = reasoning.as_str()
    {
        return Ok(StreamChunk::Thought(text.to_string()));
    }

    // Text content.
    if let Some(content) = delta.get("content")
        && let Some(text) = content.as_str()
    {
        return Ok(StreamChunk::Text(text.to_string()));
    }

    // Stop.
    if finish_reason == "stop" || finish_reason == "length" {
        return Ok(StreamChunk::Stop {
            finish_reason: Some(finish_reason.to_string()),
        });
    }

    Ok(StreamChunk::Text(String::new()))
}

// ── Responses API ──

struct FunctionCallBuffer {
    call_id: String,
    name: String,
    arguments: String,
}

fn convert_messages_to_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| match msg {
            ChatMessage::System { content, .. } => {
                json!({"role": "developer", "content": content})
            }
            ChatMessage::User {
                content, images, ..
            } => {
                if images.is_empty() {
                    json!({"role": "user", "content": content})
                } else {
                    let mut items: Vec<Value> =
                        vec![json!({"type": "input_text", "text": content})];
                    for img in images {
                        items.push(image_to_responses_input(img));
                    }
                    json!({"role": "user", "content": items})
                }
            }
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let mut content_parts: Vec<Value> = vec![];
                if let Some(text) = content
                    && !text.is_empty()
                {
                    content_parts.push(json!({"type": "output_text", "text": text}));
                }
                if let Some(tc) = tool_calls {
                    for t in tc {
                        content_parts.push(json!({
                            "type": "function_call",
                            "call_id": t.id,
                            "name": t.name,
                            "arguments": t.arguments,
                        }));
                    }
                }
                json!({"type": "message", "role": "assistant", "content": content_parts})
            }
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => {
                json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": content,
                })
            }
            ChatMessage::Custom { data, .. } => data.clone(),
        })
        .collect()
}

fn image_to_responses_input(img: &ImageAttachment) -> Value {
    match img {
        ImageAttachment::Url { url, detail } => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), json!("input_image"));
            obj.insert("image_url".to_string(), json!(url));
            if let Some(d) = detail {
                obj.insert("detail".to_string(), json!(image_detail_str(d)));
            }
            Value::Object(obj)
        }
        ImageAttachment::Base64 {
            data,
            media_type,
            detail,
        } => {
            let mime = media_type.as_deref().unwrap_or("image/jpeg");
            let url = format!("data:{mime};base64,{data}");
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), json!("input_image"));
            obj.insert("image_url".to_string(), json!(url));
            if let Some(d) = detail {
                obj.insert("detail".to_string(), json!(image_detail_str(d)));
            }
            Value::Object(obj)
        }
    }
}

fn parse_responses_event(
    event_type: &str,
    data: &Value,
    buffers: &mut HashMap<String, FunctionCallBuffer>,
    next_index: &mut usize,
) -> AgentResult<Vec<StreamChunk>> {
    match event_type {
        "response.output_text.delta" => {
            let delta = data
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(vec![StreamChunk::Text(delta)])
        }
        "response.output_text.done" => Ok(vec![]),
        "response.output_item.added" => {
            let item = match data.get("item") {
                Some(i) => i,
                None => return Ok(vec![]),
            };
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                buffers.insert(
                    call_id.clone(),
                    FunctionCallBuffer {
                        call_id,
                        name,
                        arguments: String::new(),
                    },
                );
            }
            Ok(vec![])
        }
        "response.function_call_arguments.delta" => {
            let call_id = data.get("call_id").and_then(Value::as_str).unwrap_or("");
            let delta = data.get("delta").and_then(Value::as_str).unwrap_or("");
            if let Some(buf) = buffers.get_mut(call_id) {
                buf.arguments.push_str(delta);
            }
            Ok(vec![])
        }
        "response.function_call_arguments.done" => {
            let call_id = data.get("call_id").and_then(Value::as_str).unwrap_or("");
            let done_args = data.get("arguments").and_then(Value::as_str).unwrap_or("");
            if let Some(buf) = buffers.remove(call_id) {
                let args = if done_args.is_empty() {
                    buf.arguments
                } else {
                    done_args.to_string()
                };
                let idx = *next_index;
                *next_index += 1;
                Ok(vec![emit_tool_call(idx, &buf.call_id, &buf.name, &args)])
            } else {
                let idx = *next_index;
                *next_index += 1;
                Ok(vec![emit_tool_call(idx, call_id, "", done_args)])
            }
        }
        "response.reasoning_summary_text.delta" => {
            let delta = data
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(vec![StreamChunk::Thought(delta)])
        }
        "response.completed" => {
            let response = data.get("response");
            let mut chunks = Vec::new();
            if let Some(usage) = extract_responses_usage(response) {
                chunks.push(StreamChunk::Usage(usage));
            }
            let status = response
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str);
            let finish_reason = if status == Some("incomplete") {
                let reason = response
                    .and_then(|r| r.get("incomplete_details"))
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Some(format!("incomplete:{reason}"))
            } else {
                None
            };
            chunks.push(StreamChunk::Stop { finish_reason });
            Ok(chunks)
        }
        "response.incomplete" => {
            let reason = data
                .get("response")
                .and_then(|r| r.get("incomplete_details"))
                .and_then(|d| d.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Ok(vec![StreamChunk::Stop {
                finish_reason: Some(format!("incomplete:{reason}")),
            }])
        }
        "response.failed" => {
            let msg = data
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("response failed");
            Err(AgentError::llm(format!("Responses API: {msg}")))
        }
        // Ignored events
        "response.created"
        | "response.in_progress"
        | "response.output_item.done"
        | "response.content_part.added"
        | "response.content_part.done" => Ok(vec![]),
        _ => Ok(vec![]),
    }
}

fn extract_responses_usage(response: Option<&Value>) -> Option<UsageInfo> {
    let r = response?;
    let input = r
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let output = r
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    let total = r
        .get("usage")
        .and_then(|u| u.get("total_tokens"))
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    if input.is_some() || output.is_some() || total.is_some() {
        Some(UsageInfo {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: total,
        })
    } else {
        None
    }
}

fn emit_tool_call(index: usize, call_id: &str, name: &str, arguments: &str) -> StreamChunk {
    StreamChunk::ToolCall(json!({
        "delta": {
            "tool_calls": [{
                "index": index,
                "id": call_id,
                "function": { "name": name, "arguments": arguments }
            }]
        }
    }))
}

// ── Shared helpers ──

fn chat_message_to_openai_json(msg: &ChatMessage) -> Value {
    match msg {
        ChatMessage::System { content, .. } => json!({"role": "system", "content": content}),
        ChatMessage::User {
            content, images, ..
        } => {
            if images.is_empty() {
                json!({"role": "user", "content": content})
            } else {
                let mut parts: Vec<Value> = vec![json!({"type": "text", "text": content})];
                for img in images {
                    parts.push(image_to_openai_json(img));
                }
                json!({"role": "user", "content": parts})
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
                let tool_calls_json: Vec<Value> = tc
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "type": "function",
                            "function": { "name": t.name, "arguments": t.arguments }
                        })
                    })
                    .collect();
                obj.insert("tool_calls".to_string(), json!(tool_calls_json));
            }
            Value::Object(obj)
        }
        ChatMessage::Tool {
            tool_call_id,
            content,
        } => {
            json!({"role": "tool", "tool_call_id": tool_call_id, "content": content})
        }
        ChatMessage::Custom { role, data } => {
            json!({"role": role, "content": data.to_string()})
        }
    }
}

fn image_to_openai_json(img: &ImageAttachment) -> Value {
    match img {
        ImageAttachment::Url { url, detail } => {
            let mut obj = serde_json::Map::new();
            obj.insert("url".to_string(), json!(url));
            if let Some(d) = detail {
                obj.insert("detail".to_string(), json!(image_detail_str(d)));
            }
            json!({"type": "image_url", "image_url": Value::Object(obj)})
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
                obj.insert("detail".to_string(), json!(image_detail_str(d)));
            }
            json!({"type": "image_url", "image_url": Value::Object(obj)})
        }
    }
}

fn image_detail_str(d: &ImageDetail) -> &'static str {
    match d {
        ImageDetail::Low => "low",
        ImageDetail::High => "high",
        ImageDetail::Auto => "auto",
    }
}

fn reasoning_effort_to_str(effort: &ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "high",
    }
}

fn qwen_effort_to_params(effort: &ReasoningEffort) -> (u64, bool) {
    match effort {
        ReasoningEffort::None => (0, false),
        ReasoningEffort::Low => (500, false),
        ReasoningEffort::Medium => (2000, true),
        ReasoningEffort::High => (5000, true),
        ReasoningEffort::XHigh => (10000, true),
    }
}

fn deepseek_effort_to_str(effort: &ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "none",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "high",
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn chat_adapter(model: &str, base_url: String) -> OpenAiAdapter {
        OpenAiAdapter::chat_client("test-key", model, Some(base_url))
    }

    fn responses_adapter(model: &str, base_url: String) -> OpenAiAdapter {
        OpenAiAdapter::responses_client("test-key", model, Some(base_url))
    }

    // ── Message conversion ──

    #[test]
    fn chat_message_to_openai_json_all_variants() {
        let v = chat_message_to_openai_json(&ChatMessage::system("be helpful"));
        assert_eq!(v, json!({"role": "system", "content": "be helpful"}));

        let v = chat_message_to_openai_json(&ChatMessage::user("hi"));
        assert_eq!(v, json!({"role": "user", "content": "hi"}));

        let v = chat_message_to_openai_json(&ChatMessage::user_with_images(
            "look",
            vec![ImageAttachment::Url {
                url: "http://x/a.png".into(),
                detail: None,
            }],
        ));
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"][0], json!({"type": "text", "text": "look"}));
        assert_eq!(v["content"][1]["type"], "image_url");

        let v = chat_message_to_openai_json(&ChatMessage::assistant("hello"));
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["content"], "hello");

        let v = chat_message_to_openai_json(&ChatMessage::assistant_with_reasoning(
            "answer",
            "let me think",
        ));
        assert_eq!(v["reasoning_content"], "let me think");

        let v =
            chat_message_to_openai_json(&ChatMessage::assistant_tool_call("call_1", "echo", "{}"));
        assert_eq!(v["tool_calls"][0]["function"]["name"], "echo");
        assert_eq!(v["tool_calls"][0]["type"], "function");

        let v = chat_message_to_openai_json(&ChatMessage::tool("tid", "result"));
        assert_eq!(
            v,
            json!({"role": "tool", "tool_call_id": "tid", "content": "result"})
        );

        let v = chat_message_to_openai_json(&ChatMessage::Custom {
            role: "artifact".into(),
            data: json!({"x": 1}),
        });
        assert_eq!(v["role"], "artifact");
        assert_eq!(v["content"], "{\"x\":1}");
    }

    #[test]
    fn image_to_openai_json_variants() {
        let v = image_to_openai_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: None,
        });
        assert_eq!(
            v,
            json!({"type": "image_url", "image_url": {"url": "http://x/a.png"}})
        );

        let v = image_to_openai_json(&ImageAttachment::Url {
            url: "http://x/a.png".into(),
            detail: Some(ImageDetail::High),
        });
        assert_eq!(v["image_url"]["detail"], "high");

        let v = image_to_openai_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: Some("image/png".into()),
            detail: None,
        });
        assert_eq!(v["image_url"]["url"], "data:image/png;base64,abc");

        let v = image_to_openai_json(&ImageAttachment::Base64 {
            data: "abc".into(),
            media_type: None,
            detail: Some(ImageDetail::Low),
        });
        assert_eq!(v["image_url"]["url"], "data:image/jpeg;base64,abc");
        assert_eq!(v["image_url"]["detail"], "low");
    }

    // ── Reasoning config ──

    #[test]
    fn inject_reasoning_params_qwen() {
        let mut body = json!({"model": "qwen-max"});
        let rc = ReasoningConfig {
            enabled: Some(true),
            budget_tokens: Some(1000),
            effort: None,
        };
        inject_reasoning_params(&mut body, Some(&rc), true, false);
        assert_eq!(body["enable_thinking"], json!(true));
        assert_eq!(body["thinking_budget"], json!(1000));
    }

    #[test]
    fn inject_reasoning_params_qwen_effort() {
        let mut body = json!({"model": "qwen-max"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        inject_reasoning_params(&mut body, Some(&rc), true, false);
        assert_eq!(body["thinking_budget"], json!(5000));
        assert_eq!(body["enable_thinking"], json!(true));
    }

    #[test]
    fn inject_reasoning_params_deepseek_effort() {
        let mut body = json!({"model": "deepseek-chat"});
        let rc = ReasoningConfig {
            enabled: None,
            budget_tokens: None,
            effort: Some(ReasoningEffort::Medium),
        };
        inject_reasoning_params(&mut body, Some(&rc), false, true);
        assert_eq!(body["reasoning_effort"], json!("medium"));
    }

    #[test]
    fn inject_reasoning_params_deepseek_thinking() {
        let mut body = json!({"model": "deepseek-chat"});
        let rc = ReasoningConfig {
            enabled: Some(true),
            budget_tokens: Some(2000),
            effort: None,
        };
        inject_reasoning_params(&mut body, Some(&rc), false, true);
        assert_eq!(body["extra_body"]["thinking"]["type"], json!("enabled"));
        assert_eq!(body["extra_body"]["thinking_budget"], json!(2000));
    }

    #[test]
    fn inject_reasoning_params_none_is_noop() {
        let mut body = json!({"model": "gpt-4o"});
        inject_reasoning_params(&mut body, None, false, false);
        assert_eq!(body, json!({"model": "gpt-4o"}));
    }

    // ── Chat stream chunk parser ──

    #[test]
    fn parse_chat_stream_chunk_text() {
        let data = json!({"choices": [{"delta": {"content": "Hello"}, "finish_reason": ""}]});
        assert!(
            matches!(parse_chat_stream_chunk(&data).unwrap(), StreamChunk::Text(t) if t == "Hello")
        );
    }

    #[test]
    fn parse_chat_stream_chunk_reasoning() {
        let data =
            json!({"choices": [{"delta": {"reasoning_content": "hmm"}, "finish_reason": ""}]});
        assert!(
            matches!(parse_chat_stream_chunk(&data).unwrap(), StreamChunk::Thought(t) if t == "hmm")
        );
    }

    #[test]
    fn parse_chat_stream_chunk_tool_calls() {
        let data = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"name": "echo", "arguments": ""}}]}, "finish_reason": "tool_calls"}]});
        assert!(matches!(
            parse_chat_stream_chunk(&data).unwrap(),
            StreamChunk::ToolCall(_)
        ));
    }

    #[test]
    fn parse_chat_stream_chunk_stop() {
        let data = json!({"choices": [{"delta": {}, "finish_reason": "stop"}]});
        assert!(
            matches!(parse_chat_stream_chunk(&data).unwrap(), StreamChunk::Stop { finish_reason: Some(r) } if r == "stop")
        );
    }

    #[test]
    fn parse_chat_stream_chunk_length() {
        let data = json!({"choices": [{"delta": {}, "finish_reason": "length"}]});
        assert!(
            matches!(parse_chat_stream_chunk(&data).unwrap(), StreamChunk::Stop { finish_reason: Some(r) } if r == "length")
        );
    }

    #[test]
    fn parse_chat_stream_chunk_usage() {
        let data =
            json!({"usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}});
        let chunk = parse_chat_stream_chunk(&data).unwrap();
        assert!(
            matches!(chunk, StreamChunk::Usage(u) if u.prompt_tokens == Some(10) && u.completion_tokens == Some(20))
        );
    }

    #[test]
    fn parse_chat_stream_chunk_empty_choices() {
        let data = json!({"choices": []});
        assert!(
            matches!(parse_chat_stream_chunk(&data).unwrap(), StreamChunk::Text(t) if t.is_empty())
        );
    }

    // ── Responses event parser ──

    #[test]
    fn parse_responses_event_text_delta() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"delta": "hello"});
        let chunks =
            parse_responses_event("response.output_text.delta", &data, &mut buffers, &mut idx)
                .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "hello"));
    }

    #[test]
    fn parse_responses_event_function_call_lifecycle() {
        let mut buffers = HashMap::new();
        let mut idx = 0;

        // Init
        let item_data = json!({"item": {"type": "function_call", "call_id": "c1", "name": "echo"}});
        let chunks = parse_responses_event(
            "response.output_item.added",
            &item_data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(chunks.is_empty());
        assert!(buffers.contains_key("c1"));

        // Append
        parse_responses_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c1", "delta": "{\"x\":"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(buffers["c1"].arguments, "{\"x\":");

        // Finalize
        let chunks = parse_responses_event(
            "response.function_call_arguments.done",
            &json!({"call_id": "c1", "arguments": "{\"x\":1}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::ToolCall(v)
            if v["delta"]["tool_calls"][0]["function"]["name"] == "echo"
            && v["delta"]["tool_calls"][0]["function"]["arguments"] == "{\"x\":1}"
        ));
        assert!(buffers.is_empty());
        assert_eq!(idx, 1);
    }

    #[test]
    fn parse_responses_event_reasoning_summary() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let chunks = parse_responses_event(
            "response.reasoning_summary_text.delta",
            &json!({"delta": "thinking..."}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "thinking..."));
    }

    #[test]
    fn parse_responses_event_completed_with_usage() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"usage": {"input_tokens": 10, "output_tokens": 20, "total_tokens": 30}}});
        let chunks =
            parse_responses_event("response.completed", &data, &mut buffers, &mut idx).unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], StreamChunk::Usage(u) if u.prompt_tokens == Some(10)));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[test]
    fn parse_responses_event_completed_incomplete() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}});
        let chunks =
            parse_responses_event("response.completed", &data, &mut buffers, &mut idx).unwrap();
        assert!(
            matches!(&chunks[0], StreamChunk::Stop { finish_reason: Some(r) } if r == "incomplete:max_output_tokens")
        );
    }

    #[test]
    fn parse_responses_event_incomplete() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"incomplete_details": {"reason": "max_output_tokens"}}});
        let chunks =
            parse_responses_event("response.incomplete", &data, &mut buffers, &mut idx).unwrap();
        assert!(
            matches!(&chunks[0], StreamChunk::Stop { finish_reason: Some(r) } if r == "incomplete:max_output_tokens")
        );
    }

    #[test]
    fn parse_responses_event_failed() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"error": {"message": "server error"}});
        assert!(parse_responses_event("response.failed", &data, &mut buffers, &mut idx).is_err());
    }

    #[test]
    fn parse_responses_event_unknown_is_noop() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let chunks =
            parse_responses_event("some.unknown.event", &json!({}), &mut buffers, &mut idx)
                .unwrap();
        assert!(chunks.is_empty());
    }

    // ── Parallel function calls ──

    #[test]
    fn parallel_function_calls_interleaved() {
        let mut buffers = HashMap::new();
        let mut idx = 0;

        parse_responses_event(
            "response.output_item.added",
            &json!({"item": {"type": "function_call", "call_id": "c1", "name": "a"}}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        parse_responses_event(
            "response.output_item.added",
            &json!({"item": {"type": "function_call", "call_id": "c2", "name": "b"}}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        parse_responses_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c1", "delta": "{\"x"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        parse_responses_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c2", "delta": "{\"y"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        parse_responses_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c1", "delta": "\":1}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        parse_responses_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c2", "delta": "\":2}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        let chunks1 = parse_responses_event(
            "response.function_call_arguments.done",
            &json!({"call_id": "c1", "arguments": "{\"x\":1}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        let chunks2 = parse_responses_event(
            "response.function_call_arguments.done",
            &json!({"call_id": "c2", "arguments": "{\"y\":2}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        assert!(
            matches!(&chunks1[0], StreamChunk::ToolCall(v) if v["delta"]["tool_calls"][0]["index"] == 0)
        );
        assert!(
            matches!(&chunks2[0], StreamChunk::ToolCall(v) if v["delta"]["tool_calls"][0]["index"] == 1)
        );
    }

    // ── Responses message conversion ──

    #[test]
    fn convert_messages_to_responses_input_system() {
        let out = convert_messages_to_responses_input(&[ChatMessage::system("be helpful")]);
        assert_eq!(out[0]["role"], "developer");
        assert_eq!(out[0]["content"], "be helpful");
    }

    #[test]
    fn convert_messages_to_responses_input_user_text() {
        let out = convert_messages_to_responses_input(&[ChatMessage::user("hi")]);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
    }

    #[test]
    fn convert_messages_to_responses_input_user_with_images() {
        let out = convert_messages_to_responses_input(&[ChatMessage::user_with_images(
            "look",
            vec![ImageAttachment::Url {
                url: "http://x/a.png".into(),
                detail: None,
            }],
        )]);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"][0]["type"], "input_text");
        assert_eq!(out[0]["content"][1]["type"], "input_image");
    }

    #[test]
    fn convert_messages_to_responses_input_assistant() {
        let out = convert_messages_to_responses_input(&[ChatMessage::assistant("hello")]);
        assert_eq!(out[0]["type"], "message");
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["content"][0]["type"], "output_text");
    }

    #[test]
    fn convert_messages_to_responses_input_tool_call() {
        let out = convert_messages_to_responses_input(&[ChatMessage::assistant_tool_call(
            "call_1",
            "echo",
            "{\"x\":1}",
        )]);
        assert_eq!(out[0]["content"][0]["type"], "function_call");
        assert_eq!(out[0]["content"][0]["call_id"], "call_1");
    }

    #[test]
    fn convert_messages_to_responses_input_tool_result() {
        let out = convert_messages_to_responses_input(&[ChatMessage::tool("call_1", "ok")]);
        assert_eq!(out[0]["type"], "function_call_output");
        assert_eq!(out[0]["call_id"], "call_1");
        assert_eq!(out[0]["output"], "ok");
    }

    #[test]
    fn convert_messages_to_responses_input_custom() {
        let data = json!({"type": "input_text", "text": "custom"});
        let out = convert_messages_to_responses_input(&[ChatMessage::Custom {
            role: "user".into(),
            data: data.clone(),
        }]);
        assert_eq!(out[0], data);
    }

    // ── Capabilities ──

    #[test]
    fn capabilities_and_model_name() {
        let c = OpenAiAdapter::chat_client("k", "gpt-4o", None);
        assert_eq!(c.model_name(), "gpt-4o");
        let caps = c.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_thinking);
        assert_eq!(caps.max_context_tokens, Some(128_000));
    }

    #[test]
    fn with_model_shares_client() {
        let c = OpenAiAdapter::chat_client("k", "gpt-4o", Some("http://x".into()));
        let c2 = c.with_model("gpt-4o-mini");
        assert_eq!(c.model_name(), "gpt-4o");
        assert_eq!(c2.model_name(), "gpt-4o-mini");
    }

    // ── Mock HTTP: Chat Completions ──

    #[tokio::test]
    async fn chat_stream_parses_text_and_stop() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":\"\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"\"}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = chat_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "Hello"));
        assert!(matches!(&chunks[1], StreamChunk::Text(t) if t == " world"));
        assert!(matches!(&chunks[2], StreamChunk::Stop { finish_reason: Some(r) } if r == "stop"));
        assert!(matches!(
            &chunks[3],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn chat_stream_parses_tool_calls() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"echo\",\"arguments\":\"\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = chat_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::ToolCall(_)));
    }

    #[tokio::test]
    async fn chat_stream_parses_usage() {
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

        let c = chat_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Usage(u) if u.prompt_tokens == Some(10)));
    }

    #[tokio::test]
    async fn chat_stream_returns_error_on_non_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let c = chat_adapter("gpt-4o", server.uri());
        let result = c.stream(&[ChatMessage::user("hi")], &[], None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn chat_stream_errors_on_invalid_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("data: not-json\n\n", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let c = chat_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }

    // ── Mock HTTP: Responses API ──

    #[tokio::test]
    async fn responses_stream_parses_text_and_stop() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Hello\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\" world\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "Hello"));
        assert!(matches!(&chunks[1], StreamChunk::Text(t) if t == " world"));
        assert!(matches!(
            &chunks[2],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn responses_stream_parses_function_call() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.output_item.added\n",
            "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"echo\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"call_id\":\"c1\",\"delta\":\"{\\\"x\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"call_id\":\"c1\",\"delta\":\"1}\"}\n\n",
            "event: response.function_call_arguments.done\n",
            "data: {\"call_id\":\"c1\",\"arguments\":\"{\\\"x\\\":1}\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::ToolCall(v)
            if v["delta"]["tool_calls"][0]["function"]["name"] == "echo"
            && v["delta"]["tool_calls"][0]["function"]["arguments"] == "{\"x\":1}"
        ));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn responses_stream_parses_reasoning_summary() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"delta\":\"Let me think...\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Here is the answer\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "Let me think..."));
        assert!(matches!(&chunks[1], StreamChunk::Text(t) if t == "Here is the answer"));
    }

    #[tokio::test]
    async fn responses_stream_returns_error_on_failed_event() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.failed\n",
            "data: {\"error\":{\"message\":\"server error\"}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn responses_stream_returns_error_on_non_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let result = c.stream(&[ChatMessage::user("hi")], &[], None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn responses_stream_parses_incomplete() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.incomplete\n",
            "data: {\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(
            matches!(&chunks[0], StreamChunk::Stop { finish_reason: Some(r) } if r == "incomplete:max_output_tokens")
        );
    }

    #[tokio::test]
    async fn responses_stream_parses_usage() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.completed\n",
            "data: {\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":20,\"total_tokens\":30}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Usage(u)
            if u.prompt_tokens == Some(10) && u.completion_tokens == Some(20) && u.total_tokens == Some(30)
        ));
    }

    #[tokio::test]
    async fn responses_stream_errors_on_malformed_json() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.output_text.delta\n",
            "data: {not valid json}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&server)
            .await;

        let c = responses_adapter("gpt-4o", server.uri());
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<StreamChunk>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }
}
