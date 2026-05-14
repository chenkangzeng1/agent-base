use serde_json::Value;

#[derive(Clone, Debug)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub backoff_multiplier: f64,
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    pub fn initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    pub fn max_backoff_ms(mut self, ms: u64) -> Self {
        self.max_backoff_ms = ms;
        self
    }

    pub fn no_jitter(mut self) -> Self {
        self.jitter = false;
        self
    }
}

#[derive(Clone, Debug)]
pub enum ResponseFormat {
    JsonObject,
    JsonSchema {
        name: String,
        schema: Value,
    },
}

impl ResponseFormat {
    pub fn to_api_value(&self) -> Value {
        match self {
            ResponseFormat::JsonObject => {
                serde_json::json!({ "type": "json_object" })
            }
            ResponseFormat::JsonSchema { name, schema } => {
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": name,
                        "schema": schema,
                    }
                })
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
    pub enable_thought: bool,
    pub enable_thinking: Option<bool>,
    pub max_turns: Option<u32>,
    pub tool_timeout_ms: Option<u64>,
    pub max_tool_output_chars: Option<usize>,
    pub response_format: Option<ResponseFormat>,
    pub llm_retry: Option<RetryConfig>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            enable_thought: false,
            enable_thinking: None,
            max_turns: None,
            tool_timeout_ms: None,
            max_tool_output_chars: None,
            response_format: None,
            llm_retry: None,
        }
    }
}
