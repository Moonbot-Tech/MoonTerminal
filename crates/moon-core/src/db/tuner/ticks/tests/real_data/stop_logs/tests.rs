//! Deterministic diagnostic helpers use synthetic readings and dates only.

use super::*;

/// Invalid close windows must not slice backwards; budget exhaustion must stop retention.
#[test]
fn collector_skips_reversed_windows_and_latches_budget() {
    let mut diag = StopLogDiag {
        active: true,
        kept: Vec::new(),
        skipped_big: 0,
        skipped_window: 0,
        dropped_budget: 0,
        retained_prints: RETAIN_BUDGET_PRINTS - 1,
    };
    let mut deal = super::super::super::deal();
    deal.buy_ms = 1_000_000;
    deal.close_ms = 0;
    let exit = ExitParams::default();
    let ticks = [super::super::super::tick(500_000, 99.0, Side::Buy)];
    diag.add(&deal, &ticks, &exit, None);
    assert_eq!(diag.skipped_window, 1);
    deal.buy_ms = 10_000;
    deal.close_ms = 20_000;
    let ticks = [
        super::super::super::tick(10_000, 99.0, Side::Buy),
        super::super::super::tick(20_000, 98.0, Side::Sell),
    ];
    diag.add(&deal, &ticks, &exit, None);
    diag.add(&deal, &ticks[..1], &exit, None);
    assert_eq!(diag.dropped_budget, 2);
    assert_eq!(diag.retained_prints, RETAIN_BUDGET_PRINTS - 1);
    assert!(diag.kept.is_empty());
}

/// Equal timestamps of distinct events must survive; a shared event keeps its nearest deal.
#[test]
fn event_ownership_uses_identity_and_distance() {
    let ids = [
        Some((("CORE_A".into(), 0), 500)),
        Some((("CORE_A".into(), 1), 500)),
        Some((("CORE_A".into(), 0), 100)),
        Some((("CORE_B".into(), 0), 20)),
        None,
    ];
    assert_eq!(
        exclusive_owners(&ids),
        (vec![true, false, false, false, false], 1)
    );
}

/// Miss penalties must not inflate real-stop quantiles or the fitted allowance.
#[test]
fn timing_separates_real_errors_from_miss_penalties() {
    let mut timing = Timing::default();
    timing.add(Some(110), 100, 20);
    timing.add(None, 100, 20);
    timing.add(None, 100, 20);
    assert_eq!(quantile(&mut timing.abs.clone(), 0.9), Some(10.0));
    assert_eq!(timing.second, 1);
    assert_eq!(timing.n, 3);
    assert!(timing.render().contains("penalised med|dt| 60000.000"));
}

/// Splitting relative to the first sampled day would swap fit and held when the range changes.
#[test]
fn half_uses_absolute_utc_day_parity() {
    let oct4 = 1_791_072_000_000;
    assert_eq!(day_half(oct4), Half::Fit);
    assert_eq!(day_half(oct4 + 86_400_000), Half::Held);
    assert_eq!(day_half(oct4 + 86_399_999), Half::Fit);
}

/// A short watches ask upward; using the long bid direction mislabels both examples.
#[test]
fn reading_crossings_mirror_sides_and_keep_short_ask() {
    assert_eq!(
        fired_by(
            true,
            100.0,
            Some(99.0),
            Some(101.0),
            Some(102.0),
            Some(103.0)
        ),
        FiredBy::OnlyBid
    );
    assert_eq!(
        fired_by(
            false,
            100.0,
            Some(99.0),
            Some(98.0),
            Some(97.0),
            Some(101.0)
        ),
        FiredBy::OnlyAsk
    );
    assert_eq!(
        fired_by(true, 100.0, Some(99.0), Some(99.0), Some(102.0), None),
        FiredBy::Several
    );
    assert_eq!(
        fired_by(true, 100.0, None, None, Some(101.0), None),
        FiredBy::None
    );
}

/// Preserve the parent bench's upper-middle median and p90 convention on even samples.
#[test]
fn quantile_even_sample_matches_existing_bench() {
    let mut sample = [4.0, 1.0, 3.0, 2.0];
    assert_eq!(quantile(&mut sample, 0.5), Some(3.0));
    assert_eq!(quantile(&mut sample, 0.9), Some(4.0));
    assert_eq!(quantile(&mut sample, 0.1), Some(1.0));
    assert_eq!(quantile(&mut [], 0.5), None);
    assert_eq!(
        quantile(&mut [0.123456, 0.234567, 0.345678], 0.5),
        Some(0.234567)
    );
}

/// Dropping hard deals must not beat higher all-deal agreement or default proximity.
#[test]
fn period_selection_counts_misses_and_breaks_ties() {
    assert_eq!(
        pick_period(
            &[
                (1800, vec![Some(1), None, None]),
                (2200, vec![Some(200), Some(-300), Some(400)]),
            ],
            2150
        ),
        2200
    );
    assert_eq!(
        pick_period(
            &[
                (1800, vec![Some(100), Some(200)]),
                (2150, vec![Some(100), Some(200)]),
            ],
            2150
        ),
        2150
    );
    assert_eq!(
        pick_period(
            &[
                (2150, vec![Some(1), None, None]),
                (2200, vec![Some(1), Some(2000), Some(3000)]),
            ],
            2150
        ),
        2200
    );
}

/// Wrapping to an unsigned hour would turn a one-second lead into 3599 seconds.
#[test]
fn clock_offset_wraps_with_sign() {
    assert_eq!(clock_offset_s(10, 11), -1);
    assert_eq!(clock_offset_s(3599, 0), -1);
    assert_eq!(clock_offset_s(0, 3599), 1);
}

/// Independently known constant paired improvements preserve their sign under resampling.
#[test]
fn bootstrap_is_deterministic_and_paired() {
    let days = [
        (1, 10.0, 8.0),
        (1, 20.0, 18.0),
        (3, 30.0, 28.0),
        (5, 40.0, 38.0),
    ];
    let ci = bootstrap_diff(&days, 200, 0x9E3779B97F4A7C15);
    assert_eq!(ci, bootstrap_diff(&days, 200, 0x9E3779B97F4A7C15));
    assert_eq!(ci, (2.0, 2.0));
    let identical = [(1, 10.0, 10.0), (1, 20.0, 20.0), (3, 30.0, 30.0)];
    assert_eq!(
        bootstrap_diff(&identical, 200, 0x9E3779B97F4A7C15),
        (0.0, 0.0)
    );
    let opposite = [(1, 20.0, 10.0), (3, 10.0, 20.0)];
    let ci = bootstrap_diff(&opposite, 200, 0x9E3779B97F4A7C15);
    assert_eq!(ci, (-10.0, 10.0));
    let mut unbalanced = vec![(1, 20.0, 10.0); 100];
    unbalanced.push((3, 10.0, 20.0));
    assert_eq!(
        bootstrap_diff(&unbalanced, 200, 0x9E3779B97F4A7C15),
        (-10.0, 10.0)
    );
    let paired = [
        (1, penalised_error(None), penalised_error(Some(10))),
        (1, penalised_error(Some(20)), penalised_error(None)),
        (3, penalised_error(None), penalised_error(Some(30))),
    ];
    assert_eq!(
        quantile(&mut paired.iter().map(|p| p.1).collect::<Vec<_>>(), 0.5),
        Some(60_000.0)
    );
    assert_eq!(
        quantile(&mut paired.iter().map(|p| p.2).collect::<Vec<_>>(), 0.5),
        Some(30.0)
    );
    assert_eq!(
        bootstrap_diff(&paired, 200, 0x9E3779B97F4A7C15),
        (0.0, 59_970.0)
    );
}
