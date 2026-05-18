use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{AgentResult, StepActionType, StepResult};

#[async_trait]
pub trait ReflexionHandler: Send + Sync {
    async fn reflect_on_failure(
        &self,
        failed_action: &str,
        error: &str,
        context: &str,
    ) -> AgentResult<ReflectionResult>;

    async fn generate_alternatives(
        &self,
        reflection: &ReflectionResult,
    ) -> AgentResult<Vec<AlternativeAction>>;

    async fn should_retry(
        &self,
        reflection: &ReflectionResult,
        retry_count: usize,
    ) -> AgentResult<bool>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionResult {
    pub analysis: String,
    pub root_cause: String,
    pub confidence: f32,
    pub suggested_fixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeAction {
    pub description: String,
    pub action_type: StepActionType,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionContext {
    pub objective: String,
    pub failed_step_id: String,
    pub failed_step_description: String,
    pub command: String,
    pub error: String,
    pub previous_steps: Vec<StepHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepHistoryEntry {
    pub step_id: String,
    pub description: String,
    pub success: bool,
    pub output: Option<String>,
}

impl ReflectionResult {
    pub fn new(
        analysis: impl Into<String>,
        root_cause: impl Into<String>,
        confidence: f32,
        suggested_fixes: Vec<String>,
    ) -> Self {
        Self {
            analysis: analysis.into(),
            root_cause: root_cause.into(),
            confidence,
            suggested_fixes,
        }
    }

    pub fn is_confident(&self) -> bool {
        self.confidence >= 0.7
    }
}

impl AlternativeAction {
    pub fn new(
        description: impl Into<String>,
        action_type: StepActionType,
        priority: u32,
    ) -> Self {
        Self {
            description: description.into(),
            action_type,
            priority,
        }
    }
}

impl ReflexionContext {
    pub fn from_step(
        objective: &str,
        step: &crate::types::PlanStep,
        error: &str,
    ) -> Self {
        let (command, _) = match &step.action_type {
            StepActionType::SshCommand { command, host_id } => (command.clone(), host_id.clone()),
            _ => (String::new(), String::new()),
        };

        Self {
            objective: objective.to_string(),
            failed_step_id: step.id.clone(),
            failed_step_description: step.description.clone(),
            command,
            error: error.to_string(),
            previous_steps: Vec::new(),
        }
    }

    pub fn with_previous_steps(mut self, steps: Vec<StepHistoryEntry>) -> Self {
        self.previous_steps = steps;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PlanStep;

    #[test]
    fn test_reflection_result_new() {
        let result = ReflectionResult::new("分析", "根因", 0.8, vec!["修复1".to_string()]);

        assert_eq!(result.analysis, "分析");
        assert_eq!(result.root_cause, "根因");
        assert_eq!(result.confidence, 0.8);
        assert_eq!(result.suggested_fixes.len(), 1);
    }

    #[test]
    fn test_reflection_result_is_confident() {
        let high_confidence = ReflectionResult::new("", "", 0.9, vec![]);
        assert!(high_confidence.is_confident());

        let low_confidence = ReflectionResult::new("", "", 0.5, vec![]);
        assert!(!low_confidence.is_confident());

        let threshold = ReflectionResult::new("", "", 0.7, vec![]);
        assert!(threshold.is_confident());
    }

    #[test]
    fn test_alternative_action_new() {
        let action = AlternativeAction::new(
            "description",
            StepActionType::SshCommand {
                command: "ls".to_string(),
                host_id: "host1".to_string(),
            },
            1,
        );

        assert_eq!(action.description, "description");
        assert_eq!(action.priority, 1);
    }

    #[test]
    fn test_reflexion_context_from_step() {
        let step = PlanStep::new(
            "step-1",
            "检查磁盘",
            StepActionType::SshCommand {
                command: "df -h".to_string(),
                host_id: "host1".to_string(),
            },
        );

        let context = ReflexionContext::from_step("目标", &step, "错误");

        assert_eq!(context.objective, "目标");
        assert_eq!(context.failed_step_id, "step-1");
        assert_eq!(context.command, "df -h");
        assert_eq!(context.error, "错误");
        assert!(context.previous_steps.is_empty());
    }

    #[test]
    fn test_reflexion_context_with_previous_steps() {
        let context = ReflexionContext {
            objective: "目标".to_string(),
            failed_step_id: "step-2".to_string(),
            failed_step_description: "描述".to_string(),
            command: "cmd".to_string(),
            error: "error".to_string(),
            previous_steps: Vec::new(),
        }
        .with_previous_steps(vec![StepHistoryEntry {
            step_id: "step-1".to_string(),
            description: "前置步骤".to_string(),
            success: true,
            output: Some("done".to_string()),
        }]);

        assert_eq!(context.previous_steps.len(), 1);
        assert_eq!(context.previous_steps[0].step_id, "step-1");
    }

    #[test]
    fn test_reflection_result_serialization() {
        let result = ReflectionResult::new("分析", "根因", 0.8, vec!["修复1".to_string()]);

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ReflectionResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.analysis, "分析");
        assert_eq!(deserialized.root_cause, "根因");
        assert_eq!(deserialized.confidence, 0.8);
    }

    #[test]
    fn test_step_history_entry_serialization() {
        let entry = StepHistoryEntry {
            step_id: "step-1".to_string(),
            description: "描述".to_string(),
            success: true,
            output: Some("输出".to_string()),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: StepHistoryEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.step_id, "step-1");
        assert!(deserialized.success);
        assert_eq!(deserialized.output, Some("输出".to_string()));
    }
}
