#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
    pub enable_thought: bool,
    pub enable_thinking: Option<bool>,
    pub max_turns: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            enable_thought: false,
            enable_thinking: None,
            max_turns: None,
        }
    }
}
