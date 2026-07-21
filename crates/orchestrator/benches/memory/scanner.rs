//! Bench: memory scanner. Measures the cost of `scan()` over a 4 KB
//! body that contains one secret-shaped substring (so the pattern
//! finder fires) plus prose.
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use orchestrator::memory::scan;

fn bench_scan_4kb_with_one_secret(c: &mut Criterion) {
    let prose = "the quick brown fox jumps over the lazy dog. ".repeat(85);
    let body = format!("{prose}\nAKIAIOSFODNN7EXAMPLE\n{prose}");
    debug_assert!(body.len() >= 4096);
    c.bench_function("scan_4kb_with_one_secret", |b| {
        b.iter(|| scan(black_box(&body)));
    });
}

criterion_group!(benches, bench_scan_4kb_with_one_secret);
criterion_main!(benches);
