use std::sync::Arc;

use agent_base::{AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput, UserEvent};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Skill;

pub struct SkillDetailTool {
    pub skills: Vec<Arc<dyn Skill>>,
    pub name: &'static str,
}

impl SkillDetailTool {
    pub fn new(skills: Vec<Arc<dyn Skill>>, tool_name: String) -> Self {
        let name: &'static str = Box::leak(tool_name.into_boxed_str());
        Self { skills, name }
    }
}

#[async_trait]
impl Tool for SkillDetailTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": "Get detailed instructions for a Skill. Call this when you need the complete usage guide for a Skill.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Skill name"
                        }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("");

        if name.is_empty() {
            return Ok(ToolOutput {
                summary: format!(
                    "Please provide a Skill name. Available Skills: {}",
                    self.skills
                        .iter()
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncation: None,
            });
        }

        let detail = self
            .skills
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.detailed_description());

        tracing::debug!("skill detail queried");
        ctx.emit_user_event(UserEvent::Structured {
            event_type: "skill_detail_loaded".to_string(),
            data: json!({
                "skill": name,
            }),
        });

        match detail {
            Some(desc) => Ok(ToolOutput {
                summary: desc.to_string(),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncation: None,
            }),
            None => {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name()).collect();
                Ok(ToolOutput {
                    summary: format!(
                        "Skill '{}' not found. Available Skills: {}",
                        name,
                        available.join(", ")
                    ),
                    raw: None,
                    control_flow: ToolControlFlow::Break,
                    truncation: None,
                })
            }
        }
    }
}
