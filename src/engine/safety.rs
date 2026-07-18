use async_trait::async_trait;

use crate::engine::middleware::{Middleware, PostLlmCtx};
use crate::types::{AgentResult, SafetyConfig};

/// Middleware that enforces a hard limit on tool calls per turn.
///
/// Mounted on `on_post_llm` hook. When the turn's tool call count reaches
/// `max_tool_calls_per_turn`, this middleware:
/// 1. Clears `ctx.tool_calls` (discards pending tool calls)
/// 2. Injects a `follow_up_message` forcing the LLM to summarize
///
/// This is a **hard constraint** — unlike prompt rules, the model cannot bypass it.
pub struct TurnToolLimitMiddleware {
    max_tool_calls_per_turn: usize,
}

impl TurnToolLimitMiddleware {
    pub fn new(max_tool_calls_per_turn: usize) -> Self {
        Self {
            max_tool_calls_per_turn,
        }
    }

    pub fn from_config(config: &SafetyConfig) -> Self {
        Self::new(config.max_tool_calls_per_turn)
    }
}

#[async_trait]
impl Middleware for TurnToolLimitMiddleware {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        // Only intercept when the LLM is trying to call tools
        if !ctx.is_tool_call || ctx.tool_calls.is_empty() {
            return Ok(());
        }

        // Check if this turn has already hit the limit
        if ctx.turn_tool_calls >= self.max_tool_calls_per_turn {
            tracing::warn!(
                turn_tool_calls = ctx.turn_tool_calls,
                max = self.max_tool_calls_per_turn,
                pending_calls = ctx.tool_calls.len(),
                "TurnToolLimit: blocking tool calls — limit reached"
            );

            // Discard all pending tool calls
            ctx.tool_calls.clear();
            ctx.is_tool_call = false;

            // Force LLM to summarize
            ctx.follow_up_message = Some(
                "本轮工具调用已达上限。请根据已有结果总结并向用户报告。不要再调用工具。".to_string(),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionId;

    fn make_ctx(turn_tool_calls: usize, pending_calls: usize) -> PostLlmCtx {
        let tool_calls = (0..pending_calls)
            .map(|i| (format!("call_{}", i), "test_tool".to_string(), "{}".to_string()))
            .collect();
        PostLlmCtx {
            session_id: SessionId::new(1),
            full_text: String::new(),
            is_tool_call: pending_calls > 0,
            tool_calls,
            available_tools: vec!["test_tool".to_string()],
            turn_count: 1,
            total_tool_calls: 0,
            nudge_count: 0,
            turn_tool_calls,
            skip_push: false,
            follow_up_message: None,
        }
    }

    #[tokio::test]
    async fn allows_calls_under_limit() {
        let mw = TurnToolLimitMiddleware::new(8);
        let mut ctx = make_ctx(3, 2);
        mw.on_post_llm(&mut ctx).await.unwrap();
        assert_eq!(ctx.tool_calls.len(), 2);
        assert!(ctx.follow_up_message.is_none());
    }

    #[tokio::test]
    async fn blocks_calls_at_limit() {
        let mw = TurnToolLimitMiddleware::new(8);
        let mut ctx = make_ctx(8, 3);
        mw.on_post_llm(&mut ctx).await.unwrap();
        assert!(ctx.tool_calls.is_empty());
        assert!(!ctx.is_tool_call);
        assert!(ctx.follow_up_message.is_some());
    }

    #[tokio::test]
    async fn blocks_calls_over_limit() {
        let mw = TurnToolLimitMiddleware::new(8);
        let mut ctx = make_ctx(12, 2);
        mw.on_post_llm(&mut ctx).await.unwrap();
        assert!(ctx.tool_calls.is_empty());
        assert!(ctx.follow_up_message.is_some());
    }

    #[tokio::test]
    async fn ignores_text_only_responses() {
        let mw = TurnToolLimitMiddleware::new(8);
        let mut ctx = make_ctx(10, 0); // no tool calls
        ctx.is_tool_call = false;
        ctx.full_text = "just text".to_string();
        mw.on_post_llm(&mut ctx).await.unwrap();
        assert!(ctx.follow_up_message.is_none());
    }

    #[tokio::test]
    async fn custom_limit() {
        let mw = TurnToolLimitMiddleware::new(3);
        let mut ctx = make_ctx(3, 1);
        mw.on_post_llm(&mut ctx).await.unwrap();
        assert!(ctx.tool_calls.is_empty());
        assert!(ctx.follow_up_message.is_some());
    }
}
