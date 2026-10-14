//! The group overview: its model and its layout in every locale.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively.

use std::rc::Rc;

use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, VisualTestContext, Window,
    div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_core::feed::{StrategyEditPhase, StrategyEditRow, StrategyRow};
use moon_core::session::store::CoreData;
use moon_ui::MoonTheme;

use super::{
    OverviewModel, PENDING_LISTED, PendingEdit, STOPPED_LISTED, StoppedCore, overview_model,
    overview_panel, toggle_quick,
};
use crate::strategies::filter::QuickFilter;
use crate::test_locale::force;

/// A synthetic strategy row; only `id`, `name` and `checked` matter to the overview.
fn row(id: u64, checked: bool) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("UNIT_{id:02}"),
        kind: "Test".to_string(),
        kind_ordinal: 0,
        folder_path: String::new(),
        checked,
        is_short: false,
        fields: Vec::new(),
    }
}

/// A core holding `rows`, with its engine state `engine` (confirmed when `Some`).
fn core(rows: Vec<StrategyRow>, engine: Option<bool>) -> CoreData {
    let mut cd = CoreData::default();
    cd.strategies = rows;
    cd.strategies_running = engine;
    cd.strategies_running_confirmed = engine.is_some();
    cd
}

/// Tiles count only CONFIRMED engine states, totals cover every core, and an edit for a strategy
/// the core no longer lists is left out because there is nothing to select.
#[test]
fn the_model_counts_confirmed_states_and_lists_actionable_edits() {
    let running = core(vec![row(1, true), row(2, false)], Some(true));
    let mut stopped = core(vec![row(3, true)], Some(false));
    stopped.strategy_edits.push(StrategyEditRow {
        id: 3,
        phase: StrategyEditPhase::TimedOut,
        submitted_at_ms: 0,
        fields: Vec::new(),
    });
    stopped.strategy_edits.push(StrategyEditRow {
        id: 99,
        phase: StrategyEditPhase::Pending,
        submitted_at_ms: 0,
        fields: Vec::new(),
    });
    let unknown = core(vec![row(4, true)], None);
    let cores = vec![
        (1, "CORE_A".to_string()),
        (2, "CORE_B".to_string()),
        (3, "CORE_C".to_string()),
        (4, "CORE_D".to_string()),
    ];
    let model = overview_model(&cores, |id| match id {
        1 => Some(&running),
        2 => Some(&stopped),
        3 => Some(&unknown),
        _ => None,
    });
    assert_eq!(model.cores_running, 1);
    assert_eq!(model.cores_stopped, 1);
    assert_eq!((model.enabled, model.total), (3, 4));
    assert_eq!(model.open_orders, 0);
    assert_eq!(
        model.pending,
        vec![PendingEdit {
            key: (2, 3),
            name: "UNIT_03".into(),
            core_name: "CORE_B".into(),
            timed_out: true,
        }]
    );
    assert_eq!(
        model.stopped,
        vec![StoppedCore {
            core: 2,
            name: "CORE_B".into(),
            strategies: 1,
        }]
    );
}

/// Quick filters are exclusive and a second click on the active chip removes it.
#[test]
fn a_quick_chip_toggles_and_replaces() {
    assert_eq!(
        toggle_quick(None, QuickFilter::Stopped),
        Some(QuickFilter::Stopped)
    );
    assert_eq!(
        toggle_quick(Some(QuickFilter::Stopped), QuickFilter::Stopped),
        None
    );
    assert_eq!(
        toggle_quick(Some(QuickFilter::Stopped), QuickFilter::Disabled),
        Some(QuickFilter::Disabled)
    );
}

/// The narrowest right pane the window allows: `params_panel` keeps a 420px minimum with 24px
/// padding on both sides.
const PANE_W: f32 = 420.0;

struct Host {
    model: OverviewModel,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(PANE_W)).px(px(24.0)).child(overview_panel(
            &self.model,
            Some(QuickFilter::Disabled),
            Rc::new(|_, _, _| {}),
            cx,
        ))
    }
}

/// A model filling every block past its listed cap, with long synthetic names.
fn full_model() -> OverviewModel {
    OverviewModel {
        cores_running: 123,
        cores_stopped: 45,
        enabled: 12_345,
        total: 67_890,
        open_orders: 9_999,
        pending: (0..PENDING_LISTED as u64 + 2)
            .map(|id| PendingEdit {
                key: (1, id),
                name: format!("UNIT_{id:02} with a rather long synthetic strategy name"),
                core_name: "CORE_A".into(),
                timed_out: id % 2 == 0,
            })
            .collect(),
        stopped: (0..STOPPED_LISTED as u64 + 3)
            .map(|core| StoppedCore {
                core,
                name: format!("CORE_{core:02} long synthetic core name"),
                strategies: core as usize,
            })
            .collect(),
    }
}

/// `debug_bounds` keys on `&'static str`; a test-only leak of a few short ids is harmless.
fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

/// The laid-out bounds of a debug selector, panicking with the case when it is missing.
fn bounds(visual: &mut VisualTestContext, id: impl Into<String>, at: &str) -> Bounds<Pixels> {
    let id = id.into();
    visual
        .debug_bounds(leak(id.clone()))
        .unwrap_or_else(|| panic!("{id} not laid out ({at})"))
}

fn inside(child: Bounds<Pixels>, parent: Bounds<Pixels>) -> bool {
    child.origin.x + px(0.5) >= parent.origin.x
        && child.right() <= parent.right() + px(0.5)
        && child.origin.y + px(0.5) >= parent.origin.y
        && child.bottom() <= parent.bottom() + px(0.5)
}

/// Every locale, full and empty: tiles, their figures, both blocks and the chips stay inside the
/// pane at its minimum width, and an empty block says so instead of drawing an empty box.
#[gpui::test]
fn the_overview_fits_the_narrowest_pane_in_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        );
    });
    for language in Language::ALL {
        let _locale = force(language.code());
        for full in [true, false] {
            let model = if full {
                full_model()
            } else {
                OverviewModel::default()
            };
            let window = cx.open_window(gpui::size(px(PANE_W), px(2000.0)), |_, _| Host { model });
            let mut visual = VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let at = format!("{} full={full}", language.code());
            let root = bounds(&mut visual, "ov-root", &at);
            assert!(
                f32::from(root.size.width) <= PANE_W - 48.0 + 0.5,
                "overview wider than its pane ({at})"
            );
            for id in ["ov-tiles", "ov-blocks", "ov-chips"] {
                assert!(
                    inside(bounds(&mut visual, id, &at), root),
                    "{id} escaped the pane ({at})"
                );
            }
            for index in 0..4 {
                let tile = bounds(&mut visual, format!("ov-tile-{index}"), &at);
                let value = bounds(&mut visual, format!("ov-tile-value-{index}"), &at);
                assert!(inside(tile, root), "tile {index} escaped ({at})");
                assert!(inside(value, tile), "tile {index} figure clipped ({at})");
            }
            for id in ["ov-block-pending", "ov-block-stopped"] {
                assert!(
                    inside(bounds(&mut visual, id, &at), root),
                    "{id} escaped ({at})"
                );
                let none = visual.debug_bounds(leak(format!("{id}-none")));
                assert_eq!(none.is_some(), !full, "{id} empty state ({at})");
            }
            for filter in QuickFilter::ALL {
                let chip = bounds(&mut visual, format!("ov-chip-{}", filter.key()), &at);
                assert!(inside(chip, root), "chip {} escaped ({at})", filter.key());
            }
        }
    }
}
