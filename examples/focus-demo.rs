//! Focus Demo — demonstrate structured single-purpose LLM calls.
//!
//! This example shows two Focus usage patterns:
//! 1. Simple string input — classify a single piece of text
//! 2. Structured Context input — send multiple labeled fields
//!
//! Run with:
//! ```bash
//! LLM_API_KEY=your-key cargo run --example focus-demo
//! ```

use std::sync::Arc;
use std::time::Duration;

use phi_agent::{Focus, FocusContext, FocusError, OpenAiClient};
use serde::Deserialize;

// ── Output types ────────────────────────────────────────────────────────────

/// Expected response from the sentiment classifier.
#[derive(Deserialize, Debug)]
struct SentimentResult {
    sentiment: String,
    confidence: f64,
}

/// Expected response from the task status judge.
#[derive(Deserialize, Debug)]
struct TaskStatus {
    status: String,
    suggestion: String,
}

// ── Example 1: Simple string input ──────────────────────────────────────────

/// Classify sentiment from a single text string.
async fn classify_sentiment(client: Arc<OpenAiClient>, text: String) -> Result<SentimentResult, FocusError> {
    let focus = Focus::new(
        client,
        "You are a sentiment classifier. Analyze the given text and return JSON: \
         {\"sentiment\": \"<positive|negative|neutral>\", \"confidence\": <0.0-1.0>}. \
         Only return JSON, no other text.",
    );

    let output = focus.ask::<SentimentResult>(&text, Duration::from_secs(10)).await?;

    println!("  Raw response: {}", output.raw_response);
    Ok(output.result)
}

// ── Example 2: Structured Context input ─────────────────────────────────────

/// Judge whether a terminal command completed successfully,
/// using multiple context fields.
async fn judge_task(
    client: Arc<OpenAiClient>,
    command: &str,
    elapsed: &str,
    screen_output: &str,
) -> Result<TaskStatus, FocusError> {
    let focus = Focus::new(
        client,
        "You are a terminal task status judge. Based on the command, elapsed time, \
         and screen output, determine whether the task completed successfully. \
         Return JSON: {\"status\": \"<success|running|error>\", \
         \"suggestion\": \"<next action to take>\"}. Only return JSON.",
    );

    let ctx = FocusContext::new().add("command", command).add("elapsed", elapsed).add("screen", screen_output);

    let output = focus.ask::<TaskStatus>(&ctx, Duration::from_secs(10)).await?;

    println!("  Raw response: {}", output.raw_response);
    Ok(output.result)
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("LLM_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("Set LLM_API_KEY or OPENAI_API_KEY environment variable");
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "opus".into());
    let base_url = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());

    let client = Arc::new(OpenAiClient::new(api_key, model, Some(base_url)));

    println!("═══ Focus Demo ═══\n");

    // ── Demo 1: Simple string input ──
    println!("── Demo 1: Sentiment classification (string input) ──");
    let texts = vec![
        "I love this framework, it's so clean and fast!",
        "The build failed and I have no idea why.",
        "It's 3pm on a Tuesday.",
    ];

    for text in texts {
        println!("\n  Input: \"{}\"", text);
        match classify_sentiment(Arc::clone(&client), text.to_string()).await {
            Ok(result) => {
                println!("  Result: {:?}", result);
            },
            Err(e) => {
                println!("  Error: {}", e);
            },
        }
    }

    // ── Demo 2: Structured Context input ──
    println!("\n── Demo 2: Task status judgment (Context input) ──");

    let command = "cargo build --release";
    let elapsed = "45s";
    let screen = "Compiling phi-agent v0.1.0\nCompiling agent-base v0.1.0\n\
                  error[E0433]: failed to resolve: use of undeclared type `Foo`\n\
                  --> src/main.rs:10:5\n   |\n10 |     let x: Foo = ...\n   |              ^^^";

    println!("\n  Command: {}", command);
    println!("  Elapsed: {}", elapsed);
    match judge_task(Arc::clone(&client), command, elapsed, screen).await {
        Ok(status) => {
            println!("  Status: {:?}", status);
        },
        Err(e) => {
            println!("  Error: {}", e);
        },
    }

    println!("\n═══ Done ═══");
    Ok(())
}
