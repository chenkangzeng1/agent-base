use serde::{Deserialize, Serialize};

/// Lightweight plan step status — display-only, no execution semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

/// A single step in a lightweight plan checklist.
///
/// Contains only human-readable display text and a status.
/// Does NOT carry tool names, command payloads, host info, or dependency graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub step: String,
    pub status: PlanStepStatus,
}

/// Arguments for the `update_plan` tool.
///
/// This is a lightweight progress-display protocol — the tool broadcasts a
/// structured snapshot to the UI and does nothing else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePlanArgs {
    /// A one-sentence summary of the user's goal.
    /// Required — forces the LLM to articulate and stay focused on the objective.
    /// Example: "安装 Casdoor 身份认证系统"
    pub objective: String,
    /// Optional explanation of why the plan changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// The full plan checklist (replaces any previous display).
    pub plan: Vec<PlanItem>,
}

impl UpdatePlanArgs {
    /// Validate the plan arguments.
    ///
    /// Rules:
    /// - `objective` must be non-empty after trimming and at most 200 chars.
    /// - `plan` must contain 1–50 steps.
    /// - Each `step` must be non-empty.
    /// - At most one step may be `InProgress`.
    pub fn validate(&self) -> Result<(), String> {
        let objective_trimmed = self.objective.trim();
        if objective_trimmed.is_empty() {
            return Err("objective must not be empty".to_string());
        }
        if objective_trimmed.chars().count() > 200 {
            return Err(format!(
                "objective must be at most 200 characters, got {}",
                objective_trimmed.chars().count()
            ));
        }
        if self.plan.is_empty() {
            return Err("plan must contain at least one step".to_string());
        }
        if self.plan.len() > 50 {
            return Err(format!(
                "plan must contain at most 50 steps, got {}",
                self.plan.len()
            ));
        }

        let in_progress_count = self
            .plan
            .iter()
            .filter(|item| item.status == PlanStepStatus::InProgress)
            .count();

        if in_progress_count > 1 {
            return Err(format!(
                "at most one step may be in_progress, found {}",
                in_progress_count
            ));
        }

        for (i, item) in self.plan.iter().enumerate() {
            if item.step.trim().is_empty() {
                return Err(format!("step[{}] must not be empty", i));
            }
        }

        Ok(())
    }
}
