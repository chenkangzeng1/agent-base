/// 对比 qwen-flash / qwen-plus / qwen3.7-max 在 thinking 模式下的行为差异。
///
/// 重点验证：模型思考完后是否产生 text content，还是只有 reasoning_content。
/// 这决定了 react 是否会把响应当作"空响应"而无限重试。
///
/// 运行：cargo run --example qwen_model_compare
use agent_base::{AgentResult, ChatMessage};
use futures_util::StreamExt;
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

/// 单次测试结果
#[allow(dead_code)]
struct TestResult {
    thought_chars: usize,
    text_chars: usize,
    tool_calls: usize,
    has_stop: bool,
    elapsed_ms: u128,
}

/// 对单个模型执行一次流式请求，收集 thinking / text / tool_call 数据。
async fn probe_model(
    provider: &dyn LlmProvider,
    model_label: &str,
    system_prompt: &str,
    user_input: &str,
    enable_thinking: bool,
    thinking_budget: Option<u64>,
) -> AgentResult<TestResult> {
    println!("\n{}", "=".repeat(70));
    println!(
        "{} — thinking={}, budget={:?}",
        model_label, enable_thinking, thinking_budget
    );
    println!("{}", "=".repeat(70));

    let messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_input),
    ];

    let tools: Vec<serde_json::Value> = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "执行 shell 命令",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "要执行的命令"
                    }
                },
                "required": ["command"]
            }
        }
    })];

    let reasoning = ReasoningConfig {
        enabled: Some(enable_thinking),
        budget_tokens: thinking_budget,
        effort: None,
    };

    let request = ChatRequest::new(messages).with_tools(tools).with_reasoning(reasoning);
    let start = std::time::Instant::now();
    let mut stream = provider.stream(request).await.map_err(|e| agent_base::AgentError::internal(e.to_string()))?;

    let mut thought_chars: usize = 0;
    let mut text_chars: usize = 0;
    let mut tool_calls: usize = 0;
    let mut has_stop = false;
    let mut thought_sample = String::new(); // 保留前 200 字符做样本
    let mut text_sample = String::new();

    while let Some(chunk) = stream.next().await {
        match chunk.map_err(|e| agent_base::AgentError::internal(e.to_string()))? {
            StreamChunk::Thought(t) => {
                thought_chars += t.len();
                if thought_sample.len() < 200 {
                    thought_sample.push_str(&t);
                }
            }
            StreamChunk::Text(t) => {
                text_chars += t.len();
                if text_sample.len() < 500 {
                    text_sample.push_str(&t);
                }
            }
            StreamChunk::ToolCall(_) => {
                tool_calls += 1;
            }
            StreamChunk::Stop { .. } => {
                has_stop = true;
            }
            StreamChunk::Usage(u) => {
                println!("  📊 {}", usage_summary(&u));
            }
            StreamChunk::ThinkingSignature(_) => {
                // Thinking signature — not needed for comparison
            }
            StreamChunk::Error(e) => {
                eprintln!("  ❌ Stream error: {}", e);
            }
        }
    }

    let elapsed_ms = start.elapsed().as_millis();

    // 打印摘要
    println!("  ⏱  {}ms", elapsed_ms);
    println!("  💭 思考: {} 字符", thought_chars);
    if !thought_sample.is_empty() {
        let preview: String = thought_sample.chars().take(150).collect();
        println!("     样本: {}...", preview);
    }
    println!("  📝 回复: {} 字符", text_chars);
    if !text_sample.is_empty() {
        let preview: String = text_sample.chars().take(200).collect();
        println!("     样本: {}...", preview);
    }
    println!("  🔧 工具调用: {}", tool_calls);
    println!("  🏁 Stop: {}", has_stop);

    // 关键判断
    if thought_chars > 0 && text_chars == 0 && tool_calls == 0 {
        println!("  ⚠️  问题：有思考但无正文无工具调用 → react 会当作空响应!");
    } else if text_chars > 0 {
        println!("  ✅ 正常：有正文输出");
    } else if tool_calls > 0 {
        println!("  ✅ 正常：有工具调用");
    }

    Ok(TestResult {
        thought_chars,
        text_chars,
        tool_calls,
        has_stop,
        elapsed_ms,
    })
}

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("DASHSCOPE_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .map_err(|_| agent_base::AgentError::internal("DASHSCOPE_API_KEY 未设置"))?;

    let base_url = std::env::var("DASHSCOPE_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .unwrap_or_else(|_| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());

    let system_prompt =
        "你是运维工程师助手。回复简洁直接。如果需要执行命令来回答用户问题，请调用工具。";
    let user_input = "看下磁盘空间";

    // 三个模型，分别测试 thinking=false 和 thinking=true
    let models = vec![
        ("qwen-flash", "千问 Flash"),
        ("qwen-plus", "千问 Plus"),
        ("qwen3.7-max", "千问 Max"),
    ];

    let mut all_results: Vec<(&str, Vec<TestResult>)> = Vec::new();

    for (model_id, model_label) in &models {
        let provider = create_provider(&llm_trait::LlmConfig {
            protocol: Some(llm_trait::Protocol::OpenAi),
            api_key: api_key.clone(),
            model: model_id.to_string(),
            base_url: base_url.clone(),
            options: std::collections::HashMap::new(),
        })
        .map_err(|e| agent_base::AgentError::internal(e.to_string()))?;

        let mut results = Vec::new();

        // 测试 A: 不开 thinking（对照组）
        results.push(
            probe_model(
                provider.as_ref(),
                &format!("{} (无思考)", model_label),
                system_prompt,
                user_input,
                false,
                None,
            )
            .await?,
        );

        // 测试 B: 开 thinking，不限 budget
        results.push(
            probe_model(
                provider.as_ref(),
                &format!("{} (thinking=on, 无budget)", model_label),
                system_prompt,
                user_input,
                true,
                None,
            )
            .await?,
        );

        // 测试 C: 开 thinking，budget=2000（和 ops-omni 实际配置一致）
        results.push(
            probe_model(
                provider.as_ref(),
                &format!("{} (thinking=on, budget=2000)", model_label),
                system_prompt,
                user_input,
                true,
                Some(2000),
            )
            .await?,
        );

        all_results.push((model_label, results));
    }

    // 汇总表格
    println!("\n\n{}", "=".repeat(90));
    println!("汇总对比");
    println!("{}", "=".repeat(90));
    println!(
        "{:<14} {:<32} {:>10} {:>10} {:>8} {:>8}",
        "模型", "配置", "思考(字符)", "回复(字符)", "工具调用", "耗时ms"
    );
    println!(
        "{:-<14} {:-<32} {:->10} {:->10} {:->8} {:->8}",
        "", "", "", "", "", ""
    );

    let configs = [
        "无思考",
        "thinking=on, 无budget",
        "thinking=on, budget=2000",
    ];
    for (label, results) in &all_results {
        for (i, r) in results.iter().enumerate() {
            let flag = if r.thought_chars > 0 && r.text_chars == 0 && r.tool_calls == 0 {
                " ⚠️"
            } else {
                ""
            };
            println!(
                "{:<14} {:<32} {:>10} {:>10} {:>8} {:>8}{}",
                label, configs[i], r.thought_chars, r.text_chars, r.tool_calls, r.elapsed_ms, flag
            );
        }
    }

    println!("\n⚠️  = 有思考但无正文无工具调用，react 会当作空响应无限重试");

    Ok(())
}
