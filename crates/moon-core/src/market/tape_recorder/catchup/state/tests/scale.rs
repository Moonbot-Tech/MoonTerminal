use super::*;

/// Measure repeated planning of 20,000 fresh rows across 10 synthetic cores.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_catchup_prepare() {
    let _lock = SWITCH_LOCK.lock().unwrap();
    let _restore = Restore::new();
    let rows: Vec<_> = (0..20_000_i64)
        .map(|index| {
            let mut trade = row();
            trade.id.core = (index % 10) as u64;
            trade.id.rec_id = index;
            trade.close_ms = NOW - 1_000 - index % 1_000;
            trade.open_ms = trade.close_ms - 60_000;
            trade
        })
        .collect();
    let shape = catchup::Shape {
        margin_ms: 30_000,
        long_ms: DAY,
        tail_ms: 60_000,
    };
    let mut state = RecorderCatchup::default();
    state.submit(rows);
    let mut samples = Vec::new();
    for repetition in 0..5 {
        let reads = std::cell::Cell::new(0);
        let started = std::time::Instant::now();
        let planned = state.prepare(
            |_| {
                reads.set(reads.get() + 1);
                Coverage::none()
            },
            NOW,
            &shape,
            true,
        );
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        let expected = if repetition == 0 { 20_000 } else { 0 };
        assert_eq!(planned.too_fresh, expected);
        assert_eq!(reads.get(), expected as usize);
        println!(
            "[tape_scale] catchup_prepare rows=20000 held_reads={} elapsed_ms={elapsed:.3}",
            reads.get()
        );
        samples.push(elapsed);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] catchup_prepare rows=20000 median_ms={:.3}",
        samples[2]
    );
}
