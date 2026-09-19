//! Adaptive-fit and manual-strategy target regressions for the trading toolbar.

use super::{
    ICON_BTN_W, LabelLadder, LabelWidths, TOOLBAR_LAUNCHER_BORDER_W, TOOLBAR_LAUNCHER_PAD_X,
    label_ladder, launcher_label_width, manual_strategy_core,
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
