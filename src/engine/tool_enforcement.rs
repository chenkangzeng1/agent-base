use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use crate::engine::middleware::{Middleware, PostLlmCtx};
use crate::types::AgentResult;

pub struct ToolEnforcementConfig {
    pub max_nudges: usize,
    pub nudge_message: String,
    pub first_turn_only: bool,
    pub min_tools_threshold: usize,
}

impl Default for ToolEnforcementConfig {
    fn default() -> Self {
        Self {
            max_nudges: 3,
            nudge_message: "CRITICAL: You have tools available but did not call any. \
                             Call the appropriate tool NOW. \
                             关键提示：你有可用的工具但没有调用。立即使用工具执行。"
                .to_string(),
            first_turn_only: true,
            min_tools_threshold: 1,
        }
    }
}

pub struct ToolEnforcementMiddleware {
    config: ToolEnforcementConfig,
    nudge_count: AtomicUsize,
}

impl ToolEnforcementMiddleware {
    pub fn new(config: ToolEnforcementConfig) -> Self {
        Self {
            config,
            nudge_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Middleware for ToolEnforcementMiddleware {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        if ctx.available_tools.len() < self.config.min_tools_threshold {
            return Ok(());
        }
        if self.config.first_turn_only && ctx.total_tool_calls > 0 {
            return Ok(());
        }
        if ctx.is_tool_call {
            return Ok(());
        }
        if ctx.full_text.is_empty() {
            return Ok(());
        }

        let count = self.nudge_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.config.max_nudges {
            return Ok(());
        }

        tracing::info!(
            nudge_count = count,
            full_text_len = ctx.full_text.len(),
            "ToolEnforcement: suppressing text response, injecting nudge"
        );

        ctx.skip_push = true;
        ctx.follow_up_message = Some(self.config.nudge_message.clone());

        Ok(())
    }
}
