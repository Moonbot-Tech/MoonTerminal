//! The column uses production builders, synthetic values and a short scrollable host.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_core::db::tuner::VarStats;
use moon_core::db::tuner::ticks::params::{ParamKind, ParamSection};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInputState, MoonPalette, MoonTheme, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::super::super::kpi::{KpiOpts, KpiRows, VarLabel, kpi_matrix_body_with};
use super::super::accuracy::{Accuracy, GroupCount, accuracy_footer_el, accuracy_sentence};
use super::super::cfg::{PickBarActs, PickBarModel, pick_bar_el};
use super::super::estimate::pick_all_label;
use super::super::grid::{
    FieldRowParts, current_cell_el, field_name_el, field_row_el, ranges_fit, variant_cell_el,
};
use super::super::ranges::range_cells_el;
use super::super::state::NowValue;
use super::super::summary::{
    ChangeRow, ConfirmModel, ProfitRow, SummaryActs, SummaryModel, Unchecked, Verdict, Why,
    parameters_header, sentence, summary_card_el, tag,
};
use crate::design;
use crate::load_state::LoadState;

/// Compare real editable and read-only row geometry with the same trailing range slots.
struct FixedAlignmentProbe;

impl Render for FixedAlignmentProbe {
    /// Keep the production cells, spacing and range-width decision without Analytics state.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let ranges = ranges_fit(window.viewport_size().width, cx);
        let range = || {
            ranges.then(|| {
                div()
                    .w(super::super::ranges::block_w(cx))
                    .flex_none()
                    .into_any_element()
            })
        };
        v_flex()
            .w_full()
            .child(field_row_el(
                "Editable",
                false,
                FieldRowParts {
                    checkbox: MoonCheckbox::new("review-editable-check")
                        .size(design::CONTROL_TIER)
                        .into_any_element(),
                    name: field_name_el("Editable", false, true, None, p).into_any_element(),
                    current: current_cell_el(
                        "Editable",
                        true,
                        Some(&NowValue::Same("1.31".into())),
                        p,
                        cx,
                    )
                    .into_any_element(),
                    variant: div().into_any_element(),
                    clear: div()
                        .w(design::ui_px(cx, 12.0))
                        .flex_none()
                        .into_any_element(),
                },
                range(),
                p,
                cx,
            ))
            .child(super::super::grid::fixed_row_el(
                "ReadOnly",
                field_name_el("ReadOnly", false, false, None, p).into_any_element(),
                "1.31".into(),
                range(),
                p,
                cx,
            ))
    }
}

/// Restoring either old fixed-row gap or inset must move the numeric edge and fail this check.
#[gpui::test]
fn livefix_read_only_current_values_align_with_editable_rows(cx: &mut gpui::TestAppContext) {
    for theme in [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for width in [400.0, 687.0] {
                let window =
                    cx.open_window(gpui::size(px(width), px(150.0)), |_, _| FixedAlignmentProbe);
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let editable = visual
                    .debug_bounds("an-ticks-current-cell-Editable")
                    .unwrap();
                let fixed = visual.debug_bounds("an-ticks-fixed-current").unwrap();
                assert!(
                    (f32::from(editable.right()) - f32::from(fixed.right())).abs() < 0.5,
                    "read-only current value misaligned at {width} in {}: {editable:?}/{fixed:?}",
                    language.code()
                );
                visual.update(|window, _| window.remove_window());
            }
        }
    }
}

/// Synthetic deals exercise every primary-reason family without touching a live report.
fn deal_rows() -> Vec<super::super::state::DealRow> {
    use super::super::state::{DealRow, TapeStatus};
    use super::super::tape::PackedTape;
    use moon_core::db::tuner::ticks::verify::{EntryFinding, ExitFinding, ExitMiss, Unjudged};
    use moon_core::db::tuner::ticks::{Deal, Deltas, Verdict};
    use moon_core::market::trade_replay::TickStatus;
    let base = DealRow {
        deal: Deal {
            report_uid: 1,
            core_uid: 1,
            core_name: "CORE_A".into(),
            strategy_id: 1,
            kind: "Spread".into(),
            coin: "XALPHA".into(),
            buy_ms: 1_000,
            close_ms: 2_000,
            buy_price: 1.0,
            sell_price: 1.01,
            spent: 100.0,
            is_short: false,
            sell_reason: "StopLoss".into(),
            fact_pnl: 0.0,
            sizing: None,
            pnl_pct: false,
            profit: Some(1.0),
            deltas: Deltas::default(),
            tick: None,
            pre_spike_ask: None,
            archived_take: None,
            fact_modifier: None,
            hook_depth_pct: None,
            drops_detect: None,
            hook_stated_take_pct: None,
            strike_detect: None,
            step_lag_ms: 0.0,
            round_trip_ms: None,
            stop_anchor: None,
            delta_track: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: TapeStatus::Covered,
        verdict: Some(Verdict {
            entry: Some(true),
            exit: Some(true),
            entry_finding: EntryFinding::Hit,
            exit_finding: ExitFinding::Hit,
            entry_dev_pct: None,
            exit_dev_pct: None,
            fill: None,
            exit_kind: None,
            line_points: None,
            fill_clock_ms: None,
            rules: Default::default(),
            stop: Default::default(),
        }),
        address: None,
        ticks: Some(PackedTape::pack(Vec::new())),
        entry_line: None,
        held: Some((60_000, 120_000)),
    };
    (0..12)
        .map(|i| {
            let mut row = base.clone();
            row.deal.report_uid = i + 1;
            match i {
                0 => {}
                1 => {
                    row.tape = TapeStatus::Missing;
                    row.verdict = None;
                }
                2 => row.tape = TapeStatus::NoAddress,
                3 => row.tape = TapeStatus::Fetching,
                4 => row.tape = TapeStatus::Refused(TickStatus::NoTrades),
                5 => {
                    let v = row.verdict.as_mut().unwrap();
                    v.entry = Some(false);
                    v.entry_finding = EntryFinding::Off;
                }
                6 => {
                    let v = row.verdict.as_mut().unwrap();
                    v.exit = Some(false);
                    v.exit_finding = ExitFinding::Miss(ExitMiss::NoLevel);
                }
                7 => {
                    let v = row.verdict.as_mut().unwrap();
                    v.exit = None;
                    v.exit_finding = ExitFinding::Unjudged(Unjudged::OtherRule);
                }
                8 => row.verdict = None,
                9 => row.held = Some((60_000, 30_000)),
                10 => {
                    row.deal.gap = Some(moon_core::db::tuner::ticks::gap::TapeGap {
                        from_ms: 1_100,
                        to_ms: 1_800,
                        fact_line: None,
                        fact_stop: None,
                    })
                }
                _ => row.ticks = None,
            }
            row
        })
        .collect()
}

/// Probe the actual table, legend, filter controls and filtered-empty state together.
struct DealProbe {
    active: bool,
    table: bool,
}

/// Exercise the minimum-window sample table and its narrowest filter footer independently.
struct NarrowDealProbe;

impl Render for NarrowDealProbe {
    /// Keep the real column minima and use the longest active ShortTail label.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let row = deal_rows().remove(0);
        let admission = super::super::tail::Admission::at(60, 0);
        let counts = super::super::rows::ChipCounts {
            rows_rev: 0,
            min_tail_s: 60,
            counts: [0; super::super::why::DealChip::ALL.len()],
        };
        v_flex()
            .w_full()
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .debug_selector(|| "minimum-table".into())
                    .child(super::super::deal_header_el(
                        &None,
                        false,
                        Rc::new(|_, _, _| {}),
                        p,
                        cx,
                    ))
                    .child(super::super::deal_row(
                        &row,
                        false,
                        super::super::PlanCell(None),
                        Rc::new(|_, _, _, _| {}),
                        false,
                        p,
                        design::font_scale(cx),
                        super::super::deal_row_h(cx),
                        &admission,
                        cx,
                    )),
            )
            .child(
                div()
                    .w(design::ui_px(cx, 208.0))
                    .debug_selector(|| "minimum-footer".into())
                    .child(super::super::chips_el(
                        counts,
                        false,
                        Some(super::super::why::DealChip::ShortTail),
                        (0, 0, 0),
                        Rc::new(|_, _, _| {}),
                        p,
                        cx,
                    )),
            )
    }
}

/// At the 860px window minimum the explanation precedes clipped metrics and reset remains reachable.
#[gpui::test]
fn minimum_window_keeps_reason_and_narrow_footer_reset_visible(cx: &mut gpui::TestAppContext) {
    for theme in [UiThemeMode::Light, UiThemeMode::Dark] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                // 860 minus the 470px expanded pane and the shared 28px outer spacing.
                let window = cx.open_window(
                    gpui::size(px((860.0 - 470.0 - 28.0) * zoom), px(1000.0 * zoom)),
                    |_, _| NarrowDealProbe,
                );
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|w, cx| w.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|w, cx| {
                        let _ = w.draw(cx);
                    });
                }
                let table = visual.debug_bounds("minimum-table").unwrap();
                for selector in ["an-ticks-hdr-why", "an-ticks-cell-why-1"] {
                    let cell = visual.debug_bounds(selector).unwrap();
                    assert!(
                        cell.left() >= table.left() && cell.right() <= table.right() + px(0.5),
                        "{selector}: {language:?} {theme:?} {zoom} {cell:?}/{table:?}"
                    );
                }
                let footer = visual.debug_bounds("minimum-footer").unwrap();
                for selector in [
                    "an-ticks-chip-reset-box",
                    "an-ticks-chip-filter-name",
                    "an-ticks-chip-reset-link",
                ] {
                    let cell = visual.debug_bounds(selector).unwrap();
                    assert!(
                        cell.size.width > px(0.0)
                            && cell.left() >= footer.left()
                            && cell.right() <= footer.right() + px(0.5),
                        "{selector}: {language:?} {zoom} {cell:?}/{footer:?}"
                    );
                }
                visual.update(|w, _| w.remove_window());
            }
        }
    }
}

impl Render for DealProbe {
    /// Use production builders so descriptor, tooltip and wrapping regressions reach the matrix.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::super::state::{TicksData, TicksState};
        use super::super::why::DealChip;
        use super::super::{
            PlanCell, chips_el, deal_header_el, deal_row, filtered_empty_el, legend_el,
        };
        let p = MoonPalette::active(cx);
        let mut state = TicksState::default();
        state.data.apply(Ok(TicksData {
            rows: deal_rows(),
            ..Default::default()
        }));
        let counts = super::super::rows::counts_at(&mut state, 60, 0);
        let admission = super::super::tail::Admission::at(60, 0);
        let pick: super::super::ChipAction = Rc::new(|_, _, _| {});
        let action: super::super::DealAction = Rc::new(|_, _, _, _| {});
        let mut body = v_flex()
            .debug_selector(|| "deal-probe".to_string())
            .w_full()
            .min_w_0()
            .child(legend_el(p, cx));
        if self.table {
            body = body
                .child(deal_header_el(&None, false, Rc::new(|_, _, _| {}), p, cx))
                .children(state.data.data().unwrap().rows.iter().map(|row| {
                    deal_row(
                        row,
                        false,
                        PlanCell(None),
                        action.clone(),
                        self.active,
                        p,
                        design::font_scale(cx),
                        super::super::deal_row_h(cx),
                        &admission,
                        cx,
                    )
                }));
        }
        body.child(chips_el(
            counts,
            false,
            self.active.then_some(DealChip::Hole),
            (23, 9, 14),
            pick.clone(),
            p,
            cx,
        ))
        .child(filtered_empty_el(pick, p, cx))
    }
}

/// Chips must keep complete labels and wrap; rows and headers must share bounded cells.
fn assert_deal_table_symbols_and_filters_fit(cx: &mut gpui::TestAppContext, theme: UiThemeMode) {
    use super::super::columns::{DEAL_COIN_MIN_W, DEAL_ROW_GAP, DEAL_ROW_PAD_X};
    // The coordinator retains the existing minima: rows are probed only where the whole table fits.
    let column_min = DEAL_COIN_MIN_W
        + super::super::shown_deal_cols(false)
            .map(|col| col.min_w)
            .sum::<f32>();
    let spacing =
        2.0 * DEAL_ROW_PAD_X + super::super::shown_deal_cols(false).count() as f32 * DEAL_ROW_GAP;
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(theme, 1.0),
            cx,
        )
    });
    let table_min = cx
        .update(|cx| design::font_w(cx, column_min) + f32::from(design::ui_px(cx, spacing)))
        .ceil();
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            for width in [400.0, 520.0, table_min, 1440.0] {
                for active in [false, true] {
                    let window =
                        cx.open_window(gpui::size(px(width * zoom), px(1000.0 * zoom)), |_, _| {
                            DealProbe {
                                active,
                                table: width >= table_min,
                            }
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|w, cx| {
                            let _ = w.draw(cx);
                        });
                    }
                    let host = visual.debug_bounds("deal-probe").expect("table host");
                    let inside = |bounds: gpui::Bounds<gpui::Pixels>,
                                  parent: gpui::Bounds<gpui::Pixels>,
                                  label: &str| {
                        assert!(
                            bounds.left() + px(0.5) >= parent.left()
                                && bounds.right() <= parent.right() + px(0.5),
                            "{label} outside parent: {bounds:?}/{parent:?} {} {theme:?} {zoom}/{width}",
                            language.code()
                        );
                    };
                    let footer = visual.debug_bounds("an-ticks-chips").expect("footer");
                    inside(footer, host, "footer");
                    inside(
                        visual
                            .debug_bounds("an-ticks-filter-empty")
                            .expect("empty state"),
                        host,
                        "empty state",
                    );
                    inside(
                        visual
                            .debug_bounds("an-ticks-filter-empty-text")
                            .expect("empty text"),
                        host,
                        "empty text",
                    );
                    if active {
                        inside(
                            visual
                                .debug_bounds("an-ticks-chip-reset-box")
                                .expect("reset filter"),
                            footer,
                            "reset filter",
                        );
                    }
                    let mut tops = Vec::new();
                    for key in [
                        "with_tape",
                        "no_tape",
                        "fit",
                        "accepted_short_tail",
                        "short_tail",
                        "hole",
                        "model_miss",
                        "unjudged",
                        "without_ms",
                        "service",
                        "untunable",
                    ] {
                        let chip = visual
                            .debug_bounds(selector(format!("an-ticks-chip-{key}")))
                            .expect("chip");
                        inside(chip, footer, key);
                        assert!(
                            chip.top() >= footer.top()
                                && chip.bottom() <= footer.bottom() + px(0.5)
                        );
                        tops.push(chip.top());
                    }
                    if width == 400.0 {
                        assert!(tops.iter().any(|top| *top > tops[0]), "footer must wrap");
                    }
                    for key in ["hit", "miss", "unjudged", "hole"] {
                        inside(
                            visual
                                .debug_bounds(selector(format!("an-ticks-legend-{key}")))
                                .expect("legend item"),
                            host,
                            "legend",
                        );
                    }
                    for row in if width >= table_min {
                        deal_rows()
                    } else {
                        Vec::new()
                    } {
                        for col in super::super::shown_deal_cols(false) {
                            let cell = visual
                                .debug_bounds(selector(format!(
                                    "an-ticks-cell-{}-{}",
                                    col.key, row.deal.report_uid
                                )))
                                .expect("row cell");
                            inside(cell, host, col.key);
                            let text = visual
                                .debug_bounds(selector(format!(
                                    "an-ticks-text-{}-{}",
                                    col.key, row.deal.report_uid
                                )))
                                .expect("cell text");
                            inside(text, cell, col.key);
                            let header = visual
                                .debug_bounds(selector(format!("an-ticks-hdr-{}", col.key)))
                                .expect("header trigger");
                            inside(header, host, "header");
                            inside(
                                visual
                                    .debug_bounds(selector(format!(
                                        "an-ticks-hdr-text-{}",
                                        col.key
                                    )))
                                    .expect("header text"),
                                header,
                                "header text",
                            );
                            assert!(
                                (f32::from(cell.left()) - f32::from(header.left())).abs() < 1.0
                            );
                        }
                    }
                    visual.update(|w, _| w.remove_window());
                }
            }
        }
    }
}

/// Light-theme chips, rows and headers must fit the full language/zoom/width/filter matrix.
#[gpui::test]
fn deal_table_symbols_and_filters_fit_all_light_presentations(cx: &mut gpui::TestAppContext) {
    assert_deal_table_symbols_and_filters_fit(cx, UiThemeMode::Light);
}

/// Dark-theme chips, rows and headers must fit the full language/zoom/width/filter matrix.
#[gpui::test]
fn deal_table_symbols_and_filters_fit_all_dark_presentations(cx: &mut gpui::TestAppContext) {
    assert_deal_table_symbols_and_filters_fit(cx, UiThemeMode::Dark);
}

/// The real Stops section contributes two editable numeric knobs, without copying mockup fields.
fn fields() -> [&'static str; 2] {
    let knobs = super::super::sections::scope_knobs(&["DropsDetection".to_string()]);
    let mut keys = knobs
        .iter()
        .filter(|k| k.section == ParamSection::Stops && k.kind == ParamKind::Num);
    [
        keys.next().expect("first Stops knob").key,
        keys.next().expect("second Stops knob").key,
    ]
}

/// Distinct synthetic figures exercise signed values, ratios and an estimate outside scored profit.
fn stats() -> Vec<VarStats> {
    vec![
        VarStats {
            n: 73,
            wins: 13,
            profit: 17.63,
            avg: 1.27,
            pf: 2.31,
            avg_win: 3.4,
            avg_loss: 2.1,
            max_dd: 9.7,
            ..Default::default()
        },
        VarStats {
            n: 97,
            wins: 29,
            profit: 27.42,
            avg: 1.88,
            pf: 3.17,
            avg_win: 4.8,
            avg_loss: 1.9,
            max_dd: 5.2,
            ..Default::default()
        },
    ]
}

/// Exercise the actual bias decision and translations with a material selection difference.
fn sample_bias() -> Option<String> {
    super::super::sample::bias_note(
        &VarStats {
            n: 73,
            pf: 0.42,
            ..Default::default()
        },
        &VarStats {
            n: 405,
            pf: 0.90,
            ..Default::default()
        },
    )
}

/// Cover each user-visible advice face, including a consent and the held-back open estimate.
fn model(face: usize) -> SummaryModel {
    let verdict = match face {
        0 => Verdict::Idle,
        1 => Verdict::Running {
            done: 3,
            total: 11,
            left: Some(std::time::Duration::from_secs(43)),
            points: 107,
            stopping: false,
        },
        2 => Verdict::Apply,
        3 => Verdict::DontApply(Why::HoldoutOpen { k: 7 }),
        4 => Verdict::Unchecked(Unchecked::Whole),
        6 => Verdict::Apply,
        7 => Verdict::Running {
            done: 3,
            total: 11,
            left: None,
            points: 107,
            stopping: true,
        },
        8 => Verdict::Cannot(t!("analytics.ticks.sum.stopped_empty").to_string()),
        9 => Verdict::Loading,
        _ => Verdict::Cannot(t!("analytics.ticks.sum.too_few", n = 3, min = 20).to_string()),
    };
    let active = (2..=6).contains(&face);
    let open = t!("analytics.ticks.sum.open_line", n = 7, profit = "+6.19").to_string();
    let row = |hold: bool| ProfitRow {
        label: t!(if hold {
            "analytics.ticks.sum.hold"
        } else {
            "analytics.ticks.sum.train"
        })
        .to_string(),
        sub: t!(
            if hold {
                "analytics.ticks.sum.hold_sub"
            } else if face == 6 {
                "analytics.ticks.sum.train_sub_purged"
            } else {
                "analytics.ticks.sum.train_sub"
            },
            n = 31,
            p = 7
        )
        .to_string(),
        n: 31,
        fact: 13.71,
        variant: 19.23,
        open_line: (hold && face == 3).then(|| open.clone()),
    };
    SummaryModel {
        fact_holdout: matches!(face, 2 | 3 | 6).then(|| {
            let mut fact = moon_core::db::metrics::Tally::default();
            fact.push(13.71);
            fact
        }),
        title: t!("analytics.ticks.sum.title").to_string(),
        scope_label: "fixture".to_string(),
        tag: tag(&verdict),
        sentence: sentence(&verdict, 31, 13, 19),
        verdict,
        train: (face == 2 || face == 3 || face == 4 || face == 6).then(|| row(false)),
        holdout: (face == 2 || face == 3 || face == 6).then(|| row(true)),
        changed_header: t!("analytics.ticks.sum.changed", n = 2).to_string(),
        changes: if active {
            fields()
                .into_iter()
                .map(|key| ChangeRow {
                    key: key.to_string(),
                    section: t!("analytics.ticks.group_exit").to_string(),
                    old: if face == 6 { "—" } else { "1.31" }.to_string(),
                    new: "2.47".to_string(),
                })
                .collect()
        } else {
            Vec::new()
        },
        warnings: if face == 6 {
            vec![
                t!("analytics.ticks.sum.stopped_after", n = 123, m = 1000).to_string(),
                t!("analytics.ticks.sum.stopped_after", n = 0, m = 1).to_string(),
                open.clone(),
                t!("analytics.ticks.assumptions").to_string(),
            ]
        } else if active {
            vec![open.clone(), t!("analytics.ticks.assumptions").to_string()]
        } else {
            Vec::new()
        },
        details: (face == 2 || face == 3 || face == 4)
            .then(|| {
                (
                    t!("analytics.ticks.sum.d_starts").to_string(),
                    "11".to_string(),
                )
            })
            .into_iter()
            .collect(),
        details_open: active,
        write_enabled: !matches!(face, 5 | 8),
        copy_visible: active,
        can_reset: active,
        confirm: (face == 3).then(|| ConfirmModel {
            title: t!(
                "analytics.ticks.sum.confirm",
                n = 2,
                name = "fixture",
                core = "test core"
            )
            .to_string(),
            reason: Some(t!("analytics.ticks.sum.dont_open", n = 7).to_string()),
            bold_reason: true,
            warns: vec![open],
        }),
        write_error: None,
    }
}

/// Probe the shipped cells and the same render-time host-width decision.
struct Probe {
    face: usize,
    starts: Entity<MoonInputState>,
    variants: [Entity<MoonInputState>; 2],
    ranges: [[Entity<MoonInputState>; 3]; 2],
}

impl Render for Probe {
    /// Compose the production cards and an expanded real Stops section in one short column.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let ranges_visible = ranges_fit(window.viewport_size().width - design::ui_px(cx, 2.0), cx);
        let noop: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(|_, _| {});
        let acts = SummaryActs {
            write: noop.clone(),
            confirm: noop.clone(),
            cancel: noop.clone(),
            copy: noop.clone(),
            reset: noop.clone(),
            stop: noop.clone(),
            details: noop.clone(),
        };
        let pick_acts = PickBarActs {
            one: noop.clone(),
            all: noop.clone(),
        };
        let pick = PickBarModel {
            one_enabled: true,
            all_enabled: true,
            one_tip: String::new(),
            all_tip: String::new(),
            all_label: pick_all_label(Some("3 min")),
        };
        let labels = [VarLabel::with_sub(
            t!("analytics.ticks.sum.col_v1").to_string(),
            t!("analytics.ticks.kpi.v1_sub").to_string(),
        )];
        let base = VarLabel::with_sub(
            t!("analytics.tuner.fact").to_string(),
            t!("analytics.ticks.kpi.fact_sub").to_string(),
        );
        let mut grid = v_flex().w_full().min_w_0();
        let header = h_flex()
            .w_full()
            .flex_wrap()
            .font_family(design::ui_font())
            .text_size(design::t_caption(cx))
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonCheckbox::new("probe-section-on")
                    .checked(true)
                    .size(design::CONTROL_TIER),
            )
            .child(t!("analytics.ticks.group_exit").to_string())
            .child(t!("analytics.ticks.pick.sec_changed", k = 2, m = 2).to_string());
        grid = grid.child(super::super::grid::section_body_el(
            header.into_any_element(),
            "probe-section",
            Some((
                t!("analytics.ticks.assumptions").to_string(),
                t!("analytics.ticks.assumptions").to_string(),
            )),
            p.text_muted,
            cx,
        ));
        for (i, key) in fields().into_iter().enumerate() {
            let parts = FieldRowParts {
                checkbox: div()
                    .flex_none()
                    .child(
                        MoonCheckbox::new(SharedString::from(format!("probe-on-{key}")))
                            .checked(true)
                            .size(design::CONTROL_TIER),
                    )
                    .into_any_element(),
                name: field_name_el(key, false, true, None, p).into_any_element(),
                current: current_cell_el(key, true, Some(&NowValue::Same("1.31".into())), p, cx)
                    .into_any_element(),
                variant: variant_cell_el(key, &self.variants[i], cx).into_any_element(),
                clear: div()
                    .w(design::ui_px(cx, 12.0))
                    .flex_none()
                    .child("✕")
                    .into_any_element(),
            };
            let range = ranges_visible.then(|| {
                range_cells_el(
                    key,
                    &self.ranges[i],
                    false,
                    String::new(),
                    MoonButton::new(SharedString::from(format!("probe-reset-{key}")))
                        .variant(MoonButtonVariant::Ghost)
                        .size(design::dense_glyph_btn_size())
                        .width(design::dense_glyph_btn_w(cx))
                        .render()
                        .into_any_element(),
                    p,
                    cx,
                )
            });
            grid = grid.child(field_row_el(key, false, parts, range, p, cx));
        }
        let acc = Accuracy {
            entry: GroupCount {
                hits: 17,
                misses: 3,
                unjudged: 7,
            },
            exit: GroupCount {
                hits: 19,
                misses: 5,
                unjudged: 3,
            },
            // The longest reason, so the footer wraps at its widest.
            exit_why: [3, 0, 0, 0, 0, 0, 0, 0],
        };
        v_flex()
            .id("probe-scroll")
            .font_family(design::mono())
            .debug_selector(|| "probe-scroll".to_string())
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .debug_selector(|| "probe-col".to_string())
                    .w_full()
                    .flex_none()
                    .min_w_0()
                    .gap(design::ui_px(cx, 8.0))
                    .child(parameters_header(&model(self.face), &acts, cx))
                    .child(summary_card_el(
                        &model(self.face),
                        &acts,
                        ranges_visible,
                        p,
                        cx,
                    ))
                    .child(kpi_matrix_body_with(
                        &LoadState::Ready(Arc::new(stats())),
                        String::new(),
                        &base,
                        &labels,
                        false,
                        noop,
                        p,
                        cx,
                        KpiOpts {
                            title: Some((
                                t!("analytics.tuner.kpi_title").to_string(),
                                super::super::sample::caption(73, 405, self.face == 8),
                            )),
                            sample_note: sample_bias(),
                            rows: KpiRows::Plain,
                            unclosed: Some((1_234_567, 987_654.321)),
                        },
                    ))
                    .child(pick_bar_el(
                        &pick,
                        self.starts.clone(),
                        div().into_any_element(),
                        &pick_acts,
                        p,
                        cx,
                    ))
                    .child(grid)
                    .child(accuracy_footer_el(accuracy_sentence(&acc, true, 13), p, cx)),
            )
    }
}

/// Cache each finite selector once rather than leaking on every matrix lookup.
fn selector(text: String) -> &'static str {
    thread_local! {
        static SELECTORS: std::cell::RefCell<std::collections::HashMap<String, &'static str>> = std::cell::RefCell::new(std::collections::HashMap::new());
    }
    SELECTORS.with(|selectors| {
        let mut selectors = selectors.borrow_mut();
        *selectors
            .entry(text.clone())
            .or_insert_with(|| Box::leak(text.into_boxed_str()))
    })
}

/// Reading dictionaries catches missing translations that rust-i18n would hide with fallback.
fn keys(code: &str) -> std::collections::BTreeSet<String> {
    let path = format!(
        "{}/../../locales/{code}/analytics.{code}.yml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .expect("locale")
        .lines()
        .filter_map(|line| line.split_once(':').map(|(key, _)| key.to_string()))
        .filter(|key| key.starts_with("analytics.ticks."))
        .collect()
}

/// A missing key must fail even when the English fallback would make a card readable.
#[test]
fn every_ticks_language_has_the_same_keys_and_no_raw_card_text() {
    let en = keys("en");
    assert!(
        en.contains("analytics.ticks.kpi.unclosed") && en.contains("analytics.ticks.pick.all_est")
    );
    for language in Language::ALL {
        assert_eq!(keys(language.code()), en, "{}", language.code());
        let _locale = crate::test_locale::force(language.code());
        for face in 0..10 {
            let m = model(face);
            let mut text = vec![
                m.title,
                m.tag,
                m.sentence,
                m.changed_header,
                pick_all_label(Some("3 min")),
            ];
            text.extend(m.warnings);
            for row in m.train.into_iter().chain(m.holdout) {
                text.extend([row.label, row.sub]);
                text.extend(row.open_line);
            }
            if let Some(confirm) = m.confirm {
                text.push(confirm.title);
                text.extend(confirm.reason);
                text.extend(confirm.warns);
            }
            text.extend(m.details.into_iter().map(|(label, _)| label));
            for line in text {
                assert!(!line.contains("analytics."), "raw key: {line}");
            }
        }
        for key in en.iter().filter(|key| {
            key.starts_with("analytics.ticks.kpi.")
                || key.starts_with("analytics.ticks.pick.")
                || key.starts_with("analytics.ticks.why.")
                || key.starts_with("analytics.ticks.legend.")
                || key.starts_with("analytics.ticks.col_tip.")
                || key.starts_with("analytics.ticks.chip.")
        }) {
            assert!(!t!(key.as_str()).contains("analytics."), "raw key: {key}");
        }
    }
}

/// Production card/field/range boxes must fit without truncating values in every presentation.
fn assert_ticks_column_fits(cx: &mut gpui::TestAppContext, theme: UiThemeMode) {
    cx.update(|_| {
        crate::analytics::set_pnl_unit(Some(moon_core::db::ProfitUnit::Quote(
            moon_core::db::QuoteCurrency::from_report_ordinal(8).expect("USDC report ordinal"),
        )))
    });
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(theme, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let normal_width =
            cx.update(|cx| f32::from(design::font_w_px(cx, super::super::super::layout::SIDE_W)));
        for zoom in [1.0, 1.25] {
            for width in [normal_width, 520.0, 400.0] {
                for face in 0..10 {
                    let window = cx.open_window(
                        gpui::size(px(width * zoom), px(540.0 * zoom)),
                        |window, cx| {
                            let mut input = |value: &str, cx: &mut Context<Probe>| {
                                cx.new(|cx| MoonInputState::new(window, cx).default_value(value))
                            };
                            Probe {
                                face,
                                starts: input("11", cx),
                                variants: std::array::from_fn(|_| input("2.47", cx)),
                                ranges: std::array::from_fn(|_| {
                                    [input("1.31", cx), input("2.47", cx), input("0.29", cx)]
                                }),
                            }
                        },
                    );
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    let expect_ranges = visual.update(|window, cx| {
                        let fit =
                            ranges_fit(window.viewport_size().width - design::ui_px(cx, 2.0), cx);
                        if width != 520.0 {
                            assert_eq!(
                                fit,
                                width == normal_width,
                                "pure range decision at zoom {zoom}, width {width}"
                            );
                        }
                        fit
                    });
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let col = visual.debug_bounds("probe-col").expect("column");
                    let host = visual
                        .debug_bounds("probe-scroll")
                        .expect("short scroll host");
                    assert!(
                        col.size.height > host.size.height,
                        "short host must actually scroll"
                    );
                    let at = format!(
                        "{} {theme:?} zoom {zoom} width {width} face {face}",
                        language.code()
                    );
                    let inside = |bounds: gpui::Bounds<gpui::Pixels>, label: &str| {
                        assert!(
                            bounds.left() + px(0.5) >= col.left()
                                && bounds.right() <= col.right() + px(0.5),
                            "{label} escaped ({at}): {bounds:?} / {col:?}"
                        );
                    };
                    let line_height = visual.update(|window, cx| {
                        let mut style = window.text_style();
                        style.font_size = design::t_caption(cx).into();
                        style.line_height_in_pixels(window.rem_size())
                    });
                    for sel in [
                        "an-ticks-sum",
                        "an-ticks-pick-bar",
                        "kpi-unclosed",
                        "an-ticks-accuracy",
                        "an-ticks-sec-note-probe-section",
                        "an-tuner-card-title",
                        "an-tuner-card-caption",
                    ] {
                        let bounds = visual
                            .debug_bounds(sel)
                            .unwrap_or_else(|| panic!("missing {sel} ({at})"));
                        inside(bounds, sel);
                        assert!(
                            bounds.size.height + px(0.1) >= line_height,
                            "{sel} shorter than one line ({at})"
                        );
                    }
                    let m = model(face);
                    // Unset old values remain a visible full glyph before the arrow.
                    for row in &m.changes {
                        let old = visual
                            .debug_bounds(selector(format!("an-ticks-change-old-{}", row.key)))
                            .expect("old field value");
                        inside(old, "old field value");
                        let need = visual
                            .update(|_, cx| design::mono_body_text_width(cx, &row.old, 400.0));
                        assert!(
                            f32::from(old.size.width) + 0.5 >= need,
                            "old value clipped ({at}): {}",
                            row.old
                        );
                    }
                    // Each profit head must sit over its own figures, not float as one group.
                    let rows: Vec<&str> = [
                        m.train.as_ref().map(|_| "an-ticks-sum-train-count"),
                        m.holdout.as_ref().map(|_| "an-ticks-sum-hold-count"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    for (col, suffix, text) in [
                        ("an-ticks-sum-col-fact", "fact", 13.71),
                        ("an-ticks-sum-col-v1", "v1", 19.23),
                    ] {
                        if rows.is_empty() {
                            break;
                        }
                        let head = visual
                            .debug_bounds(col)
                            .unwrap_or_else(|| panic!("missing {col} ({at})"));
                        inside(head, col);
                        for row in &rows {
                            let sel = selector(format!("{row}-{suffix}"));
                            let figure = visual
                                .debug_bounds(sel)
                                .unwrap_or_else(|| panic!("missing {sel} ({at})"));
                            inside(figure, sel);
                            assert!(
                                (f32::from(figure.right()) - f32::from(head.right())).abs() <= 1.0,
                                "{col} not over {sel} ({at}): {head:?} / {figure:?}"
                            );
                            let need = visual.update(|_, cx| {
                                design::mono_body_text_width(
                                    cx,
                                    &crate::analytics::summary::fmt_signed(text),
                                    400.0,
                                )
                            });
                            assert!(
                                f32::from(figure.size.width) + 0.5 >= need,
                                "{sel} clipped ({at}): {:?} < {need}",
                                figure.size.width
                            );
                        }
                    }
                    let mut prose = vec![m.sentence];
                    // Short labels sit beside a sibling in a wrapping row; they must wrap the row, never their glyphs.
                    let mut labels = Vec::new();
                    if face == 1 || face == 7 {
                        labels.push(super::super::summary::progress_text(3, 11, 107));
                        if face == 7 {
                            prose.push(t!("analytics.ticks.sum.stopping").to_string());
                        } else {
                            labels.push(
                                t!(
                                    "analytics.ticks.sum.left",
                                    time = super::super::estimate::duration_text(
                                        std::time::Duration::from_secs(43)
                                    )
                                )
                                .to_string(),
                            );
                        }
                    } else {
                        if m.train.is_some() || m.holdout.is_some() {
                            let unit = crate::analytics::pnl_unit_label();
                            labels.push(if unit.is_empty() {
                                t!("analytics.ticks.sum.profit_head").to_string()
                            } else {
                                format!("{}, {unit}", t!("analytics.ticks.sum.profit_head"))
                            });
                        }
                        for row in m.train.into_iter().chain(m.holdout) {
                            prose.push(row.sub);
                            prose.extend(row.open_line);
                        }
                        if !m.changes.is_empty() {
                            prose.push(m.changed_header);
                        }
                        if m.can_reset {
                            prose.extend(m.warnings);
                            prose.push(t!("analytics.ticks.sum.write_note").to_string());
                        }
                        if let Some(confirm) = m.confirm {
                            prose.push(confirm.title);
                            prose.extend(confirm.reason);
                            prose.extend(confirm.warns);
                        }
                        if !m.details.is_empty() {
                            prose.push(t!("analytics.ticks.sum.details").to_string());
                            prose.extend(m.details.into_iter().map(|(label, _)| label));
                        }
                    }
                    for text in &labels {
                        let content = visual
                            .debug_bounds(selector(format!("an-ticks-prose-content-{text}")))
                            .unwrap_or_else(|| panic!("missing label {text} ({at})"));
                        let need =
                            visual.update(|_, cx| design::ui_caption_text_width(cx, text, 400.0));
                        assert!(
                            f32::from(content.size.width) + 0.5 >= need,
                            "label squeezed below one line ({at}): {text}: {:?} < {need}",
                            content.size.width
                        );
                        assert!(
                            content.size.height < line_height * 1.5,
                            "label wrapped onto several lines ({at}): {text}: {:?}",
                            content.size.height
                        );
                    }
                    prose.extend(labels);
                    for text in prose {
                        let bounds = visual
                            .debug_bounds(selector(format!("an-ticks-prose-{text}")))
                            .unwrap_or_else(|| panic!("missing prose {text} ({at})"));
                        let content = visual
                            .debug_bounds(selector(format!("an-ticks-prose-content-{text}")))
                            .expect("natural prose content");
                        inside(bounds, &text);
                        assert!(
                            bounds.size.height + px(0.1) >= line_height,
                            "prose shorter than a line ({at}): {text}"
                        );
                        assert!(
                            content.top() + px(0.1) >= bounds.top()
                                && content.bottom() <= bounds.bottom() + px(0.1),
                            "prose clipped vertically ({at}): {text}"
                        );
                    }
                    let note = visual.debug_bounds("kpi-sample-note").expect("bias note");
                    let content = visual
                        .debug_bounds("kpi-sample-note-content")
                        .expect("bias content");
                    inside(note, "bias note");
                    inside(content, "bias content");
                    assert!(
                        content.top() >= note.top() && content.bottom() <= note.bottom(),
                        "bias note clipped vertically ({at})"
                    );
                    let caption = visual
                        .debug_bounds("an-tuner-card-caption")
                        .expect("sample caption");
                    inside(caption, "sample caption");
                    let mut needs: Vec<(String, String, bool)> = vec![
                        (
                            "tun-suggest-one-x-box".into(),
                            t!("analytics.ticks.pick.one").to_string(),
                            false,
                        ),
                        (
                            "tun-suggest-run-x-box".into(),
                            pick_all_label(Some("3 min")),
                            false,
                        ),
                        ("an-ticks-sum-tag".into(), model(face).tag, false),
                        (
                            "kpi-unclosed-label".into(),
                            t!("analytics.ticks.kpi.unclosed").to_string(),
                            false,
                        ),
                        ("kpi-unclosed-0".into(), "—".to_string(), true),
                        ("kpi-unclosed-1".into(), "1.23M · ≈ +988K".to_string(), true),
                    ];
                    for (row, key) in [
                        "analytics.kpi.trades",
                        "analytics.kpi.profit",
                        "analytics.kpi.avg_short",
                        "analytics.kpi.winrate",
                        "analytics.col.pf",
                        "analytics.tuner.avg_win",
                        "analytics.tuner.avg_loss",
                        "analytics.kpi.maxdd",
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        needs.push((
                            format!("kpi-metric-{row}"),
                            t!(key, unit = crate::analytics::pnl_unit_label()).to_string(),
                            false,
                        ));
                    }
                    for (i, values) in [
                        [
                            "73", "+17.63", "+1.27", "17.8%", "2.31", "+3.4", "-2.1", "-9.7",
                        ],
                        [
                            "97", "+27.42", "+1.88", "29.9%", "3.17", "+4.8", "-1.9", "-5.2",
                        ],
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        for (row, text) in values.into_iter().enumerate() {
                            needs.push((format!("kpi-value-{row}-{i}"), text.to_string(), true));
                        }
                    }
                    needs.push((
                        "tun-save-x".into(),
                        t!("analytics.tuner.save_btn").to_string(),
                        false,
                    ));
                    if model(face).copy_visible {
                        needs.push((
                            "tun-copy-x".into(),
                            t!("analytics.tuner.copy_btn").to_string(),
                            false,
                        ));
                    }
                    let actions = if face == 1 || face == 7 {
                        vec![("stop", "stop")]
                    } else if (2..=6).contains(&face) {
                        let mut out = vec![("reset", "reset")];
                        if face == 3 {
                            out.extend([
                                ("confirm-yes", "confirm_yes"),
                                ("confirm-no", "confirm_no"),
                            ]);
                        }
                        out
                    } else {
                        Vec::new()
                    };
                    for (sel, key) in actions {
                        needs.push((
                            format!("an-ticks-sum-{sel}"),
                            t!(format!("analytics.ticks.sum.{key}")).to_string(),
                            false,
                        ));
                    }
                    for key in fields() {
                        let row = visual
                            .debug_bounds(selector(format!("an-ticks-field-{key}")))
                            .expect("production field row");
                        inside(row, key);
                        for (sel, text) in [
                            (format!("an-ticks-current-cell-{key}"), "1.31"),
                            (format!("an-ticks-variant-cell-{key}"), "2.47"),
                        ] {
                            needs.push((sel, text.to_string(), true));
                        }
                        let range = visual.debug_bounds(selector(format!("an-ticks-range-{key}")));
                        assert_eq!(range.is_some(), expect_ranges, "range visibility ({at})");
                        if let Some(range) = range {
                            inside(range, "production range");
                            for (slot, text) in [("from", "1.31"), ("to", "2.47"), ("step", "0.29")]
                            {
                                needs.push((
                                    format!("an-ticks-range-cell-{slot}-{key}"),
                                    text.to_string(),
                                    true,
                                ));
                            }
                        }
                    }
                    for (sel, text, mono) in needs {
                        let bounds = visual
                            .debug_bounds(selector(sel.clone()))
                            .unwrap_or_else(|| panic!("missing {sel} ({at})"));
                        inside(bounds, &sel);
                        let need = visual.update(|_, cx| {
                            if mono {
                                design::mono_caption_text_width(cx, &text, 400.0)
                            } else {
                                design::ui_caption_text_width(cx, &text, 400.0)
                            }
                        });
                        assert!(
                            f32::from(bounds.size.width) + 0.1 >= need,
                            "{sel} narrower than {text} ({at}): {:?} < {need}",
                            bounds.size.width
                        );
                    }
                }
            }
        }
    }
    cx.update(|_| crate::analytics::set_pnl_unit(None));
}

/// Light-theme cards, fields and ranges must fit every language, zoom, width and face.
#[gpui::test]
fn the_ticks_column_fits_all_languages_light_theme_zooms_and_widths(cx: &mut gpui::TestAppContext) {
    assert_ticks_column_fits(cx, UiThemeMode::Light);
}

/// Dark-theme cards, fields and ranges must fit every language, zoom, width and face.
#[gpui::test]
fn the_ticks_column_fits_all_languages_dark_theme_zooms_and_widths(cx: &mut gpui::TestAppContext) {
    assert_ticks_column_fits(cx, UiThemeMode::Dark);
}
