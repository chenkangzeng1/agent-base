//! Event log: serialize turn events to JSONL files.
//!
//! Provides event persistence integrated with SessionContext,
//! shared across CLI, Web, and other consumers.
//!
//! Note: `save_turn_log` performs synchronous file I/O. Consumers should
//! call it via `tokio::task::spawn_blocking` in async contexts to avoid
//! blocking the runtime.

use std::io::Write;

use agent_base::{RuntimeEvent, UserEvent};
use anyhow::Result;

use crate::session::SessionContext;

/// Save all events from a turn to a JSONL file.
///
/// Performs synchronous file I/O. Callers should invoke this via
/// `tokio::task::spawn_blocking`.
pub fn save_turn_log(session_ctx: &SessionContext, turn: u32, events: &[RuntimeEvent], user_input: &str) -> Result<()> {
    let turn_path = session_ctx.turn_path(turn as usize);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&turn_path)?;

    let meta = serde_json::json!({
        "turn": turn,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "user_input": user_input,
    });
    writeln!(file, "{}", serde_json::to_string(&meta)?)?;

    for event in events {
        let line = event_to_jsonl(event);
        writeln!(file, "{}", line)?;
    }

    writeln!(file, "{}", serde_json::to_string(&serde_json::json!({"type": "turn_end", "turn": turn}))?)?;

    file.flush()?;

    Ok(())
}

/// Convert a RuntimeEvent to a JSON Value (shared by event_log and render).
pub fn event_to_value(event: &RuntimeEvent) -> serde_json::Value {
    match event {
        RuntimeEvent::ThoughtDelta { text, .. } => {
            serde_json::json!({"type": "thought_delta", "text": text})
        },
        RuntimeEvent::TextDelta { text, .. } => {
            serde_json::json!({"type": "text_delta", "text": text})
        },
        RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
            let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
            serde_json::json!({"type": "tool_call_started", "tool": tool_name, "args": args})
        },
        RuntimeEvent::ToolCallFinished { tool_name, summary, .. } => {
            serde_json::json!({"type": "tool_call_finished", "tool": tool_name, "summary": summary})
        },
        RuntimeEvent::AwaitingApproval { request, .. } => {
            serde_json::json!({"type": "approval_request", "title": request.title, "message": request.message})
        },
        RuntimeEvent::PlanUpdated { explanation, plan, .. } => {
            serde_json::json!({"type": "plan_updated", "explanation": explanation, "plan": plan})
        },
        RuntimeEvent::UserEvent { event: UserEvent::Structured { event_type, data }, .. } => {
            serde_json::json!({"type": "user_event", "event_type": event_type, "data": data})
        },
        RuntimeEvent::UserEvent { .. } => serde_json::json!({"type": "other"}),
        RuntimeEvent::RunCancelled { .. } => serde_json::json!({"type": "run_cancelled"}),
        RuntimeEvent::RunFinished { .. } => serde_json::json!({"type": "run_finished"}),
        RuntimeEvent::Checkpoint { .. } => serde_json::json!({"type": "checkpoint"}),
    }
}

/// Convert a RuntimeEvent to a JSONL line.
pub fn event_to_jsonl(event: &RuntimeEvent) -> String {
    let value = event_to_value(event);
    serde_json::to_string(&value).unwrap_or_else(|_| r#"{"type":"serialize_error"}"#.to_string())
}
