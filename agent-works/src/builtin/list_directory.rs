use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::path_util::validate_path;
use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct ListDirectoryTool {
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "List the contents of a directory",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the directory to list"
                        }
                    },
                    "required": []
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or(".");
        let full_path = validate_path(&self.workspace, path)?;

        tracing::debug!(path = %path, "list directory start");
        let mut entries = tokio::fs::read_dir(&full_path).await.map_err(|e| {
            tracing::error!(path = %path, error = %e, "list directory failed");
            AgentError::internal(format!("failed to read dir {}: {e}", full_path.display()))
        })?;

        let mut items: Vec<Value> = Vec::new();
        loop {
            let entry = entries.next_entry().await.map_err(|e| {
                tracing::error!(path = %path, error = %e, "list directory failed");
                AgentError::internal(format!("failed to read entry: {e}"))
            })?;
            let Some(entry) = entry else {
                break;
            };
            let file_type = entry.file_type().await.ok();
            let is_dir = file_type.as_ref().is_some_and(|ft| ft.is_dir());
            let is_file = file_type.as_ref().is_some_and(|ft| ft.is_file());
            items.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": is_dir,
                "is_file": is_file,
            }));
        }

        let names: Vec<String> = items
            .iter()
            .map(|v| v["name"].as_str().unwrap_or("").to_string())
            .collect();
        let summary = if names.is_empty() {
            format!("Directory {} is empty", path)
        } else {
            names.join("\n")
        };

        Ok(ToolOutput {
            summary,
            raw: Some(json!({"path": path, "entries": items})),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}
