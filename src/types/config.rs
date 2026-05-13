#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { system_prompt: None }
    }
}
