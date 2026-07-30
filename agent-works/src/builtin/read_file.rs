use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::path_util::validate_path;
use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct ReadFileTool {
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the contents of a file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| AgentError::internal("missing 'path' argument"))?;

        let full_path = validate_path(&self.workspace, path)?;

        tracing::debug!(path = %path, "read file start");
        let content = tokio::fs::read_to_string(&full_path).await.map_err(|e| {
            tracing::error!(path = %path, error = %e, "read file failed");
            AgentError::internal(format!("failed to read {}: {e}", full_path.display()))
        })?;

        tracing::info!(path = %path, size = content.len(), "read file success");
        Ok(ToolOutput {
            summary: content.clone(),
            raw: Some(json!({"path": path, "content": content})),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}
