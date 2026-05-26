use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 配置源枚举
#[derive(Debug, Clone)]
pub enum ConfigSource {
    File(String),
    Env,
    Memory(HashMap<String, String>),
}

/// 配置错误类型
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid configuration value: {0}")]
    InvalidValue(String),
}

/// 基础配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub llm_timeout: u64,           // LLM 调用超时时间（毫秒）
    pub tool_timeout: u64,          // 工具调用超时时间（毫秒）
    pub max_tool_output_chars: usize, // 最大工具输出字符数
    pub max_context_tokens: Option<usize>, // 最大上下文窗口
    pub retry_attempts: u32,         // 重试次数
    pub retry_delay_ms: u64,        // 重试间隔（毫秒）
    pub enable_thought: bool,       // 是否启用思考过程
    pub enable_thinking: bool,      // 是否启用思考预算
    pub thinking_budget: u64,       // 思考预算（token）
    pub event_bus_capacity: usize,  // 事件总线容量
    pub context_window_size: usize, // 上下文窗口大小
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            llm_timeout: 30000,              // 30秒
            tool_timeout: 10000,             // 10秒
            max_tool_output_chars: 10000,    // 10K字符
            max_context_tokens: None,        // 无限制
            retry_attempts: 3,               // 3次重试
            retry_delay_ms: 1000,            // 1秒延迟
            enable_thought: true,            // 默认启用思考
            enable_thinking: false,          // 默认不启用思考预算
            thinking_budget: 1000,           // 1K思考token预算
            event_bus_capacity: 100,         // 100个事件
            context_window_size: 4096,       // 4K上下文窗口
        }
    }
}

/// 配置管理器
#[derive(Debug, Clone)]
pub struct ConfigManager {
    config: AgentConfig,
    overrides: HashMap<String, String>,
}

impl ConfigManager {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            overrides: HashMap::new(),
        }
    }

    /// 获取当前配置
    pub fn get_config(&self) -> &AgentConfig {
        &self.config
    }

    /// 设置配置覆盖
    pub fn set_override(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.overrides.insert(key.into(), value.into());
    }

    /// 获取配置值（优先使用覆盖值）
    pub fn get<T>(&self, key: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Default,
    {
        // 首先检查覆盖值
        if let Some(override_val) = self.overrides.get(key) {
            if let Ok(parsed) = serde_json::from_str::<T>(override_val) {
                return Some(parsed);
            }
        }

        // 否则返回原始配置值，将其转换为所需类型
        match key {
            "llm_timeout" => serde_json::from_value(serde_json::json!(self.config.llm_timeout)).ok(),
            "tool_timeout" => serde_json::from_value(serde_json::json!(self.config.tool_timeout)).ok(),
            "max_tool_output_chars" => serde_json::from_value(serde_json::json!(self.config.max_tool_output_chars)).ok(),
            "max_context_tokens" => serde_json::from_value(serde_json::json!(self.config.max_context_tokens)).ok(),
            "retry_attempts" => serde_json::from_value(serde_json::json!(self.config.retry_attempts)).ok(),
            "retry_delay_ms" => serde_json::from_value(serde_json::json!(self.config.retry_delay_ms)).ok(),
            "enable_thought" => serde_json::from_value(serde_json::json!(self.config.enable_thought)).ok(),
            "enable_thinking" => serde_json::from_value(serde_json::json!(self.config.enable_thinking)).ok(),
            "thinking_budget" => serde_json::from_value(serde_json::json!(self.config.thinking_budget)).ok(),
            "event_bus_capacity" => serde_json::from_value(serde_json::json!(self.config.event_bus_capacity)).ok(),
            "context_window_size" => serde_json::from_value(serde_json::json!(self.config.context_window_size)).ok(),
            _ => None,
        }
    }

    /// 加载配置文件
    pub fn load_from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: AgentConfig = serde_json::from_str(&content)?;
        Ok(Self::new(config))
    }

    /// 保存配置到文件
    pub fn save_to_file(&self, path: &str) -> Result<(), ConfigError> {
        let content = serde_json::to_string_pretty(&self.config)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 从环境变量加载配置
    pub fn load_from_env() -> Result<Self, ConfigError> {
        let mut config = AgentConfig::default();

        if let Ok(val) = std::env::var("AGENT_LLM_TIMEOUT") {
            if let Ok(parsed) = val.parse::<u64>() {
                config.llm_timeout = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_TOOL_TIMEOUT") {
            if let Ok(parsed) = val.parse::<u64>() {
                config.tool_timeout = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_MAX_TOOL_OUTPUT_CHARS") {
            if let Ok(parsed) = val.parse::<usize>() {
                config.max_tool_output_chars = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_RETRY_ATTEMPTS") {
            if let Ok(parsed) = val.parse::<u32>() {
                config.retry_attempts = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_RETRY_DELAY_MS") {
            if let Ok(parsed) = val.parse::<u64>() {
                config.retry_delay_ms = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_ENABLE_THOUGHT") {
            if let Ok(parsed) = val.parse::<bool>() {
                config.enable_thought = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_ENABLE_THINKING") {
            if let Ok(parsed) = val.parse::<bool>() {
                config.enable_thinking = parsed;
            }
        }

        if let Ok(val) = std::env::var("AGENT_THINKING_BUDGET") {
            if let Ok(parsed) = val.parse::<u64>() {
                config.thinking_budget = parsed;
            }
        }

        Ok(Self::new(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AgentConfig::default();

        assert_eq!(config.llm_timeout, 30000);
        assert_eq!(config.tool_timeout, 10000);
        assert_eq!(config.max_tool_output_chars, 10000);
        assert_eq!(config.retry_attempts, 3);
        assert_eq!(config.enable_thought, true);
        assert_eq!(config.context_window_size, 4096);
    }

    #[test]
    fn test_config_manager_basics() {
        let config = AgentConfig {
            llm_timeout: 5000,
            tool_timeout: 8000,
            ..Default::default()
        };

        let manager = ConfigManager::new(config);

        assert_eq!(manager.get::<u64>("llm_timeout"), Some(5000));
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(8000));
        assert_eq!(manager.get::<bool>("enable_thought"), Some(true));
    }

    #[test]
    fn test_config_overrides() {
        let config = AgentConfig {
            llm_timeout: 5000,
            ..Default::default()
        };

        let mut manager = ConfigManager::new(config);
        manager.set_override("llm_timeout", "15000");

        // 应该返回覆盖值
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(15000));
    }

    #[test]
    fn test_missing_config_field() {
        let manager = ConfigManager::new(AgentConfig::default());

        // 无效的配置键应该返回 None
        let result: Option<u64> = manager.get("invalid_key");
        assert!(result.is_none());
    }

    #[test]
    fn test_config_serialization() {
        let original_config = AgentConfig::default();
        let serialized = serde_json::to_string(&original_config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original_config.llm_timeout, deserialized.llm_timeout);
        assert_eq!(original_config.tool_timeout, deserialized.tool_timeout);
    }
}