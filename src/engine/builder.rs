use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::broadcast;

use crate::llm::LlmClient;
use crate::tool::{Tool, ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, ResponseFormat, RetryConfig};

use super::approval::ApprovalHandler;
use super::context::ContextWindowManager;
use super::middleware::{Middleware, MiddlewareRef};
use super::session_store::{InMemorySessionStore, SessionStore};
use super::AgentRuntime;

pub struct AgentBuilder {
    client: Arc<dyn LlmClient>,
    config: AgentConfig,
    tools: ToolRegistry,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    middlewares: Vec<MiddlewareRef>,
    context_manager: Option<ContextWindowManager>,
    session_store: Option<Arc<dyn SessionStore>>,
}

impl AgentBuilder {
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self {
            client,
            config: AgentConfig::default(),
            tools: ToolRegistry::default(),
            approval_handler: None,
            tool_policy: None,
            middlewares: Vec::new(),
            context_manager: None,
            session_store: None,
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

    pub fn tool_timeout(mut self, timeout_ms: u64) -> Self {
        self.config.tool_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn max_tool_output_chars(mut self, max_chars: usize) -> Self {
        self.config.max_tool_output_chars = Some(max_chars);
        self
    }

    pub fn register_tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    pub fn register_tool_arc(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.register_arc(tool);
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

    pub fn middleware(mut self, mw: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }

    pub fn context_window(mut self, max_tokens: usize) -> Self {
        self.context_manager = Some(ContextWindowManager::new(max_tokens));
        self
    }

    pub fn context_window_manager(mut self, manager: ContextWindowManager) -> Self {
        self.context_manager = Some(manager);
        self
    }

    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.config.response_format = Some(format);
        self
    }

    pub fn llm_retry(mut self, retry: RetryConfig) -> Self {
        self.config.llm_retry = Some(retry);
        self
    }

    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    pub fn build(self) -> AgentRuntime {
        let (event_bus, _) = broadcast::channel(2048);
        let session_store = self.session_store.unwrap_or_else(|| {
            Arc::new(InMemorySessionStore::new())
        });
        AgentRuntime {
            client: self.client,
            config: self.config,
            tools: self.tools,
            approval_handler: self.approval_handler,
            tool_policy: self.tool_policy,
            middlewares: self.middlewares,
            event_bus,
            next_session_id: AtomicU64::new(1),
            sessions: HashMap::new(),
            context_manager: self.context_manager,
            session_store,
        }
    }
}
