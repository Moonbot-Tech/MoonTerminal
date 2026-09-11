//! Where each block of the empty screen goes, or that it does not: one dropdown per block, and one
//! way back.
//!
//! **A choice, not a gesture.** The blocks are placed from a list of nine named anchors rather than
//! dragged around the screen. Dragging would need a mode of its own — the tables are clickable, and
//! a drag that started on a coin would have to decide whether it meant "open this chart" — and it
//! would be unreachable for anybody who does not use a pointer. A named list is reachable, says out
//! loud where a block will land before it lands there, and needs nothing switched off to be safe.
//!
//! **"Hidden" is the tenth entry, not a second control.** A block that is drawn is drawn somewhere,
//! so "where" and "whether" are one question with ten answers, and the popup asks it once. What the
//! answer is KEPT as is still two keys — the switch and the anchor — because hiding a block must not
//! forget where it was: the person who hides the minute and brings it back finds it where they left
//! it, and the brand's switch is read by two more empty surfaces that know nothing of anchors.
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

use super::{EmptyScreen, switch_of};
use crate::chart_tabs::MainChartStack;
use crate::design;
use crate::panels::{COMPACT_CHECKBOX_FONT, POPUP_GROUP_GAP};

/// Gap between a caption and its control, and between two rows, in design units: the pitch a popup
/// group packs its rows at, so these rows and the framed ones under them read as one list.
const ROW_GAP: f32 = POPUP_GROUP_GAP;
/// Width of one position dropdown, in design units, sized for the longest localized entry.
const SELECT_WIDTH: f32 = 132.0;

/// One answer to "where does this block go": one of the nine anchors, or nowhere.
///
/// The dropdown's value. It is NOT what is stored — the layout keeps a switch and an anchor per
/// block, and this is the two of them read together — so that hiding a block leaves its anchor
/// where it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// The block is not drawn.
    Hidden,
    /// The block is drawn at this anchor.
    At(EmptySlot),
}

impl Placement {
    /// Every entry, in the order the list shows them: "hidden" first, then the anchors in reading
    /// order. The index into this is what a dropdown selects by.
    pub(super) fn all() -> impl Iterator<Item = Self> {
        std::iter::once(Self::Hidden).chain(EmptySlot::ALL.into_iter().map(Self::At))
    }

    /// Which entry of [`Placement::all`] this is.
    pub(super) fn index(self) -> usize {
        match self {
            Self::Hidden => 0,
            Self::At(slot) => 1 + slot_index(slot),
        }
    }

    /// What one block's two keys say together.
    ///
    /// Args:
    ///     block: The block asked about.
    ///     screen: What is switched on.
    ///     places: Where each block is anchored.
    pub(super) fn of(block: EmptyBlock, screen: &EmptyScreen, places: EmptyPlaces) -> Self {
        if (switch_of(block).read)(screen) {
            Self::At(places.slot(block))
        } else {
            Self::Hidden
        }
    }

    /// The locale key naming this entry.
    fn label(self) -> &'static str {
        match self {
            Self::Hidden => "crowd.slot.hidden",
            Self::At(slot) => slot_label(slot),
        }
    }
}

/// The locale key naming one block: the same one its switch carries, so a row and the switch behind
/// it cannot call the block two things.
fn block_label(block: EmptyBlock) -> &'static str {
    switch_of(block).label
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

/// Which entry of [`EmptySlot::ALL`] one anchor is.
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
    selects: Vec<(EmptyBlock, Entity<MoonSelectState<Placement>>)>,
}

impl PlaceSelects {
    /// Build the five dropdowns and wire each one to its block's two keys.
    ///
    /// Args:
    ///     screen: What is switched on right now.
    ///     places: Where the blocks stand right now; with `screen`, what each dropdown opens on.
    ///     window: The window the dropdowns belong to; a `MoonSelectState` cannot exist without one.
    ///     cx: Stack context used to create and subscribe.
    pub(super) fn seed(
        screen: &EmptyScreen,
        places: EmptyPlaces,
        window: &mut Window,
        cx: &mut Context<MainChartStack>,
    ) -> Self {
        let mut selects = Vec::with_capacity(EmptyBlock::COUNT);
        for block in EmptyBlock::ALL {
            let items: Vec<MoonSelectItem<Placement>> = Placement::all()
                .map(|placement| MoonSelectItem::new(placement, t!(placement.label()).to_string()))
                .collect();
            let chosen = IndexPath::new(Placement::of(block, screen, places).index());
            let state = cx.new(|cx| MoonSelectState::new(items, Some(chosen), window, cx));
            cx.subscribe(
                &state,
                move |this, _state, event: &MoonSelectEvent<Placement>, cx| {
                    // `Confirm(None)` is the control being CLEARED, which these never are: every
                    // block has an answer, and "not drawn" is an entry of the list, not an absence.
                    if let MoonSelectEvent::Confirm(Some(placement)) = event {
                        this.set_placement(block, *placement, cx);
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
    /// Another group window can edit the shared layout while this popup is open, and the brand's
    /// switch can be flipped by a reset. Setting the value does NOT report a confirmation back —
    /// that is emitted by the menu's own choice — so this cannot loop back into another write.
    ///
    /// Args:
    ///     screen: What is switched on now.
    ///     places: Where each block is anchored now.
    ///     cx: Any application context.
    pub(super) fn show(&self, screen: &EmptyScreen, places: EmptyPlaces, cx: &mut App) {
        for (block, state) in &self.selects {
            let placement = Placement::of(*block, screen, places);
            state.update(cx, |state, cx| {
                if state.selected_value() != Some(&placement)
                    && state.set_selected_value(&placement)
                {
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
    state: &Entity<MoonSelectState<Placement>>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .child(
            // The face a compact checkbox gives its label — its size, soft text — so this row and
            // the rule's switch under it read as one list rather than as a heading over a note.
            div()
                .flex_1()
                .min_w_0()
                .text_size(design::text_px(cx, COMPACT_CHECKBOX_FONT))
                .text_color(rgb(palette.text_soft))
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
