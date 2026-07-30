use std::collections::HashSet;
use std::sync::Arc;

use agent_base::{AgentResult, AgentRuntime, LlmClient, Tool};

#[cfg(feature = "skill")]
use crate::skill::{LazySkillPrompter, Skill, SkillDetailTool, SkillPrompter};

pub struct AgentBuilder {
    inner: agent_base::AgentBuilder,
    system_prompt: Option<String>,
    tool_names: HashSet<String>,
    #[cfg(feature = "skill")]
    skills: Vec<Arc<dyn Skill>>,
    #[cfg(feature = "skill")]
    skill_prompter: Option<Arc<dyn SkillPrompter>>,
    #[cfg(feature = "skill")]
    skill_detail_tool_name: String,
    #[cfg(feature = "skill")]
    disable_skill_prompt_injection: bool,
}

impl AgentBuilder {
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self {
            inner: agent_base::AgentBuilder::new(client),
            system_prompt: None,
            tool_names: HashSet::new(),
            #[cfg(feature = "skill")]
            skills: Vec::new(),
            #[cfg(feature = "skill")]
            skill_prompter: None,
            #[cfg(feature = "skill")]
            skill_detail_tool_name: "get_skill_detail".to_string(),
            #[cfg(feature = "skill")]
            disable_skill_prompt_injection: false,
        }
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        let prompt = prompt.into();
        self.inner = self.inner.system_prompt(prompt.clone());
        self.system_prompt = Some(prompt);
        self
    }

    pub fn enable_thought(self, enable: bool) -> Self {
        Self {
            inner: self.inner.enable_thought(enable),
            ..self
        }
    }

    pub fn reasoning(self, config: agent_base::ReasoningConfig) -> Self {
        Self {
            inner: self.inner.reasoning(config),
            ..self
        }
    }

    pub fn enable_thinking(self, enable: bool) -> Self {
        Self {
            inner: self.inner.enable_thinking(enable),
            ..self
        }
    }

    pub fn thinking_budget(self, budget: u64) -> Self {
        Self {
            inner: self.inner.thinking_budget(budget),
            ..self
        }
    }

    pub fn tool_timeout(self, timeout_ms: u64) -> Self {
        Self {
            inner: self.inner.tool_timeout(timeout_ms),
            ..self
        }
    }

    pub fn max_tool_output_chars(self, max_chars: usize) -> Self {
        Self {
            inner: self.inner.max_tool_output_chars(max_chars),
            ..self
        }
    }

    pub fn register_tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tool_names.insert(tool.name().to_string());
        self.inner = self.inner.register_tool(tool);
        self
    }

    pub fn register_tool_arc(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tool_names.insert(tool.name().to_string());
        self.inner = self.inner.register_tool_arc(tool);
        self
    }

    pub fn approval_handler(self, handler: Arc<dyn agent_base::ApprovalHandler>) -> Self {
        Self {
            inner: self.inner.approval_handler(handler),
            ..self
        }
    }

    pub fn tool_policy(self, policy: Arc<dyn agent_base::ToolPolicy>) -> Self {
        Self {
            inner: self.inner.tool_policy(policy),
            ..self
        }
    }

    pub fn middleware(self, mw: impl agent_base::Middleware + 'static) -> Self {
        Self {
            inner: self.inner.middleware(mw),
            ..self
        }
    }

    pub fn context_window(self, max_tokens: usize) -> Self {
        Self {
            inner: self.inner.context_window(max_tokens),
            ..self
        }
    }

    pub fn context_window_manager(self, manager: agent_base::ContextWindowManager) -> Self {
        Self {
            inner: self.inner.context_window_manager(manager),
            ..self
        }
    }

    pub fn response_format(self, format: agent_base::ResponseFormat) -> Self {
        Self {
            inner: self.inner.response_format(format),
            ..self
        }
    }

    pub fn llm_retry(self, retry: agent_base::RetryConfig) -> Self {
        Self {
            inner: self.inner.llm_retry(retry),
            ..self
        }
    }

    pub fn session_store(self, store: Arc<dyn agent_base::SessionStore>) -> Self {
        Self {
            inner: self.inner.session_store(store),
            ..self
        }
    }

    pub fn error_recovery(self, recovery: Arc<dyn agent_base::ToolErrorRecovery>) -> Self {
        Self {
            inner: self.inner.error_recovery(recovery),
            ..self
        }
    }

    pub fn tool_error_retry_prompt(self, prompt: impl Into<String>) -> Self {
        Self {
            inner: self.inner.tool_error_retry_prompt(prompt),
            ..self
        }
    }

    pub fn language(self, language: agent_base::Language) -> Self {
        Self {
            inner: self.inner.language(language),
            ..self
        }
    }

    pub fn event_bus_capacity(self, capacity: usize) -> Self {
        Self {
            inner: self.inner.event_bus_capacity(capacity),
            ..self
        }
    }

    pub fn session_id_generator(
        self,
        generator: Arc<dyn agent_base::types::SessionIdGenerator>,
    ) -> Self {
        Self {
            inner: self.inner.session_id_generator(generator),
            ..self
        }
    }

    #[cfg(feature = "skill")]
    pub fn register_skill(mut self, skill: impl Skill + 'static) -> Self {
        self.skills.push(Arc::new(skill));
        self
    }

    #[cfg(feature = "skill")]
    pub fn register_skills(mut self, skills: Vec<Arc<dyn Skill>>) -> Self {
        self.skills.extend(skills);
        self
    }

    #[cfg(feature = "skill")]
    pub fn skill_prompter(mut self, prompter: Arc<dyn SkillPrompter>) -> Self {
        self.skill_prompter = Some(prompter);
        self
    }

    #[cfg(feature = "skill")]
    pub fn disable_skill_prompt_injection(mut self) -> Self {
        self.disable_skill_prompt_injection = true;
        self
    }

    #[cfg(feature = "skill")]
    pub fn skill_detail_tool_name(mut self, name: impl Into<String>) -> Self {
        self.skill_detail_tool_name = name.into();
        self
    }

    pub fn build(self) -> AgentResult<AgentRuntime> {
        #[cfg(feature = "skill")]
        {
            self.build_with_skills()
        }
        #[cfg(not(feature = "skill"))]
        {
            self.inner.build()
        }
    }

    #[cfg(feature = "skill")]
    fn build_with_skills(mut self) -> AgentResult<AgentRuntime> {
        let mut ab = self.inner;

        if self.skills.is_empty() {
            return ab.build();
        }

        let prompter: Arc<dyn SkillPrompter> = self
            .skill_prompter
            .take()
            .unwrap_or_else(|| Arc::new(LazySkillPrompter::new()));

        let mut skill_refs: Vec<Arc<dyn Skill>> = Vec::new();

        for skill in self.skills {
            for tool in skill.tools() {
                let tool_name = tool.name().to_string();
                if self.tool_names.contains(&tool_name) {
                    return Err(agent_base::AgentError::internal(format!(
                        "Tool name conflict: `{}` (Skill `{}`)",
                        tool_name,
                        skill.name()
                    )));
                }
                self.tool_names.insert(tool_name);
                ab = ab.register_tool_arc(tool);
            }
            skill_refs.push(skill);
        }

        if !self.disable_skill_prompt_injection {
            let skill_prompt = prompter.build_prompt(&skill_refs, &self.skill_detail_tool_name);
            if !skill_prompt.is_empty() {
                let new_prompt = match self.system_prompt.take() {
                    Some(existing) => format!("{}\n\n---\n\n{}", existing, skill_prompt),
                    None => skill_prompt,
                };
                ab = ab.system_prompt(new_prompt);
            }
        }

        let detail_tool = SkillDetailTool::new(skill_refs, self.skill_detail_tool_name);
        ab = ab.register_tool(detail_tool);

        ab.build()
    }
}
