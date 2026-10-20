//! Verdict-color regression shared by filter, time and the displayed holdout baseline.

use moon_core::db::metrics::Tally;

/// Reverting to profit-sign coloring would advertise positive but inferior unseen profit as green.
#[gpui::test]
fn positive_below_fact_is_danger(cx: &mut gpui::TestAppContext) {
    let mut holdout = Tally::default();
    holdout.push(5.0);
    let mut fact = Tally::default();
    fact.push(6.0);
    for theme in [
        moon_core::config::UiThemeMode::Light,
        moon_core::config::UiThemeMode::Dark,
        moon_core::config::UiThemeMode::Graphite,
    ] {
        cx.update(|cx| {
            moon_ui::MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            );
            let p = moon_ui::MoonPalette::active(cx);
            assert_eq!(
                super::holdout_color(holdout.profit, Some(fact.profit), p),
                crate::design::danger_color(p)
            );
            assert_eq!(
                super::holdout_color(holdout.profit, None, p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(fact.profit, Some(holdout.profit), p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(-5.0, Some(-6.0), p),
                crate::design::positive_color(p)
            );
            assert_eq!(
                super::holdout_color(-5.0, None, p),
                crate::design::danger_color(p)
            );
        });
    }
}
