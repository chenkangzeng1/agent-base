use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::types::{AgentResult, AgentError, AgentEvent, SessionId};

pub mod policy;

pub use policy::ToolPolicy;

#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    pub summary: String,
    pub raw: Option<Value>,
    pub control_flow: ToolControlFlow,
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
    pub event_bus: broadcast::Sender<AgentEvent>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> Value;
    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput>;
}

#[async_trait]
pub trait TypedTool: Send + Sync {
    type Args: serde::de::DeserializeOwned;
    type Output: serde::Serialize;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    async fn call_typed(&self, args: Self::Args, ctx: &ToolContext) -> AgentResult<Self::Output>;

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
            control_flow: ToolControlFlow::Break,
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
}
