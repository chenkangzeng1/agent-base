use std::sync::Mutex;

use async_trait::async_trait;

use crate::engine::middleware::{Middleware, PostLlmCtx, UserMessageCtx};
use crate::types::AgentResult;

/// Turn fact summary middleware — injects structured facts from the previous
/// turn's tool results into the next user message.
///
/// This prevents long-conversation attention drift: the LLM sees deterministic
/// facts (exit codes, error messages) instead of relying on fuzzy memory of
/// tool outputs buried 20+ turns ago.
///
/// # Design
///
/// - After each LLM turn with tool calls, collects key facts from tool names
///   and stores them in a buffer.
/// - At the start of the next user message, prepends the buffered facts as a
///   structured prefix, then clears the buffer.
/// - Facts are derived from tool names only (not parsing output text), keeping
///   the logic simple and model-agnostic.
pub struct TurnFactMiddleware {
    pending_facts: Mutex<Vec<String>>,
}

impl TurnFactMiddleware {
    pub fn new() -> Self {
        Self {
            pending_facts: Mutex::new(Vec::new()),
        }
    }
}

impl Default for TurnFactMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for TurnFactMiddleware {
    async fn on_user_message(&self, ctx: &mut UserMessageCtx) -> AgentResult<()> {
        let facts = {
            let mut guard = self.pending_facts.lock().unwrap();
            if guard.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *guard)
        };

        // Prepend facts to user message as structured context
        let prefix = format!(
            "[本轮工具调用摘要 — 以下为确定性事实，请以此为准]\n{}\n",
            facts.join("\n")
        );
        ctx.user_input = format!("{prefix}\n{original}", original = ctx.user_input);

        Ok(())
    }

    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        if ctx.tool_calls.is_empty() {
            return Ok(());
        }

        let mut facts = Vec::new();
        for (_id, name, _args) in &ctx.tool_calls {
            // Record which tools were called — the actual results are in the
            // session history, but a compact reminder helps the LLM stay grounded.
            facts.push(format!("- 调用了工具: {name}"));
        }

        let mut guard = self.pending_facts.lock().unwrap();
        guard.extend(facts);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionId;

    #[tokio::test]
    async fn test_no_facts_no_prefix() {
        let mw = TurnFactMiddleware::new();
        let mut ctx = UserMessageCtx {
            session_id: SessionId::new(1),
            user_input: "hello".to_string(),
        };
        mw.on_user_message(&mut ctx).await.unwrap();
        assert_eq!(ctx.user_input, "hello");
    }

    #[tokio::test]
    async fn test_facts_injected_on_next_user_message() {
        let mw = TurnFactMiddleware::new();

        // Simulate a tool call turn
        let mut post_ctx = PostLlmCtx {
            session_id: SessionId::new(1),
            full_text: String::new(),
            is_tool_call: true,
            tool_calls: vec![
                ("id1".into(), "execute_ssh_command".into(), "{}".into()),
                ("id2".into(), "start_interactive_task".into(), "{}".into()),
            ],
            available_tools: vec![],
            turn_count: 1,
            total_tool_calls: 0,
            nudge_count: 0,
            skip_push: false,
            follow_up_message: None,
        };
        mw.on_post_llm(&mut post_ctx).await.unwrap();

        // Next user message should have facts prepended
        let mut user_ctx = UserMessageCtx {
            session_id: SessionId::new(1),
            user_input: "继续执行".to_string(),
        };
        mw.on_user_message(&mut user_ctx).await.unwrap();

        assert!(user_ctx.user_input.contains("本轮工具调用摘要"));
        assert!(user_ctx.user_input.contains("execute_ssh_command"));
        assert!(user_ctx.user_input.contains("start_interactive_task"));
        assert!(user_ctx.user_input.contains("继续执行"));
    }

    #[tokio::test]
    async fn test_facts_cleared_after_injection() {
        let mw = TurnFactMiddleware::new();

        // First turn with tool calls
        let mut post_ctx = PostLlmCtx {
            session_id: SessionId::new(1),
            full_text: String::new(),
            is_tool_call: true,
            tool_calls: vec![("id1".into(), "docker".into(), "{}".into())],
            available_tools: vec![],
            turn_count: 1,
            total_tool_calls: 0,
            nudge_count: 0,
            skip_push: false,
            follow_up_message: None,
        };
        mw.on_post_llm(&mut post_ctx).await.unwrap();

        // First user message gets facts
        let mut ctx1 = UserMessageCtx {
            session_id: SessionId::new(1),
            user_input: "next".into(),
        };
        mw.on_user_message(&mut ctx1).await.unwrap();
        assert!(ctx1.user_input.contains("本轮工具调用摘要"));

        // Second user message should NOT have facts (cleared)
        let mut ctx2 = UserMessageCtx {
            session_id: SessionId::new(1),
            user_input: "again".into(),
        };
        mw.on_user_message(&mut ctx2).await.unwrap();
        assert_eq!(ctx2.user_input, "again");
    }
}
