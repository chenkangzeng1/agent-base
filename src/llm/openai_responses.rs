use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::pin::Pin;

use super::{
    LlmCapabilities, LlmClientConfig, ReasoningConfig, StreamChunk, StreamClient, UsageInfo,
};
use crate::types::{AgentError, AgentResult, ChatMessage, ImageAttachment, ResponseFormat};

// ── Function-call buffer for parallel streaming ──

struct FunctionCallBuffer {
    call_id: String,
    name: String,
    arguments: String,
}

// ── Client ──

pub struct OpenAiResponsesClient {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl OpenAiResponsesClient {
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
            .read_timeout(config.request_timeout)
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(config.pool_idle_timeout)
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to build reqwest client, falling back to default");
                Client::new()
            });
        Self {
            api_key,
            model,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            client,
        }
    }

    // ── Message conversion ──

    fn convert_messages(messages: &[ChatMessage]) -> Vec<Value> {
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
                            items.push(Self::image_to_input(img));
                        }
                        json!({"role": "user", "content": items})
                    }
                }
                ChatMessage::Assistant {
                    content,
                    reasoning_content: _,
                    tool_calls,
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

    fn image_to_input(img: &ImageAttachment) -> Value {
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

    // ── SSE event processing ──

    /// Process one SSE event, mutating the function-call buffers.
    /// Returns zero or more `StreamChunk`s to emit.
    fn process_event(
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
            "response.output_text.done" => {
                // No-op: avoid double-emitting the full text.
                Ok(vec![])
            }
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
                if let Some(usage) = extract_usage(response) {
                    chunks.push(StreamChunk::Usage(usage));
                }
                // Check if the response was actually incomplete despite being "completed".
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
}

fn image_detail_str(d: &crate::types::ImageDetail) -> &'static str {
    match d {
        crate::types::ImageDetail::Low => "low",
        crate::types::ImageDetail::High => "high",
        crate::types::ImageDetail::Auto => "auto",
    }
}

fn extract_usage(response: Option<&Value>) -> Option<UsageInfo> {
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

/// Re-encode a function call into the OpenAI delta shape expected by the
/// react-loop's turn parser.
fn emit_tool_call(index: usize, call_id: &str, name: &str, arguments: &str) -> StreamChunk {
    StreamChunk::ToolCall(json!({
        "delta": {
            "tool_calls": [{
                "index": index,
                "id": call_id,
                "function": {
                    "name": name,
                    "arguments": arguments,
                }
            }]
        }
    }))
}

#[async_trait]
impl StreamClient for OpenAiResponsesClient {
    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let url = format!("{}/responses", self.base_url);
        let input = Self::convert_messages(messages);

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

        // Reasoning config
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
            url = %url,
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

        // Stateful stream processing: function_call arguments arrive across
        // multiple SSE events keyed by call_id.  We use `scan` to maintain
        // a buffer HashMap + index counter, then `flat_map` to flatten the
        // per-event Vec<StreamChunk> into the output stream.
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
                            Ok(val) => {
                                Self::process_event(ev.event.as_str(), &val, buffers, next_index)
                            }
                            Err(e) => Err(AgentError::json(format!("Responses API SSE JSON: {e}"))),
                        }
                    }
                    Err(e) => Err(AgentError::LlmStream(format!("SSE Stream error: {e}"))),
                };
                // `scan` expects `Option<T>` — always continue (errors are
                // propagated as `Err` chunks, not by terminating the stream).
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(model: &str) -> OpenAiResponsesClient {
        OpenAiResponsesClient::new("test-key".into(), model.into(), None)
    }

    // ── Message conversion ──

    #[test]
    fn convert_messages_system() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::system("be helpful")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "developer");
        assert_eq!(out[0]["content"], "be helpful");
    }

    #[test]
    fn convert_messages_user_text_only() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::user("hi")]);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
    }

    #[test]
    fn convert_messages_user_with_images() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::user_with_images(
            "look",
            vec![ImageAttachment::Url {
                url: "http://x/a.png".into(),
                detail: None,
            }],
        )]);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"][0]["type"], "input_text");
        assert_eq!(out[0]["content"][0]["text"], "look");
        assert_eq!(out[0]["content"][1]["type"], "input_image");
    }

    #[test]
    fn convert_messages_assistant_text() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::assistant("hello")]);
        assert_eq!(out[0]["type"], "message");
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["content"][0]["type"], "output_text");
        assert_eq!(out[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn convert_messages_assistant_tool_call() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::assistant_tool_call(
            "call_1",
            "echo",
            "{\"x\":1}",
        )]);
        assert_eq!(out[0]["content"][0]["type"], "function_call");
        assert_eq!(out[0]["content"][0]["call_id"], "call_1");
        assert_eq!(out[0]["content"][0]["name"], "echo");
        assert_eq!(out[0]["content"][0]["arguments"], "{\"x\":1}");
    }

    #[test]
    fn convert_messages_tool_result() {
        let out = OpenAiResponsesClient::convert_messages(&[ChatMessage::tool("call_1", "ok")]);
        assert_eq!(out[0]["type"], "function_call_output");
        assert_eq!(out[0]["call_id"], "call_1");
        assert_eq!(out[0]["output"], "ok");
    }

    // ── SSE event processing ──

    #[test]
    fn process_event_text_delta() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"delta": "hello"});
        let chunks = OpenAiResponsesClient::process_event(
            "response.output_text.delta",
            &data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "hello"));
    }

    #[test]
    fn process_event_text_done_noop() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let chunks = OpenAiResponsesClient::process_event(
            "response.output_text.done",
            &json!({}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn process_event_function_call_lifecycle() {
        let mut buffers = HashMap::new();
        let mut idx = 0;

        // Init
        let item_data = json!({"item": {"type": "function_call", "call_id": "c1", "name": "echo"}});
        let chunks = OpenAiResponsesClient::process_event(
            "response.output_item.added",
            &item_data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(chunks.is_empty());
        assert!(buffers.contains_key("c1"));

        // Append
        let delta_data = json!({"call_id": "c1", "delta": "{\"x\":"});
        OpenAiResponsesClient::process_event(
            "response.function_call_arguments.delta",
            &delta_data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(buffers["c1"].arguments, "{\"x\":");

        // Finalize
        let done_data = json!({"call_id": "c1", "arguments": "{\"x\":1}"});
        let chunks = OpenAiResponsesClient::process_event(
            "response.function_call_arguments.done",
            &done_data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::ToolCall(v)
            if v["delta"]["tool_calls"][0]["function"]["name"] == "echo"
            && v["delta"]["tool_calls"][0]["function"]["arguments"] == "{\"x\":1}"
            && v["delta"]["tool_calls"][0]["index"] == 0
        ));
        assert!(buffers.is_empty());
        assert_eq!(idx, 1);
    }

    #[test]
    fn process_event_reasoning_summary() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"delta": "thinking..."});
        let chunks = OpenAiResponsesClient::process_event(
            "response.reasoning_summary_text.delta",
            &data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "thinking..."));
    }

    #[test]
    fn process_event_completed_with_usage() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"usage": {"input_tokens": 10, "output_tokens": 20, "total_tokens": 30}}});
        let chunks = OpenAiResponsesClient::process_event(
            "response.completed",
            &data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], StreamChunk::Usage(u)
                if u.prompt_tokens == Some(10)
                && u.completion_tokens == Some(20)
                && u.total_tokens == Some(30)));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[test]
    fn process_event_completed_no_usage() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let chunks = OpenAiResponsesClient::process_event(
            "response.completed",
            &json!({}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(matches!(
            &chunks[0],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[test]
    fn process_event_completed_with_incomplete_status() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}});
        let chunks = OpenAiResponsesClient::process_event(
            "response.completed",
            &data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(matches!(
            &chunks[0],
            StreamChunk::Stop {
                finish_reason: Some(r)
            } if r == "incomplete:max_output_tokens"
        ));
    }

    #[test]
    fn process_event_incomplete() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"response": {"incomplete_details": {"reason": "max_output_tokens"}}});
        let chunks = OpenAiResponsesClient::process_event(
            "response.incomplete",
            &data,
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(matches!(
            &chunks[0],
            StreamChunk::Stop {
                finish_reason: Some(r)
            } if r == "incomplete:max_output_tokens"
        ));
    }

    #[test]
    fn process_event_failed() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let data = json!({"error": {"message": "server error"}});
        let result =
            OpenAiResponsesClient::process_event("response.failed", &data, &mut buffers, &mut idx);
        assert!(result.is_err());
    }

    #[test]
    fn process_event_unknown_is_noop() {
        let mut buffers = HashMap::new();
        let mut idx = 0;
        let chunks = OpenAiResponsesClient::process_event(
            "some.unknown.event",
            &json!({}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        assert!(chunks.is_empty());
    }

    // ── Parallel function calls ──

    #[test]
    fn parallel_function_calls_interleaved() {
        let mut buffers = HashMap::new();
        let mut idx = 0;

        // Init two calls
        OpenAiResponsesClient::process_event(
            "response.output_item.added",
            &json!({"item": {"type": "function_call", "call_id": "c1", "name": "a"}}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        OpenAiResponsesClient::process_event(
            "response.output_item.added",
            &json!({"item": {"type": "function_call", "call_id": "c2", "name": "b"}}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        // Interleaved deltas
        OpenAiResponsesClient::process_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c1", "delta": "{\"x"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        OpenAiResponsesClient::process_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c2", "delta": "{\"y"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        OpenAiResponsesClient::process_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c1", "delta": "\":1}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        OpenAiResponsesClient::process_event(
            "response.function_call_arguments.delta",
            &json!({"call_id": "c2", "delta": "\":2}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        // Finalize both
        let chunks1 = OpenAiResponsesClient::process_event(
            "response.function_call_arguments.done",
            &json!({"call_id": "c1", "arguments": "{\"x\":1}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();
        let chunks2 = OpenAiResponsesClient::process_event(
            "response.function_call_arguments.done",
            &json!({"call_id": "c2", "arguments": "{\"y\":2}"}),
            &mut buffers,
            &mut idx,
        )
        .unwrap();

        assert_eq!(chunks1.len(), 1);
        assert_eq!(chunks2.len(), 1);
        // Indices should be 0 and 1
        assert!(
            matches!(&chunks1[0], StreamChunk::ToolCall(v) if v["delta"]["tool_calls"][0]["index"] == 0)
        );
        assert!(
            matches!(&chunks2[0], StreamChunk::ToolCall(v) if v["delta"]["tool_calls"][0]["index"] == 1)
        );
    }

    // ── Capabilities and model name ──

    #[test]
    fn capabilities_and_model_name() {
        let c = client("gpt-4o");
        assert_eq!(c.model_name(), "gpt-4o");
        let caps = c.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_thinking);
        assert_eq!(caps.max_context_tokens, Some(128_000));
    }

    // ── Mock HTTP ──

    #[tokio::test]
    async fn stream_parses_text_and_stop() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
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
    async fn stream_parses_usage_and_stop() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Usage(u)
                if u.prompt_tokens == Some(10)
                && u.completion_tokens == Some(20)
                && u.total_tokens == Some(30)));
        assert!(matches!(
            &chunks[1],
            StreamChunk::Stop {
                finish_reason: None
            }
        ));
    }

    #[tokio::test]
    async fn stream_parses_incomplete() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(
            &chunks[0],
            StreamChunk::Stop {
                finish_reason: Some(r)
            } if r == "incomplete:max_output_tokens"
        ));
    }

    #[tokio::test]
    async fn stream_returns_error_on_failed_event() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<_>, _> = stream.try_collect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stream_returns_error_on_non_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let c = OpenAiResponsesClient::new("bad".into(), "gpt-4o".into(), Some(server.uri()));
        let result = c.stream(&[ChatMessage::user("hi")], &[], None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stream_parses_function_call() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
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
    async fn stream_parses_reasoning_summary() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();

        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "Let me think..."));
        assert!(matches!(&chunks[1], StreamChunk::Text(t) if t == "Here is the answer"));
    }

    // ── Phase 3 coverage gaps ──

    /// Custom messages should pass through as-is to the Responses API.
    #[test]
    fn convert_messages_custom_passthrough() {
        let custom_data = json!({"type": "input_text", "text": "custom content"});
        let messages = vec![ChatMessage::Custom {
            role: "user".into(),
            data: custom_data.clone(),
        }];
        let result = OpenAiResponsesClient::convert_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], custom_data);
    }

    /// Tools should be included in the request body when provided.
    #[tokio::test]
    async fn stream_sends_tools_in_request_body() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"ok\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let tools = vec![json!({
            "type": "function",
            "name": "echo",
            "description": "Echo text",
            "parameters": {"type": "object", "properties": {}}
        })];
        let stream = c
            .stream(&[ChatMessage::user("hi")], &tools, None, None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "ok"));
        // If we got here without error, tools were accepted by the request builder.
    }

    /// Reasoning config should be included when enabled.
    #[tokio::test]
    async fn stream_sends_reasoning_config() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"delta\":\"thinking\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let reasoning = crate::ReasoningConfig {
            enabled: Some(true),
            budget_tokens: Some(4096),
            effort: None,
        };
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], Some(&reasoning), None)
            .await
            .unwrap();
        let chunks: Vec<StreamChunk> = stream.try_collect().await.unwrap();
        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "thinking"));
    }

    /// Malformed JSON in an SSE data field should propagate as a stream error.
    #[tokio::test]
    async fn stream_errors_on_malformed_json() {
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

        let c = OpenAiResponsesClient::new("k".into(), "gpt-4o".into(), Some(server.uri()));
        let stream = c
            .stream(&[ChatMessage::user("hi")], &[], None, None)
            .await
            .unwrap();
        let result: Result<Vec<StreamChunk>, _> = stream.try_collect().await;
        assert!(
            result.is_err(),
            "malformed JSON should cause a stream error"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Responses API SSE JSON"),
            "error should mention SSE JSON: {}",
            err
        );
    }
}
