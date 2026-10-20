//! Per-hour labels stay present alongside the production bars without live report reads.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::analytics::HourStat;
use moon_ui::{MoonPalette, MoonTheme, h_flex};

/// Synthetic hourly aggregates exercise both profit signs and count degradation.
struct HourProbe {
    counts: bool,
}

impl Render for HourProbe {
    /// Render all twenty-four bars inside their actual production card and header.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bars = h_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .children((0..24).map(|hour| {
                super::hour_column(
                    hour,
                    HourStat {
                        trades: 17,
                        profit: if hour % 2 == 0 { 271.25 } else { -43.75 },
                        ..Default::default()
                    },
                    271.25,
                    self.counts,
                    MoonPalette::active(cx),
                    cx,
                )
            }))
            .into_any_element();
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
            for (width, counts) in [(1200.0, true), (300.0, false)] {
                let window = cx.open_window(gpui::size(px(width), px(250.0)), |_, _| HourProbe {
                    counts,
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
