use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::broadcast;

use crate::llm::LlmClient;
use crate::tool::{Tool, ToolPolicy, ToolRegistry};
use crate::types::AgentConfig;

use super::approval::ApprovalHandler;
use super::AgentRuntime;

pub struct AgentBuilder {
    client: Arc<dyn LlmClient>,
    config: AgentConfig,
    tools: ToolRegistry,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
}

impl AgentBuilder {
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self {
            client,
            config: AgentConfig::default(),
            tools: ToolRegistry::default(),
            approval_handler: None,
            tool_policy: None,
        }
    }

    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn enable_thought(mut self, enable: bool) -> Self {
        self.config.enable_thought = enable;
        self
    }

    pub fn enable_thinking(mut self, enable: bool) -> Self {
        self.config.enable_thinking = Some(enable);
        self
    }

    pub fn register_tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    pub fn approval_handler(mut self, handler: Arc<dyn ApprovalHandler>) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn tool_policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self {
        self.tool_policy = Some(policy);
        self
    }

    pub fn build(self) -> AgentRuntime {
        let (event_bus, _) = broadcast::channel(2048);
        AgentRuntime {
            client: self.client,
            config: self.config,
            tools: self.tools,
            approval_handler: self.approval_handler,
            tool_policy: self.tool_policy,
            event_bus,
            next_session_id: AtomicU64::new(1),
            sessions: HashMap::new(),
        }
    }
}
