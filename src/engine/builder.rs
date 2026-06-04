use std::sync::Arc;

use crate::llm::{LlmClient, ReasoningConfig};
use crate::tool::{Tool, ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, ResponseFormat, RetryConfig, SessionIdGenerator, AtomicU64SessionIdGenerator};

use super::approval::ApprovalHandler;
use super::context::ContextWindowManager;
use super::middleware::{Middleware, MiddlewareRef};
use super::recovery::{StopOnError, ToolErrorRecovery};
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
    error_recovery: Option<Arc<dyn ToolErrorRecovery>>,
    event_bus_capacity: usize,
    session_id_generator: Option<Arc<dyn SessionIdGenerator>>,
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
            error_recovery: None,
            event_bus_capacity: 2048,
            session_id_generator: None,
        }
    }

    pub fn event_bus_capacity(mut self, capacity: usize) -> Self {
        self.event_bus_capacity = capacity;
        self
    }

    pub fn session_id_generator(mut self, generator: Arc<dyn SessionIdGenerator>) -> Self {
        self.session_id_generator = Some(generator);
        self
    }

    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn enable_thought(mut self, enable: bool) -> Self {
        self.config.enable_thought = enable;
        self
    }

    pub fn reasoning(mut self, config: ReasoningConfig) -> Self {
        self.config.reasoning = Some(config);
        self
    }

    pub fn enable_thinking(mut self, enable: bool) -> Self {
        let mut config = self.config.reasoning.take().unwrap_or_default();
        config.enabled = Some(enable);
        self.config.reasoning = Some(config);
        self
    }

    pub fn thinking_budget(mut self, budget: u64) -> Self {
        let mut config = self.config.reasoning.take().unwrap_or_default();
        config.budget_tokens = Some(budget);
        self.config.reasoning = Some(config);
        self
    }

    pub fn tool_timeout(mut self, timeout_ms: u64) -> Self {
        self.config.tool.tool_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn max_tool_output_chars(mut self, max_chars: usize) -> Self {
        self.config.tool.max_tool_output_chars = Some(max_chars);
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
        self.config.llm.response_format = Some(format);
        self
    }

    pub fn llm_retry(mut self, retry: RetryConfig) -> Self {
        self.config.llm.llm_retry = Some(retry);
        self
    }

    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    pub fn error_recovery(mut self, recovery: Arc<dyn ToolErrorRecovery>) -> Self {
        self.error_recovery = Some(recovery);
        self
    }

    pub fn max_sessions(mut self, max: usize) -> Self {
        self.config.session.max_sessions = Some(max);
        self
    }

    pub fn max_turns_per_session(mut self, max: usize) -> Self {
        self.config.session.max_turns_per_session = Some(max);
        self
    }

    pub fn max_message_tokens(mut self, max: usize) -> Self {
        self.config.session.max_message_tokens = Some(max);
        self
    }

    pub fn tool_error_retry_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.tool.tool_error_retry_prompt = Some(prompt.into());
        self
    }

    pub fn language(mut self, language: crate::types::Language) -> Self {
        self.config.language = language;
        self
    }

    pub fn build(mut self) -> crate::types::AgentResult<AgentRuntime> {
        tracing::info!(
            tool_count = self.tools.len(),
            middleware_count = self.middlewares.len(),
            has_approval = self.approval_handler.is_some(),
            has_context_window = self.context_manager.is_some(),
            "building agent runtime"
        );

        let event_bus = super::runtime::EventBus::new(self.event_bus_capacity);

        // Inject EventBus into framework-provided tools that need it
        // (PlanOrchestrator, PlanExecTool)
        self.tools.inject_event_bus(&event_bus);

        let session_store = self
            .session_store
            .unwrap_or_else(|| Arc::new(InMemorySessionStore::new()));
        let error_recovery = self
            .error_recovery
            .unwrap_or_else(|| Arc::new(StopOnError));
        let session_id_generator = self
            .session_id_generator
            .unwrap_or_else(|| Arc::new(AtomicU64SessionIdGenerator::default()));

        let session_manager = super::runtime::SessionManager::new(
            session_id_generator,
            session_store,
            self.config.session.clone(),
        );

        let llm_engine = super::runtime::LlmEngine::new(
            self.client.clone(),
            event_bus.clone(),
        );

        let tool_engine = super::runtime::ToolEngine::new(
            self.tools,
            self.approval_handler,
            self.tool_policy,
            error_recovery,
            event_bus.clone(),
        );

        Ok(AgentRuntime {
            config: self.config,
            llm_engine,
            tool_engine,
            session_manager,
            event_bus,
            context_manager: self.context_manager,
            middlewares: self.middlewares,
        })
    }
}
