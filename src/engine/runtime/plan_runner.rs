use crate::engine::context::ContextWindowManager;
use crate::engine::middleware::MiddlewareRef;
use crate::engine::runtime::llm_engine::LlmEngine;
use crate::engine::runtime::session_manager::SessionManager;
use crate::engine::runtime::tool_engine::ToolEngine;
use crate::engine::runtime::event_bus::EventBus;
use crate::types::AgentConfig;

pub(crate) struct PlanRunner {
    pub(crate) config: AgentConfig,
    pub(crate) llm_engine: LlmEngine,
    pub(crate) tool_engine: ToolEngine,
    pub(crate) session_manager: SessionManager,
    pub(crate) event_bus: EventBus,
    pub(crate) context_manager: Option<ContextWindowManager>,
    pub(crate) middlewares: Vec<MiddlewareRef>,
}

impl PlanRunner {
    pub fn new(
        config: AgentConfig,
        llm_engine: LlmEngine,
        tool_engine: ToolEngine,
        session_manager: SessionManager,
        event_bus: EventBus,
        context_manager: Option<ContextWindowManager>,
        middlewares: Vec<MiddlewareRef>,
    ) -> Self {
        Self {
            config,
            llm_engine,
            tool_engine,
            session_manager,
            event_bus,
            context_manager,
            middlewares,
        }
    }
}
