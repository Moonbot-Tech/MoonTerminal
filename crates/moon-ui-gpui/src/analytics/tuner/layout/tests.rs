//! Production page geometry checks without launching MoonTerminal or reading report data.

use super::super::kpi::{KpiOpts, KpiRows, VarLabel, kpi_matrix_body_with};
use super::super::shared::TunerKind;
use super::super::shell::{ToolbarActs, ToolbarWrite, toolbar_el};
use crate::design;
use crate::load_state::LoadState;
use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonButton, MoonCheckbox, MoonInputState, MoonPalette, MoonTheme, v_flex};
use rust_i18n::t;
use std::rc::Rc;
use std::sync::Arc;

/// The page renderer and common write header used by all four modes, in each placeholder state.
struct Probe {
    mode: TunerKind,
    state: usize,
    input: Entity<MoonInputState>,
    scroll: gpui::ScrollHandle,
}

impl Probe {
    /// Exercise the production budget and run widgets, so wrapping failures cannot hide behind a header-only fixture.
    fn controls(&self, p: MoonPalette, cx: &gpui::App) -> gpui::AnyElement {
        use super::super::{coins::auto, shell, ticks::cfg, time::controls};
        let gear = || {
            MoonButton::new("probe-settings")
                .label("⚙")
                .render()
                .into_any_element()
        };
        let round = || {
            MoonCheckbox::new("probe-round")
                .label(t!("analytics.tuner.round_lbl").to_string())
                .into_any_element()
        };
        match self.mode {
            TunerKind::Filter => shell::filter_controls_el(
                &self.input,
                gear(),
                round(),
                &shell::FilterControlModel {
                    running: false,
                    stopping: None,
                    compose: true,
                },
                &shell::FilterControlActs {
                    one: Rc::new(|_, _| {}),
                    run: Rc::new(|_, _| {}),
                    stop: Rc::new(|_, _| {}),
                },
                p,
                cx,
            ),
            TunerKind::Time => controls::config_row(
                &self.input,
                super::super::split::train_dropdown_el(TunerKind::Time, 100, |_, _| {}, cx),
                false,
                round(),
                |_, _, _| {},
                p,
                cx,
            ),
            TunerKind::Coins => auto::auto_card_el(
                &auto::AutoCardModel::new(&Default::default(), true, false, true, 551),
                Some((&self.input, &self.input)),
                &auto::AutoCardActs {
                    on_pick: Rc::new(|_, _| {}),
                    on_reset: Rc::new(|_, _| {}),
                    on_one_by_one: Rc::new(|_, _| {}),
                    on_add: Rc::new(|_, _| {}),
                    on_save: Rc::new(|_, _| {}),
                    on_revert: Rc::new(|_, _| {}),
                    on_exclude: Rc::new(|_, _, _, _| {}),
                    on_black: Rc::new(|_, _, _| {}),
                    on_white: Rc::new(|_, _, _| {}),
                },
                p,
                cx,
            ),
            TunerKind::Ticks => cfg::pick_bar_el(
                &cfg::PickBarModel {
                    one_enabled: true,
                    all_enabled: true,
                    one_tip: String::new(),
                    all_tip: String::new(),
                    all_label: t!("analytics.ticks.pick.all").to_string(),
                },
                self.input.clone(),
                gear(),
                &cfg::PickBarActs {
                    one: Rc::new(|_, _| {}),
                    all: Rc::new(|_, _| {}),
                },
                p,
                cx,
            ),
        }
    }
}

impl Render for Probe {
    /// Render the actual page frame and shared toolbar with neutral fixture labels.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let narrow = super::is_narrow(f32::from(window.viewport_size().width), cx);
        let acts = ToolbarActs {
            on_copy: Rc::new(|_, _| {}),
            on_save: Rc::new(|_, _| {}),
        };
        let writes = ToolbarWrite {
            changed_coins: 0,
            enabled: true,
            copy: true,
            dirty: true,
        };
        let side = if self.state > 0 {
            super::empty_side(
                self.state == 2,
                super::super::shared::collapse_caret(
                    "an-tuner-kpi-collapse",
                    false,
                    String::new(),
                    String::new(),
                    p,
                    |_, _, _| {},
                ),
                p,
                cx,
            )
        } else {
            let columns = if self.mode == TunerKind::Ticks { 2 } else { 3 };
            let stats = LoadState::Ready(Arc::new(vec![
                moon_core::db::tuner::VarStats {
                    n: 479,
                    wins: 279,
                    profit: -2075.89,
                    avg: -4.33,
                    pf: 0.89,
                    avg_win: 62.54,
                    avg_loss: 97.63,
                    max_dd: 3060.0,
                    ..Default::default()
                };
                columns
            ]));
            let labels = if self.mode == TunerKind::Coins {
                super::super::coins::coin_kpi_labels(&Default::default()).to_vec()
            } else {
                (1..columns)
                    .map(|i| VarLabel::new(format!("v{i}")))
                    .collect()
            };
            let kpi = kpi_matrix_body_with(
                &stats,
                "STRATEGY_A".into(),
                &VarLabel::new(t!("analytics.tuner.fact").to_string()),
                &labels,
                false,
                Rc::new(|_, _| {}),
                p,
                cx,
                KpiOpts {
                    rows: if self.mode == TunerKind::Ticks {
                        KpiRows::Plain
                    } else {
                        KpiRows::Shared
                    },
                    ..Default::default()
                },
            );
            let params = super::parameters_with_scroll(
                v_flex()
                    .w_full()
                    .child(toolbar_el(
                        self.mode,
                        t!("analytics.tuner.fields_title").to_string(),
                        None,
                        Some(&writes),
                        &acts,
                        cx,
                    ))
                    .child(self.controls(p, cx))
                    .into_any_element(),
                v_flex()
                    .w_full()
                    .child(div().h(px(1200.0)).flex_none())
                    .child(
                        div()
                            .debug_selector(|| "last-field".into())
                            .h(px(24.0))
                            .flex_none(),
                    )
                    .into_any_element(),
                &self.scroll,
                p,
                cx,
            );
            super::side(kpi, params, cx)
        };
        let left = v_flex()
            .size_full()
            .child(div().flex_1())
            .child(
                div()
                    .debug_selector(|| "detail".into())
                    .w_full()
                    .h(px(100.0)),
            )
            .into_any_element();
        super::page(
            left,
            div().into_any_element(),
            Some(side),
            narrow,
            window,
            cx,
        )
    }
}

/// A mode-specific width regression must move the right column or the detail beyond its host.
#[gpui::test]
fn all_axes_share_column_and_write_header_geometry(cx: &mut gpui::TestAppContext) {
    for theme in [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for width in [1200.0, 470.0, 400.0, 320.0] {
                for state in 0..3 {
                    let mut geometry = None;
                    for (mode, suffix) in [
                        (TunerKind::Filter, "f"),
                        (TunerKind::Time, "t"),
                        (TunerKind::Coins, "c"),
                        (TunerKind::Ticks, "x"),
                    ] {
                        let scroll = gpui::ScrollHandle::new();
                        let window =
                            cx.open_window(gpui::size(px(width), px(900.0)), |window, cx| Probe {
                                mode,
                                state,
                                input: cx
                                    .new(|cx| MoonInputState::new(window, cx).default_value("11")),
                                scroll: scroll.clone(),
                            });
                        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                        for _ in 0..3 {
                            visual.update(|window, cx| {
                                let _ = window.draw(cx);
                            });
                        }
                        let side = visual.debug_bounds("tuner-side").expect("right column");
                        let expected_width = visual.update(|_, cx| super::side_width(width, cx));
                        assert!(
                            (f32::from(side.size.width - expected_width)).abs() < 0.5,
                            "grid width must match its actual host"
                        );
                        let detail = visual.debug_bounds("detail").expect("left detail");
                        assert!(side.left() >= px(0.0) && side.right() <= px(width + 0.5));
                        if width > 900.0 {
                            assert!(detail.right() < side.left());
                        }
                        let key = (side.origin.x, side.size.width);
                        if let Some(expected) = geometry {
                            assert_eq!(key, expected, "mode-specific page drift");
                        }
                        geometry = Some(key);
                        if state == 0 {
                            let card = visual.debug_bounds("tuner-parameters").expect("Parameters");
                            assert!(
                                (card.bottom() - side.bottom()).abs() < px(0.5),
                                "Parameters must fill the column in {} at {width}",
                                language.code()
                            );
                            let viewport = visual.debug_bounds("tuner-parameters-scroll").unwrap();
                            assert!(
                                viewport.size.height > px(0.0),
                                "no field viewport: mode {suffix}, {} at {width}; card {card:?}, side {side:?}",
                                language.code()
                            );
                            let last = visual.debug_bounds("last-field").unwrap();
                            assert!(last.bottom() > viewport.bottom(), "fixture must overflow");
                            scroll.scroll_to_bottom();
                            visual.update(|window, cx| {
                                let _ = window.draw(cx);
                            });
                            let last = visual.debug_bounds("last-field").unwrap();
                            assert!(
                                last.top() >= viewport.top()
                                    && last.bottom() <= viewport.bottom() + px(0.5),
                                "last field unreachable in {} at {width}",
                                language.code()
                            );
                            let select = |name: &str| -> &'static str {
                                Box::leak(format!("tun-{name}-{suffix}").into_boxed_str())
                            };
                            let header = visual.debug_bounds(select("toolbar")).unwrap();
                            let copy = visual.debug_bounds(select("copy")).unwrap();
                            let save = visual.debug_bounds(select("save")).unwrap();
                            assert!(copy.left() < save.left(), "Copy must precede Save");
                            assert!(
                                save.right() <= header.right() + px(0.5),
                                "write actions escaped"
                            );
                            assert!(copy.size.width > px(0.0) && save.size.width > px(0.0));
                            let run_selector = match mode {
                                TunerKind::Filter => "tuner-run-f",
                                TunerKind::Time => "tuner-run-t",
                                TunerKind::Coins => "tuner-run-c",
                                TunerKind::Ticks => "tuner-run-x",
                            };
                            let run = visual
                                .debug_bounds(run_selector)
                                .expect("production run row");
                            assert!(
                                run.top() >= header.bottom(),
                                "run row precedes Parameters header"
                            );
                            assert!(
                                run.left() >= side.left() && run.right() <= side.right() + px(0.5),
                                "run controls overflow: mode {suffix}, {} at {width}; run {run:?}, side {side:?}",
                                language.code()
                            );
                            for (i, key) in [
                                "analytics.kpi.trades",
                                "analytics.kpi.profit",
                                "analytics.kpi.avg_short",
                                "analytics.kpi.winrate",
                                "analytics.col.pf",
                                "analytics.tuner.avg_win",
                                "analytics.tuner.avg_loss",
                                "analytics.kpi.maxdd",
                            ]
                            .iter()
                            .enumerate()
                            {
                                let selector: &'static str =
                                    Box::leak(format!("kpi-metric-{i}").into_boxed_str());
                                let metric =
                                    visual.debug_bounds(selector).expect("shared metric row");
                                let need = visual.update(|_, cx| {
                                    design::ui_caption_text_width(
                                        cx,
                                        &t!(*key, unit = crate::analytics::pnl_unit_label()),
                                        400.0,
                                    )
                                });
                                assert!(
                                    metric.size.width + px(0.5) >= px(need),
                                    "clipped {key} in {} at {width}",
                                    language.code()
                                );
                                assert!(metric.right() <= side.right() + px(0.5));
                                for column in 0..if mode == TunerKind::Ticks { 2 } else { 3 } {
                                    let selector: &'static str = Box::leak(
                                        format!("kpi-value-{i}-{column}").into_boxed_str(),
                                    );
                                    let cell =
                                        visual.debug_bounds(selector).expect("numeric KPI cell");
                                    assert!(
                                        cell.left() >= side.left()
                                            && cell.right() <= side.right() + px(0.5),
                                        "numeric KPI overflow in {} at {width}",
                                        language.code()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Clipping the outer column again must not strand Parameters below an expanded KPI in a short dock.
#[gpui::test]
fn short_wide_hosts_keep_the_last_field_reachable(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for mode in [
            TunerKind::Filter,
            TunerKind::Time,
            TunerKind::Coins,
            TunerKind::Ticks,
        ] {
            let scroll = gpui::ScrollHandle::new();
            let window = cx.open_window(gpui::size(px(1200.0), px(200.0)), |window, cx| Probe {
                mode,
                state: 0,
                input: cx.new(|cx| MoonInputState::new(window, cx).default_value("11")),
                scroll: scroll.clone(),
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let host = visual.debug_bounds("tuner-side").unwrap();
            let content = visual.debug_bounds("tuner-side-content").unwrap();
            let fields = visual.debug_bounds("tuner-parameters-scroll").unwrap();
            assert!(
                content.size.height > host.size.height,
                "fixture must need outer scrolling"
            );
            assert!(
                fields.size.height > px(0.0),
                "{}: Parameters lost its field viewport",
                language.code()
            );
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: host.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-10000.0))),
                ..Default::default()
            });
            scroll.scroll_to_bottom();
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let content = visual.debug_bounds("tuner-side-content").unwrap();
            let card = visual.debug_bounds("tuner-parameters").unwrap();
            let last = visual.debug_bounds("last-field").unwrap();
            assert!(content.top() < host.top(), "outer fallback did not scroll");
            assert!((card.bottom() - content.bottom()).abs() < px(0.5));
            assert!(
                last.top() >= host.top() && last.bottom() <= host.bottom() + px(0.5),
                "{}: last field remains outside the short dock",
                language.code()
            );
        }
    }
}

/// The live before-time reference reserves about 690 of 2269 pixels for the right column.
/// Enlarging the shared width again must fail instead of silently taking space from the list.
#[gpui::test]
fn livefix_shared_width_preserves_the_before_time_ratio(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    let _locale = crate::test_locale::force("en");
    for mode in [
        TunerKind::Filter,
        TunerKind::Time,
        TunerKind::Coins,
        TunerKind::Ticks,
    ] {
        let window = cx.open_window(gpui::size(px(2269.0), px(900.0)), |window, cx| Probe {
            mode,
            state: 0,
            input: cx.new(|cx| MoonInputState::new(window, cx).default_value("11")),
            scroll: gpui::ScrollHandle::new(),
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let side = visual.debug_bounds("tuner-side").unwrap();
        let ratio = f32::from(side.size.width) / 2269.0;
        let before = 690.0 / 2269.0;
        assert!(
            ratio <= before && ratio >= before * 0.99,
            "shared column ratio {ratio} exceeds or shrinks the approved before-time allocation {before}"
        );
    }
}
