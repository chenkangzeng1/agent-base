//! Benchmarks: session creation, validation, and path operations.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use phi_agent::session::{resolve_session, validate_session_id};
use tempfile::TempDir;

fn bench_resolve_session(c: &mut Criterion) {
    let temp = TempDir::new().unwrap();
    let base = temp.path().to_path_buf();
    // Pre-create and drop to release the file lock
    let _ctx = resolve_session(Some("bench-001"), &base).unwrap();
    drop(_ctx);

    c.bench_function("session/resolve_existing", |b| {
        b.iter(|| {
            let ctx = resolve_session(Some("bench-001"), &base).unwrap();
            black_box(ctx);
        });
    });
}

fn bench_resolve_new_session(c: &mut Criterion) {
    let temp = TempDir::new().unwrap();
    let base = temp.path().to_path_buf();
    let mut counter = 0u64;

    // Each iter creates a new session with a unique ID, acquires an fs2 file
    // lock, and releases it on drop(black_box) — no contention within the bench.
    c.bench_function("session/resolve_new", |b| {
        b.iter(|| {
            counter += 1;
            let id = format!("bench-{:08}", counter);
            let ctx = resolve_session(Some(&id), &base).unwrap();
            black_box(ctx);
        });
    });
}

fn bench_validate_session_id(c: &mut Criterion) {
    c.bench_function("session/validate_valid", |b| {
        b.iter(|| {
            let _ = black_box(validate_session_id("session-abc-123"));
        });
    });

    c.bench_function("session/validate_invalid", |b| {
        b.iter(|| {
            let _ = black_box(validate_session_id("../../etc/passwd"));
        });
    });
}

criterion_group! {
    name = session_benches;
    config = Criterion::default().sample_size(200);
    targets = bench_resolve_session, bench_resolve_new_session, bench_validate_session_id
}
criterion_main!(session_benches);
