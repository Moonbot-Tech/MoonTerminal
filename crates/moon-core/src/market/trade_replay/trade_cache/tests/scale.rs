use super::*;

/// Measure indexed span-bounds reads over 5,000 intersecting rows.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_span_bounds_read() {
    let conn = conn();
    let tx = conn.unchecked_transaction().unwrap();
    for index in 0..5_000_i64 {
        let from = index * 10;
        insert_span(
            &tx,
            "CORE_A",
            "XALPHA",
            from,
            from + 9,
            &[],
            TileSource::Core,
            1,
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let mut samples = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        for _ in 0..200 {
            let held = read_span_bounds(&conn, "CORE_A", "XALPHA", 0, 49_999).unwrap();
            assert_eq!(held.len(), 5_000);
            std::hint::black_box(held);
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000.0 / 200.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] span_bounds_read intersecting=5000 median_read_ms={:.3}",
        samples[1]
    );
}

/// Measure a 1,000-span worker drain, including its SQLite commit barrier.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_insert_worker_drain() {
    let mut samples = Vec::new();
    for repetition in 0..3 {
        let dir = BenchDir::new(&format!("tape-scale-insert-{repetition}"));
        let cache = TradeCache::open_with_ceiling(dir.0.join("tape.sqlite"), || None).unwrap();
        let started = std::time::Instant::now();
        for index in 0..1_000_i64 {
            let from = index * 10;
            cache.insert("CORE_A", "XALPHA", from, from + 9, vec![], TileSource::Core);
        }
        assert!(cache.sync(Duration::from_secs(30)));
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(
            cache
                .held_spans("CORE_A", "XALPHA", 0, 9_999)
                .unwrap()
                .len(),
            1_000
        );
        samples.push(elapsed);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] insert_worker_drain spans=1000 median_ms={:.3} samples={samples:?}",
        samples[1]
    );
}
