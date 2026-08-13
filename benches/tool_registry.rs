//! Benchmarks: ToolRegistry operations (register, find, metadatas, remove).

use agent_base::tool::ToolRegistry;
use agent_base::{AgentResult, Content, Tool, ToolContext, ToolMetadata};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

/// A no-op tool for benchmarking.
#[derive(Clone)]
struct NoopTool {
    name: &'static str,
    description: &'static str,
}

impl NoopTool {
    fn new_named(name: &'static str) -> Self {
        Self {
            name,
            description: "No-op benchmark tool",
        }
    }
}

#[async_trait::async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }
    async fn call(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> AgentResult<Vec<Content>> {
        Ok(vec![Content::text("ok")])
    }
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: self.name.to_string(),
            description: self.description.to_string(),
            origin: "bench".into(),
            version: "0.0.0".into(),
            requirements: vec![],
        }
    }
}

fn bench_registry_register(c: &mut Criterion) {
    let mut registry = ToolRegistry::default();
    let tool = NoopTool::new_named("test_tool");

    c.bench_function("registry/register_1", |b| {
        b.iter(|| {
            registry.register(tool.clone());
            // clean up after each iter
            registry.remove("test_tool");
        });
    });
}

fn bench_registry_metadatas(c: &mut Criterion) {
    let mut registry = ToolRegistry::default();
    // Register 50 tools
    let tools: Vec<NoopTool> = (0..50)
        .map(|i| NoopTool::new_named(Box::leak(format!("tool_{:03}", i).into_boxed_str())))
        .collect();
    for t in &tools {
        registry.register(t.clone());
    }

    c.bench_function("registry/metadatas_50", |b| {
        b.iter(|| {
            let m = registry.metadatas();
            black_box(m);
        });
    });
}

fn bench_registry_remove(c: &mut Criterion) {
    c.bench_function("registry/remove", |b| {
        b.iter(|| {
            let mut registry = ToolRegistry::default();
            let tool = NoopTool::new_named("tmp");
            registry.register(tool);
            registry.remove("tmp");
            black_box(());
        });
    });
}

criterion_group! {
    name = tool_registry_benches;
    config = Criterion::default().sample_size(500);
    targets = bench_registry_register, bench_registry_metadatas, bench_registry_remove
}
criterion_main!(tool_registry_benches);
