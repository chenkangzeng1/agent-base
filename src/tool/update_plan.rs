use std::fmt::Write;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Value, json};

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
    last_objective: Mutex<Option<String>>,
    custom_description: Option<String>,
}

/// Normalize step text from LLM output for consistent UI rendering.
///
/// - Strips LLM-added numbering prefixes ("Step 1: ", "1. ", "1) ", "(1) ",
///   "第1步：", "第一步：")
/// - Truncates to max 60 chars (terminal-friendly)
/// - Strips leading/trailing whitespace
/// - Falls back to raw text if normalization produces empty string
fn normalize_step_text(raw: &str) -> String {
    let text = raw.trim();

    // Step 1: Strip "Step N: " / "step N. " prefix (English)
    let text = if let Some(rest) = text
        .strip_prefix("Step")
        .or_else(|| text.strip_prefix("step"))
    {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ');
        rest.trim_start_matches([':', '.', ')', ' '])
    } else {
        text
    };

    // Step 2: Strip Chinese "第N步" / "第 N 步：" / "第一步：" patterns
    let text = if let Some(rest) = text.strip_prefix('第') {
        // Strip digits, spaces, and common Chinese number characters
        let rest = rest.trim_start_matches(|c: char| {
            c.is_ascii_digit()
                || c == ' '
                || matches!(
                    c,
                    '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
                )
        });
        // Strip "步" and following punctuation/whitespace
        rest.strip_prefix('步')
            .map(|r| r.trim_start_matches(['：', ':', '、', '.', ')', ' ']))
            .unwrap_or(text)
    } else {
        text
    };

    // Step 3: Strip bare number prefixes: "1. ", "1) ", "(1) ", "1-2) ", "3/5) ", "1、"
    let text = text.trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '.' | ')' | '(' | '、' | ' ' | '-' | '/')
    });

    // Step 4: Truncate to 60 chars max
    let text = if text.chars().count() > 60 {
        let truncated: String = text.chars().take(57).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    };

    // Step 5: Final trim — avoid redundant allocation when no trimming needed
    let trimmed = text.trim();
    if trimmed.is_empty() {
        raw.trim().to_string()
    } else if trimmed.len() < text.len() {
        trimmed.to_string()
    } else {
        text
    }
}

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self {
            event_bus: Mutex::new(None),
            last_objective: Mutex::new(None),
            custom_description: None,
        }
    }

    /// Override the tool description with a custom string.
    ///
    /// The default description is written for the generic agent-base framework.
    /// Consumers like ops-agent can use this to inject domain-specific usage
    /// guidelines (e.g. step granularity rules, when to use/not use plans).
    pub fn with_description(mut self, desc: String) -> Self {
        self.custom_description = Some(desc);
        self
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
        let description = self.custom_description.as_deref().unwrap_or(
            "Record and display a structured plan / checklist to track progress on a complex task.\n\n\
            Use this to show the user what steps you plan to take and update step statuses as you go.\n\n\
            Rules:\n\
            - Always include the user's goal as `objective`\n\
            - Plan must have at least one step\n\
            - At most one step may be in_progress at a time\n\
            - Step descriptions should be concise and human-readable\n\
            - Call this again whenever step statuses change\n\
            - Skip this for simple/trivial tasks\n\n\
            When creating a plan, follow these principles:\n\
            1. 探查先行 — 第一步先确认相关组件和依赖是否就绪\n\
            2. 依赖排序 — 被依赖的先执行，独立步骤可并行但不强制\n\
            3. 每步闭环 — 一步做完可独立验证结果，不等下步才知道成败\n\
            4. 标注风险 — 涉及 rm、kill、restart、改配置文件时注明\n\
            5. 粒度适中 — 不过细也不过大，每步是独立可验证的最小逻辑单元\n\
            6. 收敛止步 — 步骤过多说明任务需要拆分或先讨论再定"
        );
        json!({
            "type": "function",
            "function": {
                "name": "update_plan",
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "objective": {
                            "type": "string",
                            "description": "One-sentence summary of the user's goal. Example: \"安装 Casdoor 身份认证系统\". Must reflect the user's original intent, not just the current sub-task. Optional on subsequent calls — the tool remembers the last objective."
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
                                        "description": "Short description of this step (5-7 words). Example: '安装 Docker 引擎'"
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
                    "required": ["plan"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn metadata(&self) -> crate::tool::ToolMetadata {
        crate::tool::ToolMetadata {
            name: self.name().to_string(),
            description: "Create or update a task plan to show the user a checklist with progress."
                .to_string(),
            origin: "agent-base".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            requirements: vec![],
        }
    }

    #[allow(private_interfaces)]
    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> {
        Some(self)
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let plan_args: UpdatePlanArgs = serde_json::from_value(args.clone()).map_err(|e| {
            crate::types::AgentError::ToolArgsInvalid {
                name: "update_plan".to_string(),
                raw: format!("deserialization error: {e}"),
            }
        })?;

        // Validate
        if let Err(validation_err) = plan_args.validate() {
            return Err(crate::types::AgentError::ToolArgsInvalid {
                name: "update_plan".to_string(),
                raw: validation_err,
            });
        }

        // Resolve objective: use provided value, or fall back to last known
        let objective = match plan_args.objective {
            Some(ref obj) => {
                *self.last_objective.lock().unwrap() = Some(obj.clone());
                obj.clone()
            }
            None => self
                .last_objective
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "(no objective specified)".to_string()),
        };

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

        // Build summary and raw output BEFORE emitting event (so we can move
        // normalized_plan into the event instead of cloning it).
        let raw = Some(serde_json::to_value(&normalized_plan).unwrap_or_default());

        let mut summary = format!("📋 {}: {}/{} steps completed", objective, completed, total);
        if in_progress > 0 {
            let current = normalized_plan
                .iter()
                .find(|item| item.status == crate::types::PlanStepStatus::InProgress);
            if let Some(item) = current {
                write!(summary, ". Current: \"{}\"", item.step).unwrap();
            }
        }
        if total == completed {
            summary = format!("📋 {} — all steps completed!", objective);
        }

        // Broadcast PlanUpdated event (normalized_plan is moved here, not cloned)
        {
            let guard = self.event_bus.lock().unwrap();
            if let Some(ref event_bus) = *guard {
                event_bus.emit(RuntimeEvent::PlanUpdated {
                    session_id: _ctx.session_id.clone(),
                    objective: objective.clone(),
                    explanation: plan_args.explanation.clone(),
                    plan: normalized_plan,
                    agent_id: None,
                    trace_id: None,
                });
            }
        }

        Ok(ToolOutput {
            summary,
            raw,
            control_flow: crate::tool::ToolControlFlow::Continue,
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
        // Valid plan (with objective)
        let args = UpdatePlanArgs {
            objective: Some("安装 Docker".into()),
            explanation: None,
            plan: vec![
                PlanItem {
                    step: "Step 1".into(),
                    status: PlanStepStatus::Completed,
                },
                PlanItem {
                    step: "Step 2".into(),
                    status: PlanStepStatus::InProgress,
                },
                PlanItem {
                    step: "Step 3".into(),
                    status: PlanStepStatus::Pending,
                },
            ],
        };
        assert!(args.validate().is_ok());

        // Valid plan (no objective — should be allowed)
        let args = UpdatePlanArgs {
            objective: None,
            explanation: None,
            plan: vec![PlanItem {
                step: "Step 1".into(),
                status: PlanStepStatus::Pending,
            }],
        };
        assert!(args.validate().is_ok());

        // Empty objective (provided but blank — should fail)
        let args = UpdatePlanArgs {
            objective: Some("".into()),
            explanation: None,
            plan: vec![PlanItem {
                step: "Step 1".into(),
                status: PlanStepStatus::Pending,
            }],
        };
        assert!(args.validate().is_err());

        // Empty plan
        let args = UpdatePlanArgs {
            objective: Some("安装 Docker".into()),
            explanation: None,
            plan: vec![],
        };
        assert!(args.validate().is_err());

        // Multiple in_progress
        let args = UpdatePlanArgs {
            objective: Some("安装 Docker".into()),
            explanation: None,
            plan: vec![
                PlanItem {
                    step: "Step 1".into(),
                    status: PlanStepStatus::InProgress,
                },
                PlanItem {
                    step: "Step 2".into(),
                    status: PlanStepStatus::InProgress,
                },
            ],
        };
        assert!(args.validate().is_err());

        // Empty step text
        let args = UpdatePlanArgs {
            objective: Some("安装 Docker".into()),
            explanation: None,
            plan: vec![PlanItem {
                step: "  ".into(),
                status: PlanStepStatus::Pending,
            }],
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn test_normalize_step_text() {
        // Strip number prefixes
        assert_eq!(normalize_step_text("1. 安装 Docker"), "安装 Docker");
        assert_eq!(normalize_step_text("2) 添加 GPG 密钥"), "添加 GPG 密钥");
        assert_eq!(
            normalize_step_text("(3) 更新 APT 包列表"),
            "更新 APT 包列表"
        );
        assert_eq!(normalize_step_text("Step 1: 安装 Docker"), "安装 Docker");
        assert_eq!(normalize_step_text("step 2: 更新包列表"), "更新包列表");
        assert_eq!(normalize_step_text("1、配置仓库"), "配置仓库");

        // Strip hyphenated and slashed number prefixes
        assert_eq!(normalize_step_text("1-2) Install Docker"), "Install Docker");
        assert_eq!(normalize_step_text("3/5) Verify config"), "Verify config");

        // Strip Chinese numbering prefixes
        assert_eq!(normalize_step_text("第一步：安装 Docker"), "安装 Docker");
        assert_eq!(normalize_step_text("第1步：添加 GPG 密钥"), "添加 GPG 密钥");
        assert_eq!(normalize_step_text("第 3 步: 更新包列表"), "更新包列表");
        assert_eq!(normalize_step_text("第二步、配置仓库"), "配置仓库");

        // Strip "第" without "步" gracefully (revert to original)
        assert_eq!(normalize_step_text("第一个任务：安装"), "第一个任务：安装");

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
