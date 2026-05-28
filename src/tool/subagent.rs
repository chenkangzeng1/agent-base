use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::engine::AgentRuntime;
use crate::types::{AgentError, AgentEvent, AgentResult, SessionId};
use super::{Tool, ToolContext, ToolControlFlow, ToolOutput};

/// Sub-Agent session policy
#[derive(Clone, Debug)]
pub enum SubAgentSessionPolicy {
    /// Create a new session per call (default)
    Ephemeral,
    /// Reuse the same session; sub-agent accumulates history
    Persistent,
}

pub struct SubAgentTool {
    name: &'static str,
    description: &'static str,
    sub_runtime: Mutex<AgentRuntime>,
    sub_session_id: Mutex<Option<SessionId>>,
    session_policy: SubAgentSessionPolicy,
}

impl SubAgentTool {
    pub fn new(
        name: &'static str,
        description: &'static str,
        sub_runtime: AgentRuntime,
    ) -> Self {
        Self {
            name,
            description,
            sub_runtime: Mutex::new(sub_runtime),
            sub_session_id: Mutex::new(None),
            session_policy: SubAgentSessionPolicy::Ephemeral,
        }
    }

    pub fn with_persistent(
        name: &'static str,
        description: &'static str,
        sub_runtime: AgentRuntime,
    ) -> Self {
        Self {
            name,
            description,
            sub_runtime: Mutex::new(sub_runtime),
            sub_session_id: Mutex::new(None),
            session_policy: SubAgentSessionPolicy::Persistent,
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
                            "description": "Task description to delegate to the sub-agent"
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
                summary: "Task description is empty, cannot execute".to_string(),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncation: None,
            });
        }

        let parent_event_bus = ctx.event_bus.clone();
        let parent_session_id = ctx.session_id.clone();

        let sub_session_id = match self.session_policy {
            SubAgentSessionPolicy::Ephemeral => {
                let runtime = self.sub_runtime.lock().await;
                let new_id = runtime.create_session().await;
                let mut sid_guard = self.sub_session_id.lock().await;
                *sid_guard = Some(new_id.clone());
                tracing::debug!(subagent = self.name, sub_session = new_id.id, "sub-agent ephemeral session created");
                new_id
            }
            SubAgentSessionPolicy::Persistent => {
                let mut sid_guard = self.sub_session_id.lock().await;
                if let Some(id) = sid_guard.clone() {
                    tracing::debug!(subagent = self.name, sub_session = id.id, "sub-agent reusing persistent session");
                    id
                } else {
                    let runtime = self.sub_runtime.lock().await;
                    let new_id = runtime.create_session().await;
                    *sid_guard = Some(new_id.clone());
                    tracing::debug!(subagent = self.name, sub_session = new_id.id, "sub-agent persistent session created");
                    new_id
                }
            }
        };

        // Add the user task as a message to the sub-agent session
        {
            let runtime = self.sub_runtime.lock().await;
            runtime.add_user_message(&sub_session_id, task).await.map_err(|e| AgentError::ToolExecution {
                name: self.name.to_string(),
                source: Box::new(e),
            })?;
        }

        let mut events = Vec::new();
        let _outcome = {
            let runtime = self.sub_runtime.lock().await;
            runtime
                .run(sub_session_id, |event| {
                    events.push(event.clone());
                    Ok(())
                })
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
                    final_text.push_str(&text);
                }
                _ => {}
            }
            let _ = parent_event_bus.send(AgentEvent::Custom {
                session_id: parent_session_id.clone(),
                payload: json!({
                    "type": "subagent_event",
                    "subagent": self.name,
                    "event": event_to_value(&event),
                }),
            });
        }

        let summary = if final_text.is_empty() {
            format!("Sub-agent [{}] finished", self.name)
        } else {
            final_text
        };

        tracing::debug!(subagent = self.name, text_len = summary.len(), event_count = events.len(), "sub-agent completed");

        Ok(ToolOutput {
            summary,
            raw: None,
            control_flow: ToolControlFlow::Continue,
            truncation: None,
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
        AgentEvent::RunFinished { .. } => json!({"type": "RunFinished"}),
        AgentEvent::Custom { payload, .. } => json!({"type": "Custom", "payload": payload}),
        AgentEvent::PlanGenerated { plan, .. } => json!({"type": "PlanGenerated", "plan_id": plan.id}),
        AgentEvent::PlanStepStarted { step_id, step_description, .. } => {
            json!({"type": "PlanStepStarted", "step_id": step_id, "step_description": step_description})
        }
        AgentEvent::PlanStepCompleted { step_id, success, result, .. } => {
            json!({"type": "PlanStepCompleted", "step_id": step_id, "success": success, "result": result})
        }
        AgentEvent::PlanCompleted { plan_id, success, .. } => {
            json!({"type": "PlanCompleted", "plan_id": plan_id, "success": success})
        }
        AgentEvent::PlanGenerating { plan_id, .. } => {
            json!({"type": "PlanGenerating", "plan_id": plan_id})
        }
        AgentEvent::PlanStepParsed { plan_id, step_index, step_id, step_description, .. } => {
            json!({"type": "PlanStepParsed", "plan_id": plan_id, "step_index": step_index, "step_id": step_id, "step_description": step_description})
        }
        AgentEvent::PlanFailed { plan_id, error, .. } => {
            json!({"type": "PlanFailed", "plan_id": plan_id, "error": error})
        }
        AgentEvent::PlanStepWaitingConfirmation { step_id, step_description, payload, .. } => {
            json!({"type": "PlanStepWaitingConfirmation", "step_id": step_id, "step_description": step_description, "payload": payload})
        }
    }
}
