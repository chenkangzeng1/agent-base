use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::types::{AgentError, AgentResult};
use super::{Tool, ToolContext, ToolControlFlow, ToolOutput};

pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpClient {
    server_url: String,
    client: Client,
}

impl McpClient {
    pub fn new(server_url: String) -> Self {
        Self {
            server_url,
            client: Client::new(),
        }
    }

    async fn send_request(&self, method: &str, params: Value) -> AgentResult<Value> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let response = self
            .client
            .post(&self.server_url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AgentError::internal(format!("MCP request failed: {e}")))?;

        let res: Value = response.json().await.map_err(|e| {
            AgentError::json(format!("MCP response parse: {e}"))
        })?;

        if let Some(error) = res.get("error") {
            return Err(AgentError::internal(format!("MCP error: {error}")));
        }

        Ok(res.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn list_tools(&self) -> AgentResult<Vec<McpToolInfo>> {
        let result = self.send_request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| AgentError::internal("MCP: invalid tools/list response"))?;

        let mut infos = Vec::new();
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"}));
            infos.push(McpToolInfo {
                name,
                description,
                input_schema,
            });
        }
        Ok(infos)
    }

    pub async fn call_tool(&self, tool_name: &str, arguments: &Value) -> AgentResult<Value> {
        self.send_request(
            "tools/call",
            json!({
                "name": tool_name,
                "arguments": arguments,
            }),
        )
        .await
    }
}

struct McpToolAdapter {
    name: &'static str,
    description: String,
    input_schema: Value,
    mcp_client: Arc<McpClient>,
}

impl McpToolAdapter {
    fn new(info: McpToolInfo, mcp_client: Arc<McpClient>) -> Self {
        let static_name: &'static str = Box::leak(info.name.into_boxed_str());
        Self {
            name: static_name,
            description: info.description,
            input_schema: info.input_schema,
            mcp_client,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let result = self.mcp_client.call_tool(self.name, args).await?;
        let content = result
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| result.to_string());

        Ok(ToolOutput {
            summary: content,
            raw: Some(result),
            control_flow: ToolControlFlow::Break,
            truncation: None,
        })
    }
}

pub struct McpToolRegistry {
    mcp_client: Arc<McpClient>,
}

impl McpToolRegistry {
    pub fn new(server_url: String) -> Self {
        Self {
            mcp_client: Arc::new(McpClient::new(server_url)),
        }
    }

    pub async fn discover_tools(&self) -> AgentResult<Vec<Arc<dyn Tool>>> {
        let infos = self.mcp_client.list_tools().await?;
        let tools: Vec<Arc<dyn Tool>> = infos
            .into_iter()
            .map(|info| {
                Arc::new(McpToolAdapter::new(info, self.mcp_client.clone())) as Arc<dyn Tool>
            })
            .collect();
        Ok(tools)
    }
}
