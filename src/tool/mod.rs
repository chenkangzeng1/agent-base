use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::engine::SessionStore;
use crate::llm::StreamClient;
use crate::types::{AgentError, AgentResult, SessionId, UserEvent};

pub mod auto_continue;
pub mod policy;
pub mod subagent;
pub mod update_plan;

pub use auto_continue::AutoContinueTool;
pub use subagent::{SubAgentSessionPolicy, SubAgentTool};
pub use update_plan::UpdatePlanTool;

pub use policy::ToolPolicy;

/// Structured content returned by a tool, aligned with the MCP `content`
/// array shape (no envelope, no orchestration/failure/truncation semantics).
///
/// Only `Text` is consumed by the first LLM adapter; `Image` is shape-reserved
/// and the adapter reports "not supported" rather than silently dropping it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    /// Base64-encoded image payload.
    Image {
        data: String,
        mime_type: String,
    },
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text { text: s.into() }
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Content::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

impl From<Content> for Vec<Content> {
    fn from(c: Content) -> Self {
        vec![c]
    }
}

/// Join the textual portion of tool output into a single string for display
/// and session history. Non-text variants (e.g. `Image`) are skipped.
pub fn content_text(contents: &[Content]) -> String {
    contents
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(text.as_str()),
            Content::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: SessionId,
    /// Channel for sending user-space events (progress, sub-agent, structured).
    /// Tools should use `emit_user_event()` or `emit_progress()`.
    pub user_event_tx: mpsc::UnboundedSender<UserEvent>,
    pub llm_client: Option<Arc<dyn StreamClient>>,
    pub session_store: Option<Arc<dyn SessionStore>>,
    /// Language preference for tool output.
    /// Defaults to `Language::En` if not set.
    pub language: crate::types::Language,
    /// Cancellation token for checking if the operation should be cancelled.
    pub cancel_token: tokio_util::sync::CancellationToken,
    /// Internal runtime event bus (framework tools emit `RuntimeEvent`s here).
    /// `pub(crate)` — engine-internal; user tools should use `emit_user_event()`.
    pub(crate) event_bus: crate::engine::EventBus,
}

impl ToolContext {
    /// Send a user-space event (progress, sub-agent forwarding, structured data).
    pub fn emit_user_event(&self, event: UserEvent) {
        let _ = self.user_event_tx.send(event);
    }

    /// Convenience: send a progress event with text.
    pub fn emit_progress(&self, text: impl Into<String>) {
        self.emit_user_event(UserEvent::Progress { text: text.into() });
    }

    /// Convenience: emit a partial result during long-running tool execution.
    /// `is_partial: true` means more output is coming; `false` means final.
    pub fn emit_partial_result(
        &self,
        tool_call_id: &str,
        content: impl Into<String>,
        is_partial: bool,
    ) {
        self.emit_user_event(UserEvent::ToolPartialResult {
            tool_call_id: tool_call_id.to_string(),
            content: content.into(),
            is_partial,
        });
    }

    /// Test-only constructor: a `ToolContext` with a disconnected event
    /// channel and no LLM/session backends. Lets downstream tests drop their
    /// bespoke `dummy_ctx()` helpers.
    pub fn for_test() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        ToolContext {
            session_id: SessionId::new(0),
            user_event_tx: tx,
            llm_client: None,
            session_store: None,
            language: crate::types::Language::En,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            event_bus: crate::engine::EventBus::new(1),
        }
    }
}

/// Machine-readable metadata for a registered tool — origin, version, and
/// runtime requirements in a stable shape consumers can inspect without
/// parsing the LLM-facing definition JSON.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolMetadata {
    /// Tool name (matches [`Tool::name`]).
    pub name: String,
    /// Human-readable description (matches the description in [`Tool::definition`]).
    pub description: String,
    /// Where this tool comes from: a crate name (e.g. `"phi-tools"`), a
    /// framework identifier (`"agent-base"`, `"agent-works"`), or
    /// `"custom"` for user-defined tools.
    pub origin: String,
    /// Crate / package version, or `"unknown"` when built outside a crate.
    pub version: String,
    /// Optional runtime requirements or capabilities this tool depends on
    /// (e.g. `["chrome-cdp"]` for browser tools). Empty when there are
    /// none.
    pub requirements: Vec<String>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    /// Human-readable description of what this tool does and when to use it.
    fn description(&self) -> &'static str;
    /// JSON Schema for the tool's input arguments (MCP `inputSchema` shape,
    /// without the provider envelope).
    fn schema(&self) -> Value;
    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>>;

    /// Machine-readable metadata for tool introspection.
    ///
    /// The default implementation derives `name` and `description` from
    /// [`Tool::name`] and [`Tool::description`], sets `origin` to `"custom"`,
    /// and leaves `requirements` empty. Tool authors are encouraged to
    /// override this to provide an accurate `origin` and `version`.
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: self.name().to_string(),
            description: self.description().to_string(),
            origin: "custom".to_string(),
            version: "unknown".to_string(),
            requirements: vec![],
        }
    }
}

#[async_trait]
pub trait TypedTool: Send + Sync {
    type Args: serde::de::DeserializeOwned + schemars::JsonSchema;
    type Output: serde::Serialize;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    async fn call_typed(&self, args: Self::Args, ctx: &ToolContext) -> AgentResult<Self::Output>;

    fn format_output(&self, output: Self::Output) -> Content {
        Content::text(serde_json::to_string(&output).unwrap_or_default())
    }

    /// Machine-readable origin of this tool (crate name, `"agent-base"`, or `"custom"`).
    fn origin(&self) -> &'static str {
        "custom"
    }

    /// Crate/package version, or `"unknown"` when built outside a crate.
    fn version(&self) -> &'static str {
        "unknown"
    }
}

#[async_trait]
impl<T: TypedTool + Send + Sync + 'static> Tool for T {
    fn name(&self) -> &'static str {
        TypedTool::name(self)
    }

    fn description(&self) -> &'static str {
        TypedTool::description(self)
    }

    fn schema(&self) -> Value {
        // Generate a provider-safe JSON Schema: Draft 7 (not 2020-12), with
        // nested subschemas inlined (no `$ref`/`$defs`/`/definitions`) and no
        // root `$schema`/meta-schema key. OpenAI-compatible function-calling
        // rejects `$ref` and 2020-12's `$defs`, so the default 2020-12 output
        // would break any `Args` containing a nested enum or struct.
        let settings = schemars::generate::SchemaSettings::draft07().with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        });
        let generator = schemars::SchemaGenerator::new(settings);
        let schema = generator.into_root_schema_for::<T::Args>();
        serde_json::to_value(schema).unwrap_or(Value::Null)
    }

    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: self.name().to_string(),
            description: self.description().to_string(),
            origin: self.origin().to_string(),
            version: self.version().to_string(),
            requirements: vec![],
        }
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        let typed_args: T::Args =
            serde_json::from_value(args.clone()).map_err(|_| AgentError::ToolArgsInvalid {
                name: self.name().to_string(),
                raw: args.to_string(),
            })?;
        let output = self.call_typed(typed_args, ctx).await?;
        Ok(vec![self.format_output(output)])
    }
}

/// Render a single tool's definition into the OpenAI function-calling
/// envelope. Tools only provide `name`/`description`/`schema`; the envelope is
/// assembled here at the LLM boundary (Anthropic gets its own renderer, and
/// tool authors stay protocol-agnostic).
pub fn render_tool_definition(tool: &dyn Tool) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name(),
            "description": tool.description(),
            "parameters": tool.schema(),
        }
    })
}

pub(crate) type ToolRef = Arc<dyn Tool>;

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolRef>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }

    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Remove a tool from the registry by name.
    pub fn remove(&mut self, name: &str) {
        self.tools.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<ToolRef> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<Value> {
        let mut tools: Vec<_> = self.tools.values().collect();
        tools.sort_by_key(|t| t.name());
        tools
            .into_iter()
            .map(|t| render_tool_definition(t.as_ref()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Collect metadata for every registered tool, sorted by name.
    ///
    /// This is the preferred introspection API for consumers — it returns a
    /// stable `ToolMetadata` struct per tool instead of having callers parse
    /// the LLM-facing JSON definitions.
    pub fn metadatas(&self) -> Vec<ToolMetadata> {
        let mut list: Vec<_> = self.tools.values().map(|tool| tool.metadata()).collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_text_ctor_and_into_vec() {
        let c = Content::text("hello");
        let v: Vec<Content> = c.clone().into();
        assert_eq!(v.len(), 1);
        assert!(matches!(v[0], Content::Text { .. }));
        assert!(matches!(&c, Content::Text { text } if text == "hello"));
    }

    #[test]
    fn content_serializes_with_type_tag() {
        let c = Content::text("hi");
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["type"], "text");
        assert_eq!(j["text"], "hi");
    }

    #[test]
    fn tool_context_for_test_constructs() {
        let ctx = ToolContext::for_test();
        assert!(ctx.llm_client.is_none());
        assert!(ctx.session_store.is_none());
        assert!(!ctx.cancel_token.is_cancelled());
        ctx.emit_progress("hello");
    }

    #[test]
    fn typed_tool_schema_is_derived_from_args() {
        #[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
        struct GreetArgs {
            name: String,
            #[serde(default)]
            times: u32,
        }

        let schema = schemars::schema_for!(GreetArgs);
        let j = serde_json::to_value(&schema).unwrap();
        // The derived schema exposes the struct's fields as object properties.
        assert!(j["properties"]["name"].is_object());
        assert!(j["properties"]["times"].is_object());
    }

    #[test]
    fn typed_tool_schema_is_provider_safe_for_nested_enum() {
        #[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
        enum Status {
            Active,
            Paused,
        }

        #[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
        struct Args {
            name: String,
            status: Status,
        }

        #[derive(Default)]
        struct NestedTool;
        #[async_trait]
        impl TypedTool for NestedTool {
            type Args = Args;
            type Output = String;
            fn name(&self) -> &'static str {
                "nested"
            }
            fn description(&self) -> &'static str {
                ""
            }
            async fn call_typed(
                &self,
                _args: Args,
                _ctx: &ToolContext,
            ) -> crate::types::AgentResult<String> {
                Ok(String::new())
            }
        }

        let schema = Tool::schema(&NestedTool);
        let raw = schema.to_string();
        // OpenAI-compatible function-calling rejects $ref / $defs; the nested
        // enum must be inlined rather than referenced.
        assert!(!raw.contains("$ref"), "schema contains $ref: {raw}");
        assert!(!raw.contains("$defs"), "schema contains $defs: {raw}");
        assert!(
            !raw.contains("definitions"),
            "schema has definitions: {raw}"
        );
        assert!(schema.get("$schema").is_none(), "schema has $schema key");

        // The enum variants are inlined directly under properties.status.
        let variants: Vec<&str> = schema["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(variants.contains(&"Active"), "missing Active: {variants:?}");
        assert!(variants.contains(&"Paused"), "missing Paused: {variants:?}");
    }

    #[test]
    fn definitions_are_sorted_by_name() {
        struct NamedTool(&'static str);
        #[async_trait::async_trait]
        impl Tool for NamedTool {
            fn name(&self) -> &'static str {
                self.0
            }
            fn description(&self) -> &'static str {
                ""
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::Value::Null
            }
            async fn call(
                &self,
                _args: &serde_json::Value,
                _ctx: &ToolContext,
            ) -> crate::types::AgentResult<Vec<Content>> {
                Ok(vec![])
            }
        }

        let mut registry = ToolRegistry::default();
        registry.register(NamedTool("zeta"));
        registry.register(NamedTool("alpha"));
        registry.register(NamedTool("mike"));

        let defs = registry.definitions();
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha", "mike", "zeta"]);
    }
}
