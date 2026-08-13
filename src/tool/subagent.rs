use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{Content, Tool, ToolContext};
use crate::engine::AgentRuntime;
use crate::types::{AgentError, AgentResult, RuntimeEvent, SessionId, UserEvent};

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
    pub fn new(name: &'static str, description: &'static str, sub_runtime: AgentRuntime) -> Self {
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

    fn description(&self) -> &'static str {
        self.description
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description to delegate to the sub-agent"
                }
            },
            "required": ["task"]
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>> {
        let task = args.get("task").and_then(Value::as_str).ok_or_else(|| {
            AgentError::ToolArgsInvalid {
                name: self.name.to_string(),
                raw: args.to_string(),
            }
        })?;

        if task.is_empty() {
            return Ok(vec![Content::text(
                "Task description is empty, cannot execute",
            )]);
        }

        let user_event_tx = ctx.user_event_tx.clone();

        let sub_session_id = match self.session_policy {
            SubAgentSessionPolicy::Ephemeral => {
                let runtime = self.sub_runtime.lock().await;
                let new_id = runtime.create_session().await;
                let mut sid_guard = self.sub_session_id.lock().await;
                *sid_guard = Some(new_id.clone());
                tracing::debug!(
                    subagent = self.name,
                    sub_session = new_id.id,
                    "sub-agent ephemeral session created"
                );
                new_id
            }
            SubAgentSessionPolicy::Persistent => {
                let mut sid_guard = self.sub_session_id.lock().await;
                if let Some(id) = sid_guard.clone() {
                    tracing::debug!(
                        subagent = self.name,
                        sub_session = id.id,
                        "sub-agent reusing persistent session"
                    );
                    id
                } else {
                    let runtime = self.sub_runtime.lock().await;
                    let new_id = runtime.create_session().await;
                    *sid_guard = Some(new_id.clone());
                    tracing::debug!(
                        subagent = self.name,
                        sub_session = new_id.id,
                        "sub-agent persistent session created"
                    );
                    new_id
                }
            }
        };

        // Add the user task as a message to the sub-agent session
        {
            let runtime = self.sub_runtime.lock().await;
            runtime
                .add_user_message(&sub_session_id, task)
                .await
                .map_err(|e| AgentError::ToolExecution {
                    name: self.name.to_string(),
                    source: Box::new(e),
                })?;
        }

        let mut runtime_events = Vec::new();
        let _outcome = {
            let runtime = self.sub_runtime.lock().await;
            runtime
                .run(sub_session_id, |event| {
                    runtime_events.push(event.clone());
                    Ok(())
                })
                .await
                .map_err(|e| AgentError::ToolExecution {
                    name: self.name.to_string(),
                    source: Box::new(e),
                })?
        };

        let mut final_text = String::new();
        for event in &runtime_events {
            if let RuntimeEvent::TextDelta { text, .. } = event {
                final_text.push_str(text);
            }
            // Forward each sub-agent event to the parent via UserEvent::SubAgentEvent
            let _ = user_event_tx.send(UserEvent::SubAgentEvent {
                subagent: self.name.to_string(),
                event: Box::new(event.clone()),
            });
        }

        let summary = if final_text.is_empty() {
            format!("Sub-agent [{}] finished", self.name)
        } else {
            final_text
        };

        tracing::debug!(
            subagent = self.name,
            text_len = summary.len(),
            event_count = runtime_events.len(),
            "sub-agent completed"
        );

        Ok(vec![Content::text(summary)])
    }
}
