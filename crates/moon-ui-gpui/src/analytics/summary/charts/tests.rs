//! Unit tests for the pure selection and normalization rules behind the per-core ranking.

use super::hover::enter_decision;
use super::{
    PopupHover, PopupKey, core_rank_rows, core_rank_stats, distinct_core_colors, overview_ranges,
    popup_limits, popup_outer_width, thinned_labels,
};
use moon_core::db::analytics::CoreSeries;

/// Adapt the production entry decision to test selection and delayed-work revision tokens.
fn model_chart_enter(
    hover: &mut PopupHover,
    selected: &mut Option<PopupKey>,
    key: PopupKey,
    from_popup: bool,
) -> Option<u64> {
    if let Some(revision) = enter_decision(hover, *selected, key, from_popup) {
        Some(revision)
    } else {
        *selected = Some(key);
        None
    }
}

/// Compare both pointer ownership and the same three highlight projections select_popup writes.
fn assert_popup_selection_equal(
    old: &PopupHover,
    new: &PopupHover,
    old_selected: Option<PopupKey>,
    new_selected: Option<PopupKey>,
) {
    assert_eq!(old.target, new.target);
    assert_eq!(old.over_popup, new.over_popup);
    let highlights = |key| match key {
        Some(PopupKey::Daily(i)) => (Some(i), None, None),
        Some(PopupKey::Cumulative(i)) => (None, Some(i), None),
        Some(PopupKey::Kind(i)) => (None, None, Some(i)),
        None => (None, None, None),
    };
    assert_eq!(highlights(old_selected), highlights(new_selected));
}

/// Omitting old source leave must still fence its timer and preserve neighbour dwell in either direction.
#[test]
fn cumulative_single_source_matches_old_column_switches_both_directions() {
    for (a, b) in [(2, 7), (7, 2)] {
        for leave_first in [true, false] {
            let (first, next) = (PopupKey::Cumulative(a), PopupKey::Cumulative(b));
            let (mut old, mut new) = (PopupHover::default(), PopupHover::default());
            let (mut old_selected, mut new_selected) = (None, None);
            model_chart_enter(&mut old, &mut old_selected, first, false);
            new.cumulative_pointer_entered();
            assert!(new.cumulative_move_should_enter(a));
            model_chart_enter(&mut new, &mut new_selected, first, false);
            let stale = leave_first.then(|| old.leave(first, false)).flatten();
            let old_reveal = model_chart_enter(&mut old, &mut old_selected, next, false).unwrap();
            if !leave_first {
                assert_eq!(old.leave(first, false), None);
            }
            assert!(new.cumulative_move_should_enter(b));
            let new_reveal = model_chart_enter(&mut new, &mut new_selected, next, false).unwrap();
            assert_popup_selection_equal(&old, &new, old_selected, new_selected);
            if let Some(revision) = stale {
                assert!(!old.expire(first, revision));
            }
            assert!(old.is_current(next, old_reveal));
            assert!(new.is_current(next, new_reveal));
            old_selected = Some(next);
            new_selected = Some(next);
            assert_popup_selection_equal(&old, &new, old_selected, new_selected);
            let old_leave = old.leave(next, false).unwrap();
            let new_key = new.cumulative_pointer_left().unwrap();
            let new_leave = new.leave(new_key, false).unwrap();
            assert!(old.expire(next, old_leave));
            assert!(new.expire(next, new_leave));
            assert_popup_selection_equal(&old, &new, None, None);
        }
    }
}

/// Same-bucket re-entry must fence popup dismissal; repeated interior moves must not restart entry.
#[test]
fn cumulative_same_bucket_reentry_survives_expire_and_repeated_moves_do_not_enter() {
    let key = PopupKey::Cumulative(3);
    let mut hover = PopupHover::default();
    let mut selected = None;
    hover.cumulative_pointer_entered();
    assert!(hover.cumulative_move_should_enter(3));
    model_chart_enter(&mut hover, &mut selected, key, false);
    let leaving_source = hover.cumulative_pointer_left().unwrap();
    let source_timer = hover.leave(leaving_source, false).unwrap();
    model_chart_enter(&mut hover, &mut selected, key, true);
    assert!(!hover.expire(key, source_timer));
    let popup_timer = hover.leave(key, true).unwrap();
    hover.cumulative_pointer_entered();
    assert!(hover.cumulative_move_should_enter(3));
    model_chart_enter(&mut hover, &mut selected, key, false);
    assert!(!hover.expire(key, popup_timer));
    assert_eq!(hover.target, Some(key));
    assert_eq!(selected, Some(key));
    let revision = hover.revision;
    for _ in 0..100 {
        assert!(!hover.cumulative_move_should_enter(3));
    }
    assert_eq!(hover.revision, revision);
    // A popup-first callback order still needs a chart entry even before a source-enter callback.
    hover.enter(key, true);
    assert!(hover.cumulative_move_should_enter(3));
    hover.enter(key, false);
    assert!(hover.cumulative_pointer_left().is_some());
    assert_eq!(
        hover.cumulative_pointer_left(),
        None,
        "gap movement must leave only once"
    );
    assert!(
        hover.cumulative_move_should_enter(3),
        "return across the uncovered region must cancel leave"
    );
}

/// Baseline label formatting and magnitude-based thinning; GUI font measurement is excluded.
#[test]
#[ignore]
fn bench_daily_labels_400() {
    let data = super::super::tests::synthetic_summary();
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        let texts: Vec<_> = data
            .days
            .iter()
            .map(|d| {
                format!(
                    "{}{}",
                    moon_core::util::fmt::compact(d.profit, 0),
                    crate::analytics::pnl_suffix()
                )
            })
            .collect();
        let profits: Vec<_> = data.days.iter().map(|d| d.profit).collect();
        std::hint::black_box((
            texts,
            thinned_labels(&profits, super::PLOT_W_NOMINAL, std::hint::black_box(48.0)),
        ));
    }
    println!(
        "bench_daily_labels_400 before_us_per_iter={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Long identities retain their entire measured row and do not lose width to the vertical track.
#[test]
fn popup_long_rows_keep_content_and_scrollbar_width_separate() {
    let outer = popup_outer_width(460.0, 22.0, 190.0, 8.0);
    assert_eq!(outer - 22.0 - 8.0, 460.0);
    assert_eq!(popup_limits(outer, 1200.0, 800.0, 8.0).0, outer);
    assert_eq!(popup_limits(outer, 400.0, 800.0, 8.0).0, 384.0);
}

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

/// `charts.rs:thinned_labels` must keep only separated bucket labels, prioritizing larger absolute profits.
/// Deleting its separation predicate labels adjacent bars together, while natural ordering hides the largest adjacent daily move.
#[test]
fn thinned_labels_keep_spaced_daily_extremes() {
    let labels = thinned_labels(&[90.0, -100.0, 80.0, 0.0, 70.0], 100.0, 30.0);

    assert_eq!(
        labels,
        vec![1, 4],
        "only the largest daily move in each overlapping label region may remain"
    );
}

/// `summary/charts/axis.rs:distinct_core_colors` must give duplicate or missing configured colors a
/// distinct picker swatch in uid order. Dropping the taken-RGB guard makes chart lines collide,
/// while using profit order instead of uid makes the same core change color after a reload.
#[test]
fn core_colors_are_unique_and_stable_by_uid() {
    let duplicate = [17, 34, 51];
    let configured = [
        (30, Some(duplicate)),
        (10, Some(duplicate)),
        (20, None),
        (40, Some(duplicate)),
        (50, Some(duplicate)),
        (60, Some(duplicate)),
        (70, Some(duplicate)),
        (80, Some(duplicate)),
        (90, Some(duplicate)),
        (100, Some(duplicate)),
    ];
    let colors = distinct_core_colors(&configured, moon_ui::MoonPalette::LIGHT);
    let rgb: Vec<_> = colors
        .into_iter()
        .map(crate::design::hsla_to_rgb8)
        .collect();
    let unique: std::collections::HashSet<_> = rgb.iter().copied().collect();

    assert_eq!(
        unique.len(),
        configured.len(),
        "every visible core needs a distinct line color"
    );
    assert_eq!(
        rgb[1], duplicate,
        "the lowest uid keeps the user-configured color"
    );
    assert_ne!(
        rgb[2], duplicate,
        "a missing color must use an unused picker swatch"
    );

    let shuffled = [
        configured[7],
        configured[1],
        configured[9],
        configured[2],
        configured[5],
        configured[0],
        configured[8],
        configured[3],
        configured[6],
        configured[4],
    ];
    let shuffled_rgb: std::collections::HashMap<_, _> = shuffled
        .iter()
        .copied()
        .zip(distinct_core_colors(&shuffled, moon_ui::MoonPalette::LIGHT))
        .map(|((uid, _), color)| (uid, crate::design::hsla_to_rgb8(color)))
        .collect();
    for ((uid, _), color) in configured.iter().zip(rgb) {
        assert_eq!(
            shuffled_rgb.get(uid),
            Some(&color),
            "uid {uid} must keep its color across order changes"
        );
    }
}

/// `charts.rs:thinned_labels` must seed the LAST bucket before the greedy pass. Deleting that seed
/// puts the final bucket back at the mercy of the magnitude order, where a flat closing day ranks
/// dead last and the chart ends on an unlabelled bar — the reader cannot tell a zero result from
/// missing data.
#[test]
fn thinned_labels_always_label_the_final_bucket() {
    let closing_zero = thinned_labels(&[90.0, -100.0, 80.0, 70.0, 0.0], 100.0, 30.0);
    let single = thinned_labels(&[0.0], 100.0, 30.0);

    assert_eq!(
        closing_zero,
        vec![1, 4],
        "the flat closing day keeps its number"
    );
    assert_eq!(single, vec![0], "a one-bucket period still labels itself");
    assert!(thinned_labels(&[], 100.0, 30.0).is_empty());
}

/// `charts.rs:thinned_labels` must make every other candidate clear the final label, not the other
/// way round. Seeding the last bucket AFTER the greedy loop instead of before it lets the period's
/// biggest day claim the neighbouring column first and the guaranteed final label then overlaps it.
#[test]
fn thinned_labels_let_the_final_bucket_displace_a_bigger_neighbour() {
    let labels = thinned_labels(&[0.0, 0.0, 1000.0, 0.0], 100.0, 40.0);

    assert!(
        !labels.contains(&2),
        "the biggest day yields when it cannot clear the final label"
    );
    assert_eq!(labels, vec![0, 3]);
}

/// `charts.rs:thinned_labels` must separate neighbours against the final label's RIGHT-ALIGNED
/// centre (`plot_w - label_w / 2`), the position `daily_bars` actually draws it at. Reverting that
/// arm to the shared column centre reports bucket 6 as clear by 60px when only 45px of the plot
/// separate the two labels, and they collide on screen.
#[test]
fn thinned_labels_measure_the_final_label_where_it_is_drawn() {
    let mut vals = vec![0.0; 10];
    vals[6] = 1000.0;

    let labels = thinned_labels(&vals, 200.0, 50.0);

    assert!(
        !labels.contains(&6),
        "a bucket inside the final label's own width may not be labelled"
    );
    assert_eq!(labels, vec![0, 3, 9]);
}

/// Oversize content must stay inside the usable window even after font/UI or DPI changes.
/// Removing the viewport cap or subtracting only one inset lets wrapped rows leave the window.
#[test]
fn popup_limits_reserve_both_window_edges() {
    assert_eq!(popup_limits(1200.0, 860.0, 520.0, 12.0), (836.0, 496.0));
    assert_eq!(popup_limits(600.0, 430.0, 260.0, 18.0), (394.0, 224.0));
    assert_eq!(popup_limits(600.0, 20.0, 20.0, 18.0), (0.0, 0.0));
}

/// A centre bucket must not squeeze a readable popup into the remaining fraction of its plot.
/// Reintroducing the nominal plot cap loses width even when the window has enough room.
#[test]
fn popup_limits_keep_measured_width_when_window_has_room() {
    assert_eq!(popup_limits(500.0, 860.0, 520.0, 8.0), (500.0, 504.0));
    assert_eq!(popup_limits(240.0, 1240.0, 800.0, 8.0), (240.0, 784.0));
}

/// Entering the popup must invalidate the source-column timer, in either callback order.
/// Without that fence the card vanishes while the user tries to scroll its lower server names.
#[test]
fn popup_hover_survives_travel_from_column_to_card() {
    let key = PopupKey::Daily(2);
    let mut hover = PopupHover::default();
    hover.enter(key, false);
    let leaving_column = hover.leave(key, false).expect("column starts grace period");
    hover.enter(key, true);
    assert!(!hover.expire(key, leaving_column));
    assert_eq!(
        hover.leave(key, false),
        None,
        "late column leave cannot dismiss hovered card"
    );

    let leaving_card = hover.leave(key, true).expect("card starts grace period");
    assert!(
        hover.expire(key, leaving_card),
        "leaving both surfaces dismisses the card"
    );
    assert!(
        !hover.expire(key, leaving_card),
        "one timer cannot dismiss twice"
    );
}

/// Switching chart or returning across the gap must reject pending dismissal from the old owner.
#[test]
fn popup_hover_rejects_stale_leave_after_switch_or_return() {
    let first = PopupKey::Cumulative(2);
    let next = PopupKey::Kind(2);
    let mut hover = PopupHover::default();
    hover.enter(first, false);
    let stale = hover.leave(first, false).expect("leave timer");
    hover.enter(next, false);
    assert!(!hover.expire(first, stale));
    assert_eq!(hover.leave(first, true), None);

    let stale = hover.leave(next, false).expect("new leave timer");
    hover.enter(next, false);
    assert!(
        !hover.expire(next, stale),
        "return to same column cancels dismissal"
    );
    let current = hover.leave(next, false).expect("current leave timer");
    assert!(hover.expire(next, current));
}

/// Crossing another bucket en route to the visible popup must not replace that popup after entry.
/// Removing the reveal-generation check makes a queued neighbour dwell steal scroll ownership.
#[test]
fn popup_entry_cancels_neighbour_dwell() {
    let visible = PopupKey::Daily(10);
    let crossed = PopupKey::Daily(11);
    let mut hover = PopupHover::default();
    hover.enter(visible, false);
    hover.enter(crossed, false);
    let dwell = hover.revision;
    hover.enter(visible, true);
    assert!(!hover.is_current(crossed, dwell));
    assert_eq!(hover.leave(crossed, false), None);
    let leave = hover
        .leave(visible, true)
        .expect("leaving popup dismisses it");
    assert!(hover.expire(visible, leave));
}

/// A queued reveal cannot resurrect a bucket whose identity was invalidated by reload.
#[test]
fn popup_reload_invalidates_stale_reveal() {
    let mut hover = PopupHover::default();
    let daily = PopupKey::Daily(2);
    hover.enter(daily, false);
    let daily_dwell = hover.revision;
    hover.reset_for_reload(false);
    assert!(
        hover.is_current(daily, daily_dwell),
        "same-range daily indices remain valid"
    );
    hover.reset_for_reload(true);
    assert!(!hover.is_current(daily, daily_dwell));

    let kind = PopupKey::Kind(2);
    hover.enter(kind, false);
    let kind_dwell = hover.revision;
    hover.reset_for_reload(false);
    assert!(
        !hover.is_current(kind, kind_dwell),
        "profit-sorted kinds can reorder on any reload"
    );
}
