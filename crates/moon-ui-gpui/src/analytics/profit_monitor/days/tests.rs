//! Synthetic money and actual GPUI layout regressions for the day surface.

use super::super::days_model::{DayReport, DayRow};
use super::{
    DayHeader, begin_days_read, core_count_noun, day_header, day_profit, day_table, newest_rows,
    table_text_width, total_cells,
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
}

/// A misspelled translation key must fail against the mockup's approved Russian total caption.
#[test]
fn total_cells_use_the_approved_caption_instead_of_a_raw_translation_key() {
    {
        let _locale = crate::test_locale::force("ru");
        assert_eq!(total_cells(&quotes(12.0, 3))[0], "Итого за период");
    }
    let _locale = crate::test_locale::force("en");
    assert_eq!(total_cells(&quotes(12.0, 3))[0], "Total for period");
}

/// Ascending reader order must not put old days above today's closes or a past month's last day.
#[test]
fn current_and_past_months_display_newest_first_without_changing_money() {
    for dates in [["2024-03-01", "2024-03-10"], ["2024-02-01", "2024-02-29"]] {
        let report = DayReport {
            refreshed: Utc.with_ymd_and_hms(2024, 3, 10, 17, 16, 0).unwrap(),
            rows: dates
                .iter()
                .enumerate()
                .map(|(index, date)| DayRow {
                    date: date.parse().unwrap(),
                    quotes: quotes(if index == 0 { 12.0 } else { -7.0 }, index as i64 + 1),
                })
                .collect(),
            total: quotes(5.0, 3),
        };
        let displayed = newest_rows(&report);
        assert_eq!(
            displayed
                .iter()
                .map(|row| row.date.to_string())
                .collect::<Vec<_>>(),
            [dates[1], dates[0]]
        );
        assert_eq!(day_profit(&displayed[0].quotes).0, "-7.00$");
        assert_eq!(displayed[0].quotes.orders, 2);
        assert_eq!(day_profit(&report.total).0, "+5.00$");
        assert_eq!(report.rows[0].date.to_string(), dates[0]);
    }
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

/// A backend-free host renders the production header, separate total and day table.
struct DaysLayoutProbe {
    report: DayReport,
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
            for viewport in [720.0, 400.0] {
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
                        table_text_width(cx, "2024-03-10", 400.0, false),
                        table_text_width(cx, "-412.60$", 400.0, false),
                        table_text_width(cx, "140", 400.0, false),
                    ]
                });
                let window = cx.open_window(gpui::size(px(viewport), px(520.0)), move |_, _| {
                    DaysLayoutProbe { report }
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
                    (f32::from(row.size.width) - viewport / zoom).abs() < 1.0,
                    "probe must use the zoomed default viewport"
                );
                let group = visual
                    .debug_bounds("days-header-group")
                    .expect("period group laid out");
                let period = visual.debug_bounds("days-period").expect("period laid out");
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
                let period_width = visual
                    .update(|_, cx| table_text_width(cx, "01.03 — 10.03 18:16", 600.0, false));
                assert!(
                    f32::from(period.size.width) + 0.1 >= period_width,
                    "period text truncated"
                );
                assert!(f32::from(group.size.height) < 40.0, "header wrapped");
                let title_width = visual.update(|_, cx| {
                    table_text_width(
                        cx,
                        &rust_i18n::t!("profit_monitor.days.title"),
                        600.0,
                        false,
                    )
                });
                if let Some(title) = visual.debug_bounds("days-header-title") {
                    assert!(
                        f32::from(title.size.width) + 0.1 >= title_width,
                        "header title truncated"
                    );
                } else {
                    assert!(viewport < 720.0, "default window must retain the title");
                }
                if let Some(count) = visual.debug_bounds("days-core-count") {
                    assert!(
                        count.origin.x >= group.right() && count.right() <= row.right(),
                        "scope count overlaps centred group or escapes row"
                    );
                }
                let summary = visual
                    .debug_bounds("days-total")
                    .expect("separate total laid out");
                let table = visual
                    .debug_bounds("days-table")
                    .expect("day table laid out");
                assert!(
                    summary.origin.y >= row.bottom() && summary.bottom() <= table.origin.y,
                    "total must be between header and day table"
                );
                for col in 0..3 {
                    let cell = visual
                        .debug_bounds(["days-cell-0-0", "days-cell-0-1", "days-cell-0-2"][col])
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
                            ["days-cell-head-0", "days-cell-head-1", "days-cell-head-2"][col],
                        )
                        .expect("heading laid out");
                    assert!(heading.right() <= row.right(), "heading escaped window");
                    let heading_width = visual.update(|_, cx| {
                        let key = [
                            "profit_monitor.days.date",
                            "profit_monitor.days.result",
                            "profit_monitor.days.trades",
                        ][col];
                        table_text_width(cx, &rust_i18n::t!(key), 400.0, true)
                    });
                    assert!(
                        f32::from(heading.size.width) + 0.1 >= heading_width,
                        "heading narrower than its text"
                    );
                    let total = visual
                        .debug_bounds(
                            [
                                "days-cell-total-0",
                                "days-cell-total-1",
                                "days-cell-total-2",
                            ][col],
                        )
                        .expect("total laid out");
                    assert!(total.right() <= row.right(), "total escaped window");
                    assert!(
                        (f32::from(total.origin.x - cell.origin.x)).abs() < 1.0,
                        "total column is not aligned with the table"
                    );
                    let total_width = visual.update(|_, cx| {
                        let text = match col {
                            0 => rust_i18n::t!("profit_monitor.days.total").to_string(),
                            1 => "-412.60$".to_string(),
                            _ => "140".to_string(),
                        };
                        table_text_width(cx, &text, 700.0, false)
                    });
                    assert!(
                        f32::from(total.size.width) + 0.1 >= total_width,
                        "total cell narrower than its text"
                    );
                }
            }
        }
    }
}
