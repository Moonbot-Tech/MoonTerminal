//! Adaptive-fit and manual-strategy target regressions for the trading toolbar.

use super::{
    ICON_BTN_W, LAUNCHER_FOLD_ORDER, LabelLadder, LabelWidths, Launcher, LauncherFoldWidths,
    TOOLBAR_LAUNCHER_BORDER_W, TOOLBAR_LAUNCHER_PAD_X, label_ladder, launcher_fold,
    launcher_label_width, manual_strategy_core,
};
use moon_core::config::UiThemeMode;
use moon_ui::MoonButtonSize;

#[test]
/// Regression target: `controls/toolbar.rs:label_ladder` changing an inclusive boundary to a
/// strict comparison hides a complete caption that exactly fits or retains it after clipping.
///
/// The user-visible consequence is a toolbar caption disappearing at its exact fit width or
/// remaining after the trailing window controls begin to clip.
fn adaptive_label_ladder_honors_every_exact_boundary() {
    let widths = LabelWidths {
        icon_only: 100.0,
        size_unit: 10.0,
        size_noun: 20.0,
        settings: 30.0,
        strategies: 40.0,
        analytics: 50.0,
        sell: 60.0,
        max_order_caption: 70.0,
    };
    let expected = |rungs: usize| LabelLadder {
        size_unit: rungs >= 1,
        size_noun: rungs >= 2,
        settings: rungs >= 3,
        strategies: rungs >= 4,
        analytics: rungs >= 5,
        sell: rungs >= 6,
        max_order_caption: rungs >= 7,
    };

    assert_eq!(label_ladder(100.0, widths), expected(0));
    for (index, boundary) in [110.0, 130.0, 160.0, 200.0, 250.0, 310.0, 380.0]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            label_ladder(boundary - 1.0, widths),
            expected(index),
            "one pixel below boundary {boundary}"
        );
        assert_eq!(
            label_ladder(boundary, widths),
            expected(index + 1),
            "exact boundary {boundary}"
        );
    }
    assert_eq!(label_ladder(500.0, widths), expected(7));
}

#[test]
/// Regression target: preferring `active_core` over `hovered_core` makes TP/SL/S applicability
/// describe core A while a click or market hotkey submits through the hovered chart on core B.
fn hovered_chart_core_controls_manual_strategy_applicability() {
    assert_eq!(manual_strategy_core(Some(1), Some(2), true), Some(2));
    assert_eq!(manual_strategy_core(Some(1), Some(2), false), Some(1));
    assert_eq!(manual_strategy_core(None, Some(2), true), Some(2));
}

/// `controls/toolbar.rs:launcher_label_width` must budget the same tier font and gap it draws.
///
/// Breakage: measuring launcher text with the old font channel or a stale icon gap makes the
/// window buttons overlap their labels before row-fit can shed a caption.
#[gpui::test]
fn launcher_label_width_matches_the_drawn_tier_geometry(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    let (actual, expected) = cx.update(|cx| {
        let tier = crate::design::CONTROL_TIER;
        let metrics = tier.control_metrics();
        let text =
            crate::design::ui_text_width_zoomed(cx, "Settings", metrics.font_size, 500.0, false);
        let chrome =
            crate::design::ui_value(cx, MoonButtonSize::tier_icon_size(tier) + metrics.gap)
                + crate::design::ui_value(cx, TOOLBAR_LAUNCHER_PAD_X) * 2.0
                + TOOLBAR_LAUNCHER_BORDER_W;
        (
            launcher_label_width(cx, "Settings"),
            (text + chrome).max(ICON_BTN_W),
        )
    });
    assert_eq!(
        actual, expected,
        "launcher budget must equal drawn geometry"
    );
}

/// Budget shared by the fold tests: a 400 px icon-only row, 30 px per launcher, 30 px overflow.
const FOLD_WIDTHS: LauncherFoldWidths = LauncherFoldWidths {
    icon_only: 400.0,
    launcher: 30.0,
    overflow: 30.0,
};

/// Regression target: `controls/toolbar.rs:launcher_fold` folding while the icon-only row still
/// fits would hide a launcher behind a menu for no reason, and draw an overflow button that
/// competes with the label ladder the row is still shedding.
#[test]
fn launcher_fold_is_empty_while_the_icon_only_row_fits() {
    // Everything fits, labels and all.
    let wide = launcher_fold(1000.0, FOLD_WIDTHS);
    assert_eq!(wide.folded, 0);
    // Labels shed, icons still fit exactly.
    let exact = launcher_fold(400.0, FOLD_WIDTHS);
    assert_eq!(
        exact.folded, 0,
        "no overflow button at the exact icon-only fit"
    );
    assert_eq!(exact.width, 400.0);
    assert!(LAUNCHER_FOLD_ORDER.iter().all(|&l| exact.shows(l)));
}

/// Regression target: a fold order other than the right-to-left clip order would pull a
/// launcher out of the middle of the cluster; Settings, Analytics, Strategies must go first.
#[test]
fn launcher_fold_takes_settings_then_analytics_then_strategies() {
    // One pixel short: folding Settings frees 30 and the overflow costs 30 — still short, so a
    // second launcher (Analytics) goes too.
    let one_short = launcher_fold(399.0, FOLD_WIDTHS);
    assert_eq!(one_short.folded, 2);
    assert!(!one_short.shows(Launcher::Settings));
    assert!(!one_short.shows(Launcher::Analytics));
    assert!(one_short.shows(Launcher::Strategies));

    let three = launcher_fold(340.0, FOLD_WIDTHS);
    assert_eq!(three.folded, 3);
    assert!(!three.shows(Launcher::Strategies));
    assert!(three.shows(Launcher::Screener));
    assert!(three.shows(Launcher::ProfitMonitor));
    assert_eq!(
        &LAUNCHER_FOLD_ORDER[..3],
        &[
            Launcher::Settings,
            Launcher::Analytics,
            Launcher::Strategies
        ]
    );
}

/// Regression target: at the group window's 520 px minimum every launcher must stay reachable —
/// drawn or in the menu — and the overflow button's right edge must stay inside the row.
#[test]
fn launcher_fold_keeps_every_launcher_reachable_at_the_minimum_window() {
    let min_w = 520.0;
    // A row whose trading controls alone take 400 px plus five 30 px launchers.
    let widths = LauncherFoldWidths {
        icon_only: 400.0 + 5.0 * 30.0,
        launcher: 30.0,
        overflow: 30.0,
    };
    let fold = launcher_fold(min_w, widths);
    assert!(fold.folded > 0, "the 550 px row must fold at 520 px");
    assert!(
        fold.width <= min_w,
        "overflow button right edge {} is past the row's {min_w}",
        fold.width
    );
    let drawn = LAUNCHER_FOLD_ORDER
        .iter()
        .filter(|&&l| fold.shows(l))
        .count();
    assert_eq!(drawn + fold.folded, LAUNCHER_FOLD_ORDER.len());

    assert!(!fold.pinned, "a row that fits keeps the button in the flow");

    // The real 520 px case: the trading controls alone are wider than the window. Every launcher
    // sits in the menu, and the overflow button pins itself to the right edge because the end of
    // the flow is off-screen.
    let starved = launcher_fold(
        min_w,
        LauncherFoldWidths {
            icon_only: 700.0 + 5.0 * 30.0,
            ..widths
        },
    );
    assert_eq!(starved.folded, LAUNCHER_FOLD_ORDER.len());
    assert!(LAUNCHER_FOLD_ORDER.iter().all(|&l| !starved.shows(l)));
    assert!(
        starved.pinned,
        "an overflow button past the window edge must pin itself inside the row"
    );
}
