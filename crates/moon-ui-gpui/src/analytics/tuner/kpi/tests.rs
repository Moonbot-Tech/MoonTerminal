//! Regression coverage for fixed-width Fact-versus-variant KPI values.

use super::{
    CellFormat, FigureTone, KpiCellText, MetricKind, VarLabel, figure_of, format_kpi_cell,
    kpi_columns, plain_factor, unclosed_cell,
};
use crate::analytics::set_pnl_unit;
use moon_core::db::ProfitUnit;
use moon_core::db::tuner::VarStats;

/// Reintroducing signed PF or positive drawdown would mislabel both schedule and filter splits.
#[test]
fn polish_split_metrics_share_matrix_signs() {
    set_pnl_unit(None);
    let mut tally = moon_core::db::metrics::Tally::default();
    tally.push(162.0);
    tally.push(-100.0);
    assert_eq!(super::split_metrics(&tally), ("1.62".into(), "-100".into()));
}

/// Compare actual laid-out text against the larger body band, without a raw pixel oracle.
struct TypographyProbe;

/// Exercise both reported subheading families in the real matrix.
struct HeadingProbe {
    mode: usize,
}

impl gpui::Render for HeadingProbe {
    /// Include the By-coin small-sample warning and Entry/Exit Fact explanation.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let fact = if self.mode == 3 {
            VarLabel::with_sub(
                rust_i18n::t!("analytics.tuner.fact").to_string(),
                rust_i18n::t!("analytics.ticks.kpi.fact_sub").to_string(),
            )
        } else {
            VarLabel::new(rust_i18n::t!("analytics.tuner.fact").to_string())
        };
        let labels = if self.mode == 2 {
            vec![
                VarLabel::with_sub(
                    rust_i18n::t!("analytics.coins.manual").to_string(),
                    rust_i18n::t!("analytics.coins.v1_lists", bl = 1, wl = 0).to_string(),
                ),
                VarLabel::with_sub(
                    rust_i18n::t!("analytics.coins.auto_col").to_string(),
                    rust_i18n::t!("analytics.coins.auto_sub_few").to_string(),
                ),
            ]
        } else {
            vec![VarLabel::new("v1".into()), VarLabel::new("v2".into())]
        };
        super::kpi_matrix_body(
            &crate::load_state::LoadState::Ready(std::sync::Arc::new(vec![
                VarStats {
                    n: 15,
                    ..Default::default()
                };
                3
            ])),
            String::new(),
            &fact,
            &labels,
            false,
            std::rc::Rc::new(|_, _| {}),
            moon_ui::MoonPalette::active(cx),
            cx,
        )
    }
}

/// Unequal widths or wrapping statuses must not inflate the coin header above the other modes.
#[gpui::test]
fn polish_subheadings_fit_every_locale(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for width in [320.0, super::super::layout::SIDE_W] {
            let mut header_height = None;
            for mode in 0..4 {
                let window = cx.open_window(gpui::size(px(width), px(600.0)), |_, _| {
                    HeadingProbe { mode }
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let header = visual.debug_bounds("kpi-header").unwrap();
                if width == super::super::layout::SIDE_W {
                    let expected = *header_height.get_or_insert(header.size.height);
                    assert!(
                        (header.size.height - expected).abs() <= px(0.5),
                        "{} mode {mode}: header height differs",
                        language.code()
                    );
                }
                let first = visual.debug_bounds("kpi-head-0").unwrap();
                for (head, title) in [
                    ("kpi-head-0", "kpi-title-0"),
                    ("kpi-head-1", "kpi-title-1"),
                    ("kpi-head-2", "kpi-title-2"),
                ] {
                    let column = visual.debug_bounds(head).unwrap();
                    assert!((column.size.width - first.size.width).abs() <= px(0.5));
                    let title = visual.debug_bounds(title).unwrap();
                    assert!(
                        title.size.width <= column.size.width + px(0.5),
                        "{} {width} mode {mode} {head}: title {:?} exceeds column {:?}",
                        language.code(),
                        title.size,
                        column.size
                    );
                }
                for selector in match mode {
                    2 => vec!["kpi-sub-1", "kpi-sub-2"],
                    3 => vec!["kpi-sub-0"],
                    _ => Vec::new(),
                } {
                    let bounds = visual.debug_bounds(selector).unwrap();
                    let line_height = visual.debug_bounds("kpi-value-0-0").unwrap().size.height;
                    assert!(
                        bounds.size.height <= line_height + px(0.5),
                        "{} {selector} wrapped",
                        language.code()
                    );
                    assert!(
                        bounds.right() <= px(width + 0.5),
                        "subheading outside shared pane"
                    );
                }
            }
        }
    }
}

/// Exercise the localized supplemental metric at actual narrow dock widths.
struct NarrowUnclosedProbe;

impl gpui::Render for NarrowUnclosedProbe {
    /// The production matrix receives an unclosed row and a synthetic two-column result.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        super::kpi_matrix_body_with(
            &crate::load_state::LoadState::Ready(std::sync::Arc::new(vec![
                VarStats {
                    n: 125,
                    ..Default::default()
                };
                2
            ])),
            String::new(),
            &VarLabel::new("Fact".into()),
            &[VarLabel::new("v1".into())],
            false,
            std::rc::Rc::new(|_, _| {}),
            moon_ui::MoonPalette::active(cx),
            cx,
            super::KpiOpts {
                rows: super::KpiRows::Plain,
                unclosed: Some((6, 8.25)),
                ..Default::default()
            },
        )
    }
}

/// Enlarging the body band must not let the unclosed label overpaint Fact or V1 in a narrow dock.
#[gpui::test]
fn livefix_unclosed_body_label_fits_without_overpainting_values(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    for theme in [
        moon_core::config::UiThemeMode::Light,
        moon_core::config::UiThemeMode::Dark,
        moon_core::config::UiThemeMode::Graphite,
    ] {
        cx.update(|cx| {
            moon_ui::MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in moon_core::config::Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for width in [320.0, 400.0, 687.0] {
                let window =
                    cx.open_window(gpui::size(px(width), px(700.0)), |_, _| NarrowUnclosedProbe);
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let label = visual.debug_bounds("kpi-unclosed-label").unwrap();
                let need = visual.update(|_, cx| {
                    crate::design::ui_body_text_width(
                        cx,
                        &rust_i18n::t!("analytics.ticks.kpi.unclosed"),
                        400.0,
                    )
                });
                assert!(
                    label.size.width + px(0.5) >= px(need),
                    "unclosed body label clipped in {} at {width}",
                    language.code()
                );
                for selector in ["kpi-unclosed-0", "kpi-unclosed-1"] {
                    let value = visual.debug_bounds(selector).unwrap();
                    assert!(
                        label.right() <= value.left() || label.bottom() <= value.top(),
                        "unclosed label overpaints a value"
                    );
                }
            }
        }
    }
}

impl gpui::Render for TypographyProbe {
    /// Render a body reference next to the production KPI matrix and Parameters header.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement, IntoElement, ParentElement, Styled};
        let p = moon_ui::MoonPalette::active(cx);
        moon_ui::v_flex()
            .w_full()
            .child(
                gpui::div()
                    .debug_selector(|| "body-reference".into())
                    .text_size(crate::design::t_body(cx))
                    .child("125"),
            )
            .child(
                super::kpi_matrix_body(
                    &crate::load_state::LoadState::Ready(std::sync::Arc::new(vec![
                        VarStats {
                            n: 125,
                            ..Default::default()
                        };
                        3
                    ])),
                    String::new(),
                    &VarLabel::new("Fact".into()),
                    &[],
                    false,
                    std::rc::Rc::new(|_, _| {}),
                    p,
                    cx,
                )
                .into_any_element(),
            )
    }
}

/// Reverting KPI labels or values to caption text visibly shrinks them relative to body text.
#[gpui::test]
fn livefix_kpi_uses_body_band(cx: &mut gpui::TestAppContext) {
    use gpui::px;
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        )
    });
    let _locale = crate::test_locale::force("en");
    let window = cx.open_window(gpui::size(px(620.0), px(900.0)), |_, _| TypographyProbe);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let reference = visual.debug_bounds("body-reference").unwrap();
    for selector in ["kpi-metric-0", "kpi-value-0-0"] {
        let actual = visual.debug_bounds(selector).unwrap();
        assert_eq!(
            actual.size.height, reference.size.height,
            "KPI typography must use the shared body band"
        );
    }
}

/// Changing `kpi.rs:format_kpi_cell` to return the exact formatter for every finite magnitude must
/// fail the large-value assertions; otherwise All-period Fact/v1/v2 values wrap or overpaint
/// adjacent cells and rows. Removing the active percent suffix from its compact profit branch must
/// fail the percent assertion, preventing abbreviated PnL from being displayed in the wrong unit.
#[test]
fn all_period_large_values_use_compact_text_and_keep_exact_tooltips() {
    set_pnl_unit(None);

    assert_eq!(
        format_kpi_cell(CellFormat::Integer, 454_520_257_399.0),
        KpiCellText {
            display: "455B".to_string(),
            tooltip: Some("454520257399".to_string()),
        }
    );
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 4_175_275_332_081.0),
        KpiCellText {
            display: "+4.18T".to_string(),
            tooltip: Some("+4175275332081".to_string()),
        }
    );
    assert_eq!(
        plain_factor(392_964_096.0),
        KpiCellText {
            display: "393M".to_string(),
            tooltip: Some("392964096.00".to_string()),
        }
    );

    assert_eq!(
        format_kpi_cell(CellFormat::Integer, 365.0),
        KpiCellText {
            display: "365".to_string(),
            tooltip: None,
        }
    );
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 71.41),
        KpiCellText {
            display: "+71.41".to_string(),
            tooltip: None,
        }
    );
    assert_eq!(
        plain_factor(2.34),
        KpiCellText {
            display: "2.34".to_string(),
            tooltip: None,
        }
    );

    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 1_234_567.0),
        KpiCellText {
            display: "+1.23M%".to_string(),
            tooltip: Some("+1234567%".to_string()),
        }
    );
    set_pnl_unit(None);
}

fn column(
    n: i64,
    wins: i64,
    profit: f64,
    pf: f64,
    avg_win: f64,
    avg_loss: f64,
    max_dd: f64,
) -> VarStats {
    VarStats {
        n,
        wins,
        profit,
        pf,
        avg: if n > 0 { profit / n as f64 } else { 0.0 },
        avg_win,
        avg_loss,
        max_dd,
        ..VarStats::default()
    }
}

/// A loss magnitude must not paint as a green plus, in either column: the same function paints
/// every column. Zero winning deals leave win rate at 0% (it has a unit), profit factor at
/// 0.00, and average win undefined.
#[test]
fn loss_metrics_are_negative_and_undefined_ratios_are_a_dash() {
    set_pnl_unit(None);
    let losing = column(2, 0, -138.32, 0.0, 0.0, 69.16, 138.32);
    let loss = figure_of(MetricKind::AvgLoss, &losing);
    assert_eq!(loss.text.display, "-69.16");
    assert_eq!(loss.tone, FigureTone::Signed);
    assert!(loss.signed < 0.0);
    let dd = figure_of(MetricKind::MaxDrawdown, &losing);
    assert_eq!(dd.text.display, "-138.32");
    assert_eq!(dd.tone, FigureTone::Signed);
    assert_eq!(figure_of(MetricKind::Winrate, &losing).text.display, "0.0%");
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &losing).text.display,
        "0.00"
    );
    assert_eq!(figure_of(MetricKind::AvgWin, &losing).text.display, "—");
    assert_eq!(
        figure_of(MetricKind::Winrate, &losing).tone,
        FigureTone::Neutral
    );
    let empty = column(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(figure_of(MetricKind::Winrate, &empty).text.display, "—");
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &empty).tone,
        FigureTone::Muted
    );
    assert_eq!(figure_of(MetricKind::AvgLoss, &empty).text.display, "—");
    assert_eq!(figure_of(MetricKind::MaxDrawdown, &empty).text.display, "—");
    // The same inputs paint the same text for every column. A second call is the other column.
    assert_eq!(
        figure_of(MetricKind::Profit, &losing).text.display,
        figure_of(MetricKind::Profit, &losing).text.display
    );
    assert_eq!(
        figure_of(MetricKind::Profit, &losing).tone,
        FigureTone::Signed
    );
    set_pnl_unit(None);
}

/// Only break-even trades have a finite profit factor of 0, because both sums are zero.
/// Painting that `0.00` says the ratio is defined. All-winners stay at `99.00`, which is
/// the figure the matrix already shows when the loss sum is zero and the win sum is not.
/// All-losers stay at `0.00`: their loss average is positive.
///
/// Dropping the both-sums-zero guard from `figure_of` turns the break-even assertion red
/// and the cell reads `0.00`. Treating 99 as undefined turns the all-winners assertion red.
#[test]
fn break_even_profit_factor_is_a_dash_and_all_winners_stay_99() {
    set_pnl_unit(None);
    let even = column(4, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let pf = figure_of(MetricKind::ProfitFactor, &even);
    assert_eq!(pf.text.display, "—");
    assert_eq!(pf.tone, FigureTone::Muted);
    let winners = column(3, 3, 30.0, 99.0, 10.0, 0.0, 0.0);
    let shown = figure_of(MetricKind::ProfitFactor, &winners);
    assert_eq!(shown.text.display, "99.00");
    assert_eq!(shown.tone, FigureTone::Neutral);
    let losers = column(2, 0, -10.0, 0.0, 0.0, 5.0, 10.0);
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &losers).text.display,
        "0.00"
    );
    set_pnl_unit(None);
}

/// Folding open estimates into the profit cell would misstate the scored result.
#[test]
fn unclosed_estimates_have_their_own_cell_and_never_join_profit() {
    let _locale = crate::test_locale::force("en");
    set_pnl_unit(None);
    let stats = VarStats {
        n: 3,
        profit: -4.94,
        ..VarStats::default()
    };
    assert_eq!(unclosed_cell(0, 7, 12.75).display, "—");
    assert_eq!(unclosed_cell(1, 7, 12.75).display, "7 · ≈ +12.75");
    let figure = figure_of(MetricKind::Profit, &stats);
    assert_eq!(figure.text.display, "-4.94");
    assert_eq!(figure.tone, FigureTone::Signed);
    assert!((figure.signed + 4.94).abs() < 1e-12);
}

/// A promised column is drawn without stats; a plain surplus label is not — so an axis that
/// passes more labels than variants keeps its column count.
#[test]
fn only_a_placeholder_label_adds_a_column() {
    let label = |t: &str| VarLabel::new(t.to_string());
    let coins = [label("Manual"), label("Auto").placeholder()];
    assert_eq!(
        kpi_columns(2, &coins),
        3,
        "Fact, Manual and the promised Auto"
    );
    assert_eq!(kpi_columns(3, &coins), 3, "scored: the stats decide");
    let other = [label("v1"), label("v2"), label("v3")];
    assert_eq!(kpi_columns(2, &other), 2, "other axes unchanged");
    assert_eq!(kpi_columns(4, &other), 4);
}
