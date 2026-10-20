//! Production page geometry checks without launching MoonTerminal or reading report data.

use super::super::kpi::{KpiOpts, KpiRows, VarLabel, kpi_matrix_body_with};
use super::super::shared::TunerKind;
use super::super::shell::{ToolbarActs, ToolbarWrite, toolbar_el};
use crate::design;
use crate::load_state::LoadState;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonPalette, MoonTheme, v_flex};
use rust_i18n::t;
use std::rc::Rc;
use std::sync::Arc;

/// The page renderer and common write header used by all four modes, in each placeholder state.
struct Probe {
    mode: TunerKind,
    state: usize,
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
            v_flex()
                .w_full()
                .child(kpi_matrix_body_with(
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
                ))
                .child(super::parameters(
                    toolbar_el(
                        self.mode,
                        t!("analytics.tuner.fields_title").to_string(),
                        Some(&writes),
                        &acts,
                        cx,
                    ),
                    p,
                    cx,
                ))
                .into_any_element()
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
        super::page(left, div().into_any_element(), Some(side), narrow, cx)
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
            for width in [1200.0, 470.0] {
                for state in 0..3 {
                    let mut geometry = None;
                    for (mode, suffix) in [
                        (TunerKind::Filter, "f"),
                        (TunerKind::Time, "t"),
                        (TunerKind::Coins, "c"),
                        (TunerKind::Ticks, "x"),
                    ] {
                        let window = cx.open_window(gpui::size(px(width), px(900.0)), |_, _| {
                            Probe { mode, state }
                        });
                        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                        for _ in 0..3 {
                            visual.update(|window, cx| {
                                let _ = window.draw(cx);
                            });
                        }
                        let side = visual.debug_bounds("tuner-side").expect("right column");
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
                            }
                        }
                    }
                }
            }
        }
    }
}
