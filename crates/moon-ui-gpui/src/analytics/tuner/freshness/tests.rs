use super::SampleSnapshot;
use moon_core::db::tuner::VarStats;

/// A delayed pre-fit cache must not count as new trades after a freshly completed run.
#[test]
fn polish_post_run_refresh_replaces_the_old_cache_baseline_once() {
    let stats = |n| {
        vec![VarStats {
            n,
            ..Default::default()
        }]
    };
    let mut snapshot = SampleSnapshot::after_run(100);
    assert!(!snapshot.materially_changed(Some(&stats(100)), 100));
    snapshot.observe(None);
    assert!(!snapshot.materially_changed(Some(&stats(110)), 100));
    snapshot.observe(Some(&stats(110)));
    assert!(!snapshot.materially_changed(Some(&stats(110)), 100));
    assert!(!snapshot.materially_changed(Some(&stats(111)), 100));
    snapshot.observe(Some(&stats(116)));
    assert!(
        snapshot.materially_changed(Some(&stats(116)), 100),
        "later refreshes must not advance the baseline"
    );
    let pick = SampleSnapshot::from_count(110, 100);
    assert!(!pick.materially_changed(Some(&stats(110)), 100));
    assert!(pick.materially_changed(Some(&stats(116)), 100));
}

/// A generation-only warning would fire on one trade; floor rounding would fire before five percent.
#[test]
fn polish_only_material_scoped_growth_or_a_new_day_warns() {
    let stats = |n| {
        vec![VarStats {
            n,
            ..Default::default()
        }]
    };
    for (sample, below, enough) in [(20, 4, 5), (201, 10, 11)] {
        let snapshot = SampleSnapshot::capture(Some(&stats(sample)), 100);
        assert!(!snapshot.materially_changed(Some(&stats(sample + 1)), 101));
        assert!(!snapshot.materially_changed(Some(&stats(sample + below)), 102));
        assert!(snapshot.materially_changed(Some(&stats(sample + enough)), 103));
        assert!(!snapshot.materially_changed(Some(&stats(sample)), 86_399));
        assert!(snapshot.materially_changed(Some(&stats(sample)), 86_400));
    }
}
