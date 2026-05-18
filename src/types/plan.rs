use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub id: String,
    pub objective: String,
    pub steps: Vec<PlanStep>,
    pub status: PlanStatus,
    #[serde(default)]
    pub context: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub action_type: StepActionType,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<StepResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum StepActionType {
    SshCommand { command: String, host_id: String },
    ToolCall { tool_name: String, args: Value },
    WaitForUser { prompt: String },
    SubPlan { plan: ExecutionPlan },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStatus {
    Created,
    Approved,
    Executing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
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
    pub fn new(id: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            objective: objective.into(),
            steps: Vec::new(),
            status: PlanStatus::Created,
            context: Value::Null,
        }
    }

    pub fn current_step(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.status == StepStatus::Running)
    }

    pub fn next_pending_step(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| s.status == StepStatus::Pending)
    }

    pub fn is_completed(&self) -> bool {
        self.status == PlanStatus::Completed
            || self
                .steps
                .iter()
                .all(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
    }

    pub fn has_failed(&self) -> bool {
        self.status == PlanStatus::Failed || self.steps.iter().any(|s| s.status == StepStatus::Failed)
    }

    pub fn progress(&self) -> (usize, usize) {
        let total = self.steps.len();
        let completed = self
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
            .count();
        (completed, total)
    }
}

impl PlanStep {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        action_type: StepActionType,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            action_type,
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

impl StepActionType {
    pub fn tool_name(&self) -> &str {
        match self {
            StepActionType::SshCommand { .. } => "ssh_command",
            StepActionType::ToolCall { tool_name, .. } => tool_name,
            StepActionType::WaitForUser { .. } => "wait_for_user",
            StepActionType::SubPlan { .. } => "sub_plan",
        }
    }
}
