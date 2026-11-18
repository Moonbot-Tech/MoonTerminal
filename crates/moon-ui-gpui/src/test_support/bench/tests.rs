use super::*;

/// The warm-ups must be outside the sample count used in the comparison table.
#[test]
fn measures_after_two_warmups_and_reports_ordered_stats() {
    let mut calls = 0;
    let stats = measure("sample", 17, || {
        calls += 1;
        calls
    });
    assert_eq!(calls, 19);
    assert!(stats.p50_ms <= stats.p95_ms && stats.p95_ms <= stats.max_ms);

    let line = stats.line("sample", 17);
    assert!(line.starts_with("BENCH sample n=17 p50_ms="), "{line}");
    assert!(line.contains(" p95_ms=") && line.contains(" max_ms="));
    assert!(line.is_ascii());
}
