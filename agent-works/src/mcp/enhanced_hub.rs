use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agent_base::{AgentError, AgentResult, ToolRegistry};
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info, warn};

use super::client::{McpClient, McpToolAdapter};
use super::types::{McpServerConfig, McpToolInfo};

#[derive(Clone, Debug)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
    Unhealthy(String),
}

struct ServerEntry {
    config: McpServerConfig,
    clients: RwLock<Vec<Arc<McpClient>>>,
    max_connections: usize,
    tools: RwLock<Vec<McpToolInfo>>,
    state: RwLock<ConnectionState>,
    last_health_check: RwLock<Option<Instant>>,
    reconnect_attempts: RwLock<u32>,
    round_robin_idx: AtomicUsize,
}

impl ServerEntry {
    fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            clients: RwLock::new(Vec::new()),
            max_connections: 5,
            tools: RwLock::new(Vec::new()),
            state: RwLock::new(ConnectionState::Disconnected),
            last_health_check: RwLock::new(None),
            reconnect_attempts: RwLock::new(0),
            round_robin_idx: AtomicUsize::new(0),
        }
    }

    async fn create_client(&self) -> AgentResult<Arc<McpClient>> {
        let client = Arc::new(McpClient::new(self.config.transport.clone()).await?);
        Ok(client)
    }

    async fn get_available_client(&self) -> Option<Arc<McpClient>> {
        let clients = self.clients.read().await;
        if clients.is_empty() {
            return None;
        }
        // Round-robin selection across pooled connections
        let idx = self.round_robin_idx.fetch_add(1, Ordering::Relaxed) % clients.len();
        clients.get(idx).cloned()
    }

    async fn add_client(&self) -> AgentResult<()> {
        if self.clients.read().await.len() >= self.max_connections {
            return Err(AgentError::resource_unavailable(
                "Maximum connections reached".to_string(),
            ));
        }

        let client = self.create_client().await?;
        self.clients.write().await.push(client);

        let mut state = self.state.write().await;
        *state = ConnectionState::Connected;

        Ok(())
    }

    async fn reconnect(&self) -> AgentResult<()> {
        let mut attempts = self.reconnect_attempts.write().await;
        *attempts += 1;
        let attempt_count = *attempts;
        drop(attempts);

        // 指数退避
        let delay = Duration::from_millis(std::cmp::min(
            1000 * (2_u64.pow(attempt_count.min(5))),
            30000,
        ));
        sleep(delay).await;

        // 清除现有连接
        self.clients.write().await.clear();

        match self.add_client().await {
            Ok(_) => {
                let mut state = self.state.write().await;
                *state = ConnectionState::Connected;

                let mut attempts = self.reconnect_attempts.write().await;
                *attempts = 0;

                info!(
                    "Successfully reconnected to MCP server: {}",
                    self.config.name
                );
                Ok(())
            }
            Err(e) => {
                let mut state = self.state.write().await;
                *state = ConnectionState::Failed(e.to_string());
                error!("Failed to reconnect to {}: {e}", self.config.name);
                Err(e)
            }
        }
    }

    async fn health_check(&self) -> bool {
        if let Some(client) = self.get_available_client().await {
            match client.list_tools().await {
                Ok(_) => {
                    let mut state = self.state.write().await;
                    *state = ConnectionState::Connected;

                    let mut last_check = self.last_health_check.write().await;
                    *last_check = Some(Instant::now());

                    true
                }
                Err(e) => {
                    let mut state = self.state.write().await;
                    *state = ConnectionState::Unhealthy(e.to_string());
                    warn!("Health check failed for {}: {e}", self.config.name);
                    false
                }
            }
        } else {
            false
        }
    }
}

pub struct EnhancedMcpHub {
    servers: Arc<StdRwLock<HashMap<String, Arc<ServerEntry>>>>,
    health_check_interval: Duration,
    shutdown: Arc<AtomicBool>,
    health_task: RwLock<Option<JoinHandle<()>>>,
    status_tx: broadcast::Sender<(String, ConnectionState)>,
}

impl EnhancedMcpHub {
    pub fn new() -> Self {
        let (status_tx, _) = broadcast::channel(100);
        Self {
            servers: Arc::new(StdRwLock::new(HashMap::new())),
            health_check_interval: Duration::from_secs(30),
            shutdown: Arc::new(AtomicBool::new(false)),
            health_task: RwLock::new(None),
            status_tx,
        }
    }

    pub fn with_health_check_interval(mut self, interval: Duration) -> Self {
        self.health_check_interval = interval;
        self
    }

    pub fn add_server(&self, config: McpServerConfig) {
        let mut servers = self.servers.write().unwrap();
        let entry = Arc::new(ServerEntry::new(config));
        servers.insert(entry.config.name.clone(), entry);
    }

    /// Connect to all configured servers. Continues connecting remaining servers
    /// even if one fails. Returns an error only if *all* servers fail to connect.
    pub async fn connect_all(&self) -> AgentResult<()> {
        let servers: Vec<(String, Arc<ServerEntry>)> = {
            let guard = self.servers.read().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        let mut errors: Vec<(String, AgentError)> = Vec::new();

        for (name, entry) in &servers {
            match entry.add_client().await {
                Ok(_) => {
                    info!(server_name = %name, "Connected to MCP server");
                }
                Err(e) => {
                    let mut state = entry.state.write().await;
                    *state = ConnectionState::Failed(e.to_string());
                    error!(server_name = %name, error = %e, "Failed to connect to MCP server");
                    errors.push((name.clone(), e));
                }
            }
        }

        // Start background health check
        self.start_health_check_task().await;

        if errors.is_empty() {
            Ok(())
        } else if errors.len() == servers.len() {
            // All servers failed
            let msg = errors
                .iter()
                .map(|(name, e)| format!("{name}: {e}"))
                .collect::<Vec<_>>()
                .join("; ");
            Err(AgentError::internal(format!(
                "All MCP servers failed to connect: {msg}"
            )))
        } else {
            // Some succeeded, some failed — log but don't error
            let failed: Vec<&str> = errors.iter().map(|(n, _)| n.as_str()).collect();
            warn!("Some MCP servers failed to connect: {}", failed.join(", "));
            Ok(())
        }
    }

    pub async fn discover_all(&self) -> AgentResult<Vec<(String, Vec<McpToolInfo>)>> {
        let servers: Vec<(String, Arc<ServerEntry>)> = {
            let guard = self.servers.read().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        let mut results = Vec::new();

        for (name, entry) in &servers {
            let Some(client) = entry.get_available_client().await else {
                warn!("Server {} not connected", name);
                continue;
            };

            match client.list_tools().await {
                Ok(tools) => {
                    {
                        let mut entry_tools = entry.tools.write().await;
                        *entry_tools = tools.clone();
                    }

                    info!("Discovered {} tools from {}", tools.len(), name);
                    results.push((name.clone(), tools));
                }
                Err(e) => {
                    warn!("Failed to discover tools from {}: {e}", name);
                    let mut state = entry.state.write().await;
                    *state = ConnectionState::Failed(format!("{e}"));
                }
            }
        }

        Ok(results)
    }

    /// Register all discovered tools into the given registry.
    /// This is an async method because it reads from `RwLock`-protected state.
    pub async fn register_all(&self, registry: &mut ToolRegistry) {
        let servers: Vec<(String, Arc<ServerEntry>)> = {
            let guard = self.servers.read().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        for (name, entry) in &servers {
            let tools = entry.tools.read().await.clone();
            for tool_info in &tools {
                if let Some(client) = entry.get_available_client().await {
                    let adapter = McpToolAdapter::new(tool_info.clone(), client, name);
                    registry.register(adapter);
                }
            }
            info!(server_name = %name, tool_count = tools.len(), "Registered tools from MCP server");
        }
    }

    pub async fn disconnect_all(&self) {
        // Signal health check loop to stop
        self.shutdown.store(true, Ordering::SeqCst);

        // Abort the health check task if it exists
        if let Some(handle) = self.health_task.write().await.take() {
            handle.abort();
        }

        let servers: Vec<(String, Arc<ServerEntry>)> = {
            let guard = self.servers.read().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        for (name, entry) in &servers {
            entry.clients.write().await.clear();
            let mut state = entry.state.write().await;
            *state = ConnectionState::Disconnected;
            info!(server_name = %name, "Disconnected from MCP server");
        }
    }

    async fn start_health_check_task(&self) {
        let servers = self.servers.clone(); // Arc<StdRwLock<...>> — cheap clone of the Arc
        let interval = self.health_check_interval;
        let shutdown = self.shutdown.clone();

        // Reset shutdown flag in case connect_all is called again after disconnect_all
        shutdown.store(false, Ordering::SeqCst);

        let handle = tokio::spawn(async move {
            loop {
                sleep(interval).await;

                if shutdown.load(Ordering::SeqCst) {
                    info!("Health check task shutting down");
                    break;
                }

                // Snapshot the current server set each iteration so newly-added
                // servers are picked up and removed servers are dropped.
                let current: Vec<(String, Arc<ServerEntry>)> = {
                    let guard = servers.read().unwrap();
                    guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                };

                for (name, entry) in &current {
                    let is_healthy = entry.health_check().await;

                    if !is_healthy
                        && matches!(*entry.state.read().await, ConnectionState::Unhealthy(_))
                    {
                        if entry.config.auto_reconnect {
                            if let Err(e) = entry.reconnect().await {
                                error!("Failed to reconnect to {}: {e}", name);
                            }
                        }
                    }
                }
            }
        });

        *self.health_task.write().await = Some(handle);
    }

    pub async fn get_connection_state(&self, server_name: &str) -> Option<ConnectionState> {
        let entry = {
            let guard = self.servers.read().unwrap();
            guard.get(server_name).cloned()
        };
        match entry {
            Some(entry) => Some(entry.state.read().await.clone()),
            None => None,
        }
    }

    pub async fn get_all_states(&self) -> HashMap<String, ConnectionState> {
        let servers: Vec<(String, Arc<ServerEntry>)> = {
            let guard = self.servers.read().unwrap();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        let mut states = HashMap::new();
        for (name, entry) in &servers {
            states.insert(name.clone(), entry.state.read().await.clone());
        }
        states
    }

    // ── Runtime server management ──

    /// Connect to a single MCP server by name.
    pub async fn connect_one(&self, name: &str) -> AgentResult<()> {
        let entry = {
            let guard = self.servers.read().unwrap();
            guard.get(name).cloned()
        };
        let entry = entry.ok_or_else(|| {
            AgentError::resource_unavailable(format!("MCP server '{name}' not found"))
        })?;

        entry.add_client().await?;

        let state = entry.state.read().await.clone();
        if let Err(e) = self.status_tx.send((name.to_string(), state)) {
            warn!("Failed to broadcast connect status for '{}': {}", name, e);
        }

        Ok(())
    }

    /// Disconnect from a single MCP server by name.
    pub async fn disconnect_one(&self, name: &str) {
        let entry = {
            let guard = self.servers.read().unwrap();
            guard.get(name).cloned()
        };
        if let Some(entry) = entry {
            entry.clients.write().await.clear();
            {
                let mut state = entry.state.write().await;
                *state = ConnectionState::Disconnected;
            }
            if let Err(e) = self
                .status_tx
                .send((name.to_string(), ConnectionState::Disconnected))
            {
                warn!(
                    "Failed to broadcast disconnect status for '{}': {}",
                    name, e
                );
            }
        }
    }

    /// Remove a server config from the hub. Disconnects existing clients first.
    /// Returns true if the server existed.
    pub async fn remove_server(&self, name: &str) -> bool {
        // Disconnect existing clients before removing
        let entry = {
            let guard = self.servers.read().unwrap();
            guard.get(name).cloned()
        };
        if let Some(entry) = entry {
            entry.clients.write().await.clear();
            *entry.state.write().await = ConnectionState::Disconnected;
        }

        let mut servers = self.servers.write().unwrap();
        servers.remove(name).is_some()
    }

    /// Update an existing server config. Disconnects the old entry's clients first,
    /// then replaces it with a fresh entry.
    pub async fn update_server(&self, config: McpServerConfig) {
        // Disconnect existing clients before replacing
        let entry = {
            let guard = self.servers.read().unwrap();
            guard.get(&config.name).cloned()
        };
        if let Some(entry) = entry {
            entry.clients.write().await.clear();
            *entry.state.write().await = ConnectionState::Disconnected;
        }

        let mut servers = self.servers.write().unwrap();
        servers.insert(config.name.clone(), Arc::new(ServerEntry::new(config)));
    }

    /// Subscribe to connection-state change events.
    /// Returns a receiver that yields `(server_name, ConnectionState)` tuples.
    pub fn subscribe_status(&self) -> broadcast::Receiver<(String, ConnectionState)> {
        self.status_tx.subscribe()
    }
}

impl Drop for EnhancedMcpHub {
    fn drop(&mut self) {
        // Signal the health check loop to stop when the hub is dropped
        self.shutdown.store(true, Ordering::SeqCst);
    }
}
