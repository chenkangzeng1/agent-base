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

#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    pub summary: String,
    pub raw: Option<Value>,
    pub control_flow: ToolControlFlow,
    pub truncation: Option<TruncationInfo>,
}

#[derive(Clone, Debug)]
pub struct TruncationInfo {
    pub original_summary_len: usize,
    pub original_raw_len: Option<usize>,
    pub max_allowed_chars: usize,
}

#[derive(Clone, Debug, Default)]
pub enum ToolControlFlow {
    #[default]
    Break,
    Continue,
}

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
    fn definition(&self) -> Value;
    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput>;

    /// Return `Some(&dyn FrameworkTool)` if this tool is a framework-internal tool
    /// that needs engine infrastructure injection (EventBus).
    /// Default returns `None` — user-defined tools need not override this.
    #[allow(private_interfaces)]
    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> {
        None
    }

    /// Machine-readable metadata for tool introspection.
    ///
    /// The default implementation extracts `name` and `description` from
    /// [`Tool::name`] and [`Tool::definition`], sets `origin` to `"custom"`,
    /// and leaves `requirements` empty. Tool authors are encouraged to
    /// override this to provide an accurate `origin` and `version`.
    fn metadata(&self) -> ToolMetadata {
        let name = self.name().to_string();
        let description = self
            .definition()
            .get("function")
            .and_then(|f| f.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        ToolMetadata {
            name,
            description,
            origin: "custom".to_string(),
            version: "unknown".to_string(),
            requirements: vec![],
        }
    }
}

/// Marker trait for framework-internal tools that require engine infrastructure.
///
/// Framework tools implement this trait to receive `EventBus`
/// references during `AgentBuilder::build()`. User-defined tools do not need this.
///
/// All methods have default no-op implementations so only the needed injection
/// points need to be overridden.
pub(crate) trait FrameworkTool: Tool {
    /// Inject the internal event bus. Called once during build.
    fn set_event_bus(&self, _event_bus: crate::engine::EventBus) {}
}

#[async_trait]
pub trait TypedTool: Send + Sync {
    type Args: serde::de::DeserializeOwned;
    type Output: serde::Serialize;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    async fn call_typed(&self, args: Self::Args, ctx: &ToolContext) -> AgentResult<Self::Output>;

    fn control_flow() -> ToolControlFlow
    where
        Self: Sized,
    {
        ToolControlFlow::Break
    }

    fn format_output(&self, output: Self::Output) -> String {
        serde_json::to_string(&output).unwrap_or_default()
    }
}

#[async_trait]
impl<T: TypedTool + Send + Sync + 'static> Tool for T {
    fn name(&self) -> &'static str {
        TypedTool::name(self)
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters_schema(),
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let typed_args: T::Args =
            serde_json::from_value(args.clone()).map_err(|_| AgentError::ToolArgsInvalid {
                name: self.name().to_string(),
                raw: args.to_string(),
            })?;
        let output = self.call_typed(typed_args, ctx).await?;
        let output_json = serde_json::to_value(&output).ok();
        let summary = self.format_output(output);
        Ok(ToolOutput {
            summary,
            raw: output_json,
            control_flow: T::control_flow(),
            truncation: None,
        })
    }
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

    pub fn update(&mut self, tool: impl Tool + 'static) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }

    /// Remove a tool from the registry by name.
    pub fn remove(&mut self, name: &str) {
        self.tools.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<ToolRef> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<Value> {
        self.tools.values().map(|tool| tool.definition()).collect()
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

    /// Inject the internal `EventBus` into framework-provided tools.
    pub(crate) fn inject_event_bus(&self, event_bus: &crate::engine::EventBus) {
        for tool in self.tools.values() {
            if let Some(fw) = tool.as_framework_tool() {
                fw.set_event_bus(event_bus.clone());
            }
        }
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
}
