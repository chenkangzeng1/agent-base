use std::sync::Arc;

use agent_base::{AgentResult, ToolRegistry};
use tracing::{info, warn};

use super::client::{McpClient, McpToolAdapter};
use super::enhanced_hub::ConnectionState;
use super::types::{McpServerConfig, McpToolInfo};

struct ServerEntry {
    config: McpServerConfig,
    client: Option<Arc<McpClient>>,
    tools: Vec<McpToolInfo>,
    state: ConnectionState,
}

pub struct McpHub {
    servers: Vec<ServerEntry>,
}

impl McpHub {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
        }
    }

    pub fn add_server(&mut self, config: McpServerConfig) {
        self.servers.push(ServerEntry {
            config,
            client: None,
            tools: Vec::new(),
            state: ConnectionState::Disconnected,
        });
    }

    pub async fn connect_all(&mut self) -> AgentResult<()> {
        for entry in &mut self.servers {
            let client = Arc::new(McpClient::new(entry.config.transport.clone()).await?);
            entry.client = Some(client);
            entry.state = ConnectionState::Connected;
            info!("connected to MCP server: {}", entry.config.name);
        }
        Ok(())
    }

    pub async fn discover_all(&mut self) -> AgentResult<Vec<(String, Vec<McpToolInfo>)>> {
        let mut results = Vec::new();
        for entry in &mut self.servers {
            let Some(client) = &entry.client else {
                warn!("server {} not connected", entry.config.name);
                continue;
            };
            match client.list_tools().await {
                Ok(tools) => {
                    info!(
                        "discovered {} tools from {}",
                        tools.len(),
                        entry.config.name
                    );
                    let cloned = tools.clone();
                    entry.tools = tools;
                    results.push((entry.config.name.clone(), cloned));
                }
                Err(e) => {
                    warn!(
                        server_name = %entry.config.name,
                        error = %e,
                        "failed to discover tools from MCP server"
                    );
                    entry.state = ConnectionState::Failed(format!("{e}"));
                }
            }
        }
        Ok(results)
    }

    pub fn register_all(&self, registry: &mut ToolRegistry) {
        for entry in &self.servers {
            let Some(client) = &entry.client else {
                continue;
            };
            for tool_info in &entry.tools {
                let adapter =
                    McpToolAdapter::new(tool_info.clone(), client.clone(), &entry.config.name);
                registry.register(adapter);
            }
        }
    }

    pub async fn disconnect_all(&mut self) {
        for entry in &mut self.servers {
            entry.client = None;
            entry.state = ConnectionState::Disconnected;
            info!(server_name = %entry.config.name, "disconnected from MCP server");
        }
    }

    pub async fn health_check(&self) -> AgentResult<()> {
        for entry in &self.servers {
            let Some(client) = &entry.client else {
                warn!("server {} not connected", entry.config.name);
                continue;
            };
            match client.list_tools().await {
                Ok(_) => {
                    info!(server_name = %entry.config.name, "health check passed");
                }
                Err(e) => {
                    warn!(server_name = %entry.config.name, error = %e, "health check failed");
                }
            }
        }
        Ok(())
    }
}
