//! Per-hour labels stay present alongside the production bars without live report reads.

use gpui::{Context, IntoElement, Render, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::analytics::HourStat;
use moon_ui::{MoonPalette, MoonTheme};

/// Synthetic hourly aggregates exercise both profit signs and count degradation.
struct HourProbe {
    side_collapsed: bool,
}

impl Render for HourProbe {
    /// Render all twenty-four bars inside their actual production card and header.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profile = std::array::from_fn(|hour| HourStat {
            trades: 17,
            profit: if hour % 2 == 0 { 271.25 } else { -43.75 },
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
