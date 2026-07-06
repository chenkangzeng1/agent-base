use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::engine::EventBus;
use crate::tool::{FrameworkTool, Tool, ToolContext, ToolOutput};
use crate::types::{AgentResult, RuntimeEvent, UpdatePlanArgs};

/// A lightweight tool that records and displays a plan checklist to the user.
///
/// # Behavior
///
/// - Validates the incoming `UpdatePlanArgs` (non-empty plan, at most one
///   `in_progress` step, no empty step text).
/// - Broadcasts a [`RuntimeEvent::PlanUpdated`] event for UI rendering.
/// - Returns a short summary — it does NOT store, execute, or drive the plan.
///
/// This is a **display-only** protocol, inspired by Codex's `update_plan`.
pub struct UpdatePlanTool {
    event_bus: Mutex<Option<EventBus>>,
}

/// Normalize step text from LLM output for consistent UI rendering.
///
/// - Strips LLM-added numbering prefixes ("Step 1: ", "1. ", "1) ", "(1) ")
/// - Truncates to max 60 chars (terminal-friendly)
/// - Strips leading/trailing whitespace
/// - Falls back to raw text if normalization produces empty string
fn normalize_step_text(raw: &str) -> String {
    let text = raw.trim();

    // First try to strip "Step N: " / "step N. " prefix
    let text = if let Some(rest) = text.strip_prefix("Step").or_else(|| text.strip_prefix("step")) {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ');
        rest.trim_start_matches(|c: char| c == ':' || c == '.' || c == ')' || c == ' ')
    } else {
        text
    };

    // Strip number prefixes: "1. xxx", "1) xxx", "(1) xxx", "1、xxx"
    let text = text.trim_start_matches(|c: char| {
        c.is_ascii_digit() || c == '.' || c == ')' || c == '(' || c == '、' || c == ' '
    });

    // Truncate to 60 chars max
    let text: String = if text.chars().count() > 60 {
        let truncated: String = text.chars().take(57).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    };

    // Strip remaining whitespace
    let text = text.trim();

    // Fallback: don't return empty
    if text.is_empty() {
        raw.trim().to_string()
    } else {
        text.to_string()
    }
}

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self {
            event_bus: Mutex::new(None),
        }
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkTool for UpdatePlanTool {
    fn set_event_bus(&self, event_bus: EventBus) {
        *self.event_bus.lock().unwrap() = Some(event_bus);
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &'static str {
        "update_plan"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "update_plan",
                "description": "Record and display a structured plan / checklist to track progress on a complex task.\n\nUse this to show the user what steps you plan to take and update step statuses as you go.\n\nRules:\n- Always include the user's goal as `objective`\n- Plan must have at least one step\n- At most one step may be in_progress at a time\n- Step descriptions should be concise and human-readable\n- Call this again whenever step statuses change\n- Skip this for simple/trivial tasks",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "objective": {
                            "type": "string",
                            "description": "One-sentence summary of the user's goal. Example: \"安装 Casdoor 身份认证系统\". Must reflect the user's original intent, not just the current sub-task."
                        },
                        "explanation": {
                            "type": "string",
                            "description": "Optional explanation of why the plan is being created or changed."
                        },
                        "plan": {
                            "type": "array",
                            "description": "The complete plan checklist. Each item has a step description and status.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "step": {
                                        "type": "string",
                                        "description": "Short description of this step (5-7 words)."
                                    },
                                    "status": {
                                        "type": "string",
                                        "enum": ["pending", "in_progress", "completed"],
                                        "description": "Current status of this step."
                                    }
                                },
                                "required": ["step", "status"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["objective", "plan"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> {
        Some(self)
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let plan_args: UpdatePlanArgs = serde_json::from_value(args.clone())
            .map_err(|e| crate::types::AgentError::ToolArgsInvalid {
                name: "update_plan".to_string(),
                raw: format!("deserialization error: {e}"),
            })?;

        // Validate
        if let Err(validation_err) = plan_args.validate() {
            return Err(crate::types::AgentError::ToolArgsInvalid {
                name: "update_plan".to_string(),
                raw: validation_err,
            });
        }

        // Normalize step text for consistent UI rendering
        let normalized_plan: Vec<crate::types::PlanItem> = plan_args
            .plan
            .into_iter()
            .map(|item| crate::types::PlanItem {
                step: normalize_step_text(&item.step),
                status: item.status,
            })
            .collect();

        // Count steps by status for summary
        let total = normalized_plan.len();
        let completed = normalized_plan
            .iter()
            .filter(|item| item.status == crate::types::PlanStepStatus::Completed)
            .count();
        let in_progress = normalized_plan
            .iter()
            .filter(|item| item.status == crate::types::PlanStepStatus::InProgress)
            .count();

        // Broadcast PlanUpdated event
        {
            let guard = self.event_bus.lock().unwrap();
            if let Some(ref event_bus) = *guard {
                event_bus.emit(RuntimeEvent::PlanUpdated {
                    session_id: _ctx.session_id.clone(),
                    objective: plan_args.objective.clone(),
                    explanation: plan_args.explanation.clone(),
                    plan: normalized_plan.clone(),
                });
            }
        }

        // Build a human-readable summary
        let mut summary = format!(
            "📋 {}: {}/{} steps completed",
            plan_args.objective, completed, total
        );
        if in_progress > 0 {
            let current = normalized_plan.iter().find(|item| {
                item.status == crate::types::PlanStepStatus::InProgress
            });
            if let Some(item) = current {
                summary.push_str(&format!(". Current: \"{}\"", item.step));
            }
        }
        if total == completed {
            summary = format!("📋 {} — all steps completed!", plan_args.objective);
        }

        Ok(ToolOutput {
            summary,
            raw: Some(serde_json::to_value(&normalized_plan).unwrap_or_default()),
            control_flow: crate::tool::ToolControlFlow::Break,
            truncation: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PlanItem, PlanStepStatus};

    #[test]
    fn test_update_plan_args_validation() {
        // Valid plan
        let args = UpdatePlanArgs {
            objective: "安装 Docker".into(),
            explanation: None,
            plan: vec![
                PlanItem { step: "Step 1".into(), status: PlanStepStatus::Completed },
                PlanItem { step: "Step 2".into(), status: PlanStepStatus::InProgress },
                PlanItem { step: "Step 3".into(), status: PlanStepStatus::Pending },
            ],
        };
        assert!(args.validate().is_ok());

        // Empty objective
        let args = UpdatePlanArgs {
            objective: "".into(),
            explanation: None,
            plan: vec![
                PlanItem { step: "Step 1".into(), status: PlanStepStatus::Pending },
            ],
        };
        assert!(args.validate().is_err());

        // Empty plan
        let args = UpdatePlanArgs {
            objective: "安装 Docker".into(),
            explanation: None,
            plan: vec![],
        };
        assert!(args.validate().is_err());

        // Multiple in_progress
        let args = UpdatePlanArgs {
            objective: "安装 Docker".into(),
            explanation: None,
            plan: vec![
                PlanItem { step: "Step 1".into(), status: PlanStepStatus::InProgress },
                PlanItem { step: "Step 2".into(), status: PlanStepStatus::InProgress },
            ],
        };
        assert!(args.validate().is_err());

        // Empty step text
        let args = UpdatePlanArgs {
            objective: "安装 Docker".into(),
            explanation: None,
            plan: vec![
                PlanItem { step: "  ".into(), status: PlanStepStatus::Pending },
            ],
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn test_normalize_step_text() {
        // Strip number prefixes
        assert_eq!(normalize_step_text("1. 安装 Docker"), "安装 Docker");
        assert_eq!(normalize_step_text("2) 添加 GPG 密钥"), "添加 GPG 密钥");
        assert_eq!(normalize_step_text("(3) 更新 APT 包列表"), "更新 APT 包列表");
        assert_eq!(normalize_step_text("Step 1: 安装 Docker"), "安装 Docker");
        assert_eq!(normalize_step_text("step 2: 更新包列表"), "更新包列表");
        assert_eq!(normalize_step_text("1、配置仓库"), "配置仓库");

        // Truncate long text
        let long = "使用 apt install -y docker-ce docker-ce-cli containerd.io 命令来安装 Docker 引擎以及相关组件";
        let result = normalize_step_text(long);
        assert!(result.chars().count() <= 60);
        assert!(result.ends_with("..."));

        // Preserve short text
        assert_eq!(normalize_step_text("安装 Docker 引擎"), "安装 Docker 引擎");

        // Strip whitespace
        assert_eq!(normalize_step_text("  安装 Docker  "), "安装 Docker");

        // Fallback: don't return empty
        let result = normalize_step_text("123");
        assert!(!result.is_empty());
    }
}
