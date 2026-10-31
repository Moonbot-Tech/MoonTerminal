//! Per-hour labels stay present alongside the production bars without live report reads.

use gpui::{Context, IntoElement, Render, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::analytics::HourStat;
use moon_ui::{MoonPalette, MoonTheme};

/// The eligible draft path must not silently grow another full-profile or strategy-baseline read.
#[test]
fn local_schedule_rescore_reads_only_variant_kpis() {
    let source = include_str!("mod.rs");
    let rescore = source
        .split_once("pub(in crate::analytics::tuner) fn rescore_time_draft")
        .unwrap()
        .1
        .split_once("pub(in crate::analytics) fn reload_time_after_report")
        .unwrap()
        .0;
    assert!(rescore.contains("moon_core::db::tuner::variant_stats(&query, &variants)"));
    assert!(rescore.contains("ReadLane::Time"));
    assert!(rescore.contains("CatchUpOutcome::of_read(&stats)"));
    assert!(rescore.contains("settle_report_refresh_retry(stats_outcome.is_transient(), cx)"));
    assert!(rescore.contains("this.time_tuner.stats.apply(stats)"));
    assert!(!rescore.contains("time_tuner_data"));
    assert!(!rescore.contains("profiles.begin()"));
    assert!(!rescore.contains("slider.begin()"));
    assert!(!rescore.contains("strategy_current_values_opt"));
    let grid = include_str!("grid.rs");
    let sliders = include_str!("sliders.rs");
    assert!(!grid.contains(".reload_time(cx)"));
    assert!(!sliders.contains(".reload_time(cx)"));
}

/// Exact decimals or repeated currency tickers would consume the default hour's entire slot.
#[test]
fn polish_hour_money_is_compact() {
    crate::analytics::set_pnl_unit(None);
    assert_eq!(super::hour_profit(462.79), "+463");
    assert_eq!(super::hour_profit(1400.0), "+1.4K");
    assert_eq!(super::hour_profit(-462.79), "-463");
}

/// Measure actual production labels so merely retaining a clipped text node does not pass.
#[gpui::test]
fn polish_hour_labels_fit_default_width(cx: &mut gpui::TestAppContext) {
    crate::analytics::set_pnl_unit(None);
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let window = cx.open_window(gpui::size(px(1367.0), px(300.0)), |_, _| HourProbe {
            side_collapsed: true,
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        for hour in 0..24 {
            let bounds = visual
                .debug_bounds(Box::leak(
                    format!("tuner-hour-profit-{hour}").into_boxed_str(),
                ))
                .unwrap();
            let text = visual
                .debug_bounds(Box::leak(
                    format!("tuner-hour-profit-content-{hour}").into_boxed_str(),
                ))
                .unwrap();
            assert!(
                text.left() >= bounds.left() && text.right() <= bounds.right(),
                "{} hour {hour} clipped",
                language.code()
            );
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("tuner-hour-count-{hour}").into_boxed_str()
                    ))
                    .is_some()
            );
        }
    }
}

/// Synthetic hourly aggregates exercise both profit signs and count degradation.
struct HourProbe {
    side_collapsed: bool,
}

impl Render for HourProbe {
    /// Render all twenty-four bars inside their actual production card and header.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = std::array::from_fn(|hour| HourStat {
            trades: 17,
            profit: if hour % 2 == 0 { 462.79 } else { -1400.0 },
            ..Default::default()
        });
        let bars = super::hour_columns(
            &profile,
            f32::from(window.viewport_size().width),
            self.side_collapsed,
            MoonPalette::active(cx),
            cx,
        );
        super::hour_card(
            bars,
            String::new(),
            gpui::div().into_any_element(),
            MoonPalette::active(cx),
            cx,
        )
    }
}

/// Removing either label from normal-width bars fails; narrow bars retain profit before count.
#[gpui::test]
fn livefix_every_hour_has_profit_and_count_labels(cx: &mut gpui::TestAppContext) {
    for theme in [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for (width, side_collapsed, counts) in [
                (2269.0, false, true),
                (1200.0, false, false),
                (1200.0, true, true),
                (300.0, false, false),
            ] {
                let window = cx.open_window(gpui::size(px(width), px(250.0)), |_, _| HourProbe {
                    side_collapsed,
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                for hour in 0..24 {
                    let profit = visual
                        .debug_bounds(Box::leak(
                            format!("tuner-hour-profit-{hour}").into_boxed_str(),
                        ))
                        .expect("hourly profit label");
                    assert!(profit.size.height > px(0.0));
                    let card = visual.debug_bounds("time-hours-card").unwrap();
                    let header = visual.debug_bounds("time-hours-header").unwrap();
                    let hour_label = visual
                        .debug_bounds(Box::leak(
                            format!("tuner-hour-label-{hour}").into_boxed_str(),
                        ))
                        .unwrap();
                    assert!(
                        profit.top() >= header.bottom(),
                        "hourly profit overlaps its card header"
                    );
                    assert!(
                        hour_label.bottom() <= card.bottom(),
                        "hour label clipped by the card frame"
                    );
                    let count = visual.debug_bounds(Box::leak(
                        format!("tuner-hour-count-{hour}").into_boxed_str(),
                    ));
                    assert_eq!(count.is_some(), counts);
                    if let Some(count) = count {
                        assert!(count.top() >= profit.bottom());
                    }
                }
            }
        }
    }
}
