//! Benchmarks: system prompt generation.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use phi_agent::{build_system_prompt, build_system_prompt_cn};

fn bench_system_prompt_en(c: &mut Criterion) {
    c.bench_function("system_prompt/build_en", |b| {
        b.iter(|| {
            black_box(build_system_prompt());
        });
    });
}

fn bench_system_prompt_cn(c: &mut Criterion) {
    c.bench_function("system_prompt/build_cn", |b| {
        b.iter(|| {
            black_box(build_system_prompt_cn());
        });
    });
}

criterion_group! {
    name = system_prompt_benches;
    config = Criterion::default().sample_size(200);
    targets = bench_system_prompt_en, bench_system_prompt_cn
}
criterion_main!(system_prompt_benches);
