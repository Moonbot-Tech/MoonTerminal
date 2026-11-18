use super::*;

/// The warm-ups must be outside the sample count used in the comparison table.
#[test]
fn measures_after_two_warmups_and_reports_stats() {
    let mut calls = 0;
    let stats = measure("sample", 17, || {
        calls += 1;
        calls
    });
    assert_eq!(calls, 19);

    let line = stats.line("sample", 17);
    assert!(line.starts_with("BENCH sample n=17 p50_ms="), "{line}");
    assert!(line.contains(" p95_ms=") && line.contains(" max_ms="));
    assert!(line.is_ascii());
}

/// Catch rounding down, always using the maximum, and one-sample underflow.
#[test]
fn nearest_rank_rounds_up_to_the_correct_sample() {
    let samples: Vec<_> = (1..=20).map(Duration::from_millis).collect();
    for (n, percent, expected_ms) in [(17, 50, 9), (17, 95, 17), (17, 100, 17), (20, 95, 19)] {
        assert_eq!(
            nearest_rank(&samples[..n], percent),
            Duration::from_millis(expected_ms)
        );
    }
    for percent in [50, 95, 100] {
        assert_eq!(nearest_rank(&samples[..1], percent), Duration::from_millis(1));
    }
}
