use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::sync::broadcast;

use crate::llm::LlmClient;
use crate::skill::{Skill, SkillDetailTool, SkillPrompter, LazySkillPrompter};
use crate::tool::{Tool, ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, ResponseFormat, RetryConfig};

use super::approval::ApprovalHandler;
use super::context::ContextWindowManager;
use super::middleware::{Middleware, MiddlewareRef};
use super::recovery::{StopOnError, ToolErrorRecovery};
use super::session_store::{InMemorySessionStore, SessionStore};
use super::AgentRuntime;

pub struct AgentBuilder {
    client: Arc<dyn LlmClient>,
    config: AgentConfig,
    tools: ToolRegistry,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    middlewares: Vec<MiddlewareRef>,
    context_manager: Option<ContextWindowManager>,
    session_store: Option<Arc<dyn SessionStore>>,
    skills: Vec<Arc<dyn Skill>>,
    skill_prompter: Option<Arc<dyn SkillPrompter>>,
    skill_detail_tool_name: String,
    disable_skill_prompt_injection: bool,
    error_recovery: Option<Arc<dyn ToolErrorRecovery>>,
}

impl AgentBuilder {
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self {
            client,
            config: AgentConfig::default(),
            tools: ToolRegistry::default(),
            approval_handler: None,
            tool_policy: None,
            middlewares: Vec::new(),
            context_manager: None,
            session_store: None,
            skills: Vec::new(),
            skill_prompter: None,
            skill_detail_tool_name: "get_skill_detail".to_string(),
            disable_skill_prompt_injection: false,
            error_recovery: None,
        }
    }

    pub fn system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn enable_thought(mut self, enable: bool) -> Self {
        self.config.enable_thought = enable;
        self
    }

    pub fn enable_thinking(mut self, enable: bool) -> Self {
        self.config.enable_thinking = Some(enable);
        self
    }

    pub fn tool_timeout(mut self, timeout_ms: u64) -> Self {
        self.config.tool_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn max_tool_output_chars(mut self, max_chars: usize) -> Self {
        self.config.max_tool_output_chars = Some(max_chars);
        self
    }

    pub fn register_tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    pub fn register_tool_arc(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.register_arc(tool);
        self
    }

    pub fn approval_handler(mut self, handler: Arc<dyn ApprovalHandler>) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn tool_policy(mut self, policy: Arc<dyn ToolPolicy>) -> Self {
        self.tool_policy = Some(policy);
        self
    }

    pub fn middleware(mut self, mw: impl Middleware + 'static) -> Self {
        self.middlewares.push(Arc::new(mw));
        self
    }

    pub fn context_window(mut self, max_tokens: usize) -> Self {
        self.context_manager = Some(ContextWindowManager::new(max_tokens));
        self
    }

    pub fn context_window_manager(mut self, manager: ContextWindowManager) -> Self {
        self.context_manager = Some(manager);
        self
    }

    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.config.response_format = Some(format);
        self
    }

    pub fn llm_retry(mut self, retry: RetryConfig) -> Self {
        self.config.llm_retry = Some(retry);
        self
    }

    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    pub fn register_skill(mut self, skill: impl Skill + 'static) -> Self {
        self.skills.push(Arc::new(skill));
        self
    }

    pub fn skill_prompter(mut self, prompter: Arc<dyn SkillPrompter>) -> Self {
        self.skill_prompter = Some(prompter);
        self
    }

    pub fn disable_skill_prompt_injection(mut self) -> Self {
        self.disable_skill_prompt_injection = true;
        self
    }

    pub fn skill_detail_tool_name(mut self, name: impl Into<String>) -> Self {
        self.skill_detail_tool_name = name.into();
        self
    }

    pub fn error_recovery(mut self, recovery: Arc<dyn ToolErrorRecovery>) -> Self {
        self.error_recovery = Some(recovery);
        self
    }

    pub fn tool_error_retry_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.tool_error_retry_prompt = Some(prompt.into());
        self
    }

    pub fn build(mut self) -> AgentRuntime {
        let prompter: Arc<dyn SkillPrompter> = self
            .skill_prompter
            .unwrap_or_else(|| Arc::new(LazySkillPrompter::new()));

        let mut skill_refs: Vec<Arc<dyn Skill>> = Vec::new();
        let mut known_tool_names: HashSet<String> = self
            .tools
            .definitions()
            .iter()
            .filter_map(|d| {
                d.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        for skill in self.skills {
            for tool in skill.tools() {
                let tool_name = tool.name().to_string();
                if known_tool_names.contains(&tool_name) {
                    panic!(
                        "Tool name conflict: `{}` (Skill `{}`)",
                        tool_name,
                        skill.name()
                    );
                }
                known_tool_names.insert(tool_name.clone());
                self.tools.register_arc(tool);
            }
            skill_refs.push(skill);
        }

        if !skill_refs.is_empty() && !self.disable_skill_prompt_injection {
            let skill_prompt = prompter.build_prompt(&skill_refs);
            if !skill_prompt.is_empty() {
                let new_prompt = match self.config.system_prompt.take() {
                    Some(existing) => format!("{}\n\n---\n\n{}", existing, skill_prompt),
                    None => skill_prompt,
                };
                self.config.system_prompt = Some(new_prompt);
            }
        }

        if !skill_refs.is_empty() {
            let detail_tool = SkillDetailTool::new(
                skill_refs.clone(),
                std::mem::take(&mut self.skill_detail_tool_name),
            );
            self.tools.register(detail_tool);
        }

        let (event_bus, _) = broadcast::channel(2048);
        let session_store = self
            .session_store
            .unwrap_or_else(|| Arc::new(InMemorySessionStore::new()));
        let error_recovery = self
            .error_recovery
            .unwrap_or_else(|| Arc::new(StopOnError));

        AgentRuntime {
            client: self.client,
            config: self.config,
            tools: self.tools,
            approval_handler: self.approval_handler,
            tool_policy: self.tool_policy,
            middlewares: self.middlewares,
            event_bus,
            next_session_id: AtomicU64::new(1),
            sessions: HashMap::new(),
            context_manager: self.context_manager,
            session_store,
            skills: skill_refs,
            skill_prompter: prompter,
            error_recovery,
        }
    }
}
