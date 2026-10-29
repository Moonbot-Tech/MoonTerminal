use super::*;

/// Measure rebuilding every live coverage map on 250 ms ticks with 2,000 open rows.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_live_map_ticks() {
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: 3_600_000,
        tail_ms: 60_000,
    };
    let mut tasks = HashMap::new();
    for index in 0..2_000_i64 {
        let key = (format!("CORE_{}", index % 10), format!("COIN_{index}"));
        let mut task = KeyTask::new();
        task.opened(
            TradeId {
                core: (index % 10) as u64,
                rec_id: index,
            },
            index * 100_000,
            &shape,
            Some(false),
        );
        tasks.insert(key, task);
    }
    let mut live = HashMap::new();
    let mut revisions = HashMap::new();
    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        for _ in 0..100 {
            sync_live(&tasks, &mut revisions, &mut live);
            std::hint::black_box(&live);
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000.0 / 100.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] live_map tasks=2000 ticks_per_sample=100 median_tick_ms={:.3}",
        samples[2]
    );
}
