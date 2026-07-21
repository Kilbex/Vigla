//! Bench: confidence scoring over a typical mature note (25 witnesses).
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_schema::memory::WitnessKind;
use orchestrator::memory::{confidence, Witness};

fn make_witness(i: usize) -> Witness {
    Witness {
        id: format!("w-{i}"),
        note_id: "note-bench".to_owned(),
        kind: WitnessKind::WorkerProposed,
        weight: 1.0,
        source_event_id: format!("ev-{i}"),
        observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
    }
}

fn bench_confidence_25_witnesses(c: &mut Criterion) {
    let ws: Vec<Witness> = (0..25).map(make_witness).collect();
    let now_ms = 1_747_526_400_000u64;
    c.bench_function("confidence_25_witnesses", |b| {
        b.iter(|| confidence(black_box(&ws), black_box(now_ms)));
    });
}

criterion_group!(benches, bench_confidence_25_witnesses);
criterion_main!(benches);
