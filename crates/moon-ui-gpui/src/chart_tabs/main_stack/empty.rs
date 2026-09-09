//! The empty Main screen: what it shows, and the ⚙ that decides.
//!
//! Everything here is about a stack with no chart in it. It is its own file because the empty
//! screen has grown a state of its own — up to five layers, a live view over the crowd's
//! statistics among them — and none of that belongs in the middle of the stack's layout code.
//!
//! **One screen, five switches, any combination.** The logo, the hint under it and the three crowd
//! tables are independent layers of the same screen: the brand sits in the middle, the tables are
//! anchored to the edges, and the statistics are drawn LAST so a figure is never behind the mark.
//! Nothing here is exclusive — the point of separate switches is that somebody who wants only the
//! trader board gets only the trader board.
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

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, rgb,
};
use moon_core::config::layout::WindowLayout;
use moon_ui::{MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement};
use rust_i18n::t;

use super::MainChartStack;
use crate::crowd::{CrowdParts, CrowdStatsView};
use crate::design;
use crate::panels::{popup_close_button, popup_gear_trigger, popup_title};

/// How far the button sits from the top-right corner, in design units.
///
/// The same corner whatever is switched on, because it is the control that switches it: one that
/// moved with the setting would be a control nobody could find twice. The minute table starts
/// below it — see `crowd::table`'s own top inset — so the two share the corner without overlapping.
const GEAR_INSET: f32 = 10.0;

/// Popup CONTENT width in design units, before the group frame's own inset. Sized for the longest
/// localized label rather than for the control.
const CONTENT_WIDTH: f32 = 300.0;
/// Gap between the popup's title row and the switches under it, in design units.
const POPUP_GAP: f32 = 8.0;
/// Gap between the logo and the line under it, in design units.
const LOGO_GAP: f32 = 10.0;
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
struct Switch {
    /// Element-identity suffix, unique within the popup.
    id: &'static str,
    /// Locale key of the visible label.
    label: &'static str,
    /// What it is worth on a profile that has never opened this popup.
    default: bool,
    /// Read the current value out of the arrangement.
    read: fn(&EmptyScreen) -> bool,
    /// Apply an edited value.
    set: fn(&mut EmptyScreen, bool),
    /// Read this switch's saved value, or `None` when it was never chosen.
    saved: fn(&WindowLayout) -> Option<bool>,
    /// Save an edited value under this switch's own `layout.toml` key.
    ///
    /// Only the EDITED key is ever written. Stamping the others would turn "never chosen" into an
    /// explicit value for switches nobody touched.
    store: fn(&mut WindowLayout, bool),
}

/// Every switch, in the order the popup shows them: what the screen already was, then what can be
/// added to it, then the one that is not a layer at all.
const SWITCHES: [Switch; 6] = [
    Switch {
        id: "logo",
        label: "crowd.settings.logo",
        default: true,
        read: |screen| screen.logo,
        set: |screen, value| screen.logo = value,
        saved: |layout| layout.main_empty_logo,
        store: |layout, value| layout.main_empty_logo = Some(value),
    },
    Switch {
        id: "hint",
        label: "crowd.settings.hint",
        default: true,
        read: |screen| screen.hint,
        set: |screen, value| screen.hint = value,
        saved: |layout| layout.main_empty_hint,
        store: |layout, value| layout.main_empty_hint = Some(value),
    },
    Switch {
        id: "minute",
        label: "crowd.settings.minute",
        default: false,
        read: |screen| screen.minute,
        set: |screen, value| screen.minute = value,
        saved: |layout| layout.main_empty_minute,
        store: |layout, value| layout.main_empty_minute = Some(value),
    },
    Switch {
        id: "traders",
        label: "crowd.settings.traders",
        default: false,
        read: |screen| screen.traders,
        set: |screen, value| screen.traders = value,
        saved: |layout| layout.main_empty_traders,
        store: |layout, value| layout.main_empty_traders = Some(value),
    },
    Switch {
        id: "coins",
        label: "crowd.settings.coins",
        default: false,
        read: |screen| screen.coins,
        set: |screen, value| screen.coins = value,
        saved: |layout| layout.main_empty_coins,
        store: |layout, value| layout.main_empty_coins = Some(value),
    },
    Switch {
        id: "detect",
        label: "crowd.settings.detect",
        default: DETECT_DEFAULT,
        read: |screen| screen.detect,
        set: |screen, value| screen.detect = value,
        saved: |layout| layout.main_empty_detect,
        store: |layout, value| layout.main_empty_detect = Some(value),
    },
];

pub(super) mod detect;

#[cfg(test)]
mod tests;

use detect::DETECT_DEFAULT;
pub(crate) use detect::{CrowdCards, DetectInputs, crowd_cards, crowd_rule_for_run};

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
                self.crowd = Some((
                    parts,
                    cx.new(|cx| CrowdStatsView::new(parts, backend, group, cx)),
                ));
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
            store(&mut backend.layout, on);
            backend.layout_dirty = true;
        });
        self.publish_crowd_rule(cx);
        // The view itself is built or dropped by `sync_crowd_stats` on the repaint this asks for,
        // so one place decides whether it exists rather than two.
        cx.notify();
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
    ///     palette: Active MoonUI palette.
    ///     cx: Stack context used to read the switches and wire the toggles.
    ///
    /// Returns:
    ///     The corner control, absolutely placed.
    pub(super) fn empty_settings(
        &self,
        palette: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .content_width(
                f32::from(design::ui_px(cx, CONTENT_WIDTH))
                    + crate::panels::popup_group_inset_px(cx),
            )
            .close_on_content_click(false)
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
            popover = popover.content(settings_content(
                self.empty_arrangement(cx),
                crowd_cards(&self.backend.read(cx).layout),
                self.empty_detect.as_ref(),
                view,
                &id,
                palette,
                cx,
            ));
        }
        div()
            .absolute()
            .right(inset)
            .top(inset)
            .child(popover)
            .into_any_element()
    }
}

/// The popup body: a title with its ✕, and one switch per layer.
///
/// Args:
///     screen: What the checkboxes show.
///     cards: How the rule's cards are set to behave.
///     inputs: The rule's fields, once the popup has been opened at least once.
///     view: Stack entity receiving the edits.
///     id: Group-scoped element identities.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
fn settings_content(
    screen: EmptyScreen,
    cards: CrowdCards,
    inputs: Option<&DetectInputs>,
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
        .children(SWITCHES.iter().map(|switch| {
            let view = view.clone();
            MoonCheckbox::new(id(switch.id))
                .label(t!(switch.label).to_string())
                .checked((switch.read)(&screen))
                .size(MoonCheckboxSize::Compact)
                .on_change(move |checked: &bool, _window, app| {
                    let checked = *checked;
                    view.update(app, |this, cx| this.set_switch(switch, checked, cx));
                })
                .into_any_element()
        }))
        // The rule's own controls, under the switch that decides whether they mean anything.
        .children(
            inputs
                .map(|inputs| detect::block(screen.detect(), cards, inputs, view, id, palette, cx)),
        )
        .into_any_element()
}

/// The empty screen: whichever layers are switched on, and the ⚙ that chooses them.
///
/// Args:
///     stack: The stack, for the corner control.
///     screen: What is switched on.
///     stats: The statistics view, when any of its tables is.
///     palette: Active MoonUI palette.
///     cx: Stack context.
///
/// Returns:
///     The whole screen, ready for the size probe the caller wraps it in.
pub(super) fn empty_screen(
    stack: &mut MainChartStack,
    screen: EmptyScreen,
    stats: Option<Entity<CrowdStatsView>>,
    palette: MoonPalette,
    cx: &mut Context<MainChartStack>,
) -> AnyElement {
    let settings = stack.empty_settings(palette, cx);
    div()
        .relative()
        .size_full()
        .bg(rgb(palette.chart_bg))
        // The brand and its line, centred in a layer of their own rather than in the screen's flow:
        // the tables are anchored to the edges, and laying them out as siblings of a centred column
        // would give them the height of that column instead of the height of the panel.
        .when(screen.logo || screen.hint, |body| {
            body.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(design::ui_px(cx, LOGO_GAP))
                    .when(screen.logo, |middle| {
                        middle.child(design::logo_glow_sized(cx, design::EMPTY_STACK_LOGO_W))
                    })
                    // A logo alone says the stack is empty but not what to do about it. One muted
                    // line, naming the ONE gesture that actually opens a chart from here: there is
                    // no double-click on a core row that does it — the rail only RETARGETS a chart
                    // that already exists (`sync_auto_workspace_chart` returns early on an empty
                    // Main).
                    .when(screen.hint, |middle| {
                        middle.child(
                            div()
                                .max_w(design::font_w_px(cx, HINT_WIDTH))
                                .text_center()
                                .text_size(design::t_body(cx))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("chart.empty.hint").to_string()),
                        )
                    }),
            )
        })
        // Last, so a figure is never drawn behind the mark it shares the screen with.
        .children(stats.map(|view| div().absolute().inset_0().child(view)))
        .child(settings)
        .into_any_element()
}
