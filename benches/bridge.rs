//! Benchmarks: bridge protocol server overhead.

use agent_base::{AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ResponseFormat, StreamChunk};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use futures_core::Stream;
use phi_agent::bridge::server::ProtocolServer;
use phi_agent::{base_agent_builder, build_system_prompt};
use std::pin::Pin;
use std::sync::Arc;

/// Minimal mock LLM client.
struct BenchLlmClient;
#[async_trait::async_trait]
impl LlmClient for BenchLlmClient {
    async fn chat(
        &self,
        _: &[ChatMessage],
        _: &[serde_json::Value],
        _: Option<&ReasoningConfig>,
        _: Option<&ResponseFormat>,
    ) -> AgentResult<serde_json::Value> {
        Ok(serde_json::json!({}))
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
        _: &[serde_json::Value],
        _: Option<&ReasoningConfig>,
        _: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        struct EmptyStream;
        impl Stream for EmptyStream {
            type Item = AgentResult<StreamChunk>;
            fn poll_next(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
                std::task::Poll::Ready(None)
            }
        }
        Ok(Box::pin(EmptyStream))
    }
    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_thinking: false,
            supports_streaming: false,
            supports_tools: true,
            supports_vision: false,
            max_context_tokens: Some(4096),
            max_output_tokens: Some(4096),
        }
    }
}

fn bench_build_server(c: &mut Criterion) {
    let client = agent_base::llm::adapt(Arc::new(BenchLlmClient));
    let prompt = build_system_prompt();

    c.bench_function("bridge/build_from_builder", |b| {
        b.iter(|| {
            let builder = base_agent_builder(client.clone()).system_prompt(prompt.clone());
            let server = ProtocolServer::from_builder(builder).unwrap();
            black_box(server);
        });
    });
}

fn bench_create_session(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = agent_base::llm::adapt(Arc::new(BenchLlmClient));
    let builder = base_agent_builder(client).system_prompt(build_system_prompt());
    let server = ProtocolServer::from_builder(builder).unwrap();
    let mut counter = 0u64;

    c.bench_function("bridge/get_or_create_session", |b| {
        b.iter(|| {
            counter += 1;
            let sid = rt.block_on(server.get_or_create_session(Some(format!("ext-{}", counter))));
            black_box(sid);
        });
    });
}

criterion_group! {
    name = bridge_benches;
    config = Criterion::default().sample_size(200);
    targets = bench_build_server, bench_create_session
}
criterion_main!(bridge_benches);
