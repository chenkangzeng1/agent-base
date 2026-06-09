use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::llm::LlmClient;
use crate::types::{AgentResult, AgentError, SessionId, UserEvent};
use crate::engine::SessionStore;

pub mod auto_continue;
pub mod policy;
pub mod subagent;

pub use auto_continue::AutoContinueTool;
pub use subagent::{SubAgentSessionPolicy, SubAgentTool};

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

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: SessionId,
    /// Channel for sending user-space events (progress, sub-agent, structured).
    /// Tools should use `emit_user_event()` or `emit_progress()`.
    pub user_event_tx: mpsc::UnboundedSender<UserEvent>,
    pub llm_client: Option<Arc<dyn LlmClient>>,
    pub session_store: Option<Arc<dyn SessionStore>>,
    /// Language preference for tool output.
    /// Defaults to `Language::En` if not set.
    pub language: crate::types::Language,
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
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> Value;
    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput>;

    /// Return `Some(&dyn FrameworkTool)` if this tool is a framework-internal tool
    /// that needs engine infrastructure injection (EventBus, PlanRunner).
    /// Default returns `None` — user-defined tools need not override this.
    #[allow(private_interfaces)]
    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> { None }
}

/// Marker trait for framework-internal tools that require engine infrastructure.
///
/// Framework tools implement this trait to receive `EventBus` and `PlanRunner`
/// references during `AgentBuilder::build()`. User-defined tools do not need this.
///
/// All methods have default no-op implementations so only the needed injection
/// points need to be overridden.
pub(crate) trait FrameworkTool: Tool {
    /// Inject the internal event bus. Called once during build.
    fn set_event_bus(&self, _event_bus: crate::engine::EventBus) {}

    /// Inject the PlanRunner reference. Called once after PlanRunner construction.
    fn set_plan_runner(&self, _runner: &Arc<crate::engine::PlanRunner>) {}
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
        let typed_args: T::Args = serde_json::from_value(args.clone())
            .map_err(|_| AgentError::ToolArgsInvalid {
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

    /// Inject the internal `EventBus` into framework-provided tools.
    pub fn inject_event_bus(&self, event_bus: &crate::engine::EventBus) {
        for tool in self.tools.values() {
            if let Some(fw) = tool.as_framework_tool() {
                fw.set_event_bus(event_bus.clone());
            }
        }
    }

    /// Inject the `PlanRunner` into framework-provided tools (via `Weak` to avoid circular Arc).
    pub fn inject_plan_runner(&self, runner: &Arc<crate::engine::PlanRunner>) {
        for tool in self.tools.values() {
            if let Some(fw) = tool.as_framework_tool() {
                fw.set_plan_runner(runner);
            }
        }
    }
}
