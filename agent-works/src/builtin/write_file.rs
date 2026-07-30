use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::path_util::validate_path;
use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct WriteFileTool {
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write content to a file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'path' argument"))?;

        let content = args["content"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'content' argument"))?;

        let full_path = validate_path(&self.workspace, path)?;

        tracing::debug!(path = %path, "write file start");

        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                tracing::error!(path = %path, error = %e, "write file failed");
                AgentError::internal(format!(
                    "failed to create directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        tokio::fs::write(&full_path, content).await.map_err(|e| {
            tracing::error!(path = %path, error = %e, "write file failed");
            AgentError::internal(format!("failed to write {}: {e}", full_path.display()))
        })?;

        tracing::info!(path = %path, "write file success");
        Ok(ToolOutput {
            summary: format!("Successfully wrote to {}", path),
            raw: Some(json!({"path": path, "written": true})),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}
