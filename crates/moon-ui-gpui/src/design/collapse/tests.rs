use super::*;

fn header() -> HeaderWidths {
    HeaderWidths {
        fixed: 300.0,
        right_fixed: 180.0,
        delta_1h: 60.0,
        free_sub: 90.0,
        sleep_label: 40.0,
        ticker: 150.0,
        logo: 140.0,
        launcher_labels: 200.0,
        balance_caption: 55.0,
        clock_city: 70.0,
        separators: 12.0,
        right_separators: 8.0,
        usdt_unit: 35.0,
        inactive_mode_seg: 60.0,
        core_name_full: 240.0,
        core_name_cap_l5: 110.0,
        core_name_cap_l6: 70.0,
        core_gear: 28.0,
    }
}

fn toolbar() -> ToolbarWidths {
    ToolbarWidths {
        fixed: 400.0,
        strat_controls: 0.0,
        sell_strip: 260.0,
        sell_drop: 70.0,
        max_readout: 110.0,
        ms_summary: 220.0,
        size_strip: 300.0,
        size_drop: 80.0,
        captions: 180.0,
        chart_tools: 64.0,
        overflow: 50.0,
        own_icon: 28.0,
        tp_pod: 90.0,
        sell_pod: 20.0,
        size_pod: 20.0,
        sl_pod: 75.0,
        live: 70.0,
    }
}

fn header_levels() -> impl Iterator<Item = HeaderLevel> {
    (0..=HeaderLevel::MAX).map(HeaderLevel)
}

fn toolbar_levels() -> impl Iterator<Item = ToolbarLevel> {
    (0..=ToolbarLevel::MAX).map(ToolbarLevel)
}

#[test]
fn header_level_boundary_is_exact_and_inclusive() {
    let w = header();
    for n in 0..HeaderLevel::MAX {
        let at = w.width_at(HeaderLevel(n));
        assert!(
            w.width_at(HeaderLevel(n + 1)) < at,
            "level {n} must narrow the row"
        );
        assert_eq!(w.level(at), HeaderLevel(n), "level {n} at its own width");
        assert_eq!(
            w.level(at - 1.0),
            HeaderLevel(n + 1),
            "level {} one px below",
            n + 1
        );
    }
}

#[test]
fn toolbar_level_boundary_is_exact_and_inclusive() {
    let w = toolbar();
    for n in 0..ToolbarLevel::MAX {
        let at = w.width_at(ToolbarLevel(n));
        assert!(
            w.width_at(ToolbarLevel(n + 1)) < at,
            "level {n} must narrow the row"
        );
        assert_eq!(w.level(at), ToolbarLevel(n), "level {n} at its own width");
        assert_eq!(
            w.level(at - 1.0),
            ToolbarLevel(n + 1),
            "level {} one px below",
            n + 1
        );
    }
}

#[test]
fn nothing_fits_returns_the_narrowest_level() {
    assert_eq!(header().level(0.0), HeaderLevel(HeaderLevel::MAX));
    assert_eq!(toolbar().level(0.0), ToolbarLevel(ToolbarLevel::MAX));
    assert_eq!(lowest_fitting_level(5.0, |_| 10.0, 3), 3);
    assert_eq!(lowest_fitting_level(10.0, |_| 10.0, 3), 0);
}

#[test]
fn header_accessors_hide_cumulatively_in_order() {
    let flags = |l: HeaderLevel| {
        [
            l.delta_1h(),
            l.free_sub(),
            l.sleep_label(),
            l.ticker(),
            l.logo(),
            l.launcher_labels(),
            l.balance_caption(),
            l.clock_city(),
            l.separators(),
            l.usdt_unit(),
            l.inactive_mode_seg(),
            l.core_gear(),
        ]
    };
    // The level each group first disappears at, in the accessor order above.
    let hides_at = [1, 1, 1, 2, 2, 3, 3, 4, 4, 4, 4, 6];
    for level in header_levels() {
        for (shown, at) in flags(level).into_iter().zip(hides_at) {
            assert_eq!(shown, level.0 < at, "level {}", level.0);
        }
    }
}

#[test]
fn toolbar_accessors_hide_cumulatively_in_order() {
    for level in toolbar_levels() {
        let l = level.0;
        assert_eq!(level.sell_as_dropdown(), l >= 1);
        assert_eq!(level.max_readout(), l < 2);
        assert_eq!(level.ms_summary(), l < 3);
        assert_eq!(level.size_as_dropdown(), l >= 4);
        assert_eq!(level.captions(), l < 5);
        assert_eq!(level.chart_tools_folded(), l >= 6);
        assert_eq!(level.own_icon_folded(), l >= 7);
        assert_eq!(level.take_sell_folded(), l >= 8);
        assert_eq!(level.size_folded(), l >= 9);
        assert_eq!(level.sl_folded(), l >= 10);
    }
}

#[test]
fn ms_summary_shown_at_l2_and_hidden_at_l3() {
    assert!(ToolbarLevel(2).ms_summary());
    assert!(!ToolbarLevel(3).ms_summary());
}

#[test]
fn core_name_is_untouched_below_l5() {
    let w = header();
    for level in header_levels().filter(|l| l.0 < 5) {
        assert_eq!(level.core_name_cap(&w), w.core_name_full);
    }
    assert_eq!(HeaderLevel(5).core_name_cap(&w), w.core_name_cap_l5);
    assert_eq!(HeaderLevel(6).core_name_cap(&w), w.core_name_cap_l6);
}

#[test]
fn short_core_name_is_never_widened_by_its_cap() {
    let w = HeaderWidths {
        core_name_full: 50.0,
        ..header()
    };
    for level in header_levels() {
        assert_eq!(level.core_name_cap(&w), 50.0);
    }
}

#[test]
fn right_of_ticker_is_the_sum_of_its_parts() {
    let w = header();
    // Hand sums: right_fixed + sleep_label + clock_city + right_separators + launcher_labels.
    let expected = [
        180.0 + 40.0 + 70.0 + 8.0 + 200.0,
        180.0 + 70.0 + 8.0 + 200.0,
        180.0 + 70.0 + 8.0 + 200.0,
        180.0 + 70.0 + 8.0,
        180.0,
        180.0,
        180.0,
    ];
    for (level, want) in header_levels().zip(expected) {
        assert_eq!(w.right_of_ticker(level), want, "level {}", level.0);
    }
}

#[test]
fn a_wide_strategy_pod_sheds_the_summary_before_the_size_strip() {
    let w = ToolbarWidths {
        strat_controls: 500.0,
        ..toolbar()
    };
    // Room for L3 exactly: the summary goes while the size strip stays a strip.
    let level = w.level(w.width_at(ToolbarLevel(3)));
    assert_eq!(level, ToolbarLevel(3));
    assert!(!level.ms_summary());
    assert!(!level.size_as_dropdown());
    // The pod is counted at every level, never shed by the outer ladder.
    for level in toolbar_levels() {
        assert_eq!(
            w.width_at(level) - toolbar().width_at(level),
            500.0,
            "level {}",
            level.0
        );
    }
}

#[test]
fn width_never_grows_with_the_level() {
    let header = HeaderWidths {
        core_name_cap_l5: 90.0,
        core_name_cap_l6: 150.0,
        ..header()
    };
    let toolbar = ToolbarWidths {
        sell_drop: 300.0,
        size_drop: 320.0,
        overflow: 90.0,
        ..toolbar()
    };
    for n in 0..HeaderLevel::MAX {
        assert!(
            header.width_at(HeaderLevel(n + 1)) <= header.width_at(HeaderLevel(n)),
            "header level {}",
            n + 1
        );
    }
    for n in 0..ToolbarLevel::MAX {
        assert!(
            toolbar.width_at(ToolbarLevel(n + 1)) <= toolbar.width_at(ToolbarLevel(n)),
            "toolbar level {}",
            n + 1
        );
    }
}

/// Regression target: L8 must take the Take pod and the whole sell control off the row (they live
/// in "More" from there), L9 the size control, L10 the Stop pod; each fold costs exactly what the
/// pods drew.
#[test]
fn take_sell_then_size_fold_out_of_the_row() {
    let w = toolbar();
    let l7 = w.width_at(ToolbarLevel(7));
    let l8 = w.width_at(ToolbarLevel(8));
    let l9 = w.width_at(ToolbarLevel(9));
    let l10 = w.width_at(ToolbarLevel(10));
    assert_eq!(l7 - l8, w.tp_pod + w.sell_pod + w.sell_drop);
    assert_eq!(l8 - l9, w.size_pod + w.size_drop);
    assert_eq!(l9 - l10, w.sl_pod);
    assert_eq!(w.trailing_at(ToolbarLevel(9)), w.overflow + w.live);
    assert_eq!(w.trailing_at(ToolbarLevel(0)), w.chart_tools + w.live);
}

/// Regression target: an open menu or popup holds the level inside the band that draws its
/// trigger — never below the level introducing a collapsed form, never at the one folding it away.
#[test]
fn an_open_trigger_holds_the_level_inside_its_band() {
    let w = toolbar();
    let max = ToolbarLevel(ToolbarLevel::MAX);
    let (floor, ceil) = (ToolbarLevel(1), ToolbarLevel(7));
    assert_eq!(w.level(0.0), max);
    assert_eq!(w.level_within(0.0, floor, ceil), ceil);
    assert_eq!(w.level_within(1.0e6, floor, ceil), floor);
    let at_l3 = w.width_at(ToolbarLevel(3));
    assert_eq!(w.level_within(at_l3, floor, ceil), ToolbarLevel(3));
    assert_eq!(w.level_within(0.0, ToolbarLevel(0), max), max);
}
