use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::path_util::validate_path;
use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct FileExistsTool {
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for FileExistsTool {
    fn name(&self) -> &'static str {
        "file_exists"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "file_exists",
                "description": "Check if a file exists",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file or directory to check"
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

        let metadata = tokio::fs::metadata(&full_path).await;

        let exists = metadata.is_ok();
        let is_file = metadata.as_ref().is_ok_and(|m| m.is_file());
        let is_dir = metadata.as_ref().is_ok_and(|m| m.is_dir());

        tracing::trace!(path = %path, exists = exists, "file exists check");
        Ok(ToolOutput {
            summary: if exists {
                format!("{} exists", path)
            } else {
                format!("{} does not exist", path)
            },
            raw: Some(
                json!({"path": path, "exists": exists, "is_file": is_file, "is_dir": is_dir}),
            ),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}
