//! Unit tests for the pure selection and normalization rules behind the per-core ranking.

use super::{core_rank_rows, core_rank_stats, overview_ranges};
use moon_core::db::analytics::CoreSeries;

/// Build the minimum core series needed by ranking helpers.
///
/// Args:
///     uid: Stable identity and readable test name suffix.
///     total: Period profit used by the ranking.
///
/// Returns:
///     A core series with no bucket detail because these helpers only consume `total`.
fn core(uid: u64, total: f64) -> CoreSeries {
    CoreSeries {
        uid,
        name: format!("core-{uid}"),
        per_bucket: Vec::new(),
        per_bucket_trades: Vec::new(),
        total,
        trades: 0,
    }
}

/// `charts.rs:overview_ranges` must advance the outsider start past the leader range when fewer
/// than twenty cores exist. Replacing `.max(leaders_end)` with the raw tail start repeats cores
/// 6-10 in both columns, making the summary claim more ranked servers than it actually has.
#[test]
fn overview_never_repeats_a_core_between_columns() {
    let (leaders, outsiders) = overview_ranges(15, 10);

    assert_eq!(leaders, 0..10);
    assert_eq!(outsiders, 10..15);
    assert!(leaders.clone().all(|ix| !outsiders.contains(&ix)));
}

/// `charts.rs:core_rank_stats` must divide the best core by the NET positive result. Changing the
/// denominator to gross positive profit turns the expected 93.75% into 75% and hides how strongly
/// the leader carries a period whose losses offset part of its gain.
#[test]
fn ranking_stats_use_the_positive_net_result_for_concentration() {
    let cores = [core(1, 75.0), core(2, 25.0), core(3, 0.0), core(4, -20.0)];

    let stats = core_rank_stats(&cores);

    assert_eq!(stats.total, 4);
    assert_eq!(stats.profitable, 2);
    assert_eq!(stats.losing, 1);
    assert_eq!(stats.leader_share_pct, Some(93.75));
}

/// `charts.rs:core_rank_stats` must suppress concentration without a positive net result. Dropping
/// the `net > f64::EPSILON` guard prints infinity or a negative percentage in the card header for
/// flat and losing periods.
#[test]
fn ranking_stats_omit_concentration_for_flat_or_losing_periods() {
    let flat = [core(1, 10.0), core(2, -10.0), core(3, 0.0)];
    let losing = [core(1, 10.0), core(2, -20.0)];

    assert_eq!(core_rank_stats(&flat).leader_share_pct, None);
    assert_eq!(core_rank_stats(&losing).leader_share_pct, None);
}

/// `charts.rs:core_rank_rows` must normalize against absolute magnitude. Replacing `total.abs()`
/// in the scale calculation with the signed total makes the largest loss overflow its track and
/// visually underweights every profitable core beside it.
#[test]
fn ranking_bars_share_one_absolute_scale() {
    let rows = core_rank_rows(&[core(1, 100.0), core(2, -200.0)]);

    assert_eq!(rows[0].magnitude_pct, 50.0);
    assert_eq!(rows[1].magnitude_pct, 100.0);
}
