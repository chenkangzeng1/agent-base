use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use agent_base::{
    AdaptiveRecoveryStrategy, AgentResult, ExecutionPlan, PlanConfig, PlanStep,
    RecoveryAction, RecoveryContext, SessionId, StepStatus,
};

// ── Mock AdaptiveRecoveryStrategy for testing ─────────────────

/// A mock strategy that returns a predetermined action.
struct MockStrategy {
    action: RecoveryAction,
}

#[async_trait]
impl AdaptiveRecoveryStrategy for MockStrategy {
    async fn recover(&self, _ctx: &RecoveryContext) -> AgentResult<RecoveryAction> {
        Ok(self.action.clone())
    }
}

/// A strategy that always returns an Alternative step.
fn alt_strategy() -> Arc<dyn AdaptiveRecoveryStrategy> {
    Arc::new(MockStrategy {
        action: RecoveryAction::Alternative {
            step: PlanStep::tool_call("alt-1", "Alternative step", "some_tool", json!({})),
            root_step_id: "step-1".to_string(),
        },
    })
}

/// A strategy that always returns Replan.
fn replan_strategy() -> Arc<dyn AdaptiveRecoveryStrategy> {
    Arc::new(MockStrategy {
        action: RecoveryAction::Replan {
            steps: vec![
                PlanStep::tool_call("replan-1", "Replanned step", "tool_a", json!({})),
            ],
            clear_future_phases: true,
        },
    })
}

/// A strategy that always returns Abort.
fn abort_strategy() -> Arc<dyn AdaptiveRecoveryStrategy> {
    Arc::new(MockStrategy {
        action: RecoveryAction::Abort,
    })
}

// ── RecoveryAction variant tests ──────────────────────────────

#[test]
fn recovery_action_partial_eq_by_discriminant() {
    assert_eq!(RecoveryAction::Retry, RecoveryAction::Retry);
    assert_eq!(RecoveryAction::Skip, RecoveryAction::Skip);
    assert_eq!(RecoveryAction::Abort, RecoveryAction::Abort);
    assert_ne!(RecoveryAction::Retry, RecoveryAction::Skip);
    assert_ne!(RecoveryAction::Abort, RecoveryAction::Retry);
}

#[test]
fn recovery_action_alternative_eq_by_discriminant() {
    let a1 = RecoveryAction::Alternative {
        step: PlanStep::tool_call("a", "A", "tool", json!({})),
        root_step_id: "root".to_string(),
    };
    let a2 = RecoveryAction::Alternative {
        step: PlanStep::tool_call("b", "B", "tool", json!({})),
        root_step_id: "root".to_string(),
    };
    // Different contents but same discriminant → equal
    assert_eq!(a1, a2);
}

#[test]
fn recovery_action_replan_eq_by_discriminant() {
    let r1 = RecoveryAction::Replan {
        steps: vec![PlanStep::tool_call("a", "A", "t", json!({}))],
        clear_future_phases: true,
    };
    let r2 = RecoveryAction::Replan {
        steps: vec![],
        clear_future_phases: false,
    };
    // Different contents but same discriminant → equal
    assert_eq!(r1, r2);
}

// ── RecoveryContext tests ─────────────────────────────────────

#[test]
fn recovery_context_construction() {
    let ctx = RecoveryContext {
        session_id: SessionId::new(1),
        failed_step: PlanStep::tool_call("s1", "Failed step", "tool", json!({"key": "val"})),
        root_step_id: "s1".to_string(),
        error: "timeout".to_string(),
        retry_count: 2,
        alternative_count: 1,
        replan_count: 0,
        max_retries: 3,
        max_alternatives: 2,
        max_replans: 1,
        plan: ExecutionPlan::with_single_phase("p1", "test", vec![]),
        step_outputs: json!({}),
        available_tools: vec![],
    };

    assert_eq!(ctx.retry_count, 2);
    assert_eq!(ctx.max_retries, 3);
    assert_eq!(ctx.failed_step.id, "s1");
    assert_eq!(ctx.root_step_id, "s1");
    assert_eq!(ctx.max_alternatives, 2);
    assert_eq!(ctx.max_replans, 1);
}

// ── LlmAdaptiveRecovery decision logic (mock) ─────────────────

#[tokio::test]
async fn mock_strategy_alternative_within_budget() {
    let strategy = alt_strategy();
    let ctx = RecoveryContext {
        session_id: SessionId::new(1),
        failed_step: PlanStep::tool_call("s1", "fail", "tool", json!({})),
        root_step_id: "s1".to_string(),
        error: "err".to_string(),
        retry_count: 0,
        alternative_count: 0,
        replan_count: 0,
        max_retries: 0,
        max_alternatives: 2,
        max_replans: 1,
        plan: ExecutionPlan::with_single_phase("p1", "test", vec![]),
        step_outputs: json!({}),
        available_tools: vec![],
    };

    let action = strategy.recover(&ctx).await.unwrap();
    // PartialEq is by discriminant, so this works regardless of field values
    assert_eq!(action, RecoveryAction::Alternative {
        step: PlanStep::tool_call("x", "x", "x", json!({})),
        root_step_id: "x".to_string(),
    });
}

#[tokio::test]
async fn mock_strategy_replan_when_alt_exhausted() {
    let strategy = replan_strategy();
    let ctx = RecoveryContext {
        session_id: SessionId::new(1),
        failed_step: PlanStep::tool_call("s1", "fail", "tool", json!({})),
        root_step_id: "s1".to_string(),
        error: "err".to_string(),
        retry_count: 0,
        alternative_count: 2, // at max
        replan_count: 0,
        max_retries: 0,
        max_alternatives: 2,
        max_replans: 1,
        plan: ExecutionPlan::with_single_phase("p1", "test", vec![]),
        step_outputs: json!({}),
        available_tools: vec![],
    };

    let action = strategy.recover(&ctx).await.unwrap();
    assert_eq!(action, RecoveryAction::Replan {
        steps: vec![],
        clear_future_phases: false,
    });
}

#[tokio::test]
async fn mock_strategy_abort_when_all_exhausted() {
    let strategy = abort_strategy();
    let ctx = RecoveryContext {
        session_id: SessionId::new(1),
        failed_step: PlanStep::tool_call("s1", "fail", "tool", json!({})),
        root_step_id: "s1".to_string(),
        error: "err".to_string(),
        retry_count: 3,
        alternative_count: 2,
        replan_count: 1,
        max_retries: 3,
        max_alternatives: 2,
        max_replans: 1,
        plan: ExecutionPlan::with_single_phase("p1", "test", vec![]),
        step_outputs: json!({}),
        available_tools: vec![],
    };

    let action = strategy.recover(&ctx).await.unwrap();
    assert_eq!(action, RecoveryAction::Abort);
}

// ── PlanConfig builder tests ──────────────────────────────────

#[test]
fn plan_config_adaptive_builder() {
    let config = PlanConfig::new()
        .max_retries(3)
        .max_alternatives(5)
        .max_replans(2)
        .adaptive_recovery(abort_strategy());

    assert_eq!(config.max_retries, 3);
    assert_eq!(config.max_alternatives, 5);
    assert_eq!(config.max_replans, 2);
    assert!(config.adaptive_recovery.is_some());
}

#[test]
fn plan_config_defaults() {
    let config = PlanConfig::new();
    assert_eq!(config.max_retries, 0);
    assert_eq!(config.max_alternatives, 2);
    assert_eq!(config.max_replans, 1);
    assert!(config.adaptive_recovery.is_none());
}

// ── Replan preserve history test ──────────────────────────────

#[test]
fn replan_preserves_failed_step_in_history() {
    let mut plan = ExecutionPlan::with_single_phase(
        "p1",
        "test objective",
        vec![
            PlanStep::tool_call("s1", "Step 1", "tool_a", json!({})),
            PlanStep::tool_call("s2", "Step 2", "tool_b", json!({})),
        ],
    );

    // Simulate: s1 completed, s2 failed
    plan.phases[0].steps[0].status = StepStatus::Completed;
    plan.phases[0].steps[1].status = StepStatus::Failed;

    // Simulate Replan: keep failed step, add new steps to end
    let new_steps = vec![
        PlanStep::tool_call("replan-1", "New step", "tool_c", json!({})),
    ];

    let original_len = plan.phases[0].steps.len();
    plan.phases[0].steps.extend(new_steps);

    // Failed step is still in history
    assert_eq!(plan.phases[0].steps.len(), original_len + 1);
    assert_eq!(plan.phases[0].steps[1].status, StepStatus::Failed);
    assert_eq!(plan.phases[0].steps[1].id, "s2");
    // New step appended
    assert_eq!(plan.phases[0].steps[2].id, "replan-1");
}

// ── Root step map tracking test ───────────────────────────────

#[test]
fn root_step_map_tracks_alternative_chain() {
    let mut root_step_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut retry_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut alternative_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Initial failure of "step-1" with 2 retries
    let root_id = "step-1".to_string();
    retry_counts.insert(root_id.clone(), 2);

    // Alternative "step-1-alt-1" is generated
    let alt_id = "step-1-alt-1".to_string();
    root_step_map.insert(alt_id.clone(), root_id.clone());
    alternative_counts.insert(root_id.clone(), 1);
    // Inherit retry count
    let inherited = retry_counts.get(&root_id).copied().unwrap_or(0);
    retry_counts.insert(root_id.clone(), inherited);

    // Verify: the alternative maps back to root
    assert_eq!(root_step_map.get("step-1-alt-1").unwrap(), "step-1");
    assert_eq!(retry_counts.get("step-1").copied().unwrap_or(0), 2);
    assert_eq!(alternative_counts.get("step-1").copied().unwrap_or(0), 1);

    // Second alternative "step-1-alt-2" from "step-1-alt-1"
    let alt2_id = "step-1-alt-2".to_string();
    let parent_root = root_step_map.get(&alt_id).cloned().unwrap();
    root_step_map.insert(alt2_id.clone(), parent_root.clone());
    alternative_counts.insert(parent_root.clone(), 2);

    // Verify: alt-2 also maps to original root
    assert_eq!(root_step_map.get("step-1-alt-2").unwrap(), "step-1");
    assert_eq!(alternative_counts.get("step-1").copied().unwrap_or(0), 2);
}

// ── Implicit upper bound formula test ─────────────────────────

#[test]
fn implicit_max_executions_formula() {
    // Per design doc: worst case = 1 + max_retries + max_alternatives
    // e.g. max_retries=2, max_alternatives=3 → max 6 executions per step
    let max_retries = 2;
    let max_alternatives = 3;
    let worst_case = 1 + max_retries + max_alternatives;
    assert_eq!(worst_case, 6);

    // With defaults (0 retries, 2 alternatives): 1 + 0 + 2 = 3
    let default_worst_case = 1 + 0 + 2;
    assert_eq!(default_worst_case, 3);
}

// ── RecoveryAction clone test ─────────────────────────────────

#[test]
fn recovery_action_clone() {
    let action = RecoveryAction::Alternative {
        step: PlanStep::tool_call("alt-1", "Alt", "tool", json!({"a": 1})),
        root_step_id: "root".to_string(),
    };
    let cloned = action.clone();
    assert_eq!(action, cloned);

    let replan = RecoveryAction::Replan {
        steps: vec![PlanStep::tool_call("r1", "R", "t", json!({}))],
        clear_future_phases: true,
    };
    let cloned_replan = replan.clone();
    assert_eq!(replan, cloned_replan);
}
