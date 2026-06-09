use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::engine::pipeline::{DefaultPipeline, ToolExecutionPipeline};
use crate::tool::{ToolContext, ToolRegistry};
use crate::types::{AgentError, AgentResult, PlanStep, StepResult};

use super::StepExecutor;

/// A generic StepExecutor that delegates step execution to tools via ToolRegistry.
///
/// The step payload must contain:
/// - `tool_name`: the name of the tool to call
/// - `args`: the arguments to pass to the tool (optional, defaults to null)
///
/// This enables plan steps to invoke any registered tool, making the plan execution
/// fully business-agnostic.
///
/// # Pipeline support
///
/// When constructed with `with_pipeline()`, tools execute through the pipeline
/// (policy hooks, timeout, output truncation) — same guarantees as direct tool
/// calls via `ToolEngine`. Without a pipeline, tools are called directly (bare call).
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use agent_base::engine::{ToolCallingStepExecutor, DefaultPipeline};
/// # use agent_base::tool::ToolRegistry;
/// let registry = Arc::new(ToolRegistry::default());
/// let pipeline = DefaultPipeline::new(None, Some(30_000), Some(8192));
/// let executor = ToolCallingStepExecutor::new(registry).with_pipeline(pipeline);
/// ```
#[derive(Clone)]
pub struct ToolCallingStepExecutor {
    tool_registry: Arc<ToolRegistry>,
    pipeline: Option<DefaultPipeline>,
}

impl ToolCallingStepExecutor {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            tool_registry,
            pipeline: None,
        }
    }

    /// Inject an execution pipeline for policy hooks, timeout, and output truncation.
    ///
    /// When set, plan steps execute through the same pipeline as direct tool calls,
    /// ensuring experience consistency.
    pub fn with_pipeline(mut self, pipeline: DefaultPipeline) -> Self {
        self.pipeline = Some(pipeline);
        self
    }
}

#[async_trait]
impl StepExecutor for ToolCallingStepExecutor {
    async fn execute_step(
        &self,
        step: &PlanStep,
        _step_outputs: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        let start = std::time::Instant::now();

        tracing::debug!(
            "ToolCallingStepExecutor: executing step {} with payload: {}",
            step.id,
            step.payload
        );

        let tool_name = step
            .payload
            .get("tool_name")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::plan_execution("missing tool_name in step payload"))?;

        let args = step.payload.get("args").unwrap_or(&Value::Null);

        tracing::debug!(
            "ToolCallingStepExecutor: calling tool '{}' with args: {}",
            tool_name,
            args
        );

        let tool = self
            .tool_registry
            .get(tool_name)
            .ok_or_else(|| AgentError::plan_execution(format!("tool '{}' not found", tool_name)))?;

        let result = if let Some(pipeline) = &self.pipeline {
            pipeline.execute(tool.as_ref(), args, ctx).await
        } else {
            tool.call(args, ctx).await
        };

        let duration = start.elapsed().as_millis() as u64;
        match result {
            Ok(output) => {
                tracing::debug!(
                    "ToolCallingStepExecutor: tool '{}' succeeded in {}ms",
                    tool_name,
                    duration
                );
                Ok(StepResult {
                    success: true,
                    output: Some(output.summary),
                    error: None,
                    duration_ms: duration,
                })
            }
            Err(e) => {
                tracing::warn!(
                    "ToolCallingStepExecutor: tool '{}' failed in {}ms: {}",
                    tool_name,
                    duration,
                    e
                );
                Ok(StepResult {
                    success: false,
                    output: None,
                    error: Some(e.to_string()),
                    duration_ms: duration,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolOutput;

    struct MockTool {
        name: &'static str,
    }

    #[async_trait]
    impl crate::tool::Tool for MockTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn definition(&self) -> Value {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": self.name,
                    "description": "A mock tool for testing",
                }
            })
        }

        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            Ok(ToolOutput {
                summary: "mock output".to_string(),
                raw: Some(serde_json::json!({"success": true})),
                control_flow: crate::tool::ToolControlFlow::Break,
                truncation: None,
            })
        }
    }

    struct FailingMockTool;

    #[async_trait]
    impl crate::tool::Tool for FailingMockTool {
        fn name(&self) -> &'static str {
            "failing_tool"
        }

        fn definition(&self) -> Value {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "failing_tool",
                    "description": "A mock tool that always fails",
                }
            })
        }

        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
            Err(AgentError::tool_not_found("failing_tool"))
        }
    }

    fn create_test_tool_context() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext {
            session_id: crate::types::SessionId::new(1),
            user_event_tx: tx,
            llm_client: None,
            session_store: None,
            language: crate::types::Language::En,
        }
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_success() {
        let mut registry = ToolRegistry::default();
        registry.register(MockTool { name: "test_tool" });
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            serde_json::json!({
                "tool_name": "test_tool",
                "args": {"key": "value"}
            }),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("mock output".to_string()));
        assert!(result.error.is_none());
        assert!(result.duration_ms >= 0);
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_missing_tool_name() {
        let registry = ToolRegistry::default();
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            serde_json::json!({"args": {"key": "value"}}),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("missing tool_name"));
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_tool_not_found() {
        let registry = ToolRegistry::default();
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            serde_json::json!({
                "tool_name": "nonexistent_tool",
                "args": {"key": "value"}
            }),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent_tool"));
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_tool_error() {
        let mut registry = ToolRegistry::default();
        registry.register(FailingMockTool);
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            serde_json::json!({
                "tool_name": "failing_tool",
                "args": {}
            }),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await.unwrap();

        assert!(!result.success);
        assert!(result.output.is_none());
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_no_args() {
        let mut registry = ToolRegistry::default();
        registry.register(MockTool { name: "test_tool" });
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            serde_json::json!({"tool_name": "test_tool"}),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("mock output".to_string()));
    }
}
