use std::io::{self, Write};

use agent_base::{AgentResult, RuntimeEvent, UserEvent};
use serde_json::{json, Value};

use crate::render::EventRenderer;

/// JSON stream renderer: outputs one JSON line per event (JSONL format).
/// Suitable for IDE integrations and programmatic consumers.
pub struct JsonStreamRenderer {
    writer: Box<dyn Write + Send>,
    turn_start: Option<std::time::Instant>,
    tool_call_count: u32,
    last_assistant_text: String,
}

impl JsonStreamRenderer {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer,
            turn_start: None,
            tool_call_count: 0,
            last_assistant_text: String::new(),
        }
    }

    pub fn stdout() -> Self {
        Self::new(Box::new(io::stdout()))
    }

    fn emit(&mut self, value: &Value) -> AgentResult<()> {
        let line = serde_json::to_string(value)
            .map_err(|e| agent_base::AgentError::internal(format!("JSON serialize error: {e}")))?;
        writeln!(self.writer, "{}", line)
            .map_err(|e| agent_base::AgentError::internal(format!("write error: {e}")))?;
        Ok(())
    }
}

impl EventRenderer for JsonStreamRenderer {
    fn render(&mut self, event: RuntimeEvent) -> AgentResult<()> {
        if self.turn_start.is_none() {
            self.turn_start = Some(std::time::Instant::now());
        }

        match &event {
            RuntimeEvent::ThoughtDelta { text, .. } => {
                self.emit(&json!({ "type": "thought_delta", "text": text }))?;
            }
            RuntimeEvent::TextDelta { text, .. } => {
                self.last_assistant_text.push_str(text);
                self.emit(&json!({ "type": "text_delta", "text": text }))?;
            }
            RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
                self.tool_call_count += 1;
                let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
                self.emit(&json!({
                    "type": "tool_call_started",
                    "tool": tool_name,
                    "args": args,
                }))?;
            }
            RuntimeEvent::ToolCallFinished { tool_name, summary, .. } => {
                self.emit(&json!({
                    "type": "tool_call_finished",
                    "tool": tool_name,
                    "summary": summary,
                }))?;
            }
            RuntimeEvent::AwaitingApproval { request, .. } => {
                self.emit(&json!({
                    "type": "approval_request",
                    "title": request.title,
                    "risk": format!("{:?}", request.risk_level),
                    "message": request.message,
                }))?;
            }
            RuntimeEvent::PlanUpdated { explanation, plan, .. } => {
                self.emit(&json!({
                    "type": "plan_updated",
                    "explanation": explanation,
                    "plan": plan,
                }))?;
            }
            RuntimeEvent::UserEvent {
                event: UserEvent::Structured { event_type, data },
                ..
            } => {
                self.emit(&json!({
                    "type": "user_event",
                    "event_type": event_type,
                    "data": data,
                }))?;
            }
            RuntimeEvent::UserEvent { .. } => {}
            RuntimeEvent::Checkpoint { .. } => {}
            RuntimeEvent::RunFinished { .. } => {}
            RuntimeEvent::RunCancelled { .. } => {
                self.emit(&json!({ "type": "run_cancelled" }))?;
            }
        }

        Ok(())
    }

    fn finish_turn(&mut self) -> AgentResult<()> {
        let duration_ms = self
            .turn_start
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);

        self.emit(&json!({
            "type": "turn_finished",
            "duration_ms": duration_ms,
            "tool_call_count": self.tool_call_count,
            "assistant_text": self.last_assistant_text.trim(),
        }))?;

        self.turn_start = None;
        self.tool_call_count = 0;
        self.last_assistant_text.clear();

        Ok(())
    }
}
