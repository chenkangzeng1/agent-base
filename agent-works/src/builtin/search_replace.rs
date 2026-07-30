use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::path_util::validate_path;
use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct SearchReplaceTool {
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for SearchReplaceTool {
    fn name(&self) -> &'static str {
        "search_replace"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "search_replace",
                "description": "Search and replace text in a file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file"
                        },
                        "old_str": {
                            "type": "string",
                            "description": "Text to search for (first occurrence will be replaced)"
                        },
                        "new_str": {
                            "type": "string",
                            "description": "Text to replace with"
                        }
                    },
                    "required": ["path", "old_str", "new_str"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'path' argument"))?;

        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'old_str' argument"))?;

        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'new_str' argument"))?;

        // Short-circuit when old and new are identical — no I/O needed
        if old_str == new_str {
            return Ok(ToolOutput {
                summary: format!("No changes needed in {} (old_str == new_str)", path),
                raw: Some(
                    json!({"path": path, "found": true, "replaced": false, "reason": "identical strings"}),
                ),
                control_flow: ToolControlFlow::Break,
                truncation: None,
            });
        }

        let full_path = validate_path(&self.workspace, path)?;

        tracing::debug!(file = %path, "search replace start");
        let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
            tracing::error!(file = %path, error = %e, "search replace failed");
            AgentError::internal(format!("failed to read {}: {e}", full_path.display()))
        })?;

        let replaced = content.replacen(old_str, new_str, 1);

        if replaced == content {
            return Ok(ToolOutput {
                summary: format!("Text not found in {}", path),
                raw: Some(json!({"path": path, "found": false})),
                control_flow: ToolControlFlow::Break,
                truncation: None,
            });
        }

        tokio::fs::write(&full_path, &replaced).await.map_err(|e| {
            AgentError::internal(format!("failed to write {}: {e}", full_path.display()))
        })?;

        Ok(ToolOutput {
            summary: format!("Successfully replaced text in {}", path),
            raw: Some(json!({"path": path, "found": true, "replaced": true})),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}
