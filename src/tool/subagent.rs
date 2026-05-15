use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::engine::AgentRuntime;
use crate::types::{AgentError, AgentEvent, AgentResult, SessionId};
use super::{Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct SubAgentTool {
    name: &'static str,
    description: &'static str,
    sub_runtime: Mutex<AgentRuntime>,
    sub_session_id: SessionId,
}

impl SubAgentTool {
    pub fn new(
        name: &'static str,
        description: &'static str,
        mut sub_runtime: AgentRuntime,
    ) -> Self {
        let sub_session_id = sub_runtime.create_session();
        Self {
            name,
            description,
            sub_runtime: Mutex::new(sub_runtime),
            sub_session_id,
        }
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "需要委托给子 Agent 执行的任务描述"
                        }
                    },
                    "required": ["task"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolArgsInvalid {
                name: self.name.to_string(),
                raw: args.to_string(),
            })?;

        if task.is_empty() {
            return Ok(ToolOutput {
                summary: "任务描述为空，无法执行".to_string(),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncated: false,
            });
        }

        let parent_event_bus = ctx.event_bus.clone();
        let parent_session_id = ctx.session_id.clone();

        let events = {
            let mut runtime = self.sub_runtime.lock().await;
            runtime
                .run_turn_stream(self.sub_session_id.clone(), task)
                .await
                .map_err(|e| AgentError::ToolExecution {
                    name: self.name.to_string(),
                    source: Box::new(e),
                })?
        };

        let mut final_text = String::new();
        for event in &events {
            match event {
                AgentEvent::TextDelta { text, .. } => {
                    final_text.push_str(text);
                }
                _ => {}
            }
            let _ = parent_event_bus.send(AgentEvent::Custom {
                session_id: parent_session_id.clone(),
                payload: json!({
                    "type": "subagent_event",
                    "subagent": self.name,
                    "event": event_to_value(event),
                }),
            });
        }

        let summary = if final_text.is_empty() {
            format!("子 Agent [{}] 已执行完成", self.name)
        } else {
            final_text
        };

        Ok(ToolOutput {
            summary,
            raw: None,
            control_flow: ToolControlFlow::Continue,
            truncated: false,
        })
    }
}

fn event_to_value(event: &AgentEvent) -> Value {
    match event {
        AgentEvent::TextDelta { text, .. } => json!({"type": "TextDelta", "text": text}),
        AgentEvent::ThoughtDelta { text, .. } => json!({"type": "ThoughtDelta", "text": text}),
        AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
            json!({"type": "ToolCallStarted", "tool_name": tool_name, "args_json": args_json})
        }
        AgentEvent::ToolCallFinished { tool_name, summary, .. } => {
            json!({"type": "ToolCallFinished", "tool_name": tool_name, "summary": summary})
        }
        AgentEvent::AwaitingApproval { request, .. } => {
            json!({"type": "AwaitingApproval", "title": request.title})
        }
        AgentEvent::Checkpoint { .. } => json!({"type": "Checkpoint"}),
        AgentEvent::RunCompleted { .. } => json!({"type": "RunCompleted"}),
        AgentEvent::RunFailed { error, .. } => json!({"type": "RunFailed", "error": error}),
        AgentEvent::Custom { payload, .. } => json!({"type": "Custom", "payload": payload}),
    }
}
