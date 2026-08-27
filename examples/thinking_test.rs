use agent_base::{AgentResult, ChatMessage};
use llm_trait::{ChatRequest, LlmProvider, ReasoningConfig, StreamChunk};
use llm_unified::create_provider;

fn usage_summary(usage: &llm_trait::UsageInfo) -> String {
    format!(
        "prompt={}, completion={}, total={}",
        usage.prompt_tokens.unwrap_or(0),
        usage.completion_tokens.unwrap_or(0),
        usage.total_tokens.unwrap_or(0),
    )
}

async fn run_test(
    provider: &dyn LlmProvider,
    label: &str,
    system_prompt: &str,
    user_input: &str,
    enable_thinking: Option<bool>,
    thinking_budget: Option<u64>,
) -> AgentResult<(usize, usize)> {
    println!("\n{}", "=".repeat(60));
    println!("{}", label);
    println!("{}", "=".repeat(60));

    let messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_input),
    ];

    let reasoning = ReasoningConfig {
        enabled: enable_thinking,
        budget_tokens: thinking_budget,
        effort: None,
    };

    let request = ChatRequest::new(messages).with_reasoning(reasoning);
    let mut stream = provider
        .stream(request)
        .await
        .map_err(|e| agent_base::AgentError::internal(e.to_string()))?;

    let mut in_thought = false;
    let mut in_text = false;
    let mut thought_len = 0;
    let mut text_len = 0;

    while let Some(chunk) = stream.next().await {
        match chunk.map_err(|e| agent_base::AgentError::internal(e.to_string()))? {
            StreamChunk::Thought(text) => {
                if !in_thought {
                    print!("\n💭 [思考过程]: ");
                    in_thought = true;
                }
                thought_len += text.len();
                print!("{}", text);
            }
            StreamChunk::Text(text) => {
                if in_thought && !in_text {
                    print!("\n\n📝 [回复内容]: ");
                    in_text = true;
                } else if !in_text {
                    print!("📝 [回复内容]: ");
                    in_text = true;
                }
                text_len += text.len();
                print!("{}", text);
            }
            StreamChunk::Stop { .. } => {
                println!("\n\n--- 流结束 ---");
            }
            StreamChunk::ToolCall(_) => {
                println!("\n🔧 [工具调用]");
            }
            StreamChunk::Usage(usage) => {
                println!("\n📊 Token 用量: {}", usage_summary(&usage));
            }
            StreamChunk::ThinkingSignature(_) => {
                // Thinking signature — not needed for this test
            }
            StreamChunk::Error(e) => {
                eprintln!("\n❌ Stream error: {}", e);
            }
        }
    }

    println!("思考内容: {} 字符", thought_len);
    println!("回复内容: {} 字符", text_len);

    Ok((thought_len, text_len))
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .map_err(|_| agent_base::AgentError::internal("OPENAI_API_KEY 未设置"))?;

    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "qwen-flash".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());

    println!("=== thinking_budget 在 {} 上的效果测试 ===", model);
    println!("enable_thinking 和 thinking_budget 都作为顶级参数传递（DashScope 正确用法）");
    println!();

    let provider = create_provider(&llm_trait::LlmConfig {
        protocol: Some(llm_trait::Protocol::OpenAi),
        api_key,
        model,
        base_url,
        options: std::collections::HashMap::new(),
    })
    .map_err(|e| agent_base::AgentError::internal(e.to_string()))?;

    let user_input = "看下磁盘空间";
    let system_prompt = "你是一个资深的服务器运维工程师助手，回复简洁直接，不要客套。";

    let mut results = Vec::new();

    // 测试1: enable_thinking=false（无思考，对照）
    results.push(
        run_test(
            provider.as_ref(),
            "测试1: enable_thinking=false",
            system_prompt,
            user_input,
            Some(false),
            None,
        )
        .await?,
    );

    // 测试2: enable_thinking=true, 无 budget
    results.push(
        run_test(
            provider.as_ref(),
            "测试2: enable_thinking=true, 无 thinking_budget",
            system_prompt,
            user_input,
            Some(true),
            None,
        )
        .await?,
    );

    // 测试3: enable_thinking=true, thinking_budget=128
    results.push(
        run_test(
            provider.as_ref(),
            "测试3: thinking_budget=128",
            system_prompt,
            user_input,
            Some(true),
            Some(128),
        )
        .await?,
    );

    // 测试4: enable_thinking=true, thinking_budget=50（极低）
    results.push(
        run_test(
            provider.as_ref(),
            "测试4: thinking_budget=50",
            system_prompt,
            user_input,
            Some(true),
            Some(50),
        )
        .await?,
    );

    // 测试5: enable_thinking=true, thinking_budget=10（极低）
    results.push(
        run_test(
            provider.as_ref(),
            "测试5: thinking_budget=10",
            system_prompt,
            user_input,
            Some(true),
            Some(10),
        )
        .await?,
    );

    println!("\n\n{}", "=".repeat(60));
    println!("结果汇总");
    println!("{}", "=".repeat(60));
    println!(
        "{:<6} {:<30} {:>12} {:>12}",
        "测试", "配置", "思考(字符)", "回复(字符)"
    );
    println!("{:-<6} {:-<30} {:->12} {:->12}", "", "", "", "");
    for (i, (t, r)) in results.iter().enumerate() {
        let label = match i {
            0 => "enable_thinking=false",
            1 => "thinking=true, no budget",
            2 => "thinking_budget=128",
            3 => "thinking_budget=50",
            4 => "thinking_budget=10",
            _ => "",
        };
        println!("{:<6} {:<30} {:>12} {:>12}", i + 1, label, t, r);
    }

    Ok(())
}
