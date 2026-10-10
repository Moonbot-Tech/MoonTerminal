//! Synthetic money and actual GPUI layout regressions for the day surface.

use super::super::days_model::{DayReport, DayRow};
use super::{
    DayHeader, begin_days_read, core_count_noun, day_average, day_header, day_profit, day_table,
    total_cells,
};
use crate::design;
use crate::load_state::LoadState;
use chrono::{NaiveDate, TimeZone, Utc};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, px};
use moon_core::db::{QuoteBreakdown, UsdtTotal, ValuationCoverage};
use moon_ui::{MoonPalette, MoonTheme, v_flex};

/// Synthetic valued money has an independent expected display amount and deal count.
fn quotes(profit: f64, orders: i64) -> QuoteBreakdown {
    QuoteBreakdown::from_groups([(Some(0), profit, orders)]).with_valuation(ValuationCoverage {
        eligible_orders: orders,
        valued_orders: orders,
        unavailable_orders: 0,
        usdt: Some(UsdtTotal {
            profit,
            spent: None,
        }),
    })
}

/// Withholding incomplete valuation must survive even when native money remains useful.
#[test]
fn incomplete_money_never_becomes_zero_and_sign_follows_rounding() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(day_profit(&quotes(-0.001, 1)).0, "0.00$");
    assert_eq!(day_profit(&quotes(12.125, 3)).0, "+12.13$");
    assert_eq!(
        day_profit(&QuoteBreakdown::from_groups([(Some(0), 25.0, 1)])).0,
        rust_i18n::t!("telegram.report_unvalued").to_string()
    );
    assert_eq!(
        day_average(&QuoteBreakdown::default()),
        rust_i18n::t!("telegram.report_unvalued").to_string()
    );
}

/// A misspelled translation key must fail against the mockup's approved Russian total caption.
#[test]
fn total_cells_use_the_approved_caption_instead_of_a_raw_translation_key() {
    {
        let _locale = crate::test_locale::force("ru");
        assert_eq!(total_cells(&quotes(12.0, 3))[0], "Итого");
    }
    let _locale = crate::test_locale::force("en");
    assert_eq!(total_cells(&quotes(12.0, 3))[0], "Total");
}

/// Resetting background reads like month navigation would hide figures on every new close.
#[test]
fn background_read_retains_figures_but_navigation_and_failure_withhold_them() {
    let report = DayReport {
        refreshed: Utc.with_ymd_and_hms(2024, 3, 10, 17, 16, 0).unwrap(),
        rows: vec![DayRow {
            date: NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
            quotes: quotes(12.0, 3),
        }],
        total: quotes(12.0, 3),
    };
    let mut state = LoadState::default();
    state.apply(Ok(report.clone()));
    begin_days_read(&mut state, true);
    assert!(
        matches!(&state, LoadState::Loading { stale: Some(_) }),
        "catch-up must remain visibly populated"
    );
    assert!(
        state.data().is_some(),
        "the renderer must retain its month rows during catch-up"
    );
    state.apply(Err(moon_core::db::ReadFail::IncomparableQuote));
    assert!(
        state.data().is_none(),
        "a completed failure must remove figures even after catch-up"
    );
    state.apply(Ok(report));
    begin_days_read(&mut state, false);
    assert!(
        matches!(&state, LoadState::Loading { stale: None }),
        "different months must never inherit old figures"
    );
}

/// Using last-digit rules for English or forgetting Slavic teens breaks 21 cores and 11 cores.
#[test]
fn core_captions_use_real_count_grammar_instead_of_empty_templates() {
    for (locale, cases) in [
        (
            "ru",
            vec![
                (1, "ядро"),
                (2, "ядра"),
                (5, "ядер"),
                (11, "ядер"),
                (12, "ядер"),
                (21, "ядро"),
                (42, "ядра"),
            ],
        ),
        (
            "uk",
            vec![
                (1, "ядро"),
                (2, "ядра"),
                (5, "ядер"),
                (11, "ядер"),
                (21, "ядро"),
            ],
        ),
        (
            "en",
            vec![(1, "core"), (2, "cores"), (11, "cores"), (21, "cores")],
        ),
        ("es", vec![(1, "núcleo"), (21, "núcleos")]),
        ("pt", vec![(1, "núcleo"), (21, "núcleos")]),
        ("tr", vec![(1, "çekirdek"), (21, "çekirdek")]),
        ("vi", vec![(1, "lõi"), (21, "lõi")]),
    ] {
        let _locale = crate::test_locale::force(locale);
        for (count, noun) in cases {
            assert_eq!(core_count_noun(count), noun, "core caption grammar");
        }
    }
}

/// A backend-free host renders the production header and table, including the expanded column.
struct DaysLayoutProbe {
    report: DayReport,
    extra: bool,
}

impl Render for DaysLayoutProbe {
    /// Lay out the actual report at the monitor's default logical width, without launching the app.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = crate::window::windowing::responsive_width(window);
        let palette = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .font_family(design::mono())
            .child(day_header(
                DayHeader {
                    month: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                    now: self.report.refreshed,
                    zone: chrono_tz::Europe::Warsaw,
                    count: 42,
                    width,
                    palette,
                },
                cx,
                (|_, _| {}, |_, _| {}),
            ))
            .child(div().px(px(12.0)).child(day_table(
                &self.report,
                self.extra,
                width,
                chrono_tz::Europe::Warsaw,
                palette,
                cx,
            )))
    }
}

/// Shrinking fitted columns or allowing header flex shrink clips text in non-English locales.
#[gpui::test]
fn every_locale_lays_out_the_header_and_cells_inside_the_default_window(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Dark,
                1.0,
            ),
            cx,
        )
    });
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            for extra in [false, true] {
                let report = DayReport {
                    refreshed: Utc.with_ymd_and_hms(2024, 3, 10, 17, 16, 0).unwrap(),
                    rows: vec![DayRow {
                        date: NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
                        quotes: quotes(-412.60, 140),
                    }],
                    total: quotes(-412.60, 140),
                };
                let measured = cx.update(|cx| {
                    [
                        design::mono_caption_text_width(cx, "2024-03-10", 600.0),
                        design::mono_caption_text_width(cx, "-412.60$", 600.0),
                        design::mono_caption_text_width(cx, "140", 600.0),
                        design::mono_caption_text_width(cx, &day_average(&report.total), 600.0),
                    ]
                });
                let window = cx.open_window(gpui::size(px(720.0), px(520.0)), move |_, _| {
                    DaysLayoutProbe { report, extra }
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| {
                    window.set_content_zoom(zoom, cx);
                });
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let row = visual.debug_bounds("days-header").expect("header laid out");
                assert!(
                    (f32::from(row.size.width) - 720.0 / zoom).abs() < 1.0,
                    "probe must use the zoomed default viewport"
                );
                let group = visual
                    .debug_bounds("days-header-group")
                    .expect("period group laid out");
                let period = visual.debug_bounds("days-period").expect("period laid out");
                let title = visual
                    .debug_bounds("days-header-title")
                    .expect("title laid out");
                let count = visual
                    .debug_bounds("days-core-count")
                    .expect("scope count laid out");
                assert!(
                    group.origin.x >= row.origin.x && group.right() <= row.right(),
                    "centred header group escaped the row"
                );
                assert!(
                    (f32::from(
                        group.origin.x + group.size.width / 2.0
                            - row.origin.x
                            - row.size.width / 2.0
                    ))
                    .abs()
                        < 1.0,
                    "period group is not centred"
                );
                let period_width = visual.update(|_, cx| {
                    design::mono_caption_text_width(cx, "01.03 — 10.03 18:16", 600.0)
                });
                assert!(
                    f32::from(period.size.width) + 0.1 >= period_width,
                    "period text truncated"
                );
                assert!(f32::from(group.size.height) < 40.0, "header wrapped");
                let title_width = visual.update(|_, cx| {
                    design::ui_caption_text_width(
                        cx,
                        &rust_i18n::t!("profit_monitor.days.title"),
                        600.0,
                    )
                });
                assert!(
                    f32::from(title.size.width) + 0.1 >= title_width,
                    "header title truncated"
                );
                assert!(
                    count.origin.x >= group.right() && count.right() <= row.right(),
                    "scope count overlaps centred group or escapes row"
                );
                for col in 0..if extra { 4 } else { 3 } {
                    let cell = visual
                        .debug_bounds(
                            [
                                "days-cell-0-0",
                                "days-cell-0-1",
                                "days-cell-0-2",
                                "days-cell-0-3",
                            ][col],
                        )
                        .expect("day cell laid out");
                    assert!(
                        f32::from(cell.size.width) + 0.1 >= measured[col],
                        "cell narrower than text"
                    );
                    assert!(
                        cell.origin.x >= row.origin.x && cell.right() <= row.right(),
                        "cell escaped window"
                    );
                    let heading = visual
                        .debug_bounds(
                            [
                                "days-cell-head-0",
                                "days-cell-head-1",
                                "days-cell-head-2",
                                "days-cell-head-3",
                            ][col],
                        )
                        .expect("heading laid out");
                    assert!(heading.right() <= row.right(), "heading escaped window");
                    let total = visual
                        .debug_bounds(
                            [
                                "days-cell-total-0",
                                "days-cell-total-1",
                                "days-cell-total-2",
                                "days-cell-total-3",
                            ][col],
                        )
                        .expect("total laid out");
                    assert!(total.right() <= row.right(), "total escaped window");
                }
            }
        }
    }
}
