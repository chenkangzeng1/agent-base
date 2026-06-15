use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::engine::EventBus;
use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, TruncationInfo};
use crate::types::{AgentEvent, AgentResult};

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
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                tool.call(args, ctx),
            )
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
        if let Some(max_chars) = self.max_output_chars {
            if output.summary.len() > max_chars {
                let original_summary_len = output.summary.len();
                let original_raw_len = output.raw.as_ref().map(|v| v.to_string().len());
                let suffix = "...(truncated)";
                let keep = max_chars.saturating_sub(suffix.len());
                if keep > 0 {
                    output.summary.truncate(keep);
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
        }

        // 4. after_call hook
        if let Some(policy) = &self.tool_policy {
            policy.after_call(tool.name(), args, &output, ctx)?;
        }

        Ok(output)
    }
}

/// Event-emitting decorator — wraps any pipeline to emit ToolCallStarted/Finished
/// events on the internal [`EventBus`] and forward [`UserEvent`]s from tools.
pub(crate) struct EventEmittingPipeline<P: ToolExecutionPipeline> {
    inner: P,
    event_bus: EventBus,
}

impl<P: ToolExecutionPipeline> EventEmittingPipeline<P> {
    pub fn new(inner: P, event_bus: EventBus) -> Self {
        Self { inner, event_bus }
    }
}

#[async_trait]
impl<P: ToolExecutionPipeline + Send + Sync> ToolExecutionPipeline for EventEmittingPipeline<P> {
    async fn execute(
        &self,
        tool: &dyn Tool,
        args: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<ToolOutput> {
        self.event_bus.emit(AgentEvent::ToolCallStarted {
            session_id: ctx.session_id.clone(),
            tool_name: tool.name().to_string(),
            args_json: args.to_string(),
        });

        let result = self.inner.execute(tool, args, ctx).await;

        let summary = match &result {
            Ok(output) => output.summary.clone(),
            Err(e) => e.to_string(),
        };
        self.event_bus.emit(AgentEvent::ToolCallFinished {
            session_id: ctx.session_id.clone(),
            tool_name: tool.name().to_string(),
            summary,
        });

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::tool::ToolRegistry;
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
        }
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str { "echo" }
        fn definition(&self) -> Value { json!({}) }
        async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            Ok(ToolOutput {
                summary: args.get("msg").and_then(|v| v.as_str()).unwrap_or("ok").to_string(),
                ..Default::default()
            })
        }
    }

    struct SlowTool;
    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &'static str { "slow" }
        fn definition(&self) -> Value { json!({}) }
        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(ToolOutput { summary: "done".to_string(), ..Default::default() })
        }
    }

    struct FailingTool;
    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &'static str { "fail" }
        fn definition(&self) -> Value { json!({}) }
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
            Self { before_count: AtomicU32::new(0), after_count: AtomicU32::new(0), fail_before: false }
        }
        fn fail_before_call() -> Self {
            Self { before_count: AtomicU32::new(0), after_count: AtomicU32::new(0), fail_before: true }
        }
    }
    #[async_trait]
    impl ToolPolicy for TrackingPolicy {
        async fn evaluate_approval(&self, _: &str, _: &Value) -> Option<crate::types::ApprovalRequest> { None }
        fn before_call(&self, _name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
            self.before_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_before {
                return Err(AgentError::internal("before_call denied"));
            }
            Ok(())
        }
        fn after_call(&self, _name: &str, _args: &Value, _output: &ToolOutput, _ctx: &ToolContext) -> AgentResult<()> {
            self.after_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // ── DefaultPipeline tests ──

    #[tokio::test]
    async fn basic_execution() {
        let pipeline = DefaultPipeline::new(None, None, None);
        let output = pipeline.execute(&EchoTool, &json!({"msg": "hello"}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "hello");
    }

    #[tokio::test]
    async fn policy_before_and_after_called() {
        let policy = Arc::new(TrackingPolicy::new());
        let pipeline = DefaultPipeline::new(Some(policy.clone()), None, None);

        pipeline.execute(&EchoTool, &json!({}), &test_ctx()).await.unwrap();

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
        let output = pipeline.execute(&SlowTool, &json!({}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "[Tool Timeout]");
    }

    #[tokio::test]
    async fn no_timeout_when_tool_fast() {
        let pipeline = DefaultPipeline::new(None, Some(5000), None);
        let output = pipeline.execute(&EchoTool, &json!({"msg": "fast"}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "fast");
    }

    #[tokio::test]
    async fn truncation_applies() {
        let pipeline = DefaultPipeline::new(None, None, Some(10)); // max 10 chars
        let output = pipeline.execute(&EchoTool, &json!({"msg": "this is a very long message"}), &test_ctx()).await.unwrap();
        assert!(output.summary.len() <= 10);
        assert!(output.truncation.is_some());
        let t = output.truncation.unwrap();
        assert_eq!(t.original_summary_len, 27);
        assert_eq!(t.max_allowed_chars, 10);
    }

    #[tokio::test]
    async fn no_truncation_when_short() {
        let pipeline = DefaultPipeline::new(None, None, Some(100));
        let output = pipeline.execute(&EchoTool, &json!({"msg": "short"}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "short");
        assert!(output.truncation.is_none());
    }

    #[tokio::test]
    async fn timeout_plus_truncation() {
        let pipeline = DefaultPipeline::new(None, Some(50), Some(100));
        let output = pipeline.execute(&SlowTool, &json!({}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "[Tool Timeout]");
        assert!(output.truncation.is_none()); // timeout output is short
    }

    #[tokio::test]
    async fn tool_error_propagates() {
        let pipeline = DefaultPipeline::new(None, None, None);
        let result = pipeline.execute(&FailingTool, &json!({}), &test_ctx()).await;
        assert!(result.is_err());
    }

    // ── EventEmittingPipeline tests ──

    #[tokio::test]
    async fn emits_start_and_finish_events() {
        let inner = DefaultPipeline::new(None, None, None);
        let event_bus = EventBus::new(64);
        let mut rx = event_bus.subscribe();
        let pipeline = EventEmittingPipeline::new(inner, event_bus);

        let _ = pipeline.execute(&EchoTool, &json!({"msg": "test"}), &test_ctx()).await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert_eq!(events.len(), 2);

        match &events[0] {
            AgentEvent::ToolCallStarted { tool_name, .. } => assert_eq!(tool_name, "echo"),
            _ => panic!("expected ToolCallStarted"),
        }
        match &events[1] {
            AgentEvent::ToolCallFinished { tool_name, summary, .. } => {
                assert_eq!(tool_name, "echo");
                assert_eq!(summary, "test");
            }
            _ => panic!("expected ToolCallFinished"),
        }
    }

    #[tokio::test]
    async fn emits_finish_with_error_on_failure() {
        let inner = DefaultPipeline::new(None, None, None);
        let event_bus = EventBus::new(64);
        let mut rx = event_bus.subscribe();
        let pipeline = EventEmittingPipeline::new(inner, event_bus);

        let _ = pipeline.execute(&FailingTool, &json!({}), &test_ctx()).await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert_eq!(events.len(), 2);

        match &events[1] {
            AgentEvent::ToolCallFinished { summary, .. } => {
                assert!(summary.contains("intentional"));
            }
            _ => panic!("expected ToolCallFinished"),
        }
    }

    #[tokio::test]
    async fn event_emitting_delegates_to_inner() {
        let policy = Arc::new(TrackingPolicy::new());
        let inner = DefaultPipeline::new(Some(policy.clone()), None, None);
        let event_bus = EventBus::new(64);
        let pipeline = EventEmittingPipeline::new(inner, event_bus);

        let output = pipeline.execute(&EchoTool, &json!({"msg": "delegated"}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "delegated");
        assert_eq!(policy.before_count.load(Ordering::SeqCst), 1);
        assert_eq!(policy.after_count.load(Ordering::SeqCst), 1);
    }

    // ── ToolCallingStepExecutor with pipeline tests ──

    use crate::engine::plan::{StepExecutor, ToolCallingStepExecutor};
    use crate::types::PlanStep;

    #[tokio::test]
    async fn step_executor_with_pipeline() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let pipeline = DefaultPipeline::new(None, None, None);
        let executor = ToolCallingStepExecutor::new(Arc::new(registry)).with_pipeline(pipeline);

        let step = PlanStep::new("s1", "test", json!({"tool_name": "echo", "args": {"msg": "via pipeline"}}));
        let result = executor.execute_step(&step, &Value::Null, &test_ctx()).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("via pipeline".to_string()));
    }

    #[tokio::test]
    async fn step_executor_without_pipeline() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new("s1", "test", json!({"tool_name": "echo", "args": {"msg": "bare call"}}));
        let result = executor.execute_step(&step, &Value::Null, &test_ctx()).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("bare call".to_string()));
    }

    #[tokio::test]
    async fn step_executor_pipeline_timeout() {
        let mut registry = ToolRegistry::default();
        registry.register(SlowTool);

        let pipeline = DefaultPipeline::new(None, Some(50), None);
        let executor = ToolCallingStepExecutor::new(Arc::new(registry)).with_pipeline(pipeline);

        let step = PlanStep::new("s1", "test", json!({"tool_name": "slow", "args": {}}));
        let result = executor.execute_step(&step, &Value::Null, &test_ctx()).await.unwrap();

        assert!(result.success); // timeout returns ToolOutput, not an error
        assert_eq!(result.output, Some("[Tool Timeout]".to_string()));
    }

    #[tokio::test]
    async fn step_executor_pipeline_truncation() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let pipeline = DefaultPipeline::new(None, None, Some(5));
        let executor = ToolCallingStepExecutor::new(Arc::new(registry)).with_pipeline(pipeline);

        let step = PlanStep::new("s1", "test", json!({"tool_name": "echo", "args": {"msg": "hello world"}}));
        let result = executor.execute_step(&step, &Value::Null, &test_ctx()).await.unwrap();

        assert!(result.success);
        assert!(result.output.as_ref().unwrap().len() <= 5);
    }

    #[tokio::test]
    async fn step_executor_pipeline_policy_hooks() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let policy = Arc::new(TrackingPolicy::new());
        let pipeline = DefaultPipeline::new(Some(policy.clone()), None, None);
        let executor = ToolCallingStepExecutor::new(Arc::new(registry)).with_pipeline(pipeline);

        let step = PlanStep::new("s1", "test", json!({"tool_name": "echo", "args": {}}));
        executor.execute_step(&step, &Value::Null, &test_ctx()).await.unwrap();

        assert_eq!(policy.before_count.load(Ordering::SeqCst), 1);
        assert_eq!(policy.after_count.load(Ordering::SeqCst), 1);
    }

    // ── Clone test ──

    #[tokio::test]
    async fn pipeline_is_cloneable() {
        let policy = Arc::new(TrackingPolicy::new());
        let pipeline = DefaultPipeline::new(Some(policy.clone()), Some(1000), Some(1024));
        let pipeline2 = pipeline.clone();

        let output = pipeline2.execute(&EchoTool, &json!({"msg": "clone"}), &test_ctx()).await.unwrap();
        assert_eq!(output.summary, "clone");
    }
}
