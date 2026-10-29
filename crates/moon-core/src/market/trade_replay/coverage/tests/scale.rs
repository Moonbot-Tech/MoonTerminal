use super::*;

/// Measure construction of 20,000 disjoint windows in a deterministic non-sorted order.
#[test]
#[ignore = "synthetic tape scale baseline"]
fn tape_scale_coverage_add() {
    let spans: Vec<_> = (0..20_000_i64)
        .map(|index| {
            let shuffled = (index * 7_919) % 20_000;
            let from = shuffled * 10_002;
            (from, from + 10_000)
        })
        .collect();
    let mut samples = Vec::new();
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let coverage = Coverage::from_spans(spans.iter().copied());
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(coverage.spans().len(), spans.len());
        samples.push(elapsed);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[tape_scale] coverage_add spans=20000 median_ms={:.3} samples={samples:?}",
        samples[1]
    );
}
