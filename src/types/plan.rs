use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPhase {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<PlanStep>,
    pub status: PhaseStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhaseStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl PlanPhase {
    pub fn new(id: impl Into<String>, title: impl Into<String>, steps: Vec<PlanStep>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: None,
            steps,
            status: PhaseStatus::Pending,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// (completed, total) for steps in this phase.
    pub fn progress(&self) -> (usize, usize) {
        let total = self.steps.len();
        let completed = self
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
            .count();
        (completed, total)
    }

    pub fn is_completed(&self) -> bool {
        !self.steps.is_empty()
            && self
                .steps
                .iter()
                .all(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
    }

    pub fn has_failed(&self) -> bool {
        self.steps.iter().any(|s| s.status == StepStatus::Failed)
    }
}

impl Default for PhaseStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub id: String,
    pub objective: String,
    pub phases: Vec<PlanPhase>,
    pub status: PlanStatus,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    /// Business-agnostic payload. Each domain defines its own schema.
    /// For example, an ops step might be:
    /// `{"type":"ssh_command","command":"df -h","host_id":"host1"}`
    pub payload: Value,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<StepResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStatus {
    Created,
    AwaitingConfirmation,
    Approved,
    Executing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    WaitingConfirmation,
    Running,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Retry the failed step as-is.
    Retry,


    /// Replace the failed step with an alternative step that achieves the same goal.
    /// `root_step_id` tracks the original step for retry/alternative budget accounting.
    Alternative {
        step: PlanStep,
        root_step_id: String,
    },

    /// Replan: replace the current step and subsequent steps with a new sequence.
    /// `clear_future_phases` controls whether pending steps in later phases are also cleared.
    Replan {
        steps: Vec<PlanStep>,
        clear_future_phases: bool,
    },

    /// Skip the failed step and continue with the next one.
    Skip,

    /// Abort the entire plan.
    Abort,
}

impl PartialEq for RecoveryAction {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// Full context provided to [`AdaptiveRecoveryStrategy`](crate::engine::AdaptiveRecoveryStrategy)
/// for making recovery decisions.
#[derive(Debug, Clone)]
pub struct RecoveryContext {
    /// Current session ID (business strategies may query session history).
    pub session_id: super::SessionId,
    /// The step that failed.
    pub failed_step: PlanStep,
    /// Original step ID for tracking the alternative chain's recovery budget.
    /// Equals `failed_step.id` on first failure.
    pub root_step_id: String,
    /// Error message from the failed execution.
    pub error: String,
    /// Number of retries already attempted for this step (root).
    pub retry_count: usize,
    /// Number of alternative steps already attempted for this step (root).
    pub alternative_count: usize,
    /// Number of replans already performed for this plan.
    pub replan_count: usize,
    /// Framework-configured maximum retry count.
    pub max_retries: usize,
    /// Framework-configured maximum alternative count.
    pub max_alternatives: usize,
    /// Framework-configured maximum replan count.
    pub max_replans: usize,
    /// The full execution plan (including completed step statuses).
    pub plan: ExecutionPlan,
    /// Accumulated outputs from completed steps (keyed by step_id).
    pub step_outputs: Value,
    /// Available tool definitions for the agent.
    pub available_tools: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStoreData {
    pub plan: ExecutionPlan,
    #[serde(default)]
    pub metadata: Value,
}

impl ExecutionPlan {
    /// Create an empty plan with no phases.
    pub fn new(id: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            objective: objective.into(),
            phases: Vec::new(),
            status: PlanStatus::Created,
            context: Value::Null,
        }
    }

    /// Create a plan with a single phase wrapping all steps.
    ///
    /// Convenience constructor for simple plans that don't need phase grouping.
    pub fn with_single_phase(
        id: impl Into<String>,
        objective: impl Into<String>,
        steps: Vec<PlanStep>,
    ) -> Self {
        let id = id.into();
        let objective = objective.into();
        let phase = PlanPhase::new(format!("{id}-phase-0"), &objective, steps);
        Self {
            id,
            objective,
            phases: vec![phase],
            status: PlanStatus::Created,
            context: Value::Null,
        }
    }

    /// Semantic alias for `with_single_phase`.
    ///
    /// Users don't need to know about `PlanPhase` — this method makes it
    /// clear that you're creating a plan from a flat list of steps.
    pub fn of_steps(
        id: impl Into<String>,
        objective: impl Into<String>,
        steps: Vec<PlanStep>,
    ) -> Self {
        Self::with_single_phase(id, objective, steps)
    }

    // ── Flat step access (cross-phase) ─────────────────────────────

    /// All steps across all phases, in order.
    pub fn all_steps(&self) -> impl Iterator<Item = &PlanStep> {
        self.phases.iter().flat_map(|p| p.steps.iter())
    }

    /// Mutable access to all steps across all phases, in order.
    pub fn all_steps_mut(&mut self) -> impl Iterator<Item = &mut PlanStep> {
        self.phases.iter_mut().flat_map(|p| p.steps.iter_mut())
    }

    /// Total number of steps across all phases.
    pub fn total_steps(&self) -> usize {
        self.phases.iter().map(|p| p.steps.len()).sum()
    }

    /// Find a step by its id across all phases.
    pub fn find_step(&self, step_id: &str) -> Option<&PlanStep> {
        self.all_steps().find(|s| s.id == step_id)
    }

    /// Find a step by its id across all phases (mutable).
    pub fn find_step_mut(&mut self, step_id: &str) -> Option<&mut PlanStep> {
        self.all_steps_mut().find(|s| s.id == step_id)
    }

    // ── Step queries ───────────────────────────────────────────────

    /// First currently-running step across all phases.
    pub fn current_step(&self) -> Option<&PlanStep> {
        self.all_steps().find(|s| s.status == StepStatus::Running)
    }

    /// First pending step across all phases.
    pub fn next_pending_step(&self) -> Option<&PlanStep> {
        self.all_steps().find(|s| s.status == StepStatus::Pending)
    }

    // ── Phase queries ──────────────────────────────────────────────

    /// First currently-running phase.
    pub fn current_phase(&self) -> Option<&PlanPhase> {
        self.phases.iter().find(|p| p.status == PhaseStatus::Running)
    }

    /// First pending phase.
    pub fn next_pending_phase(&self) -> Option<&PlanPhase> {
        self.phases.iter().find(|p| p.status == PhaseStatus::Pending)
    }

    // ── Completion / progress ──────────────────────────────────────

    pub fn is_completed(&self) -> bool {
        self.status == PlanStatus::Completed
            || (!self.phases.is_empty() && self.phases.iter().all(|p| p.is_completed()))
    }

    pub fn has_failed(&self) -> bool {
        self.status == PlanStatus::Failed
            || self.all_steps().any(|s| s.status == StepStatus::Failed)
    }

    /// (completed_steps, total_steps) across all phases.
    pub fn progress(&self) -> (usize, usize) {
        let total = self.total_steps();
        let completed = self
            .all_steps()
            .filter(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
            .count();
        (completed, total)
    }
}

impl PlanStep {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            payload,
            dependencies: Vec::new(),
            status: StepStatus::Pending,
            result: None,
        }
    }

    /// Construct a tool-call step.
    ///
    /// The payload format aligns with `ToolCallingStepExecutor`:
    /// `{"tool_name": "...", "args": {...}}`
    pub fn tool_call(
        id: impl Into<String>,
        description: impl Into<String>,
        tool_name: impl Into<String>,
        args: Value,
    ) -> Self {
        Self::new(
            id,
            description,
            json!({"tool_name": tool_name.into(), "args": args}),
        )
    }

    /// Construct a step with a custom payload (full flexibility).
    pub fn with_payload(
        id: impl Into<String>,
        description: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self::new(id, description, payload)
    }

    pub fn with_dependencies(mut self, dependencies: Vec<String>) -> Self {
        self.dependencies = dependencies;
        self
    }
}

impl StepResult {
    pub fn success(output: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
            duration_ms,
        }
    }

    pub fn failure(error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
            duration_ms,
        }
    }
}
