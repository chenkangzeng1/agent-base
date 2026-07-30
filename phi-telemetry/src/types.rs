//! Structured metrics types for Agent observability.
//!
//! These types were moved from agent-base to phi-telemetry so that agent-base
//! remains a pure runtime kernel with no knowledge of metrics.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ── Outcome enums ──

/// Outcome of a single turn (one LLM interaction).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// Turn completed successfully (text response, no tool calls).
    Completed,
    /// Turn ended with tool calls (agent will loop back for another turn).
    ToolCalls,
    /// Turn ended with an error.
    Error,
    /// Turn hit the max-turns safety limit.
    MaxTurns,
    /// User cancelled this turn.
    Cancelled,
}

/// Outcome of a session (the entire conversation).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// Session completed normally.
    Completed,
    /// Session ended with an unrecoverable error.
    Failed,
    /// User cancelled the session.
    Cancelled,
    /// Max turns exceeded (safety limit).
    MaxTurns,
}

// ── TurnMetrics ──

/// Per-turn metrics — one per LLM interaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TurnMetrics {
    // Timing
    pub turn_number: u32,
    pub started_at: String, // ISO8601
    pub duration_ms: u64,

    // Latency breakdown
    pub time_to_first_token_ms: u64,
    pub llm_duration_ms: u64,
    pub tool_duration_ms: u64,

    // LLM
    pub llm_calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,

    // Tool
    pub tool_call_count: u32,
    pub tools_used: Vec<String>,
    pub tool_success: u32,
    pub tool_failed: u32,

    // Result
    pub outcome: TurnOutcome,
    pub text_length: u64,
    pub error_message: Option<String>,
    pub has_thinking: bool,

    // Meta events
    #[serde(default)]
    pub plan_updates: u32,
    #[serde(default)]
    pub approval_count: u32,

    // User input (truncated to 80 chars)
    pub user_input: String,

    // Business extension
    #[serde(default)]
    pub custom: Value,
}

impl TurnMetrics {
    /// Create a new TurnMetrics with defaults (custom = {}).
    pub fn new(
        turn_number: u32,
        started_at: String,
        duration_ms: u64,
        model: String,
        user_input: String,
        outcome: TurnOutcome,
    ) -> Self {
        Self {
            turn_number,
            started_at,
            duration_ms,
            time_to_first_token_ms: 0,
            llm_duration_ms: 0,
            tool_duration_ms: 0,
            llm_calls: 1,
            input_tokens: 0,
            output_tokens: 0,
            model,
            tool_call_count: 0,
            tools_used: Vec::new(),
            tool_success: 0,
            tool_failed: 0,
            outcome,
            text_length: 0,
            error_message: None,
            has_thinking: false,
            plan_updates: 0,
            approval_count: 0,
            user_input: truncate_str(&user_input, 80),
            custom: Value::Object(serde_json::Map::new()),
        }
    }
}

// ── SessionMetrics ──

/// Accumulated session metrics. Written incrementally to `session_metrics.json`
/// at the end of each turn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetrics {
    // Identity
    pub session_id: String,
    #[serde(default)]
    pub node_id: String,
    pub created_at: String,

    // LLM summary
    pub model: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub estimated_cost: f64,

    // Tool summary
    pub total_tool_calls: u32,
    pub tool_breakdown: HashMap<String, u32>,
    pub tool_fail_rate: f64,
    /// Running total of failed tool calls.
    #[serde(default)]
    pub total_failed: u32,

    // Timing
    pub total_duration_ms: u64,
    pub total_llm_ms: u64,
    pub total_tool_ms: u64,
    pub total_turns: u32,
    pub avg_turn_ms: u64,
    pub p50_turn_ms: u64,
    pub p95_turn_ms: u64,
    pub p99_turn_ms: u64,

    // Outcome
    pub outcome: SessionOutcome,
    pub error_count: u32,

    // Meta events
    #[serde(default)]
    pub total_plan_updates: u32,
    #[serde(default)]
    pub total_approvals: u32,

    // Business extension
    #[serde(default)]
    pub custom: Value,

    // Multi-agent reservation (Phase 1: always None / "default")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default = "default_session_type")]
    pub session_type: String,

    // Per-turn details
    pub turns: Vec<TurnMetrics>,
}

fn default_session_type() -> String {
    "default".to_string()
}

impl SessionMetrics {
    /// Create a new session metrics accumulator (empty turns).
    pub fn new(session_id: String, node_id: String, model: String) -> Self {
        Self {
            session_id,
            node_id,
            created_at: chrono::Utc::now().to_rfc3339(),
            model,
            total_input_tokens: 0,
            total_output_tokens: 0,
            estimated_cost: 0.0,
            total_tool_calls: 0,
            tool_breakdown: HashMap::new(),
            tool_fail_rate: 0.0,
            total_failed: 0,
            total_duration_ms: 0,
            total_llm_ms: 0,
            total_tool_ms: 0,
            total_turns: 0,
            avg_turn_ms: 0,
            p50_turn_ms: 0,
            p95_turn_ms: 0,
            p99_turn_ms: 0,
            outcome: SessionOutcome::Completed,
            error_count: 0,
            total_plan_updates: 0,
            total_approvals: 0,
            custom: Value::Object(serde_json::Map::new()),
            parent_session_id: None,
            session_type: "default".to_string(),
            turns: Vec::new(),
        }
    }

    /// Append a turn and recompute session-level aggregates.
    pub fn append_turn(&mut self, turn: TurnMetrics) {
        self.total_turns += 1;
        self.total_input_tokens += turn.input_tokens;
        self.total_output_tokens += turn.output_tokens;
        self.total_duration_ms += turn.duration_ms;
        self.total_llm_ms += turn.llm_duration_ms;
        self.total_tool_ms += turn.tool_duration_ms;
        self.total_tool_calls += turn.tool_call_count;

        // Tool breakdown
        for tool_name in &turn.tools_used {
            *self.tool_breakdown.entry(tool_name.clone()).or_insert(0) += 1;
        }

        // Tool fail rate (incremental — O(1) per turn)
        self.total_failed += turn.tool_failed;
        if self.total_tool_calls > 0 {
            self.tool_fail_rate = self.total_failed as f64 / self.total_tool_calls as f64;
        }

        // Error count
        if matches!(turn.outcome, TurnOutcome::Error) {
            self.error_count += 1;
        }

        // Meta events
        self.total_plan_updates += turn.plan_updates;
        self.total_approvals += turn.approval_count;

        // Average turn duration
        self.avg_turn_ms = self.total_duration_ms / self.total_turns as u64;

        // Update model to the most frequently used one
        {
            let mut model_counts: HashMap<String, u32> = HashMap::new();
            for t in &self.turns {
                if !t.model.is_empty() {
                    *model_counts.entry(t.model.clone()).or_insert(0) += 1;
                }
            }
            if !turn.model.is_empty() {
                *model_counts.entry(turn.model.clone()).or_insert(0) += 1;
            }
            if let Some(top_model) = model_counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(m, _)| m)
            {
                self.model = top_model;
            }
        }

        // Percentiles
        self.turns.push(turn);
        self.recompute_percentiles();
    }

    /// Recompute P50/P95/P99 from stored turn durations.
    fn recompute_percentiles(&mut self) {
        if self.turns.is_empty() {
            self.p50_turn_ms = 0;
            self.p95_turn_ms = 0;
            self.p99_turn_ms = 0;
            return;
        }

        let mut durations: Vec<u64> = self.turns.iter().map(|t| t.duration_ms).collect();
        durations.sort_unstable();

        self.p50_turn_ms = percentile_from_sorted(&durations, 50.0);
        self.p95_turn_ms = percentile_from_sorted(&durations, 95.0);
        self.p99_turn_ms = percentile_from_sorted(&durations, 99.0);
    }

    /// Finalize the session — recompute percentiles and set outcome.
    pub fn finalize(&mut self, outcome: SessionOutcome) {
        self.outcome = outcome;
        self.recompute_percentiles();
    }
}

// ── Summary for CLI listing ──

/// Lightweight summary returned by `list_all()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub node_id: String,
    pub created_at: String,
    pub model: String,
    pub total_turns: u32,
    pub total_tokens: u64,
    pub estimated_cost: f64,
    pub outcome: SessionOutcome,
    /// Product name from custom field, if set (e.g. "phi-bard").
    pub product: Option<String>,
}

// ── Helpers ──

/// Compute the `p`-th percentile from an already-sorted slice of values.
pub(crate) fn percentile_from_sorted(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len() as f64;
    let idx = ((p / 100.0) * (n - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Truncate a string to `max_chars` characters, appending "..." if truncated.
pub(crate) fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

// ── Conversion from agent-base ──

/// Convert an agent-base `RunOutcome` to a turn-level `TurnOutcome`.
/// Checks `tools_used` to distinguish `ToolCalls` from `Completed`.
pub fn run_outcome_to_turn_outcome(
    outcome: &agent_base::RunOutcome,
    tools_used: &[String],
) -> TurnOutcome {
    match outcome {
        agent_base::RunOutcome::Completed => {
            if tools_used.is_empty() {
                TurnOutcome::Completed
            } else {
                TurnOutcome::ToolCalls
            }
        }
        agent_base::RunOutcome::Failed { .. } => TurnOutcome::Error,
        agent_base::RunOutcome::Cancelled => TurnOutcome::Cancelled,
        agent_base::RunOutcome::MaxTurnsExceeded { .. } => TurnOutcome::MaxTurns,
    }
}

/// Convert agent-base `RunOutcome` to `SessionOutcome`.
pub fn run_outcome_to_session_outcome(outcome: &agent_base::RunOutcome) -> SessionOutcome {
    match outcome {
        agent_base::RunOutcome::Completed => SessionOutcome::Completed,
        agent_base::RunOutcome::Failed { .. } => SessionOutcome::Failed,
        agent_base::RunOutcome::Cancelled => SessionOutcome::Cancelled,
        agent_base::RunOutcome::MaxTurnsExceeded { .. } => SessionOutcome::MaxTurns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(
            truncate_str("hello world this is long", 10),
            "hello worl..."
        );
        assert_eq!(truncate_str("你好世界测试文本", 4), "你好世界...");
    }

    #[test]
    fn test_percentile_from_sorted() {
        let data = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile_from_sorted(&data, 50.0), 60);
        assert_eq!(percentile_from_sorted(&data, 95.0), 100);
        assert_eq!(percentile_from_sorted(&data, 0.0), 10);
        assert_eq!(percentile_from_sorted(&data, 100.0), 100);
    }

    #[test]
    fn test_percentile_empty() {
        assert_eq!(percentile_from_sorted(&[], 50.0), 0);
    }

    #[test]
    fn test_session_metrics_new() {
        let m = SessionMetrics::new(
            "20260729_test".to_string(),
            "".to_string(),
            "claude-sonnet".to_string(),
        );
        assert_eq!(m.session_id, "20260729_test");
        assert_eq!(m.node_id, "");
        assert_eq!(m.model, "claude-sonnet");
        assert_eq!(m.total_turns, 0);
        assert_eq!(m.parent_session_id, None);
        assert_eq!(m.session_type, "default");
        assert!(m.turns.is_empty());
    }

    #[test]
    fn test_session_metrics_append_turn() {
        let mut m = SessionMetrics::new(
            "test".to_string(),
            "".to_string(),
            "claude-sonnet".to_string(),
        );
        let turn = TurnMetrics {
            turn_number: 1,
            started_at: "2026-07-29T00:00:00Z".to_string(),
            duration_ms: 1000,
            time_to_first_token_ms: 200,
            llm_duration_ms: 800,
            tool_duration_ms: 100,
            llm_calls: 1,
            input_tokens: 500,
            output_tokens: 300,
            model: "claude-sonnet".to_string(),
            tool_call_count: 1,
            tools_used: vec!["shell".to_string()],
            tool_success: 1,
            tool_failed: 0,
            outcome: TurnOutcome::ToolCalls,
            text_length: 200,
            error_message: None,
            has_thinking: true,
            plan_updates: 0,
            approval_count: 0,
            user_input: "test input".to_string(),
            custom: Value::Object(serde_json::Map::new()),
        };
        m.append_turn(turn);
        assert_eq!(m.total_turns, 1);
        assert_eq!(m.total_input_tokens, 500);
        assert_eq!(m.total_output_tokens, 300);
        assert_eq!(m.total_duration_ms, 1000);
        assert_eq!(m.total_tool_calls, 1);
        assert_eq!(m.tool_breakdown.get("shell"), Some(&1));
        assert_eq!(m.tool_fail_rate, 0.0);
        assert_eq!(m.avg_turn_ms, 1000);
        assert_eq!(m.p50_turn_ms, 1000);
    }

    #[test]
    fn test_session_metrics_append_multiple_turns() {
        let mut m = SessionMetrics::new(
            "test".to_string(),
            "".to_string(),
            "claude-sonnet".to_string(),
        );
        for i in 1..=5 {
            let turn = TurnMetrics {
                turn_number: i,
                started_at: "2026-07-29T00:00:00Z".to_string(),
                duration_ms: i as u64 * 1000,
                time_to_first_token_ms: 200,
                llm_duration_ms: 800,
                tool_duration_ms: 100,
                llm_calls: 1,
                input_tokens: 500,
                output_tokens: 300,
                model: "claude-sonnet".to_string(),
                tool_call_count: 1,
                tools_used: vec!["shell".to_string()],
                tool_success: 1,
                tool_failed: 0,
                outcome: TurnOutcome::ToolCalls,
                text_length: 200,
                error_message: None,
                has_thinking: true,
                plan_updates: 0,
                approval_count: 0,
                user_input: "test".to_string(),
                custom: Value::Object(serde_json::Map::new()),
            };
            m.append_turn(turn);
        }
        assert_eq!(m.total_turns, 5);
        assert_eq!(m.total_duration_ms, 15000);
        assert_eq!(m.avg_turn_ms, 3000);
        assert_eq!(m.p50_turn_ms, 3000);
    }

    #[test]
    fn test_session_metrics_finalize() {
        let mut m = SessionMetrics::new(
            "test".to_string(),
            "".to_string(),
            "claude-sonnet".to_string(),
        );
        m.finalize(SessionOutcome::Completed);
        assert_eq!(m.outcome, SessionOutcome::Completed);
    }

    #[test]
    fn test_session_metrics_json_roundtrip() {
        let m = SessionMetrics::new(
            "20260729_test".to_string(),
            "node-1".to_string(),
            "claude-sonnet".to_string(),
        );
        let json = serde_json::to_string(&m).unwrap();
        let m2: SessionMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.session_id, "20260729_test");
        assert_eq!(m2.node_id, "node-1");
        assert_eq!(m2.parent_session_id, None);
        assert_eq!(m2.session_type, "default");
    }

    #[test]
    fn test_session_metrics_backward_compat() {
        let old_json = r#"{
            "session_id": "test",
            "node_id": "",
            "created_at": "2026-07-29T00:00:00Z",
            "model": "claude-sonnet",
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "estimated_cost": 0.0,
            "total_tool_calls": 0,
            "tool_breakdown": {},
            "tool_fail_rate": 0.0,
            "total_duration_ms": 0,
            "total_llm_ms": 0,
            "total_tool_ms": 0,
            "total_turns": 0,
            "avg_turn_ms": 0,
            "p50_turn_ms": 0,
            "p95_turn_ms": 0,
            "p99_turn_ms": 0,
            "outcome": "completed",
            "error_count": 0,
            "custom": {},
            "turns": []
        }"#;
        let m: SessionMetrics = serde_json::from_str(old_json).unwrap();
        assert_eq!(m.parent_session_id, None);
        assert_eq!(m.session_type, "default");
    }

    #[test]
    fn test_turn_metrics_custom_default() {
        let turn = TurnMetrics::new(
            1,
            "2026-07-29T00:00:00Z".to_string(),
            1000,
            "claude-sonnet".to_string(),
            "hello world".to_string(),
            TurnOutcome::Completed,
        );
        assert_eq!(turn.custom, Value::Object(serde_json::Map::new()));
        assert_eq!(turn.user_input, "hello world");
        let turn_long = TurnMetrics::new(
            1,
            "2026-07-29T00:00:00Z".to_string(),
            1000,
            "claude-sonnet".to_string(),
            "this is a very long user input that should definitely be truncated at eighty characters because that's the max we allow for storage".to_string(),
            TurnOutcome::Completed,
        );
        assert!(turn_long.user_input.chars().count() <= 83);
        assert!(turn_long.user_input.ends_with("..."));
    }

    #[test]
    fn test_run_outcome_conversion() {
        use agent_base::RunOutcome;

        // Completed with no tools → TurnOutcome::Completed
        assert_eq!(
            run_outcome_to_turn_outcome(&RunOutcome::Completed, &[]),
            TurnOutcome::Completed
        );

        // Completed with tools → TurnOutcome::ToolCalls
        assert_eq!(
            run_outcome_to_turn_outcome(&RunOutcome::Completed, &["shell".to_string()]),
            TurnOutcome::ToolCalls
        );

        // Failed → Error
        assert_eq!(
            run_outcome_to_turn_outcome(
                &RunOutcome::Failed {
                    error: "oops".to_string()
                },
                &[]
            ),
            TurnOutcome::Error
        );

        // Cancelled → Cancelled
        assert_eq!(
            run_outcome_to_turn_outcome(&RunOutcome::Cancelled, &[]),
            TurnOutcome::Cancelled
        );
    }
}
