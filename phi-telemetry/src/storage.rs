//! Metrics persistence — save, load, and list session metrics from disk.

use std::path::Path;

use anyhow::{Context, Result};

use crate::types::{SessionMetrics, SessionSummary};

/// Metrics file name within a session directory.
const METRICS_FILE: &str = "session_metrics.json";

/// Incrementally write session metrics to `session_metrics.json`.
/// Overwrites the file on each call — safe for per-turn writes.
pub fn save_metrics(metrics: &SessionMetrics, session_dir: &Path) -> Result<()> {
    let path = session_dir.join(METRICS_FILE);
    let json = serde_json::to_string_pretty(metrics)?;
    std::fs::write(&path, json)?;
    tracing::debug!(path = %path.display(), turns = metrics.total_turns, "session_metrics saved");
    Ok(())
}

/// Load session metrics from `session_metrics.json` in a session directory.
pub fn load_metrics(session_dir: &Path) -> Result<SessionMetrics> {
    let path = session_dir.join(METRICS_FILE);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read metrics file: {}", path.display()))?;
    let metrics: SessionMetrics = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse metrics file: {}", path.display()))?;
    Ok(metrics)
}

/// Try to load session metrics, returning `None` if the file doesn't exist.
pub fn try_load_metrics(session_dir: &Path) -> Option<SessionMetrics> {
    load_metrics(session_dir).ok()
}

/// List all session summaries by scanning the sessions directory.
/// Reads each `session_metrics.json` and extracts summary fields.
pub fn list_all_metrics(base_dir: &Path) -> Result<Vec<SessionSummary>> {
    let sessions_dir = base_dir.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if let Some(metrics) = try_load_metrics(&path) {
            let product = metrics
                .custom
                .get("product")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            summaries.push(SessionSummary {
                session_id: metrics.session_id,
                node_id: metrics.node_id,
                created_at: metrics.created_at,
                model: metrics.model,
                total_turns: metrics.total_turns,
                total_tokens: metrics.total_input_tokens + metrics.total_output_tokens,
                estimated_cost: metrics.estimated_cost,
                outcome: metrics.outcome,
                product,
            });
        }
    }

    // Sort by created_at descending (newest first)
    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(summaries)
}
