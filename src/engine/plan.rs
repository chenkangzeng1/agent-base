use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::types::{AgentResult, ExecutionPlan, PlanStep, PlanStoreData, RecoveryAction, StepResult};

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
        on_raw_chunk: Box<dyn Fn(String) + Send>,
    ) -> AgentResult<ExecutionPlan> {
        let plan = self.generate_plan(objective, context, tools).await?;
        on_generating();
        for (i, step) in plan.steps.iter().enumerate() {
            on_step_parsed(i, step.id.clone(), step.description.clone());
        }
        let plan_json = serde_json::to_string(&plan).unwrap_or_default();
        on_raw_chunk(plan_json);
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
