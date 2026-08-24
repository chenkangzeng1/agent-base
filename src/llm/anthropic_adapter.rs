//! Anthropic adapter using `anthropic-rs-api` SDK.
//!
//! This adapter bridges agent-base's `LlmClient` trait with the `anthropic-rs-api`
//! crate, providing a type-safe, well-tested implementation with automatic retry,
//! streaming, and comprehensive Anthropic API support.

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

use anthropic_rs_api::types::{
    ContentBlock, ContentBlockDelta, Message, MessagesRequestBuilder, MessagesStreamEvent,
    Role, StopReason, ThinkingConfig, Tool, ToolChoice,
};
use anthropic_rs_api::{Client, AnthropicError};

use super::{LlmCapabilities, LlmClient, LlmClientConfig, ReasoningConfig, StreamChunk, UsageInfo};
use crate::types::{AgentError, AgentResult, ChatMessage, ImageAttachment, ResponseFormat};

/// Anthropic client powered by `anthropic-rs-api`.
pub struct AnthropicAdapter {
    client: Client,
    model: String,
    max_tokens: u32,
}

impl AnthropicAdapter {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
        Self::new_with_config(api_key, model, base_url, LlmClientConfig::default())
    }

    pub fn new_with_config(
        api_key: String,
        model: String,
        base_url: Option<String>,
        config: LlmClientConfig,
    ) -> Self {
        let mut builder = Client::builder()
            .api_key(api_key)
            .timeout(config.request_timeout);

        if let Some(url) = base_url {
            builder = builder.api_base(url);
        }

        let client = builder.build().expect("failed to build Anthropic client");

        Self {
            client,
            model,
            max_tokens: 8192,
        }
    }

    /// Set max_tokens for requests.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Convert agent-base ChatMessages to anthropic-rs-api Messages.
    fn convert_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<Message>) {
        let mut system_prompt: Option<String> = None;
        let mut result: Vec<Message> = Vec::new();

        for msg in messages {
            match msg {
                ChatMessage::System { content, .. } => {
                    system_prompt = Some(content.clone());
                }
                ChatMessage::User { content, images, .. } => {
                    let mut blocks = vec![ContentBlock::text(content)];

                    for img in images {
                        match img {
                            ImageAttachment::Url { url, .. } => {
                                blocks.push(ContentBlock::image_url(url));
                            }
                            ImageAttachment::Base64 {
                                data,
                                media_type,
                                ..
                            } => {
                                let mime = media_type.as_deref().unwrap_or("image/jpeg");
                                blocks.push(ContentBlock::image_base64(mime, data));
                            }
                        }
                    }

                    result.push(Message::new(Role::User, blocks));
                }
                ChatMessage::Assistant {
                    content,
                    tool_calls,
                    ..
                } => {
                    let mut blocks = Vec::new();

                    if let Some(text) = content {
                        if !text.is_empty() {
                            blocks.push(ContentBlock::text(text));
                        }
                    }

                    if let Some(tc) = tool_calls {
                        for t in tc {
                            let input: Value =
                                serde_json::from_str(&t.arguments).unwrap_or(Value::Object(Default::default()));
                            blocks.push(ContentBlock::tool_use(&t.id, &t.name, input));
                        }
                    }

                    if !blocks.is_empty() {
                        result.push(Message::new(Role::Assistant, blocks));
                    }
                }
                ChatMessage::Tool {
                    tool_call_id,
                    content,
                } => {
                    result.push(Message::new(
                        Role::User,
                        vec![ContentBlock::tool_result_text(tool_call_id, content)],
                    ));
                }
                ChatMessage::Custom { role: _, data } => {
                    result.push(Message::new(
                        Role::User,
                        vec![ContentBlock::text(data.to_string())],
                    ));
                }
            }
        }

        // Merge consecutive user messages where the first contains only tool_result blocks.
        // Also merge consecutive assistant messages.
        let mut merged: Vec<Message> = Vec::with_capacity(result.len());
        for msg in result {
            let is_user = msg.role == Role::User;
            let is_assistant = msg.role == Role::Assistant;
            let prev_is_user = merged.last().map(|m| m.role == Role::User).unwrap_or(false);
            let prev_is_assistant = merged.last().map(|m| m.role == Role::Assistant).unwrap_or(false);

            if is_user && prev_is_user {
                let prev_all_tool_results = merged
                    .last()
                    .map(|m| {
                        m.content
                            .iter()
                            .all(|c| matches!(c, ContentBlock::ToolResult { .. }))
                    })
                    .unwrap_or(false);

                if prev_all_tool_results {
                    if let Some(prev) = merged.last_mut() {
                        prev.content.extend(msg.content);
                        continue;
                    }
                }
            }

            // Merge consecutive assistant messages
            if is_assistant && prev_is_assistant {
                if let Some(prev) = merged.last_mut() {
                    prev.content.extend(msg.content);
                    continue;
                }
            }

            merged.push(msg);
        }

        (system_prompt, merged)
    }

    /// Convert OpenAI-style tools to anthropic-rs-api Tools.
    fn convert_tools(tools: &[Value]) -> Vec<Tool> {
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
                    .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                Some(Tool::new(name, description, input_schema))
            })
            .collect()
    }

    /// Convert MessagesStreamEvent to Vec<StreamChunk>.
    fn convert_event(event: MessagesStreamEvent) -> Vec<StreamChunk> {
        match event {
            MessagesStreamEvent::MessageStart { message } => {
                vec![StreamChunk::Usage(UsageInfo {
                    prompt_tokens: Some(message.usage.input_tokens as u32),
                    completion_tokens: Some(message.usage.output_tokens as u32),
                    total_tokens: None,
                })]
            }
            MessagesStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                ContentBlock::ToolUse { id, name, .. } => {
                    vec![StreamChunk::ToolCall(serde_json::json!({
                        "delta": {
                            "tool_calls": [{
                                "index": index,
                                "id": id,
                                "function": {
                                    "name": name,
                                    "arguments": "",
                                }
                            }]
                        }
                    }))]
                }
                _ => vec![StreamChunk::Text(String::new())],
            },
            MessagesStreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentBlockDelta::TextDelta { text } => {
                    vec![StreamChunk::Text(text)]
                }
                ContentBlockDelta::InputJsonDelta { partial_json } => {
                    vec![StreamChunk::ToolCall(serde_json::json!({
                        "delta": {
                            "tool_calls": [{
                                "index": index,
                                "function": {
                                    "arguments": partial_json,
                                }
                            }]
                        }
                    }))]
                }
                ContentBlockDelta::ThinkingDelta { thinking } => {
                    vec![StreamChunk::Thought(thinking)]
                }
                _ => vec![StreamChunk::Text(String::new())],
            },
            MessagesStreamEvent::ContentBlockStop { .. } => {
                vec![StreamChunk::Text(String::new())]
            }
            MessagesStreamEvent::MessageDelta { delta, usage } => {
                let mut chunks = vec![StreamChunk::Usage(UsageInfo {
                    prompt_tokens: None,
                    completion_tokens: Some(usage.output_tokens as u32),
                    total_tokens: None,
                })];

                if let Some(reason) = delta.stop_reason {
                    let finish_reason = match reason {
                        StopReason::EndTurn => "end_turn".to_string(),
                        StopReason::ToolUse => "tool_use".to_string(),
                        StopReason::MaxTokens => "max_tokens".to_string(),
                        StopReason::StopSequence => "stop_sequence".to_string(),
                        StopReason::PauseTurn => "pause_turn".to_string(),
                        StopReason::Refusal => "refusal".to_string(),
                    };
                    chunks.push(StreamChunk::Stop {
                        finish_reason: Some(finish_reason),
                    });
                }

                chunks
            }
            MessagesStreamEvent::MessageStop => {
                vec![]
            }
        }
    }

    /// Build a MessagesRequest from parameters.
    fn build_request(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
    ) -> Result<anthropic_rs_api::types::MessagesRequest, AgentError> {
        let (system, msgs) = Self::convert_messages(messages);
        let mut builder =
            MessagesRequestBuilder::new(&self.model, msgs, self.max_tokens);

        if let Some(sys) = system {
            builder = builder.system(sys);
        }

        let anthropic_tools = Self::convert_tools(tools);
        if !anthropic_tools.is_empty() {
            builder = builder.tools(anthropic_tools);
            builder = builder.tool_choice(ToolChoice::Auto);
        }

        if let Some(rc) = reasoning {
            if rc.enabled == Some(true) || rc.budget_tokens.is_some() {
                let budget = rc.budget_tokens.unwrap_or(2048) as u32;
                builder = builder.thinking(ThinkingConfig::enabled(budget));
            }
        }

        builder
            .build()
            .map_err(|e| AgentError::llm(format!("Failed to build request: {e}")))
    }
}

impl From<AnthropicError> for AgentError {
    fn from(err: AnthropicError) -> Self {
        match err {
            AnthropicError::Api(api) => AgentError::LlmApi {
                message: format!("{}: {}", api.error_type, api.message),
            },
            AnthropicError::Http(e) => AgentError::llm(format!("HTTP error: {e}")),
            AnthropicError::InvalidRequest(msg) => AgentError::llm(format!("Invalid request: {msg}")),
            AnthropicError::MissingEnvironment(var) => {
                AgentError::llm(format!("Missing environment variable: {var}"))
            }
            AnthropicError::Deserialize(e) => AgentError::llm(format!("Deserialize error: {e}")),
            AnthropicError::UnexpectedResponse { status, body } => {
                AgentError::llm(format!("Unexpected response (status {status}): {body}"))
            }
            other => AgentError::llm(format!("Anthropic error: {other}")),
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicAdapter {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        let request = self.build_request(messages, tools, reasoning)?;
        let response = self.client.messages(request).await?;

        // Convert response to JSON Value for compatibility
        let content: Vec<Value> = response
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text, .. } => {
                    serde_json::json!({"type": "text", "text": text})
                }
                ContentBlock::ToolUse { id, name, input, .. } => {
                    serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
                }
                ContentBlock::Thinking { thinking, .. } => {
                    serde_json::json!({"type": "thinking", "thinking": thinking})
                }
                _ => serde_json::json!({"type": "text", "text": ""}),
            })
            .collect();

        Ok(serde_json::json!({
            "id": response.id,
            "type": "message",
            "role": "assistant",
            "content": content,
            "model": response.model,
            "stop_reason": response.stop_reason.map(|r| match r {
                StopReason::EndTurn => "end_turn",
                StopReason::ToolUse => "tool_use",
                StopReason::MaxTokens => "max_tokens",
                _ => "other",
            }),
            "usage": {
                "input_tokens": response.usage.input_tokens,
                "output_tokens": response.usage.output_tokens,
            }
        }))
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let request = self.build_request(messages, tools, reasoning)?;
        let stream = self.client.messages_stream(request).await?;

        let converted = stream.flat_map(|event| match event {
            Ok(ev) => {
                let chunks = Self::convert_event(ev);
                futures_util::stream::iter(chunks.into_iter().map(Ok).collect::<Vec<_>>())
            }
            Err(e) => futures_util::stream::iter(vec![Err(AgentError::from(e))]),
        });

        Ok(Box::pin(converted))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_thinking: true,
            max_context_tokens: Some(200_000),
            max_output_tokens: Some(self.max_tokens),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_messages_basic() {
        let msgs = vec![ChatMessage::user("hi")];
        let (sys, out) = AnthropicAdapter::convert_messages(&msgs);
        assert!(sys.is_none());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn convert_messages_with_system() {
        let msgs = vec![ChatMessage::system("sys"), ChatMessage::user("hi")];
        let (sys, out) = AnthropicAdapter::convert_messages(&msgs);
        assert_eq!(sys.as_deref(), Some("sys"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn convert_messages_with_images() {
        let msgs = vec![ChatMessage::user_with_images(
            "pic",
            vec![
                ImageAttachment::Url {
                    url: "http://example.com/img.png".into(),
                    detail: None,
                },
                ImageAttachment::Base64 {
                    data: "abc".into(),
                    media_type: Some("image/png".into()),
                    detail: None,
                },
            ],
        )];
        let (_, out) = AnthropicAdapter::convert_messages(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.len(), 3); // text + 2 images
    }

    #[test]
    fn convert_messages_with_tool_calls() {
        let msgs = vec![
            ChatMessage::assistant("thinking"),
            ChatMessage::assistant_tool_call("id1", "echo", r#"{"x":1}"#),
            ChatMessage::tool("id1", "result"),
        ];
        let (_, out) = AnthropicAdapter::convert_messages(&msgs);
        // assistant + tool_use merged, then tool_result
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::Assistant);
        assert_eq!(out[1].role, Role::User);
    }

    #[test]
    fn convert_messages_merges_consecutive_user_tool_results() {
        let msgs = vec![
            ChatMessage::tool("id1", "result1"),
            ChatMessage::user("follow up"),
        ];
        let (_, out) = AnthropicAdapter::convert_messages(&msgs);
        // Should merge into one user message
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.len(), 2); // tool_result + text
    }

    #[test]
    fn convert_tools_maps_correctly() {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "echo",
                "description": "echo back",
                "parameters": {"type": "object", "properties": {}}
            }
        })];
        let out = AnthropicAdapter::convert_tools(&tools);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn convert_event_text_delta() {
        let event = MessagesStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ContentBlockDelta::TextDelta {
                text: "hello".into(),
            },
        };
        let chunks = AnthropicAdapter::convert_event(event);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::Text(t) if t == "hello"));
    }

    #[test]
    fn convert_event_thinking_delta() {
        let event = MessagesStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ContentBlockDelta::ThinkingDelta {
                thinking: "hmm".into(),
            },
        };
        let chunks = AnthropicAdapter::convert_event(event);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], StreamChunk::Thought(t) if t == "hmm"));
    }

    #[test]
    fn convert_event_message_delta_with_stop() {
        let event = MessagesStreamEvent::MessageDelta {
            delta: anthropic_rs_api::types::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: anthropic_rs_api::types::MessageDeltaUsage {
                output_tokens: 100,
                input_tokens: None,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };
        let chunks = AnthropicAdapter::convert_event(event);
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], StreamChunk::Usage(_)));
        assert!(
            matches!(&chunks[1], StreamChunk::Stop { finish_reason: Some(r) } if r == "end_turn")
        );
    }

    #[test]
    fn convert_event_message_stop() {
        let event = MessagesStreamEvent::MessageStop;
        let chunks = AnthropicAdapter::convert_event(event);
        assert!(chunks.is_empty());
    }

    #[test]
    fn error_conversion() {
        let err = AnthropicError::InvalidRequest("bad".into());
        let agent_err = AgentError::from(err);
        assert!(format!("{agent_err}").contains("Invalid request"));
    }
}
