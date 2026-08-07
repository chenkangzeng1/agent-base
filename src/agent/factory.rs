use std::sync::Arc;

use agent_base::{AgentResult, AgentRuntime, ReasoningEffort, RunOutcome, RuntimeEvent, SafetyConfig, SessionId};

use agent_works::AgentBuilder;

use crate::agent::builder::base_agent_builder;

/// phi-agent configuration (tool-agnostic).
///
/// This config covers model and safety settings only. Tools are registered
/// externally on [`agent_works::AgentBuilder`] — phi-agent itself never bundles tools
/// beyond kernel tools (multi-agent, skills) which are opt-in via feature flags.
#[derive(Clone, Default)]
pub struct PhiAgentConfig {
    /// Model name passed to the LLM provider (e.g. `"opus"`, `"gpt-4o"`).
    pub model: String,
    /// Enable extended thinking / chain-of-thought.
    pub enable_thinking: bool,
    /// Token budget for thinking (provider-dependent). `None` means use the
    /// provider default.
    pub thinking_budget: Option<u64>,
    /// Reasoning intensity: Low / Medium / High / XHigh.
    pub thinking_effort: ReasoningEffort,
    /// Per-turn safety limits (max tool calls, max consecutive failures, etc.).
    pub safety: SafetyConfig,
    /// React-loop iteration cap for a single run (one user input).
    /// `None` means use the builder default (200 in [`base_agent_builder`]).
    pub max_turns: Option<u32>,
}

/// A built Agent instance.
///
/// Wraps [`AgentRuntime`] with common operations behind a simpler API.
///
/// ## Example
///
/// ```ignore
/// let agent = PhiAgent::build(builder, config)?;
/// let session = agent.create_session().await;
/// agent.run_turn(session, "Hello!", |event| renderer.render(event)).await?;
/// ```
#[derive(Clone)]
pub struct PhiAgent {
    runtime: AgentRuntime,
    /// The configuration this agent was built with.
    pub config: PhiAgentConfig,
    /// MCP hub for runtime server management. Only available with the `mcp` feature.
    #[cfg(feature = "mcp")]
    mcp_hub: Arc<std::sync::Mutex<Option<Arc<agent_works::mcp::EnhancedMcpHub>>>>,
}

impl PhiAgent {
    /// Create a pre-configured AgentBuilder.
    ///
    /// Equivalent to `base_agent_builder(llm_client).system_prompt(system_prompt)`,
    /// after which you register tools, middleware, and approval handlers,
    /// then call `Self::build`.
    pub fn builder(llm_client: Arc<dyn agent_base::LlmClient>, system_prompt: String) -> AgentBuilder {
        base_agent_builder(llm_client).system_prompt(system_prompt)
    }

    /// Build from an AgentBuilder.
    pub fn build(builder: AgentBuilder, config: PhiAgentConfig) -> AgentResult<Self> {
        let runtime = builder.build()?;
        Ok(Self {
            runtime,
            config,
            #[cfg(feature = "mcp")]
            mcp_hub: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Create an agent session.
    pub async fn create_session(&self) -> SessionId {
        self.runtime.create_session().await
    }

    /// Execute one turn.
    pub async fn run_turn<F>(&self, session_id: SessionId, query: &str, on_event: F) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runtime.run_turn(session_id, query, on_event).await
    }

    /// Cancel the currently executing turn.
    pub fn cancel(&self) {
        self.runtime.cancel();
    }

    /// Check whether the agent has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.runtime.is_cancelled()
    }

    /// Set the reasoning effort.
    pub async fn set_reasoning_effort(&self, effort: ReasoningEffort) {
        self.runtime.set_reasoning_effort(effort).await;
    }

    /// Access the underlying runtime (for advanced use like hook registration).
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }

    /// List all registered tools with their metadata, sorted by name.
    pub async fn list_tools(&self) -> Vec<agent_base::ToolMetadata> {
        let tools = self.runtime.tools_mut();
        let registry = tools.read().await;
        registry.metadatas()
    }
}

// ── MCP Runtime Management (Phase 1.2) ──

#[cfg(feature = "mcp")]
impl PhiAgent {
    /// Get or lazily initialize the MCP hub.
    fn get_or_init_hub(&self) -> Arc<agent_works::mcp::EnhancedMcpHub> {
        let mut guard = self.mcp_hub.lock().unwrap();
        if let Some(ref hub) = *guard {
            return hub.clone();
        }
        let hub = Arc::new(agent_works::mcp::EnhancedMcpHub::new());
        *guard = Some(hub.clone());
        hub
    }

    /// Dynamically attach an MCP server at runtime.
    ///
    /// Adds the server config, connects, discovers tools, and registers them
    /// into the agent's [`ToolRegistry`]. Tools are registered with the
    /// `mcp.<server_name>.<tool_name>` naming convention.
    ///
    /// Returns an error if the server cannot be connected or tools cannot be
    /// discovered. On failure, the server config is rolled back (removed from
    /// the hub) so a partial entry is never left behind.
    ///
    /// # Performance note
    ///
    /// Currently calls `hub.register_all()` which re-registers all servers'
    /// tools (O(total-servers)). For the common case this is fine because
    /// re-registration is a no-op HashMap insert. A future optimization would
    /// register only the newly attached server's tools.
    pub async fn attach_mcp(&self, config: agent_works::mcp::McpServerConfig) -> AgentResult<()> {
        let name = config.name.clone();
        let hub = self.get_or_init_hub();

        // Add server config and attempt connection
        hub.add_server(config);
        if let Err(e) = hub.connect_one(&name).await {
            hub.remove_server(&name).await;
            return Err(e);
        }

        // Discover tools; rollback on failure
        let discovered = match hub.discover_all().await {
            Ok(d) => d,
            Err(e) => {
                hub.remove_server(&name).await;
                return Err(e);
            },
        };

        // Register discovered tools into the runtime
        // TODO(perf): register only the new server's tools instead of all servers
        let tools = self.runtime.tools_mut();
        let mut registry = tools.write().await;
        hub.register_all(&mut registry).await;

        // Inject framework dependencies (e.g. EventBus) into the new tools
        self.runtime.inject_framework_deps(&registry);

        let count: usize = discovered.iter().filter(|(n, _)| n == &name).map(|(_, t)| t.len()).sum();
        if count == 0 {
            tracing::warn!(
                server_name = %name,
                "attached MCP server but discovered zero tools — server may be misconfigured"
            );
        }
        tracing::info!(server_name = %name, tool_count = count, "attached MCP server at runtime");
        Ok(())
    }

    /// Dynamically detach an MCP server at runtime.
    ///
    /// Unregisters all tools belonging to this server from the agent's
    /// [`ToolRegistry`], disconnects the server, and removes its config
    /// from the hub.
    ///
    /// This is a no-op if the server is not attached.
    ///
    /// # Concurrency note
    ///
    /// There is a TOCTOU window between collecting tool names (read lock) and
    /// removing them (write lock). If another thread re-attaches a server with
    /// the same name during this window, its tools may be prematurely removed.
    /// In practice this race is harmless: the new attach will re-register tools
    /// on the next turn, and tool calls in flight will fail with a clear error
    /// since `hub.remove_server` disconnects clients.
    pub async fn detach_mcp(&self, name: &str) {
        let hub = {
            let guard = self.mcp_hub.lock().unwrap();
            match *guard {
                Some(ref hub) => hub.clone(),
                None => return,
            }
        };

        // Collect tool names matching the mcp.<server>.<tool> prefix.
        // NOTE: the "mcp.<server>.<tool>" naming convention is defined by
        // agent_works::mcp::McpToolAdapter. If that convention changes, this
        // prefix must be updated.
        let mcp_prefix = format!("mcp.{}.", name);
        let tool_names: Vec<String> = {
            let tools = self.runtime.tools_mut();
            let registry = tools.read().await;
            registry.metadatas().iter().filter(|m| m.name.starts_with(&mcp_prefix)).map(|m| m.name.clone()).collect()
        };

        // Unregister tools from the runtime
        if !tool_names.is_empty() {
            let tools = self.runtime.tools_mut();
            let mut registry = tools.write().await;
            for tool_name in &tool_names {
                registry.remove(tool_name);
            }
        }

        // Remove the server from the hub (disconnects clients)
        hub.remove_server(name).await;

        tracing::info!(
            server_name = %name,
            tool_count = tool_names.len(),
            "detached MCP server at runtime"
        );
    }

    // ── MCP Server (Phase 4.1) ──

    /// Convert this agent into an MCP server that external orchestrators can call.
    #[cfg(feature = "mcp")]
    pub fn into_mcp_server(&self, config: agent_works::mcp::McpServeConfig) -> agent_works::mcp::McpServer {
        agent_works::mcp::McpServer::new(self.runtime.clone(), config)
    }
}
