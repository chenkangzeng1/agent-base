use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryAction {
    Retry,
    Skip,
    Abort,
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
