use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, TruncationInfo};
use crate::types::AgentResult;

/// Pure execution pipeline — cares about *how* to safely execute a tool.
///
/// Responsibilities: policy hooks, timeout, output truncation.
/// Does NOT do: tool lookup, event emission, user-event forwarding.
#[async_trait]
pub trait ToolExecutionPipeline: Send + Sync {
    async fn execute(
        &self,
        tool: &dyn Tool,
        args: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<ToolOutput>;
}

/// Default pipeline: ToolPolicy hooks + timeout + output truncation.
#[derive(Clone)]
pub struct DefaultPipeline {
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    tool_timeout_ms: Option<u64>,
    max_output_chars: Option<usize>,
}

impl DefaultPipeline {
    pub fn new(
        tool_policy: Option<Arc<dyn ToolPolicy>>,
        tool_timeout_ms: Option<u64>,
        max_output_chars: Option<usize>,
    ) -> Self {
        Self {
            tool_policy,
            tool_timeout_ms,
            max_output_chars,
        }
    }

    pub fn policy(&self) -> Option<Arc<dyn ToolPolicy>> {
        self.tool_policy.clone()
    }
}

#[async_trait]
impl ToolExecutionPipeline for DefaultPipeline {
    async fn execute(
        &self,
        tool: &dyn Tool,
        args: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<ToolOutput> {
        // 1. before_call hook
        if let Some(policy) = &self.tool_policy {
            policy.before_call(tool.name(), args, ctx)?;
        }

        // 2. Execute with optional timeout
        let result = if let Some(timeout_ms) = self.tool_timeout_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), tool.call(args, ctx))
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        tool = tool.name(),
                        timeout_ms = timeout_ms,
                        "tool execution timed out"
                    );
                    return Ok(ToolOutput {
                        summary: "[Tool Timeout]".to_string(),
                        control_flow: ToolControlFlow::Continue,
                        ..Default::default()
                    });
                }
            }
        } else {
            tool.call(args, ctx).await
        };

        let mut output = result?;

        // 3. Output truncation
        if let Some(max_chars) = self.max_output_chars
            && output.summary.len() > max_chars
        {
            let original_summary_len = output.summary.len();
            let original_raw_len = output.raw.as_ref().map(|v| v.to_string().len());
            let suffix = "...(truncated)";
            let keep = max_chars.saturating_sub(suffix.len());
            if keep > 0 {
                // Use floor_char_boundary to avoid panicking on multi-byte
                // UTF-8 characters (e.g. CJK, emoji) where `keep` falls
                // in the middle of a character.
                let truncate_at = output.summary.floor_char_boundary(keep);
                output.summary.truncate(truncate_at);
                output.summary.push_str(suffix);
            } else {
                output.summary = suffix[..max_chars].to_string();
            }
            output.truncation = Some(TruncationInfo {
                original_summary_len,
                original_raw_len,
                max_allowed_chars: max_chars,
            });
            tracing::debug!(
                tool = tool.name(),
                original_summary_len = original_summary_len,
                original_raw_len = original_raw_len,
                max_allowed_chars = max_chars,
                "tool output truncated"
            );
        }

        // 4. after_call hook
        if let Some(policy) = &self.tool_policy {
            policy.after_call(tool.name(), args, &output, ctx)?;
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::types::{AgentError, Language, SessionId};

    // ── Test helpers ──

    use tokio::sync::mpsc;

    fn test_ctx() -> ToolContext {
        let (tx, _rx) = mpsc::unbounded_channel();
        ToolContext {
            session_id: SessionId::new(1),
            user_event_tx: tx,
            llm_client: None,
            session_store: None,
            language: Language::En,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            event_bus: crate::engine::EventBus::new(1),
        }
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn definition(&self) -> Value {
            json!({})
        }
        async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            Ok(ToolOutput {
                summary: args
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ok")
                    .to_string(),
                ..Default::default()
            })
        }
    }

    struct SlowTool;
    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn definition(&self) -> Value {
            json!({})
        }
        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(ToolOutput {
                summary: "done".to_string(),
                ..Default::default()
            })
        }
    }

    struct FailingTool;
    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn definition(&self) -> Value {
            json!({})
        }
        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            Err(AgentError::tool_not_found("intentional"))
        }
    }

    struct TrackingPolicy {
        before_count: AtomicU32,
        after_count: AtomicU32,
        fail_before: bool,
    }
    impl TrackingPolicy {
        fn new() -> Self {
            Self {
                before_count: AtomicU32::new(0),
                after_count: AtomicU32::new(0),
                fail_before: false,
            }
        }
        fn fail_before_call() -> Self {
            Self {
                before_count: AtomicU32::new(0),
                after_count: AtomicU32::new(0),
                fail_before: true,
            }
        }
    }
    #[async_trait]
    impl ToolPolicy for TrackingPolicy {
        async fn evaluate_approval(
            &self,
            _: &str,
            _: &Value,
        ) -> Option<crate::types::ApprovalRequest> {
            None
        }
        fn before_call(&self, _name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
            self.before_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_before {
                return Err(AgentError::internal("before_call denied"));
            }
            Ok(())
        }
        fn after_call(
            &self,
            _name: &str,
            _args: &Value,
            _output: &ToolOutput,
            _ctx: &ToolContext,
        ) -> AgentResult<()> {
            self.after_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // ── DefaultPipeline tests ──

    #[tokio::test]
    async fn basic_execution() {
        let pipeline = DefaultPipeline::new(None, None, None);
        let output = pipeline
            .execute(&EchoTool, &json!({"msg": "hello"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(output.summary, "hello");
    }

    #[tokio::test]
    async fn policy_before_and_after_called() {
        let policy = Arc::new(TrackingPolicy::new());
        let pipeline = DefaultPipeline::new(Some(policy.clone()), None, None);

        pipeline
            .execute(&EchoTool, &json!({}), &test_ctx())
            .await
            .unwrap();

        assert_eq!(policy.before_count.load(Ordering::SeqCst), 1);
        assert_eq!(policy.after_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn policy_before_call_aborts() {
        let policy = Arc::new(TrackingPolicy::fail_before_call());
        let pipeline = DefaultPipeline::new(Some(policy.clone()), None, None);

        let result = pipeline.execute(&EchoTool, &json!({}), &test_ctx()).await;
        assert!(result.is_err());
        // after_call should NOT be called
        assert_eq!(policy.after_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn timeout_fires() {
        let pipeline = DefaultPipeline::new(None, Some(50), None); // 50ms timeout
        let output = pipeline
            .execute(&SlowTool, &json!({}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(output.summary, "[Tool Timeout]");
    }

    #[tokio::test]
    async fn no_timeout_when_tool_fast() {
        let pipeline = DefaultPipeline::new(None, Some(5000), None);
        let output = pipeline
            .execute(&EchoTool, &json!({"msg": "fast"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(output.summary, "fast");
    }

    #[tokio::test]
    async fn truncation_applies() {
        let pipeline = DefaultPipeline::new(None, None, Some(10)); // max 10 chars
        let output = pipeline
            .execute(
                &EchoTool,
                &json!({"msg": "this is a very long message"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(output.summary.len() <= 10);
        assert!(output.truncation.is_some());
        let t = output.truncation.unwrap();
        assert_eq!(t.original_summary_len, 27);
        assert_eq!(t.max_allowed_chars, 10);
    }

    #[tokio::test]
    async fn no_truncation_when_short() {
        let pipeline = DefaultPipeline::new(None, None, Some(100));
        let output = pipeline
            .execute(&EchoTool, &json!({"msg": "short"}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(output.summary, "short");
        assert!(output.truncation.is_none());
    }

    #[tokio::test]
    async fn truncation_cjk_no_panic() {
        // CJK chars are 3 bytes each. With max_chars=20, keep=6 bytes,
        // which falls inside a 3-byte CJK char. floor_char_boundary
        // should round down to the nearest char boundary.
        let pipeline = DefaultPipeline::new(None, None, Some(20));
        let output = pipeline
            .execute(
                &EchoTool,
                &json!({"msg": "这是一个很长的中文消息，用于测试多字节字符的截断处理"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(output.summary.len() <= 20);
        assert!(output.summary.ends_with("...(truncated)") || output.summary.len() <= 20);
        assert!(output.truncation.is_some());
        // Must not panic — that's the main assertion
    }

    #[tokio::test]
    async fn timeout_plus_truncation() {
        let pipeline = DefaultPipeline::new(None, Some(50), Some(100));
        let output = pipeline
            .execute(&SlowTool, &json!({}), &test_ctx())
            .await
            .unwrap();
        assert_eq!(output.summary, "[Tool Timeout]");
        assert!(output.truncation.is_none()); // timeout output is short
    }

    #[tokio::test]
    async fn tool_error_propagates() {
        let pipeline = DefaultPipeline::new(None, None, None);
        let result = pipeline
            .execute(&FailingTool, &json!({}), &test_ctx())
            .await;
        assert!(result.is_err());
    }
}
