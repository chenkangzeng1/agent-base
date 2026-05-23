use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    En,
    Zh,
}

impl Default for Language {
    fn default() -> Self {
        Language::En
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::En => write!(f, "en"),
            Language::Zh => write!(f, "zh"),
        }
    }
}

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

use crate::llm::ReasoningConfig;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
    pub enable_thought: bool,
    pub reasoning: Option<ReasoningConfig>,
    pub language: Language,
    pub execution: ExecutionConfig,
    pub llm: LlmConfig,
    pub tool: ToolConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            enable_thought: false,
            reasoning: None,
            language: Language::default(),
            execution: ExecutionConfig::default(),
            llm: LlmConfig::default(),
            tool: ToolConfig::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionConfig {
    pub max_turns: Option<u32>,
    pub approval_timeout_ms: Option<u64>,
    pub fail_on_persist_error: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            approval_timeout_ms: None,
            fail_on_persist_error: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub response_format: Option<ResponseFormat>,
    pub llm_retry: Option<RetryConfig>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            response_format: None,
            llm_retry: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolConfig {
    pub tool_timeout_ms: Option<u64>,
    pub max_tool_output_chars: Option<usize>,
    pub tool_error_retry_prompt: Option<String>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            tool_timeout_ms: None,
            max_tool_output_chars: None,
            tool_error_retry_prompt: None,
        }
    }
}
