use super::SampleSnapshot;
use moon_core::db::tuner::VarStats;

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
