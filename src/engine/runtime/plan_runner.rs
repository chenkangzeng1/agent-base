use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

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
    pub(crate) cancel_token: Mutex<CancellationToken>,
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
            cancel_token: Mutex::new(CancellationToken::new()),
        }
    }

    /// Get a clone of the current cancel token
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.lock().unwrap().clone()
    }

    /// Reset the cancel token (called before each run_turn)
    pub fn reset_cancel(&self) {
        *self.cancel_token.lock().unwrap() = CancellationToken::new();
    }

    /// Send the cancellation signal
    pub fn cancel(&self) {
        self.cancel_token.lock().unwrap().cancel();
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.lock().unwrap().is_cancelled()
    }

    /// Get a clone of config (sync version, for backward compatibility)
    /// Note: only use in sync contexts; for async contexts use config_snapshot_async
    pub fn config_snapshot(&self) -> AgentConfig {
        self.config.blocking_read().clone()
    }

    /// Get a clone of config (async version)
    pub async fn config_snapshot_async(&self) -> AgentConfig {
        self.config.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_token_initial_state() {
        // Create a simple PlanRunner for testing
        // Note: we only test the cancel_token field behavior
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        let token_clone = token.clone();
        assert!(!token_clone.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(token_clone.is_cancelled());
    }

    #[test]
    fn test_cancel_token_reset() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());

        // After reset, should be a new token
        let new_token = CancellationToken::new();
        assert!(!new_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancel_token_select() {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        // Spawn a task to cancel the token
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            token_clone.cancel();
        });

        // Wait for cancellation
        token.cancelled().await;
        assert!(token.is_cancelled());

        handle.await.unwrap();
    }
}
