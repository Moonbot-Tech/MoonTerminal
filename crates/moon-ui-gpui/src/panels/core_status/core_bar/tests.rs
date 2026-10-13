//! Toolbar: segments, search, and the trailing group stay inside the row.
//!
//! Explicit imports (no `use super::*`): the parent module glob-imports `gpui`.

use std::collections::HashSet;

use gpui::{AppContext, Context, Entity, IntoElement, Render, Styled, Window, div, px};
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonInput, MoonInputState};
use rust_i18n::t;

use super::super::layout_fixture::{self, LOCALES, WIDTHS};
use super::super::mode::CoreStatusMode;
use super::{columns_menu_el, mode_switch_control, toolbar_row, toolbar_tail_el};
use crate::design;

/// Production toolbar slots. The scope chip is a fixed stand-in for the combo, which needs a
/// backend; the segments, search field, column menu, and gear button are the real controls.
struct ToolbarProbe {
    /// Search field state. The slot's width, not the text, is what the test reads.
    search: Entity<MoonInputState>,
    /// Measured toolbar width passed to the mode strip. `0` keeps the full labels.
    available: f32,
}

impl Render for ToolbarProbe {
    /// Paint the production row. The gear popover stays closed, so only its button takes space.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = mode_switch_control(
            4,
            2,
            1,
            2,
            1,
            CoreStatusMode::Flat,
            self.available,
            |_, _, _| {},
            cx,
        );
        let search = MoonInput::new("core-status-search")
            .state(&self.search)
            .placeholder(t!("core_status.search.placeholder").to_string())
            .size(design::INPUT_SIZE)
            .cleanable(true);
        let hidden = HashSet::new();
        let menu = columns_menu_el(&hidden, |_, _| {}, cx);
        let gear = MoonButton::new("core-status-warn-gear")
            .label("⚙")
            .variant(MoonButtonVariant::Ghost)
            .size(MoonButtonSize::density(cx))
            .render();
        let tail = toolbar_tail_el(false, menu, gear, |_, _, _| {}, cx);
        let scope = div().w(px(170.0)).h(px(28.0)).flex_none();
        toolbar_row(scope, modes, search, tail, cx)
    }
}

/// Segments, the search slot, and the trailing group do not overlap or cross the row's right edge.
///
/// A second line is allowed: overlap is tested in both axes, so a wrapped tail is not a collision.
#[gpui::test]
fn toolbar_slots_stay_inside_the_row_in_every_locale(cx: &mut gpui::TestAppContext) {
    layout_fixture::install_dark(cx);
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        for width in WIDTHS {
            let mut visual = layout_fixture::open(cx, width, 320.0, |window, cx| {
                let search = cx.new(|cx| MoonInputState::new(window, cx));
                ToolbarProbe {
                    search,
                    available: 0.0,
                }
            });
            layout_fixture::draw(&mut visual);
            let at = format!("{code} @ {width}");
            let row = visual.debug_bounds("cs-toolbar").expect("toolbar");
            let slots = [
                visual.debug_bounds("cs-toolbar-scope").expect("scope"),
                visual.debug_bounds("cs-toolbar-modes").expect("modes"),
                visual.debug_bounds("cs-toolbar-search").expect("search"),
                visual.debug_bounds("cs-toolbar-tail").expect("tail"),
            ];
            let names = ["scope", "modes", "search", "tail"];
            for (name, slot) in names.iter().zip(slots) {
                assert!(
                    layout_fixture::inside(slot, row),
                    "{name} left the toolbar at {at}: {slot:?} / {row:?}"
                );
            }
            // The scope control is set apart from the view switch by its own padded rule, so
            // it never reads as one more segment.
            let rule_pad = visual.update(|_, app| f32::from(design::ui_px(app, 8.0)));
            assert!(
                layout_fixture::width_of(slots[0]) + 0.5 >= 170.0 + rule_pad,
                "scope slot lost its separating pad at {at}: {:?}",
                slots[0]
            );
            if (f32::from(slots[0].origin.y) - f32::from(slots[1].origin.y)).abs() <= 0.5 {
                assert!(
                    f32::from(slots[1].origin.x) - f32::from(slots[0].right()) + 0.5 >= rule_pad,
                    "view switch touches the scope rule at {at}: {:?} / {:?}",
                    slots[0],
                    slots[1]
                );
            }
            let search_floor = visual.update(|_, app| f32::from(design::ui_px(app, 140.0)));
            assert!(
                layout_fixture::width_of(slots[2]) + 0.5 >= search_floor,
                "search slot fell below its 140px floor at {at}: {:?}",
                slots[2]
            );
            for left in 0..slots.len() {
                for right in (left + 1)..slots.len() {
                    assert!(
                        !layout_fixture::overlaps(slots[left], slots[right]),
                        "{} overlaps {} at {at}: {:?} / {:?}",
                        names[left],
                        names[right],
                        slots[left],
                        slots[right]
                    );
                }
            }
        }
    }
}

/// At 300 px every locale keeps five non-overlapping segments inside the strip, on the short
/// label tier. The width is passed in: the production measure is deferred, so the first paint
/// of this window has not stored one.
#[gpui::test]
fn narrow_dock_keeps_every_mode_inside_the_toolbar(cx: &mut gpui::TestAppContext) {
    layout_fixture::install_dark(cx);
    let width = 300.0;
    for code in LOCALES {
        let _locale = crate::test_locale::force(code);
        let mut visual = layout_fixture::open(cx, width, 160.0, move |window, cx| {
            let search = cx.new(|cx| MoonInputState::new(window, cx));
            ToolbarProbe {
                search,
                available: width,
            }
        });
        layout_fixture::draw(&mut visual);
        let at = format!("{code} @ {width}");
        let row = visual.debug_bounds("cs-toolbar").expect("toolbar");
        let modes = visual.debug_bounds("cs-toolbar-modes").expect("modes");
        assert!(
            layout_fixture::inside(modes, row),
            "modes left the toolbar at {at}: {modes:?} / {row:?}"
        );
        let cell_ids = [
            "moon-segment-cell:0",
            "moon-segment-cell:1",
            "moon-segment-cell:2",
            "moon-segment-cell:3",
            "moon-segment-cell:4",
        ];
        let cells: Vec<_> = cell_ids
            .iter()
            .map(|id| {
                visual
                    .debug_bounds(id)
                    .unwrap_or_else(|| panic!("segment {id} missing at {at}"))
            })
            .collect();
        for (index, cell) in cells.iter().enumerate() {
            assert!(
                layout_fixture::inside(*cell, modes),
                "segment {index} left the strip at {at}: {cell:?} / {modes:?}"
            );
        }
        for left in 0..cells.len() {
            for right in (left + 1)..cells.len() {
                assert!(
                    !layout_fixture::overlaps(cells[left], cells[right]),
                    "segment {left} overlaps {right} at {at}: {:?} / {:?}",
                    cells[left],
                    cells[right]
                );
            }
        }
        assert!(
            visual.debug_bounds("cs-mode-tier-short").is_some(),
            "300px chose a tier other than short at {at}"
        );
        assert!(
            visual.debug_bounds("cs-mode-tier-full").is_none()
                && visual.debug_bounds("cs-mode-tier-plain").is_none(),
            "300px kept a wider tier at {at}"
        );
    }
}
