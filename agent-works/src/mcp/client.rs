use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agent_base::{AgentError, AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::types::{McpToolInfo, McpTransport};

struct StdioProcess {
    #[allow(dead_code)] // held for RAII: child process is killed on drop
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

enum TransportInner {
    Http {
        url: String,
        client: reqwest::Client,
    },
    Stdio {
        process: Mutex<StdioProcess>,
    },
}

pub struct McpClient {
    transport: TransportInner,
    request_id: AtomicU64,
}

impl McpClient {
    pub async fn new(transport: McpTransport) -> AgentResult<Self> {
        match transport {
            McpTransport::Http { url } => Ok(Self {
                transport: TransportInner::Http {
                    url,
                    client: reqwest::Client::new(),
                },
                request_id: AtomicU64::new(1),
            }),
            McpTransport::Stdio { command, args } => {
                let mut child = Command::new(&command)
                    .args(&args)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                    .map_err(|e| AgentError::internal(format!("spawn MCP process: {e}")))?;

                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| AgentError::internal("no stdin"))?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| AgentError::internal("no stdout"))?;

                Ok(Self {
                    transport: TransportInner::Stdio {
                        process: Mutex::new(StdioProcess {
                            child,
                            stdin,
                            stdout: BufReader::new(stdout),
                        }),
                    },
                    request_id: AtomicU64::new(1),
                })
            }
        }
    }

    async fn send_request(&self, method: &str, params: Value) -> AgentResult<Value> {
        match &self.transport {
            TransportInner::Http { url, client } => {
                self.send_request_http(url, client, method, params).await
            }
            TransportInner::Stdio { process } => {
                self.send_request_stdio(process, method, params).await
            }
        }
    }

    async fn send_request_http(
        &self,
        url: &str,
        client: &reqwest::Client,
        method: &str,
        params: Value,
    ) -> AgentResult<Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response = client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| AgentError::internal(format!("MCP request failed: {e}")))?;

        let res: Value = response
            .json()
            .await
            .map_err(|e| AgentError::json(format!("MCP response parse: {e}")))?;

        if let Some(error) = res.get("error") {
            return Err(AgentError::internal(format!("MCP error: {error}")));
        }

        Ok(res.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn send_request_stdio(
        &self,
        process: &Mutex<StdioProcess>,
        method: &str,
        params: Value,
    ) -> AgentResult<Value> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut request_line = serde_json::to_string(&request)
            .map_err(|e| AgentError::json(format!("serialize: {e}")))?;
        request_line.push('\n');

        let response_line = {
            let mut proc = process.lock().await;
            proc.stdin
                .write_all(request_line.as_bytes())
                .await
                .map_err(|e| AgentError::internal(format!("stdio write: {e}")))?;

            let mut line = String::new();
            proc.stdout
                .read_line(&mut line)
                .await
                .map_err(|e| AgentError::internal(format!("stdio read: {e}")))?;
            line
        };

        let res: Value = serde_json::from_str(&response_line)
            .map_err(|e| AgentError::json(format!("parse: {e}")))?;

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

/// Wraps an MCP server tool as an agent-base `Tool`.
///
/// Tool names are prefixed with `mcp.{server_name}.` to prevent collisions
/// between tools from different MCP servers.
pub struct McpToolAdapter {
    /// Prefixed name exposed to the LLM: `mcp.{server}.{tool}`
    name: &'static str,
    /// Original tool name used when calling the MCP server
    original_name: String,
    description: String,
    input_schema: Value,
    mcp_client: Arc<McpClient>,
}

impl McpToolAdapter {
    pub fn new(info: McpToolInfo, mcp_client: Arc<McpClient>, server_name: &str) -> Self {
        let original_name = info.name;
        let canonical = format!("mcp.{}.{}", server_name, original_name);
        let static_name: &'static str = Box::leak(canonical.into_boxed_str());
        Self {
            name: static_name,
            original_name,
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
        let result = self.mcp_client.call_tool(&self.original_name, args).await?;
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
