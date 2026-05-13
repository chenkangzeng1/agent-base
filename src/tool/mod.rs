use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{AgentEvent, SessionId};

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
    async fn call(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput>;
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
