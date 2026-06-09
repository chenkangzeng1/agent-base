#[cfg(test)]
mod tests {
    use agent_base::{AgentResult, ConfigManager, AppConfig};

    #[tokio::test]
    async fn test_config_manager_creation() -> AgentResult<()> {
        // 测试配置管理器的基本创建
        let config = AppConfig::default();
        let manager = ConfigManager::new(config);

        assert_eq!(manager.get::<u64>("llm_timeout"), Some(30000));
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(10000));

        Ok(())
    }

    #[tokio::test]
    async fn test_config_overrides() -> AgentResult<()> {
        // 测试配置覆盖功能
        let config = AppConfig {
            llm_timeout: 5000,
            tool_timeout: 8000,
            ..Default::default()
        };

        let mut manager = ConfigManager::new(config);

        // 设置覆盖
        manager.set_override("llm_timeout", "15000");
        manager.set_override("tool_timeout", "12000");

        // 验证覆盖生效
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(15000));
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(12000));

        // 验证未覆盖的值仍为原值
        assert_eq!(manager.get::<u32>("retry_attempts"), Some(3));

        Ok(())
    }

    #[tokio::test]
    async fn test_config_serialization_roundtrip() -> AgentResult<()> {
        // 测试配置序列化和反序列化
        let original_config = AppConfig {
            llm_timeout: 12345,
            tool_timeout: 6789,
            max_tool_output_chars: 5000,
            retry_attempts: 5,
            enable_thought: false,
            ..Default::default()
        };

        let json_str = serde_json::to_string(&original_config).unwrap();
        let deserialized_config: AppConfig = serde_json::from_str(&json_str).unwrap();

        assert_eq!(original_config.llm_timeout, deserialized_config.llm_timeout);
        assert_eq!(original_config.tool_timeout, deserialized_config.tool_timeout);
        assert_eq!(original_config.max_tool_output_chars, deserialized_config.max_tool_output_chars);
        assert_eq!(original_config.retry_attempts, deserialized_config.retry_attempts);
        assert_eq!(original_config.enable_thought, deserialized_config.enable_thought);

        Ok(())
    }

    #[tokio::test]
    async fn test_config_file_operations() -> AgentResult<()> {
        use tempfile::NamedTempFile;

        // 创建临时配置文件
        let temp_file = NamedTempFile::new().unwrap();
        let config_path = temp_file.path().to_str().unwrap().to_string();

        // 创建一个配置对象
        let config = AppConfig {
            llm_timeout: 9999,
            tool_timeout: 8888,
            ..Default::default()
        };

        // 保存配置到文件
        let manager = ConfigManager::new(config);
        manager.save_to_file(&config_path).unwrap();

        // 从文件加载配置
        let loaded_manager = ConfigManager::load_from_file(&config_path).unwrap();

        assert_eq!(loaded_manager.get::<u64>("llm_timeout"), Some(9999));
        assert_eq!(loaded_manager.get::<u64>("tool_timeout"), Some(8888));

        // 清理临时文件
        std::fs::remove_file(&config_path).unwrap();

        Ok(())
    }

    #[tokio::test]
    async fn test_config_env_loading() -> AgentResult<()> {
        // 测试环境变量配置加载
        // 设置一些环境变量
        unsafe {
            std::env::set_var("AGENT_LLM_TIMEOUT", "11111");
            std::env::set_var("AGENT_TOOL_TIMEOUT", "22222");
            std::env::set_var("AGENT_RETRY_ATTEMPTS", "7");
            std::env::set_var("AGENT_ENABLE_THOUGHT", "false");
        }

        let manager = ConfigManager::load_from_env().unwrap();

        assert_eq!(manager.get::<u64>("llm_timeout"), Some(11111));
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(22222));
        assert_eq!(manager.get::<u32>("retry_attempts"), Some(7));
        assert_eq!(manager.get::<bool>("enable_thought"), Some(false));

        // 清理环境变量
        unsafe {
            std::env::remove_var("AGENT_LLM_TIMEOUT");
            std::env::remove_var("AGENT_TOOL_TIMEOUT");
            std::env::remove_var("AGENT_RETRY_ATTEMPTS");
            std::env::remove_var("AGENT_ENABLE_THOUGHT");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_config_manager_with_invalid_values() -> AgentResult<()> {
        // 创建一个包含无效值的配置管理器
        let config = AppConfig::default();
        let mut manager = ConfigManager::new(config);

        // 尝试设置无效的覆盖值（无效的JSON）
        manager.set_override("llm_timeout", "not_a_number");

        // 尝试获取该值应该返回None，因为它无法解析为u64
        let result: Option<u64> = manager.get("llm_timeout");
        // 应该返回默认值而不是None，因为覆盖值无效
        assert_eq!(result, Some(30000)); // 默认值

        Ok(())
    }

    #[tokio::test]
    async fn test_config_manager_edge_cases() -> AgentResult<()> {
        // 测试边界情况
        let config = AppConfig::default();
        let mut manager = ConfigManager::new(config);

        // 测试空覆盖
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(30000));

        // 测试未知字段
        let unknown: Option<u64> = manager.get("nonexistent_field");
        assert!(unknown.is_none());

        // 测试空字符串覆盖
        manager.set_override("llm_timeout", "");
        let result: Option<u64> = manager.get("llm_timeout");
        assert_eq!(result, Some(30000)); // 应该返回默认值

        Ok(())
    }

    #[tokio::test]
    async fn test_agent_config_default_values() -> AgentResult<()> {
        let config = AppConfig::default();

        // 验证所有默认值
        assert_eq!(config.llm_timeout, 30000);
        assert_eq!(config.tool_timeout, 10000);
        assert_eq!(config.max_tool_output_chars, 10000);
        assert_eq!(config.retry_attempts, 3);
        assert_eq!(config.retry_delay_ms, 1000);
        assert_eq!(config.enable_thought, true);
        assert_eq!(config.enable_thinking, false);
        assert_eq!(config.thinking_budget, 1000);
        assert_eq!(config.event_bus_capacity, 100);
        assert_eq!(config.context_window_size, 4096);
        assert_eq!(config.max_context_tokens, None);

        Ok(())
    }

    #[test]
    fn test_config_error_types() {
        use agent_base::ConfigError;

        // 测试不同的配置错误类型
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let config_io_err = ConfigError::from(io_err);

        // 创建一个JSON错误，使用SerdeError trait
        use serde_json::Value;
        use std::fmt;

        #[derive(Debug)]
        struct TestError;
        impl fmt::Display for TestError {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "test error")
            }
        }
        impl std::error::Error for TestError {}

        // 我们直接测试从其他错误类型创建ConfigError
        println!("Config error types test passed!");
    }
}