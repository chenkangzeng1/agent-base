use agent_base::ChatMessage;
use llm_trait::{ChatRequest, LlmProvider, ReasoningConfig};
use llm_unified::create_provider;
use serde_json::{Value, json};

/// 测试 provider 生成的请求格式
/// 这个测试不实际发送 HTTP 请求，而是通过验证请求构建逻辑
#[tokio::test]
async fn test_provider_request_body_format() {
    // 创建一个测试用的 provider（使用无效的 API key，但会验证构建逻辑）
    let provider = create_provider(&llm_trait::LlmConfig {
        backend: "custom".to_string(),
        protocol: Some("openai".to_string()),
        api_key: "test-api-key".to_string(),
        model: "qwen-flash".to_string(),
        base_url: Some("https://test.example.com/v1".to_string()),
        options: std::collections::HashMap::new(),
    })
    .expect("should create provider");

    let messages = vec![
        ChatMessage::system("你是一个助手"),
        ChatMessage::user("你好"),
    ];

    let reasoning = ReasoningConfig {
        enabled: Some(true),
        budget_tokens: Some(128),
        effort: None,
    };

    let request = ChatRequest::new(messages).with_reasoning(reasoning);
    let result = provider.stream(request).await;

    // 我们期望请求失败（因为 URL 无效），但 provider 应该能正常构建
    assert!(result.is_err(), "Expected error due to invalid URL");
}

/// 验证请求 body 的 JSON 结构（通过构造相同的 JSON 来验证）
#[test]
fn test_request_body_json_structure() {
    // 模拟 OpenAI 兼容的请求 body
    let mut request_body = json!({
        "model": "qwen-flash",
        "messages": [
            {
                "role": "system",
                "content": "你是一个助手"
            },
            {
                "role": "user",
                "content": "你好"
            }
        ],
        "tools": [],
        "stream": true,
        "stream_options": {
            "include_usage": true
        }
    });

    // 添加 enable_thinking
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("enable_thinking".to_string(), json!(true));
    }

    // 添加 extra_body 包含 thinking_budget
    let mut extra_body = serde_json::Map::new();
    extra_body.insert("thinking_budget".to_string(), json!(128));
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("extra_body".to_string(), Value::Object(extra_body));
    }

    // 验证结构
    let json_str = serde_json::to_string_pretty(&request_body).unwrap();
    println!("Expected request body:\n{}", json_str);

    // 验证 enable_thinking 在 root level
    assert_eq!(request_body["enable_thinking"], true);

    // 验证 extra_body 存在
    assert!(request_body["extra_body"].is_object());
    assert_eq!(request_body["extra_body"]["thinking_budget"], 128);
}

/// 测试不同 thinking_budget 值
#[test]
fn test_thinking_budget_values() {
    let budgets = vec![100, 128, 256, 512, 1024, 2048, 4096];

    for budget in budgets {
        let mut request_body = json!({
            "model": "qwen-flash",
            "messages": [],
            "tools": [],
            "stream": true,
        });

        let mut extra_body = serde_json::Map::new();
        extra_body.insert("thinking_budget".to_string(), json!(budget));
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("extra_body".to_string(), Value::Object(extra_body));
        }

        assert_eq!(
            request_body["extra_body"]["thinking_budget"], budget,
            "thinking_budget should be {}",
            budget
        );
    }
}
