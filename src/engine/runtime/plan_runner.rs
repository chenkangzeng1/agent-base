use std::sync::Arc;
use tokio::sync::RwLock;

use crate::engine::context::ContextWindowManager;
use crate::engine::middleware::MiddlewareRef;
use crate::engine::runtime::llm_engine::LlmEngine;
use crate::engine::runtime::session_manager::SessionManager;
use crate::engine::runtime::tool_engine::ToolEngine;
use crate::engine::runtime::event_bus::EventBus;
use crate::types::AgentConfig;

pub(crate) struct PlanRunner {
    pub(crate) config: Arc<RwLock<AgentConfig>>,
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
            config: Arc::new(RwLock::new(config)),
            llm_engine,
            tool_engine,
            session_manager,
            event_bus,
            context_manager,
            middlewares,
        }
    }

    /// 获取 config 的克隆（同步版本，用于兼容旧代码）
    /// 注意：只在同步上下文中使用，异步上下文请使用 config_snapshot_async
    pub fn config_snapshot(&self) -> AgentConfig {
        self.config.blocking_read().clone()
    }

    /// 获取 config 的克隆（异步版本）
    pub async fn config_snapshot_async(&self) -> AgentConfig {
        self.config.read().await.clone()
    }
}
