use std::sync::Arc;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::engine::pipeline::{DefaultPipeline, ToolExecutionPipeline};
use crate::tool::{ToolContext, ToolRegistry};
use crate::types::{AgentError, AgentResult, ExecutionPlan, PlanStep, PlanStoreData, RecoveryAction, StepResult};

use log;

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Generates an `ExecutionPlan` from a high-level objective.
///
/// The generator may use LLM prompting, rule engines, or templates.
#[async_trait]
pub trait PlanGenerator: Send + Sync {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
    ) -> AgentResult<ExecutionPlan>;

    async fn generate_plan_streaming(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        on_generating: Box<dyn Fn() + Send>,
        on_step_parsed: Box<dyn Fn(usize, String, String) + Send>,
        _on_thought: Box<dyn Fn(String) + Send>,
    ) -> AgentResult<ExecutionPlan> {
        // Default implementation falls back to non-streaming
        let plan = self.generate_plan(objective, context, tools).await?;
        on_generating();
        for (i, step) in plan.steps.iter().enumerate() {
            on_step_parsed(i, step.id.clone(), step.description.clone());
        }
        Ok(plan)
    }
}

/// Executes a single `PlanStep` and returns its result.
///
/// Implementors know how to interpret `step.payload` for their domain.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute_step(
        &self,
        step: &PlanStep,
        plan_context: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<StepResult>;
}

/// Decides whether the plan should continue executing a given step.
#[async_trait]
pub trait StepContinuePolicy: Send + Sync {
    async fn should_continue(
        &self,
        plan: &ExecutionPlan,
        current_step: &PlanStep,
    ) -> AgentResult<bool>;
}

/// Decides what to do when a step fails.
#[async_trait]
pub trait RecoveryStrategy: Send + Sync {
    async fn handle_step_failure(
        &self,
        step: &PlanStep,
        error: &str,
        retry_count: usize,
    ) -> AgentResult<RecoveryAction>;
}

// ---------------------------------------------------------------------------
// ToolCallingStepExecutor
// ---------------------------------------------------------------------------

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
        _plan_context: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<StepResult> {
        let start = std::time::Instant::now();

        log::debug!(
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

        log::debug!(
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
                log::debug!(
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
                log::warn!(
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

// ---------------------------------------------------------------------------
// Default / convenience implementations
// ---------------------------------------------------------------------------

/// Always continues.
pub struct AlwaysContinue;

#[async_trait]
impl StepContinuePolicy for AlwaysContinue {
    async fn should_continue(
        &self,
        _plan: &ExecutionPlan,
        _current_step: &PlanStep,
    ) -> AgentResult<bool> {
        Ok(true)
    }
}

/// Always aborts on failure.
pub struct AbortOnFailure;

#[async_trait]
impl RecoveryStrategy for AbortOnFailure {
    async fn handle_step_failure(
        &self,
        _step: &PlanStep,
        _error: &str,
        _retry_count: usize,
    ) -> AgentResult<RecoveryAction> {
        Ok(RecoveryAction::Abort)
    }
}

// ---------------------------------------------------------------------------
// Streaming JSON parser (generic)
// ---------------------------------------------------------------------------

/// Parses JSON objects of type `T` from a stream of text chunks.
///
/// It scans for objects inside a JSON array (by default) and yields each
/// fully-formed object as soon as braces are balanced. Useful when an LLM
/// streams a JSON plan and you want to display / process steps incrementally.
#[derive(Debug)]
pub struct StreamingJsonParser<T> {
    buffer: String,
    scan_offset: usize,
    items: Vec<T>,
    items_start_byte: usize,
    in_items: bool,
    in_string: bool,
    escape_next: bool,
    array_key: Option<String>,
}

impl<T: DeserializeOwned + Clone> StreamingJsonParser<T> {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            scan_offset: 0,
            items: Vec::new(),
            items_start_byte: 0,
            in_items: false,
            in_string: false,
            escape_next: false,
            array_key: None,
        }
    }

    /// Set the array key to look for. e.g. `with_key("steps")` will look for
    /// `"steps":[...]` in the JSON.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.array_key = Some(key.into());
        self
    }

    /// Append a new chunk and return any newly parsed items.
    pub fn process_chunk(&mut self, chunk: &str) -> Vec<T> {
        let mut new_items = Vec::new();
        self.buffer.push_str(chunk);

        if !self.in_items {
            if let Some(pos) = self.find_items_array_start() {
                self.items_start_byte = pos + 1;
                self.scan_offset = 0;
                self.in_items = true;
            }
        }

        if self.in_items {
            new_items = self.extract_items();
            self.items.extend(new_items.clone());
        }

        new_items
    }

    /// Return all accumulated items so far.
    pub fn accumulated(&self) -> &[T] {
        &self.items
    }

    /// Consume parser and return the full raw text.
    pub fn into_buffer(self) -> String {
        self.buffer
    }

    fn find_items_array_start(&self) -> Option<usize> {
        if let Some(ref key) = self.array_key {
            if let Some(pos) = self.buffer.find(&format!("\"{}\"", key)) {
                let after = &self.buffer[pos..];
                if let Some(bracket_pos) = after.find('[') {
                    return Some(pos + bracket_pos);
                }
            }
        } else {
            // Fallback: look for any quoted key followed by '['
            if let Some(pos) = self.buffer.find('"') {
                let after = &self.buffer[pos..];
                if let Some(bracket_pos) = after.find('[') {
                    return Some(pos + bracket_pos);
                }
            }
        }
        // Last fallback: raw array
        self.buffer.find('[')
    }

    fn extract_items(&mut self) -> Vec<T> {
        let mut results = Vec::new();
        let slice = &self.buffer[self.items_start_byte..];
        let mut brace_depth: i32 = 0;
        let mut item_start_byte: Option<usize> = None;

        for (byte_offset, ch) in slice.char_indices().skip(self.scan_offset) {
            if self.escape_next {
                self.escape_next = false;
                self.scan_offset = byte_offset + ch.len_utf8();
                continue;
            }

            if self.in_string {
                if ch == '\\' {
                    self.escape_next = true;
                } else if ch == '"' {
                    self.in_string = false;
                }
                self.scan_offset = byte_offset + ch.len_utf8();
                continue;
            }

            match ch {
                '"' => self.in_string = true,
                '{' => {
                    if brace_depth == 0 {
                        let abs_byte = self.items_start_byte + byte_offset;
                        item_start_byte = Some(abs_byte);
                    }
                    brace_depth += 1;
                }
                '}' => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        if let Some(start) = item_start_byte.take() {
                            let end = self.items_start_byte + byte_offset + ch.len_utf8();
                            let item_json = &self.buffer[start..end];
                            if let Ok(item) = serde_json::from_str::<T>(item_json) {
                                results.push(item);
                            }
                        }
                    }
                }
                _ => {}
            }

            self.scan_offset = byte_offset + ch.len_utf8();
        }

        results
    }
}

impl<T: DeserializeOwned + Clone> Default for StreamingJsonParser<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PlanStore
// ---------------------------------------------------------------------------

#[async_trait]
pub trait PlanStore: Send + Sync {
    async fn save_plan(&self, plan: &ExecutionPlan, metadata: Value) -> AgentResult<()>;

    async fn load_plan(&self, plan_id: &str) -> AgentResult<Option<PlanStoreData>>;

    async fn delete_plan(&self, plan_id: &str) -> AgentResult<()>;

    async fn list_plans(&self) -> AgentResult<Vec<String>>;
}

pub struct InMemoryPlanStore {
    plans: tokio::sync::RwLock<std::collections::HashMap<String, PlanStoreData>>,
}

impl InMemoryPlanStore {
    pub fn new() -> Self {
        Self {
            plans: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryPlanStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlanStore for InMemoryPlanStore {
    async fn save_plan(&self, plan: &ExecutionPlan, metadata: Value) -> AgentResult<()> {
        let mut plans = self.plans.write().await;
        plans.insert(
            plan.id.clone(),
            PlanStoreData {
                plan: plan.clone(),
                metadata,
            },
        );
        Ok(())
    }

    async fn load_plan(&self, plan_id: &str) -> AgentResult<Option<PlanStoreData>> {
        let plans = self.plans.read().await;
        Ok(plans.get(plan_id).cloned())
    }

    async fn delete_plan(&self, plan_id: &str) -> AgentResult<()> {
        let mut plans = self.plans.write().await;
        plans.remove(plan_id);
        Ok(())
    }

    async fn list_plans(&self) -> AgentResult<Vec<String>> {
        let plans = self.plans.read().await;
        Ok(plans.keys().cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::types::{SessionId, Language, StepStatus};
    use crate::tool::{ToolOutput, ToolControlFlow};

    /// A mock tool for testing ToolCallingStepExecutor
    struct MockTool {
        name: &'static str,
    }

    #[async_trait]
    impl crate::tool::Tool for MockTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn definition(&self) -> Value {
            json!({
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
                raw: Some(json!({"success": true})),
                control_flow: crate::tool::ToolControlFlow::Break,
                truncation: None,
            })
        }
    }

    /// A mock tool that fails for testing error handling
    struct FailingMockTool;

    #[async_trait]
    impl crate::tool::Tool for FailingMockTool {
        fn name(&self) -> &'static str {
            "failing_tool"
        }

        fn definition(&self) -> Value {
            json!({
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
        ToolContext {
            session_id: crate::types::SessionId::new(1),
            event_bus: tokio::sync::broadcast::channel(1).0,
            event_sender: None,
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
            json!({
                "tool_name": "test_tool",
                "args": {"key": "value"}
            }),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("mock output".to_string()));
        assert!(result.error.is_none());
        // duration_ms could be 0 if the mock tool executes very fast
        assert!(result.duration_ms >= 0);
    }

    #[tokio::test]
    async fn test_tool_calling_step_executor_missing_tool_name() {
        let registry = ToolRegistry::default();
        let executor = ToolCallingStepExecutor::new(Arc::new(registry));

        let step = PlanStep::new(
            "step-1",
            "Test step",
            json!({"args": {"key": "value"}}),
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
            json!({
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
            json!({
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
            json!({"tool_name": "test_tool"}),
        );

        let ctx = create_test_tool_context();
        let result = executor.execute_step(&step, &Value::Null, &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.output, Some("mock output".to_string()));
    }

    #[test]
    fn test_in_memory_plan_store() {
        use tokio::runtime::Runtime;
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryPlanStore::new();
            let plan = ExecutionPlan::new("plan-1", "Test Plan");

            store.save_plan(&plan, json!({"key": "value"})).await.unwrap();

            let loaded = store.load_plan("plan-1").await.unwrap();
            assert!(loaded.is_some());
            let data = loaded.unwrap();
            assert_eq!(data.plan.id, "plan-1");
            assert_eq!(data.plan.objective, "Test Plan");

            let plans = store.list_plans().await.unwrap();
            assert_eq!(plans.len(), 1);
            assert_eq!(plans[0], "plan-1");

            store.delete_plan("plan-1").await.unwrap();
            let loaded = store.load_plan("plan-1").await.unwrap();
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn test_execution_plan_progress() {
        let mut plan = ExecutionPlan::new("plan-1", "Test");
        plan.steps.push(PlanStep::new("s1", "Step 1", Value::Null));
        plan.steps.push(PlanStep::new("s2", "Step 2", Value::Null));
        plan.steps.push(PlanStep::new("s3", "Step 3", Value::Null));

        assert_eq!(plan.progress(), (0, 3));
        assert!(!plan.is_completed());

        plan.steps[0].status = StepStatus::Completed;
        assert_eq!(plan.progress(), (1, 3));

        plan.steps[1].status = StepStatus::Skipped;
        assert_eq!(plan.progress(), (2, 3));

        plan.steps[2].status = StepStatus::Completed;
        assert_eq!(plan.progress(), (3, 3));
        assert!(plan.is_completed());
    }

    #[test]
    fn test_execution_plan_has_failed() {
        let mut plan = ExecutionPlan::new("plan-1", "Test");
        plan.steps.push(PlanStep::new("s1", "Step 1", Value::Null));

        assert!(!plan.has_failed());

        plan.steps[0].status = StepStatus::Failed;
        assert!(plan.has_failed());
    }
}
