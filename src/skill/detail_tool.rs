use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput};
use crate::types::{AgentEvent, AgentResult};

use super::Skill;

pub(crate) struct SkillDetailTool {
    pub(crate) skills: Vec<Arc<dyn Skill>>,
    pub(crate) name: &'static str,
}

impl SkillDetailTool {
    pub(crate) fn new(skills: Vec<Arc<dyn Skill>>, tool_name: String) -> Self {
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
                "description": "获取指定 Skill 的详细操作指南。当你需要了解某个 Skill 的完整使用方式时调用。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Skill 名称"
                        }
                    },
                    "required": ["name"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");

        if name.is_empty() {
            return Ok(ToolOutput {
                summary: format!(
                    "请提供 Skill 名称。可用 Skills: {}",
                    self.skills
                        .iter()
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncated: false,
            });
        }

        let detail = self
            .skills
            .iter()
            .find(|s| s.name() == name)
            .map(|s| s.detailed_description());

        let _ = ctx.event_bus.send(AgentEvent::Custom {
            session_id: ctx.session_id.clone(),
            payload: json!({
                "type": "skill_detail_loaded",
                "skill": name,
            }),
        });

        match detail {
            Some(desc) => Ok(ToolOutput {
                summary: desc.to_string(),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncated: false,
            }),
            None => {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name()).collect();
                Ok(ToolOutput {
                    summary: format!(
                        "未找到 Skill '{}'。可用 Skills: {}",
                        name,
                        available.join(", ")
                    ),
                    raw: None,
                    control_flow: ToolControlFlow::Break,
                    truncated: false,
                })
            }
        }
    }
}
