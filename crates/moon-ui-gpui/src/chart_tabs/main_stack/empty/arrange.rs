//! Where each block of the empty screen goes: one dropdown per block, and one way back.
//!
//! **A choice, not a gesture.** The blocks are placed from a list of nine named anchors rather than
//! dragged around the screen. Dragging would need a mode of its own — the tables are clickable, and
//! a drag that started on a coin would have to decide whether it meant "open this chart" — and it
//! would be unreachable for anybody who does not use a pointer. A named list is reachable, says out
//! loud where a block will land before it lands there, and needs nothing switched off to be safe.
//!
//! **Five dropdowns, not four.** The mark and the line under it are placed separately, because they
//! are switched separately: somebody who wants the line without the mark is asking a question one
//! control could not answer. On the shipped arrangement both name the middle, where they stack.
//!
//! **A dropdown is state, and state needs a window.** Exactly like the rule's fields beside them,
//! these are built the first time the popup is opened (`detect::seed_empty_detect`) and live on the
//! stack from then on, so a re-render does not throw away an open menu.

use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
    Window, div, rgb,
};
use moon_core::config::layout::{EmptyBlock, EmptyPlaces, EmptySlot};
use moon_ui::{
    IndexPath, MoonButton, MoonButtonSize, MoonButtonVariant, MoonMenuSize, MoonPalette,
    MoonSelect, MoonSelectEvent, MoonSelectItem, MoonSelectState, h_flex, v_flex,
};
use rust_i18n::t;

use crate::chart_tabs::MainChartStack;
use crate::design;

/// Gap between a caption and its control, and between two rows, in design units. The rule's own
/// fields under these use the same one — they are one list of settings, not two.
const ROW_GAP: f32 = 8.0;
/// Width of one position dropdown, in design units, sized for the longest localized anchor name.
const SELECT_WIDTH: f32 = 132.0;

/// The locale key naming one block.
fn block_label(block: EmptyBlock) -> &'static str {
    match block {
        EmptyBlock::Logo => "crowd.block.logo",
        EmptyBlock::Hint => "crowd.block.hint",
        EmptyBlock::Minute => "crowd.block.minute",
        EmptyBlock::Traders => "crowd.block.traders",
        EmptyBlock::Coins => "crowd.block.coins",
    }
}

/// The locale key naming one anchor.
fn slot_label(slot: EmptySlot) -> &'static str {
    match slot {
        EmptySlot::TopStart => "crowd.slot.top_start",
        EmptySlot::TopCenter => "crowd.slot.top_center",
        EmptySlot::TopEnd => "crowd.slot.top_end",
        EmptySlot::MiddleStart => "crowd.slot.middle_start",
        EmptySlot::MiddleCenter => "crowd.slot.middle_center",
        EmptySlot::MiddleEnd => "crowd.slot.middle_end",
        EmptySlot::BottomStart => "crowd.slot.bottom_start",
        EmptySlot::BottomCenter => "crowd.slot.bottom_center",
        EmptySlot::BottomEnd => "crowd.slot.bottom_end",
    }
}

/// Which entry of [`EmptySlot::ALL`] one anchor is, which is the index a dropdown selects by.
fn slot_index(slot: EmptySlot) -> usize {
    EmptySlot::ALL
        .into_iter()
        .position(|candidate| candidate == slot)
        .unwrap_or(0)
}

/// One dropdown per block, in the order the popup lists them.
///
/// Cloneable because a reset has to reach the controls while the stack is already borrowed for the
/// write: the entities inside are handles, so a clone is the same five dropdowns.
#[derive(Clone)]
pub(crate) struct PlaceSelects {
    selects: Vec<(EmptyBlock, Entity<MoonSelectState<EmptySlot>>)>,
}

impl PlaceSelects {
    /// Build the five dropdowns and wire each one to its block's key.
    ///
    /// Args:
    ///     places: Where the blocks stand right now, which is what each dropdown opens on.
    ///     window: The window the dropdowns belong to; a `MoonSelectState` cannot exist without one.
    ///     cx: Stack context used to create and subscribe.
    pub(super) fn seed(
        places: EmptyPlaces,
        window: &mut Window,
        cx: &mut Context<MainChartStack>,
    ) -> Self {
        let mut selects = Vec::with_capacity(EmptyBlock::COUNT);
        for block in EmptyBlock::ALL {
            let items: Vec<MoonSelectItem<EmptySlot>> = EmptySlot::ALL
                .into_iter()
                .map(|slot| MoonSelectItem::new(slot, t!(slot_label(slot)).to_string()))
                .collect();
            let chosen = IndexPath::new(slot_index(places.slot(block)));
            let state = cx.new(|cx| MoonSelectState::new(items, Some(chosen), window, cx));
            cx.subscribe(
                &state,
                move |this, _state, event: &MoonSelectEvent<EmptySlot>, cx| {
                    // `Confirm(None)` is the control being CLEARED, which these never are: every
                    // block is somewhere, and "nowhere" is what its visibility switch means.
                    if let MoonSelectEvent::Confirm(Some(slot)) = event {
                        this.set_place(block, *slot, cx);
                    }
                },
            )
            .detach();
            selects.push((block, state));
        }
        Self { selects }
    }

    /// Put the dropdowns back on an arrangement that was changed from somewhere other than them.
    ///
    /// Another group window can edit the shared layout while this popup is open. Setting the value does NOT report a confirmation back — that
    /// is emitted by the menu's own choice — so this cannot loop back into another write.
    ///
    /// Args:
    ///     places: The arrangement the controls must now show.
    ///     cx: Any application context.
    pub(super) fn show(&self, places: EmptyPlaces, cx: &mut App) {
        for (block, state) in &self.selects {
            let slot = places.slot(*block);
            state.update(cx, |state, cx| {
                if state.selected_value() != Some(&slot) && state.set_selected_value(&slot) {
                    cx.notify();
                }
            });
        }
    }
}

/// The popup's placement section: a heading with the way back, and one row per block.
///
/// Args:
///     places: The five dropdowns.
///     view: Stack entity receiving the reset.
///     id: Group-scoped element identities.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
pub(super) fn block(
    places: &PlaceSelects,
    view: Entity<MainChartStack>,
    id: &dyn Fn(&str) -> SharedString,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, ROW_GAP);
    v_flex()
        .w_full()
        .gap(gap)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(gap)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("crowd.settings.places").to_string()),
                )
                .child(
                    MoonButton::new(id("places-reset"))
                        .label(t!("crowd.settings.places_reset").to_string())
                        .size(MoonButtonSize::Micro)
                        .variant(MoonButtonVariant::Ghost)
                        .tooltip(t!("crowd.settings.places_reset_tip").to_string())
                        .on_click(move |_, _window, app: &mut App| {
                            view.update(app, |this, cx| this.reset_places(cx));
                        }),
                ),
        )
        .children(
            places
                .selects
                .iter()
                .map(|(block, state)| row(*block, state, palette, cx)),
        )
        .into_any_element()
}

/// One block and where it goes.
///
/// Args:
///     block: Which block this row places.
///     state: Its dropdown.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
fn row(
    block: EmptyBlock,
    state: &Entity<MoonSelectState<EmptySlot>>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(design::t_body(cx))
                .text_color(rgb(palette.text))
                .child(t!(block_label(block)).to_string()),
        )
        .child(
            // The trigger box and the menu take the same width from one place: written out twice
            // they drift, and the menu opens narrower or wider than the control that opened it.
            div().w(design::font_w_px(cx, SELECT_WIDTH)).child(
                MoonSelect::new(state)
                    .in_popover()
                    .trigger_size(MoonButtonSize::Micro)
                    .menu_width(design::font_w(cx, SELECT_WIDTH))
                    .menu_size(MoonMenuSize::Compact),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
