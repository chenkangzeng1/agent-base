#[cfg(test)]
mod tests {
    use agent_base::{AgentResult, AgentConfig, ConfigManager, AgentError, ConfigError};

    #[tokio::test]
    async fn test_enhanced_error_types() {
        // 测试第一阶段的错误处理改进

        // 测试新增的错误类型
        let rate_limit_err = AgentError::rate_limit_exceeded();
        assert!(rate_limit_err.is_rate_limited());
        assert!(rate_limit_err.is_retryable());

        let service_unavailable_err = AgentError::service_unavailable("temporarily down".to_string());
        assert!(service_unavailable_err.is_retryable());

        let resource_unavailable_err = AgentError::resource_unavailable("connections exhausted".to_string());
        assert!(resource_unavailable_err.is_resource_unavailable());

        let config_err = AgentError::config_error("invalid setting".to_string());
        assert_eq!(format!("{}", config_err), "Configuration error: invalid setting");

        let tool_timeout_err = AgentError::tool_timeout();
        assert_eq!(format!("{}", tool_timeout_err), "Tool timeout exceeded");

        println!("✓ Enhanced error types work correctly");
    }

    #[tokio::test]
    async fn test_config_manager_functionality() {
        // 测试第二阶段的配置管理器

        // 创建默认配置
        let config = AgentConfig::default();
        let mut manager = ConfigManager::new(config);

        // 验证默认值
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(30000));
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(10000));
        assert_eq!(manager.get::<bool>("enable_thought"), Some(true));

        // 测试覆盖功能
        manager.set_override("llm_timeout", "45000");
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(45000));

        // 验证其他值未受影响
        assert_eq!(manager.get::<u64>("tool_timeout"), Some(10000));

        // 测试无效覆盖（应返回默认值）
        manager.set_override("llm_timeout", "not_a_number");
        assert_eq!(manager.get::<u64>("llm_timeout"), Some(30000)); // 回退到默认值

        println!("✓ Config manager functionality works correctly");
    }

    #[tokio::test]
    async fn test_config_file_operations() {
        use tempfile::NamedTempFile;

        // 测试配置文件操作
        let temp_file = NamedTempFile::new().unwrap();
        let config_path = temp_file.path().to_str().unwrap().to_string();

        // 创建一个带有自定义值的配置
        let config = AgentConfig {
            llm_timeout: 25000,
            tool_timeout: 15000,
            max_tool_output_chars: 20000,
            retry_attempts: 5,
            enable_thought: false,
            ..Default::default()
        };

        // 保存配置到文件
        let manager = ConfigManager::new(config);
        let save_result = manager.save_to_file(&config_path);
        assert!(save_result.is_ok());

        // 从文件加载配置
        let load_result = ConfigManager::load_from_file(&config_path);
        assert!(load_result.is_ok());

        if let Ok(loaded_manager) = load_result {
            // 验证加载的配置值
            assert_eq!(loaded_manager.get::<u64>("llm_timeout"), Some(25000));
            assert_eq!(loaded_manager.get::<u64>("tool_timeout"), Some(15000));
            assert_eq!(loaded_manager.get::<usize>("max_tool_output_chars"), Some(20000));
            assert_eq!(loaded_manager.get::<u32>("retry_attempts"), Some(5));
            assert_eq!(loaded_manager.get::<bool>("enable_thought"), Some(false));
        }

        // 清理临时文件
        std::fs::remove_file(&config_path).unwrap();

        println!("✓ Config file operations work correctly");
    }

    #[tokio::test]
    async fn test_config_environment_loading() {
        // 测试环境变量配置加载
        unsafe {
            std::env::set_var("AGENT_LLM_TIMEOUT", "35000");
            std::env::set_var("AGENT_TOOL_TIMEOUT", "12000");
            std::env::set_var("AGENT_RETRY_ATTEMPTS", "7");
            std::env::set_var("AGENT_ENABLE_THOUGHT", "false");
        }

        let load_result = ConfigManager::load_from_env();
        assert!(load_result.is_ok());

        if let Ok(manager) = load_result {
            assert_eq!(manager.get::<u64>("llm_timeout"), Some(35000));
            assert_eq!(manager.get::<u64>("tool_timeout"), Some(12000));
            assert_eq!(manager.get::<u32>("retry_attempts"), Some(7));
            assert_eq!(manager.get::<bool>("enable_thought"), Some(false));
        }

        // 清理环境变量
        unsafe {
            std::env::remove_var("AGENT_LLM_TIMEOUT");
            std::env::remove_var("AGENT_TOOL_TIMEOUT");
            std::env::remove_var("AGENT_RETRY_ATTEMPTS");
            std::env::remove_var("AGENT_ENABLE_THOUGHT");
        }

        println!("✓ Config environment loading works correctly");
    }

    #[tokio::test]
    async fn test_config_serialization() {
        // 测试配置序列化和反序列化

        // 原始配置
        let original_config = AgentConfig {
            llm_timeout: 18000,
            tool_timeout: 9000,
            max_tool_output_chars: 15000,
            retry_attempts: 2,
            retry_delay_ms: 2000,
            enable_thought: false,
            enable_thinking: true,
            thinking_budget: 2000,
            event_bus_capacity: 200,
            context_window_size: 8192,
            max_context_tokens: Some(32000),
        };

        // 序列化到JSON
        let json_str = serde_json::to_string(&original_config).unwrap();
        // 从JSON反序列化
        let deserialized_config: AgentConfig = serde_json::from_str(&json_str).unwrap();

        // 验证值一致
        assert_eq!(original_config.llm_timeout, deserialized_config.llm_timeout);
        assert_eq!(original_config.tool_timeout, deserialized_config.tool_timeout);
        assert_eq!(original_config.max_tool_output_chars, deserialized_config.max_tool_output_chars);
        assert_eq!(original_config.retry_attempts, deserialized_config.retry_attempts);
        assert_eq!(original_config.retry_delay_ms, deserialized_config.retry_delay_ms);
        assert_eq!(original_config.enable_thought, deserialized_config.enable_thought);
        assert_eq!(original_config.enable_thinking, deserialized_config.enable_thinking);
        assert_eq!(original_config.thinking_budget, deserialized_config.thinking_budget);
        assert_eq!(original_config.event_bus_capacity, deserialized_config.event_bus_capacity);
        assert_eq!(original_config.context_window_size, deserialized_config.context_window_size);
        assert_eq!(original_config.max_context_tokens, deserialized_config.max_context_tokens);

        println!("✓ Config serialization works correctly");
    }

    #[test]
    fn test_error_categorization() {
        // 测试错误分类功能

        let llm_err = AgentError::llm("network error".to_string());
        assert!(llm_err.is_retryable());

        let json_err = AgentError::json("parse error".to_string());
        assert!(!json_err.is_retryable()); // JSON 解析错误通常是永久性的

        let tool_not_found_err = AgentError::tool_not_found("missing_tool".to_string());
        assert!(!tool_not_found_err.is_retryable()); // 工具不存在是永久性错误

        let cancelled_err = AgentError::Cancelled;
        assert!(cancelled_err.is_cancelled());

        // 测试新添加的错误类型
        let service_unavailable_err = AgentError::service_unavailable("server down".to_string());
        assert!(service_unavailable_err.is_retryable());

        let rate_limit_err = AgentError::rate_limit_exceeded();
        assert!(rate_limit_err.is_rate_limited());
        assert!(rate_limit_err.is_retryable());

        println!("✓ Error categorization works correctly");
    }
}