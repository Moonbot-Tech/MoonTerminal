use super::*;

/// Measure hourly keep-map construction from 100,000 rows in 1,000 markets.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_retention_keep_map() {
    let keys: Vec<Key> = (0..1_000)
        .map(|index| ("synthetic".into(), format!("COIN_{index}")))
        .collect();
    let trades: Vec<_> = (0..100_000_i64)
        .map(|index| {
            let mut trade = trade(100_000, 110_000, false);
            trade.id.core = (index % 10) as u64;
            trade.id.rec_id = index;
            trade.market = keys[index as usize % keys.len()].1.clone();
            trade
        })
        .collect();
    let inventory = Inventory {
        keys: keys.clone(),
        range_ms: Some((0, 1_000_000)),
    };
    let policy = KeepPolicy {
        edge: None,
        listed_from: i64::MIN,
        record_emulator: true,
        tuner_only: false,
        shape: catchup::Shape {
            margin_ms: 30_000,
            long_ms: DAY,
            tail_ms: 60_000,
        },
    };
    let mut samples = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let kept = keep_map(
            &inventory,
            &trades,
            &HashMap::new(),
            &HashSet::new(),
            KeepPolicy { ..policy },
        );
        assert_eq!(kept.len(), keys.len());
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] retention_keep_map trades=100000 markets=1000 median_ms={:.3} samples={samples:?}",
        samples[1]
    );
}
