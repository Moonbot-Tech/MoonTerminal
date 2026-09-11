//! The empty Main screen: what it shows, and the ⚙ that decides.
//!
//! Everything here is about a stack with no chart in it. It is its own file because the empty
//! screen has grown a state of its own — up to five layers, a live view over the crowd's
//! statistics among them — and none of that belongs in the middle of the stack's layout code.
//!
//! **One screen, five blocks, any combination.** The logo, the hint under it and the three crowd
//! tables are independent layers of the same screen: the brand sits in the middle, the tables are
//! anchored to the edges, and the statistics are drawn LAST so a figure is never behind the mark.
//! Nothing here is exclusive — the point of separate blocks is that somebody who wants only the
//! trader board gets only the trader board. Each block is asked ONE question in the popup — where
//! it goes, "nowhere" being an answer — and keeps two keys: the switch, and the anchor it goes back
//! to when it is shown again (`arrange`).
//!
//! One of the five reaches past this screen. The brand is drawn on three empty surfaces — here, an
//! AddToChart stack with no charts, and a chart slot waiting for data — and one switch governs all
//! three, because it says "show the logo" and not "show it here". The two outside this file read
//! [`empty_logo`]; this one draws from the switch's own state, and a test pins that the two
//! readings of that key cannot drift. Changing it refreshes every window, since neither of the
//! other two observes this stack.
//!
//! **The default is the terminal as it always was:** the logo and its one line of help, and no
//! network at all. The tables read a public service, which is not something an update may start
//! doing on somebody's behalf.
//!
//! **The sixth switch is not a layer.** The crowd's rule draws nothing here — it puts a card in the
//! Detects panel when a coin's minute crosses its lines — and it is the one switch that costs a
//! connection while nobody is looking at this screen at all, because a detection that only fired
//! while somebody happened to be watching an empty Main would be worth nothing. Everything else
//! about it — its lines, how long its cards stay, what happens at the last seat — lives in
//! [`detect`], which is also where the controls under that switch are built.
//!
//! **A table nobody shows is a table nobody reads.** The three crowd switches decide which halves
//! of the service this screen CLAIMS — the minute is the live trade socket, the two day boards are
//! one REST poller — so switching a table off releases its claim rather than merely hiding what it
//! delivers. The reading itself belongs to `crowd::service`, one per terminal, which closes
//! whatever no screen and no rule is asking for.

use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use moon_core::config::layout::{EmptyBlock, EmptyPlaces, WindowLayout};
use moon_ui::{MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement};
use rust_i18n::t;

use super::MainChartStack;
use crate::crowd::{CrowdParts, CrowdStatsView, place};
use crate::design;
use crate::panels::{popup_close_button, popup_gear_trigger, popup_title};

/// How far the button sits from the top-right corner, in design units.
///
/// The same corner whatever is switched on, because it is the control that switches it: one that
/// moved with the setting would be a control nobody could find twice. The minute table starts
/// below it — see `crowd::table`'s own top inset — so the two share the corner without overlapping.
const GEAR_INSET: f32 = 10.0;

/// Preferred popup width before the group inset. The complete outer box is capped to the
/// viewport so enlarged UI chrome cannot hide the placement controls.
const CONTENT_WIDTH: f32 = 360.0;
/// Gap between the popup's sections — the title row, the placement rows, the rule's frame — in
/// design units. Wider than the pitch inside a section, so the sections read as sections.
const POPUP_GAP: f32 = 8.0;
/// Widest the hint under the logo is allowed to run before it wraps, in design units.
const HINT_WIDTH: f32 = 420.0;
/// What the empty screen shows.
///
/// Five independent layers rather than a mode, because they answer five different questions and a
/// person may want any mixture of them. Anything the screen grows later is one more row in
/// [`SWITCHES`], which is the whole reason that is a table rather than five fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EmptyScreen {
    /// The Moonbot mark in the middle.
    logo: bool,
    /// The one line under it naming the gesture that opens a chart.
    hint: bool,
    /// The rolling minute of the crowd's trades, top right.
    minute: bool,
    /// The service's trader board for the day, down the left.
    traders: bool,
    /// The service's coin board for the day, bottom right.
    coins: bool,
    /// Whether the crowd's rule is watched. It draws nothing HERE — see the module doc.
    detect: bool,
}

impl EmptyScreen {
    /// Read the arrangement out of the saved layout.
    ///
    /// Every switch is stored as an OPTION: an absent key means "never chosen" and takes the
    /// default below, which is what lets a default change later without silently overriding
    /// somebody who deliberately switched something off.
    ///
    /// Args:
    ///     layout: Persisted window layout.
    fn restore(layout: &WindowLayout) -> Self {
        let mut screen = Self::default();
        for switch in &SWITCHES {
            (switch.set)(
                &mut screen,
                (switch.saved)(layout).unwrap_or(switch.default),
            );
        }
        screen
    }

    /// Whether the rule is switched on, which is what makes its thresholds editable.
    fn detect(self) -> bool {
        self.detect
    }

    /// Every block that is switched on, in [`EmptyBlock::ALL`] order — what the frame has to make
    /// room for. The rule is not in it: it draws nothing here.
    pub(super) fn blocks_on(self) -> Vec<EmptyBlock> {
        EmptyBlock::ALL
            .into_iter()
            .filter(|block| match block {
                EmptyBlock::Logo => self.logo,
                EmptyBlock::Hint => self.hint,
                EmptyBlock::Minute => self.minute,
                EmptyBlock::Traders => self.traders,
                EmptyBlock::Coins => self.coins,
            })
            .collect()
    }

    /// Which tables are on, for the view that draws them.
    fn parts(self) -> CrowdParts {
        CrowdParts {
            minute: self.minute,
            traders: self.traders,
            coins: self.coins,
        }
    }
}

/// One switch: what it says, what it is worth when nobody has chosen, and where it is kept.
///
/// Every row is DATA. Adding a switch is one entry here, and the four mechanical parts — restore
/// it, show it, set it, save it — cannot drift apart because none of them is written twice.
///
/// A switch that governs a BLOCK is not shown as a checkbox: its block's dropdown carries "hidden"
/// as an entry (`arrange::Placement`), and that entry is this switch. The one switch with no block
/// — the rule — is the checkbox the popup still draws.
struct Switch {
    /// Element-identity suffix, unique within the popup.
    id: &'static str,
    /// Locale key of the visible label: the block's name for a block, the rule's for the rule.
    label: &'static str,
    /// The block this switch draws, or `None` for the rule, which draws nothing.
    block: Option<EmptyBlock>,
    /// What it is worth on a profile that has never opened this popup.
    default: bool,
    /// Read the current value out of the arrangement.
    read: fn(&EmptyScreen) -> bool,
    /// Apply an edited value.
    set: fn(&mut EmptyScreen, bool),
    /// Read this switch's saved value, or `None` when it was never chosen.
    saved: fn(&WindowLayout) -> Option<bool>,
    /// Save an edited value under this switch's own `layout.toml` key, or `None` to forget it.
    ///
    /// Only the EDITED key is ever written. Stamping the others would turn "never chosen" into an
    /// explicit value for switches nobody touched. Forgetting is how a reset works: a key that is
    /// cleared takes the default again, including a default that changes later.
    store: fn(&mut WindowLayout, Option<bool>),
}

/// The switch behind one block.
///
/// Args:
///     block: The block asked about.
fn switch_of(block: EmptyBlock) -> &'static Switch {
    SWITCHES
        .iter()
        .find(|switch| switch.block == Some(block))
        // Every block has a row in `SWITCHES`, and a test pins it; reaching this would mean the
        // table lost one, which the test catches before any popup is drawn.
        .unwrap_or(&SWITCHES[0])
}

/// Every switch, in the order the popup shows them: what the screen already was, then what can be
/// added to it, then the one that is not a layer at all.
const SWITCHES: [Switch; 6] = [
    Switch {
        id: "logo",
        label: "crowd.block.logo",
        block: Some(EmptyBlock::Logo),
        default: LOGO_DEFAULT,
        read: |screen| screen.logo,
        set: |screen, value| screen.logo = value,
        saved: |layout| layout.main_empty_logo,
        store: |layout, value| layout.main_empty_logo = value,
    },
    Switch {
        id: "hint",
        label: "crowd.block.hint",
        block: Some(EmptyBlock::Hint),
        default: true,
        read: |screen| screen.hint,
        set: |screen, value| screen.hint = value,
        saved: |layout| layout.main_empty_hint,
        store: |layout, value| layout.main_empty_hint = value,
    },
    Switch {
        id: "minute",
        label: "crowd.block.minute",
        block: Some(EmptyBlock::Minute),
        default: false,
        read: |screen| screen.minute,
        set: |screen, value| screen.minute = value,
        saved: |layout| layout.main_empty_minute,
        store: |layout, value| layout.main_empty_minute = value,
    },
    Switch {
        id: "traders",
        label: "crowd.block.traders",
        block: Some(EmptyBlock::Traders),
        default: false,
        read: |screen| screen.traders,
        set: |screen, value| screen.traders = value,
        saved: |layout| layout.main_empty_traders,
        store: |layout, value| layout.main_empty_traders = value,
    },
    Switch {
        id: "coins",
        label: "crowd.block.coins",
        block: Some(EmptyBlock::Coins),
        default: false,
        read: |screen| screen.coins,
        set: |screen, value| screen.coins = value,
        saved: |layout| layout.main_empty_coins,
        store: |layout, value| layout.main_empty_coins = value,
    },
    Switch {
        id: "detect",
        label: "crowd.settings.detect",
        block: None,
        default: DETECT_DEFAULT,
        read: |screen| screen.detect,
        set: |screen, value| screen.detect = value,
        saved: |layout| layout.main_empty_detect,
        store: |layout, value| layout.main_empty_detect = value,
    },
];

pub(super) mod arrange;
pub(super) mod detect;

#[cfg(test)]
mod tests;

use arrange::PlaceSelects;
use detect::DETECT_DEFAULT;
pub(crate) use detect::{CrowdCards, DetectInputs, crowd_cards, crowd_rule_for_run};

/// Whether a profile that has never opened this popup sees the brand.
///
/// Named rather than read out of [`SWITCHES`] by position, because it is now asked for from three
/// places: this screen, an empty AddToChart stack, and an empty chart slot.
const LOGO_DEFAULT: bool = true;

/// Whether the brand is drawn on an empty surface.
///
/// The switch says "show the logo", not "show the logo HERE", and a reader who switched it off went
/// looking for a mark and found it again on the next empty tab. So the surfaces that draw it
/// outside this file ask here: an AddToChart or Custom stack holding no charts, and a chart slot
/// waiting for its data.
///
/// The empty screen in this file draws its own mark from [`EmptyScreen`], which is the SWITCH's
/// state rather than a second opinion about the key: both come from `main_empty_logo` with
/// [`LOGO_DEFAULT`] behind it, and a test pins that they agree, exactly as one does for the rule's
/// three keys.
///
/// What it does NOT govern is the opaque plate under the mark. That plate is not decoration: an
/// empty slot covers a stale graph left in the own pass beneath the GPUI scene, and a detached
/// window covers its own white backing. Taking the mark away must leave both standing.
///
/// Args:
///     layout: Persisted window layout.
pub(crate) fn empty_logo(layout: &WindowLayout) -> bool {
    layout.main_empty_logo.unwrap_or(LOGO_DEFAULT)
}

/// Clear every block's anchor and switch, and nothing else.
///
/// Cleared rather than written with today's defaults, for the reason [`EmptyPlaces::reset`] gives:
/// a profile that has been reset is a profile that has never chosen. The rule's keys are left
/// alone — it is not a block.
///
/// Args:
///     layout: Persisted window layout, edited in place.
fn forget_arrangement(layout: &mut WindowLayout) {
    EmptyPlaces::reset(layout);
    for switch in SWITCHES.iter().filter(|switch| switch.block.is_some()) {
        (switch.store)(layout, None);
    }
}

impl MainChartStack {
    /// How this profile has arranged the empty screen.
    pub(super) fn empty_arrangement(&self, cx: &App) -> EmptyScreen {
        EmptyScreen::restore(&self.backend.read(cx).layout)
    }

    /// Bring the statistics view into line with the switches and with the stack.
    ///
    /// Called from `render` BEFORE it branches, which is the only place both answers are reachable:
    /// called from inside the empty branch instead, a chart opening would never reach it and the
    /// view would go on drawing under a chart it cannot be seen through.
    ///
    /// A change of SET is passed to the view rather than rebuilding it: the set decides which
    /// reader threads exist, and the view opens and closes them in place. Rebuilding would throw
    /// away the minute counted so far, the socket already open and the boards already read — the
    /// whole screen starting over because one more table was asked for.
    ///
    /// Args:
    ///     cx: Stack context, used to build the view and read the switches.
    ///
    /// Returns:
    ///     The view to draw, or `None` when no table is switched on or a chart is covering it. It
    ///     is only the DRAWING that a covering chart stops — the view itself stays and keeps
    ///     reading.
    pub(super) fn sync_crowd_stats(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Entity<CrowdStatsView>> {
        let parts = self.empty_arrangement(cx).parts();
        // A chart drawn over the empty screen COVERS it; it does not end it. The view goes on
        // reading so that closing the chart comes back to a minute that has been counted all
        // along, and stops drawing, which is all a covered screen costs.
        let drawn = self.charts.is_empty();
        // A popup left open on a screen that is no longer DRAWN would open itself again the next
        // time the last chart closed. It has nothing to do with which tables are on — which is
        // exactly the mistake this used to make: clearing it whenever no table was on meant that
        // on the shipped defaults every render closed the popup a person had just opened, and no
        // switch could ever be reached.
        if !drawn && self.empty_settings_open {
            // A chart has covered the screen the popup belongs to. That is a way out like any
            // other, so a threshold typed and not yet blurred is kept rather than dropped.
            self.empty_settings_open = false;
            self.commit_empty_detect(cx);
        }
        if !parts.any() {
            // Dropping the entity is the teardown: there is no "stop" to call, because the view
            // holds nothing but a lease on the shared service, and a released lease stops counting
            // towards what that service reads. Whether anything closes is the service's answer —
            // the rule may still want the same stream.
            self.crowd = None;
            return None;
        }
        match &self.crowd {
            Some((had, view)) if *had != parts => {
                view.update(cx, |view, cx| view.show(parts, cx));
                let view = view.clone();
                self.crowd = Some((parts, view));
            }
            // Built only once the screen is actually SHOWN. A profile that opted in and spent the
            // session on a chart would otherwise hold a live connection for a screen it never
            // looked at; once built, it stays and keeps reading under a chart.
            None if drawn => {
                let backend = self.backend.clone();
                let group = self.group.clone();
                let view = cx.new(|cx| CrowdStatsView::new(parts, backend, group, cx));
                // The view is COMPOSED by the empty screen rather than drawn as a child of it: its
                // boards have to stack with the brand in a shared cell, which they could not do
                // from a layer of their own. Nothing therefore re-renders when the view notifies,
                // so this stack listens and repaints itself — which is what carries the view's
                // twelve-a-second frame chain, and its wake from the service, on to the screen.
                cx.observe(&view, |_, _view, cx| cx.notify()).detach();
                self.crowd = Some((parts, view));
            }
            _ => {}
        }
        let view = self.crowd.as_ref().map(|(_, view)| view.clone())?;
        view.update(cx, |view, cx| view.set_drawn(drawn, cx));
        drawn.then_some(view)
    }

    /// Write one switch and let the screen follow on the next frame.
    ///
    /// Args:
    ///     switch: Which one.
    ///     on: What the checkbox now says.
    ///     cx: Stack context used to persist and repaint.
    fn set_switch(&mut self, switch: &'static Switch, on: bool, cx: &mut Context<Self>) {
        if (switch.read)(&self.empty_arrangement(cx)) == on {
            return;
        }
        let store = switch.store;
        self.backend.update(cx, |backend, _| {
            store(&mut backend.layout, Some(on));
            backend.layout_dirty = true;
        });
        self.publish_crowd_rule(cx);
        // The view itself is built or dropped by `sync_crowd_stats` on the repaint this asks for,
        // so one place decides whether it exists rather than two.
        cx.notify();
        // Every window, and every switch. There is ONE `layout` behind all of them, and more than
        // one group window can be drawing from it: a second window's empty Main shows the same five
        // dropdowns and the same layers, and the logo reaches further still — an empty AddToChart
        // stack, an empty chart slot. None of those observes this one, and `cx.notify()` reaches
        // only the stack that was clicked. This marks every window dirty, which is what it costs;
        // a switch is a click, so the price is one frame for a setting that means the same thing
        // everywhere at once.
        cx.refresh_windows();
    }

    /// Answer one block's dropdown: draw it at an anchor, or stop drawing it.
    ///
    /// The two keys behind the one control, written in ONE update and followed by one repaint: an
    /// answer is a click, and un-hiding a block at a new anchor is not two clicks. An anchor is
    /// written only when one was chosen, so hiding a block leaves its anchor exactly where it was —
    /// that is what "show it again" goes back to. Only the answered block's keys are touched, for
    /// the reason [`Switch::store`] gives. Answering what the block already is costs no frame.
    ///
    /// The rule is not republished: it reads its own keys (`crowd_rule`), and none of them is
    /// written here. The tables' connections follow the switches on the repaint this asks for
    /// (`sync_crowd_stats`), which is the one place that decides whether the view exists.
    ///
    /// Args:
    ///     block: Which block was answered for.
    ///     placement: The answer.
    ///     cx: Stack context used to persist and repaint.
    pub(super) fn set_placement(
        &mut self,
        block: EmptyBlock,
        placement: arrange::Placement,
        cx: &mut Context<Self>,
    ) {
        let layout = &self.backend.read(cx).layout;
        let screen = EmptyScreen::restore(layout);
        let places = EmptyPlaces::restore(layout);
        if arrange::Placement::of(block, &screen, places) == placement {
            return;
        }
        // Only what the answer CHANGES is written: a shown block moved to a new anchor keeps its
        // switch key as it was (a never-chosen `None` included), and a hidden block shown again at
        // the anchor it already resolves to keeps that anchor unwritten.
        let switch = switch_of(block);
        let shown = (switch.read)(&screen);
        let moved = match placement {
            arrange::Placement::Hidden => None,
            arrange::Placement::At(slot) => (places.slot(block) != slot).then_some(slot),
        };
        let store = switch.store;
        self.backend.update(cx, |backend, _| {
            match placement {
                arrange::Placement::Hidden if shown => store(&mut backend.layout, Some(false)),
                arrange::Placement::At(_) if !shown => store(&mut backend.layout, Some(true)),
                _ => {}
            }
            if let Some(slot) = moved {
                block.store(&mut backend.layout, Some(slot));
            }
            backend.layout_dirty = true;
        });
        cx.notify();
        // Every window, for the reason `set_switch` refreshes them all: one `layout` stands behind
        // every group window's empty screen, and none of them observes this stack.
        cx.refresh_windows();
    }

    /// Forget every block's anchor AND switch, so the screen comes back to the one it shipped with.
    ///
    /// Both, because the popup asks one question per block and a reset answers it the way a fresh
    /// profile would: the brand and its line in the middle, no table. The rule is not a block and
    /// keeps its setting — it is not part of the arrangement, and switching a watch off is not
    /// something a layout button may do.
    ///
    /// The dropdowns are put back by hand afterwards: they hold their own selection, and a reset
    /// that moved the blocks without moving the controls would leave five dropdowns naming places
    /// nothing is drawn in.
    ///
    /// Args:
    ///     cx: Stack context used to persist and repaint.
    pub(super) fn reset_places(&mut self, cx: &mut Context<Self>) {
        self.backend.update(cx, |backend, _| {
            forget_arrangement(&mut backend.layout);
            backend.layout_dirty = true;
        });
        // No rule to republish: its keys are not among the forgotten ones. A table that just went
        // off drops its connection on the repaint below, in `sync_crowd_stats`.
        if let Some(selects) = self.empty_places.clone() {
            let layout = &self.backend.read(cx).layout;
            selects.show(
                &EmptyScreen::restore(layout),
                EmptyPlaces::restore(layout),
                cx,
            );
        }
        cx.notify();
        cx.refresh_windows();
    }

    /// Close the popup, guarding the double report a popover makes when its own trigger is clicked.
    ///
    /// The ✕ is a way of FINISHING an edit, so it commits like every other way out. A controlled
    /// popover applies its closed state during render without reporting it through
    /// `on_open_change`, so this path cannot rely on that one — and a field that never lost focus
    /// never blurred either.
    fn close_empty_settings(&mut self, cx: &mut Context<Self>) {
        if !self.empty_settings_open {
            return;
        }
        self.empty_settings_open = false;
        self.commit_empty_detect(cx);
        cx.notify();
    }

    /// The ⚙ in the corner of the empty screen, with its popup.
    ///
    /// Args:
    ///     window: Host viewport used to bound the scrolling popup.
    ///     palette: Active MoonUI palette.
    ///     cx: Stack context used to read the switches and wire the toggles.
    ///
    /// Returns:
    ///     The corner control, absolutely placed.
    pub(super) fn empty_settings(
        &self,
        window: &Window,
        palette: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(selects) = &self.empty_places {
            let layout = &self.backend.read(cx).layout;
            selects.show(
                &EmptyScreen::restore(layout),
                EmptyPlaces::restore(layout),
                cx,
            );
        }
        let view = cx.entity();
        let inset = design::ui_px(cx, GEAR_INSET);
        let open = self.empty_settings_open;
        // Scoped by group, like every other element identity in this stack: two group windows are
        // two empty screens, and a shared id would give them one hover and one open state.
        let group = self.group.clone();
        let id = move |part: &str| SharedString::from(format!("main-empty-{group}-{part}"));
        let mut popover = MoonPopover::new(id("settings-popover"))
            // Down and to the left, out of the corner it is anchored in.
            .placement(MoonPopoverPlacement::BottomEnd)
            // Limit the OUTER box; MoonPopover owns its padding and border inside this width.
            .width(f32::from(popup_outer_width(
                design::ui_px(cx, CONTENT_WIDTH) + px(crate::panels::popup_group_inset_px(cx)),
                window.viewport_size().width,
                inset + window.client_inset().unwrap_or(px(0.0)),
            )))
            .close_on_content_click(false)
            // Nested select menus may extend beyond this popover; the close button remains live.
            .overlay_closable(false)
            .open(open)
            .on_open_change({
                let view = view.clone();
                move |open, window, app| {
                    view.update(app, |this, cx| {
                        this.empty_settings_open = open;
                        if open {
                            this.seed_empty_detect(window, cx);
                        } else {
                            // Closing the popup is a way of finishing an edit, and a field that
                            // never lost focus never blurred: without this, a typed threshold
                            // followed by a click on the ✕ would be thrown away.
                            this.commit_empty_detect(cx);
                        }
                        cx.notify();
                    });
                }
            })
            .trigger(popup_gear_trigger(
                id("settings"),
                t!("crowd.settings.btn").to_string(),
                open,
            ));
        if open {
            popover = popover.content(
                div()
                    .id(id("settings-scroll"))
                    .max_h(popup_body_height(
                        window.viewport_size().height,
                        design::ui_px(cx, 64.0),
                    ))
                    .overflow_y_scroll()
                    .child(settings_content(
                        self.empty_arrangement(cx),
                        crowd_cards(&self.backend.read(cx).layout),
                        self.empty_detect.as_ref(),
                        self.empty_places.as_ref(),
                        view,
                        &id,
                        palette,
                        cx,
                    )),
            );
        }
        div()
            .absolute()
            .right(inset)
            .top(inset)
            .child(popover)
            .into_any_element()
    }
}

/// Reserve space for the trigger, outer popup chrome and viewport margins without a minimum
/// that could make the popup taller than the window.
fn popup_body_height(viewport: gpui::Pixels, chrome: gpui::Pixels) -> gpui::Pixels {
    (viewport - chrome).max(px(0.0))
}

/// Fit the complete popup, including internally owned chrome, between both window insets.
fn popup_outer_width(
    preferred: gpui::Pixels,
    viewport: gpui::Pixels,
    inset: gpui::Pixels,
) -> gpui::Pixels {
    preferred.min((viewport - inset * 2.0).max(px(0.0)))
}

/// The popup body: a title with its ✕, one dropdown per block, and the rule under them.
///
/// Args:
///     screen: What the rule's checkbox shows.
///     cards: How the rule's cards are set to behave.
///     inputs: The rule's fields, once the popup has been opened at least once.
///     places: The blocks' position dropdowns, built with the same window as those fields.
///     view: Stack entity receiving the edits.
///     id: Group-scoped element identities.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
fn settings_content(
    screen: EmptyScreen,
    cards: CrowdCards,
    inputs: Option<&DetectInputs>,
    places: Option<&PlaceSelects>,
    view: Entity<MainChartStack>,
    id: &dyn Fn(&str) -> SharedString,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Chrome belongs to MoonPopover; a second surface here would double the popup's background.
    // The labels are prose, so the popup flips to the UI face on its own root rather than
    // inheriting the screen's family.
    moon_ui::v_flex()
        .id(id("settings-popup"))
        .w_full()
        .font_family(design::ui_font())
        .gap(design::ui_px(cx, POPUP_GAP))
        .child(
            div()
                .flex()
                .w_full()
                .items_center()
                .child(popup_title(
                    t!("crowd.settings.title").to_string(),
                    palette,
                    cx,
                ))
                .child(popup_close_button(id("settings-close"), {
                    let view = view.clone();
                    move |_, _window, app: &mut App| {
                        view.update(app, |this, cx| this.close_empty_settings(cx));
                    }
                })),
        )
        // Where each block goes, "nowhere" included: one question per block. What is kept is still
        // two keys, so a block that is hidden keeps its place and comes back to it.
        .children(places.map(|places| arrange::block(places, view.clone(), id, palette, cx)))
        // The one switch that is not a block: the rule, which draws nothing and has no place. It
        // heads a frame of its own, with its rows under it — the border is what marks it as the
        // thing that is not a block.
        .children(inputs.map(|inputs| {
            let mut frame = crate::panels::popup_frame(id("detect-group"));
            for switch in SWITCHES.iter().filter(|switch| switch.block.is_none()) {
                let view = view.clone();
                frame = frame.child(
                    MoonCheckbox::new(id(switch.id))
                        .label(t!(switch.label).to_string())
                        .checked((switch.read)(&screen))
                        .size(MoonCheckboxSize::Compact)
                        .on_change(move |checked: &bool, _window, app| {
                            let checked = *checked;
                            view.update(app, |this, cx| this.set_switch(switch, checked, cx));
                        }),
                );
            }
            frame.child(detect::block(
                screen.detect(),
                cards,
                inputs,
                view,
                id,
                palette,
                cx,
            ))
        }))
        .into_any_element()
}

/// One frame's answer to the three questions the empty screen is drawn from.
///
/// Resolved ONCE in the render and handed down, rather than asked again inside: the size probe
/// repaints on the form this width produces, and a screen that re-derived the form from a second
/// reading could disagree with the probe that decided whether to repaint it.
pub(super) struct EmptyFrame {
    /// What is switched on.
    pub(super) screen: EmptyScreen,
    /// Where each block goes.
    pub(super) places: EmptyPlaces,
    /// The grid with its side widths, or the one-column form, for this width.
    pub(super) form: place::Form,
    /// Measured Main width, used for board presentation (full board or records).
    pub(super) width: gpui::Pixels,
}

/// The empty screen: whichever blocks are switched on, where this profile has put them, and the
/// ⚙ that decides both.
///
/// The blocks are handed to `crowd::place` as a flat list rather than laid out here: two of them
/// may be anchored to the same cell, where they stack, and a screen that placed the brand itself
/// and left the tables to place themselves could never stack one with the other.
///
/// Args:
///     stack: The stack, for the corner control and the saved anchors.
///     frame: What is on, where it goes and which form it takes, resolved once by the caller.
///     stats: The statistics view, when any of its tables is.
///     window: Host viewport used to bound the settings popup.
///     palette: Active MoonUI palette.
///     cx: Stack context.
///
/// Returns:
///     The whole screen, ready for the size probe the caller wraps it in.
pub(super) fn empty_screen(
    stack: &mut MainChartStack,
    frame: EmptyFrame,
    stats: Option<Entity<CrowdStatsView>>,
    window: &Window,
    palette: MoonPalette,
    cx: &mut Context<MainChartStack>,
) -> AnyElement {
    let EmptyFrame {
        screen,
        places,
        form,
        width,
    } = frame;
    let settings = stack.empty_settings(window, palette, cx);
    // Scoped by group like every other identity in this stack: two group windows are two empty
    // screens, and a shared id would give the narrow form's scroll one position for both.
    let frame_id = SharedString::from(format!("main-empty-frame-{}", stack.group));
    let mut blocks: Vec<(EmptyBlock, AnyElement)> = Vec::new();
    if screen.logo {
        blocks.push((
            EmptyBlock::Logo,
            design::logo_glow_sized(cx, design::EMPTY_STACK_LOGO_W).into_any_element(),
        ));
    }
    // A block of its own rather than part of the mark, because it is its own switch and its own
    // anchor: somebody may want the line without the logo, or the line somewhere other than the
    // middle. One muted line, naming the ONE gesture that actually opens a chart from here — there
    // is no double-click on a core row that does it, since the rail only RETARGETS a chart that
    // already exists (`sync_auto_workspace_chart` returns early on an empty Main).
    if screen.hint {
        blocks.push((
            EmptyBlock::Hint,
            div()
                .max_w(design::font_w_px(cx, HINT_WIDTH))
                .text_center()
                .text_size(design::t_body(cx))
                .text_color(rgb(palette.text_muted))
                .child(t!("chart.empty.hint").to_string())
                .into_any_element(),
        ));
    }
    // The statistics are added LAST so that, in a cell they share with the brand, a figure is drawn
    // under the mark rather than behind it.
    if let Some(view) = stats {
        blocks.extend(crate::crowd::boards(&view, width, cx));
    }
    div()
        .relative()
        .size_full()
        .bg(rgb(palette.chart_bg))
        .child(place::frame(frame_id, blocks, places, form, cx))
        .child(settings)
        .into_any_element()
}
