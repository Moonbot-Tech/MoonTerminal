//! The row a ⧉ press opens: WHICH kinds of tab it should reach, and what it will overwrite.
//!
//! The press used to be the button itself — one click, and the values went to every tab in the
//! group plus the global default. It now names its targets, because the kinds of chart
//! ([`ChartTabKind`]) are exactly what a reader wants to keep apart: a dense main chart and sparse
//! torn-off windows is a normal thing to want, and one button could not express it. A kind with no
//! tabs — the trade-detail window — is offered only where the press stores a DEFAULT, since that is
//! the only thing it has.
//!
//! Rendered INLINE inside the settings popup rather than as a popover of its own. A nested overlay
//! would count as an outside click for the popup hosting it and close the thing the reader is
//! editing — see the dismissal note in `common::layout_popup_host`, which is why the popups render
//! their own controls inline too.

use gpui::*;
use moon_core::config::ChartTabKind;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::apply_all::{ApplyAll, ApplyMode, KindTargets};
use super::common::StackSetting;
use crate::design;

/// A press armed by ⧉ and not yet performed: the CHOICE only.
///
/// Deliberately no values. They are read from the popup at the moment the button is pressed, for
/// two reasons: the popup stays fully editable while the row is up, so a snapshot taken at ⧉ would
/// silently discard everything typed afterwards; and one of these is shared by all four popups on a
/// host, so a snapshot could be performed from a popup that did not take it.
#[derive(Default, Clone, Debug, PartialEq)]
pub(crate) struct ApplyPress {
    /// Whether the row is showing.
    pub open: bool,
    /// Kinds the press will reach. Starts as the source tab's own kind.
    pub targets: KindTargets,
}

impl ApplyPress {
    /// Arm a press, or close the row when it is already showing.
    ///
    /// Args:
    ///     source: The kind of the tab the press came from, ticked to begin with.
    pub(crate) fn arm(&mut self, source: ChartTabKind) {
        if self.open {
            self.open = false;
            return;
        }
        self.targets = KindTargets::only(source);
        self.open = true;
    }
}

/// A popup host that can arm and perform a ⧉ press.
pub(crate) trait ApplyRowHost: 'static + Sized {
    fn apply_press(&self) -> &ApplyPress;
    fn apply_press_mut(&mut self) -> &mut ApplyPress;
    /// How many STORED tabs of each kind hold an override this press would drop.
    fn apply_row_counts(
        &self,
        values: &[StackSetting],
        cx: &App,
    ) -> [usize; ChartTabKind::ALL.len()];
    /// Perform it: the strip walks its group, a detached window queues the walk for its strip.
    fn perform_apply(&mut self, apply: ApplyAll, cx: &mut Context<Self>);
}

/// Render the row, or nothing when no press is armed.
///
/// Args:
///     this: The popup's host.
///     id_prefix: Per-host element identity prefix.
///     p: Active palette.
///     cx: Host context.
pub(super) fn render_apply_row<T: ApplyRowHost>(
    this: &T,
    id_prefix: &'static str,
    values: Vec<StackSetting>,
    x_ppm: Option<f32>,
    p: MoonPalette,
    cx: &mut Context<T>,
) -> Option<AnyElement> {
    let press = this.apply_press();
    if !press.open {
        return None;
    }
    // Every value has a default of its own, or none does: the four popups each carry one kind of
    // setting. A press whose values have nowhere to be stored can only be written into tabs.
    let as_default = !values.is_empty() && values.iter().all(|v| v.global_slot().is_some());
    let targets = press.targets;
    // Only a default-setting press overwrites anything: the other kind writes its values into the
    // tabs of the ticked kinds, and every one of them takes it. A count there would name a number
    // the reader cannot act on.
    let counts = match as_default {
        true => this.apply_row_counts(&values, cx),
        false => [0; ChartTabKind::ALL.len()],
    };
    let entity = cx.entity();
    let mut ticks = h_flex().gap_2().flex_wrap();
    // A press that sets DEFAULTS can address every kind; one that writes values into TABS can only
    // address the kinds that have any. Two sets rather than a question asked per element, so the
    // row cannot offer a tick that reaches nothing.
    let offered: &[ChartTabKind] = match as_default {
        true => &ChartTabKind::ALL,
        false => &ChartTabKind::TAB_KINDS,
    };
    for kind in offered.iter().copied() {
        let index = KindTargets::index(kind);
        // The count is what the press OVERWRITES: a tab following the default already is not
        // changed by a press that moves it, and saying otherwise would overstate the damage.
        let count = counts[index];
        let label = match count {
            0 => t!(kind.locale_key()).to_string(),
            n => format!("{} ({n})", t!(kind.locale_key())),
        };
        let toggle_entity = entity.clone();
        ticks = ticks.child(
            MoonCheckbox::new(SharedString::from(format!("{id_prefix}-apply-{index}")))
                .label(label)
                .checked(targets.has(kind))
                .size(MoonCheckboxSize::Compact)
                .on_change(move |checked: &bool, _w, app| {
                    let checked = *checked;
                    toggle_entity.update(app, |this, cx| {
                        this.apply_press_mut().targets.set(kind, checked);
                        cx.notify();
                    });
                }),
        );
    }
    // The slots the reset addresses, taken before the values are moved into the press: the reset
    // needs nothing else from them, and the press it travels in is built from these alone.
    let reset_slots: Vec<_> = values.iter().filter_map(|v| v.global_slot()).collect();
    let pressed = ApplyAll {
        values,
        x_ppm,
        targets,
        mode: match as_default {
            true => ApplyMode::SetDefault,
            false => ApplyMode::Tabs,
        },
    };
    // One builder for both buttons: they differ in their wording, their weight and the press they
    // carry, and in nothing else — the click does the same three things either way.
    //
    // NO tooltip on either, deliberately: this row renders INSIDE a popover, and MoonUI defers a
    // tooltip at priority 2 against the popover's 30 000, so a hint here would paint under the
    // surface it belongs to and never be seen (docs-internal/FORK_BUGS.md). What the buttons do is
    // stated in the sentence above them instead.
    let press_button =
        |suffix: &str, label: String, variant: MoonButtonVariant, apply: ApplyAll| {
            let entity = entity.clone();
            MoonButton::new(SharedString::from(format!("{id_prefix}-apply-{suffix}")))
                .label(label)
                .size(MoonButtonSize::Micro)
                .variant(variant)
                // A press with no ticks reaches nothing, so the button says so instead of
                // accepting the click. The click path is guarded too — `ChartTabs::apply_all`
                // returns on an empty target set, which is also where a detached window's queued
                // press lands — so this is the wording, not the safety.
                .disabled(!targets.any())
                .on_click(move |_, _w, app| {
                    let apply = apply.clone();
                    entity.update(app, |this, cx| {
                        this.apply_press_mut().open = false;
                        this.perform_apply(apply, cx);
                        cx.notify();
                    });
                })
                .render()
        };
    let go = press_button(
        "go",
        match as_default {
            true => t!("chart.defaults.set").to_string(),
            false => t!("chart.defaults.apply").to_string(),
        },
        MoonButtonVariant::Soft,
        pressed,
    );
    // Offered only where the values HAVE a default: the ⚙ popup's settings are per-tab, and a
    // button to reset a default they do not have would name nothing.
    let reset = as_default.then(|| {
        press_button(
            "reset",
            t!("chart.defaults.reset").to_string(),
            MoonButtonVariant::Ghost,
            ApplyAll {
                values: Vec::new(),
                x_ppm: None,
                targets,
                mode: ApplyMode::ResetDefault(reset_slots),
            },
        )
    });
    Some(
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    // Two different sentences on purpose: setting a default also CLEARS what hides
                    // it, and a reader about to overwrite the tuning of several windows should read
                    // that in the popup rather than discover it afterwards. It is also the ONLY
                    // explanation these buttons get — a tooltip inside a popover is invisible in
                    // this fork — so it says what each of them does.
                    .child(match as_default {
                        true => t!("chart.defaults.set_hint").to_string(),
                        false => t!("chart.defaults.apply_hint").to_string(),
                    }),
            )
            .child(
                // WRAPS at both levels, because NOTHING in this row can shrink: a `MoonPopover`
                // is a fixed width, a `MoonButton` is `flex_shrink_0` with a label that does not
                // truncate, and so is a `MoonCheckbox`. A row of four ticks and two buttons
                // therefore has a hard minimum, and in a language with long words it used to be
                // past the popup's edge. The outer wrap puts the buttons on a line of their own,
                // the inner one stacks the pair when even that line is too narrow.
                //
                // What this still does NOT bound is one button whose label alone is wider than the
                // popup: nothing can break that, so the labels are kept short instead.
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(ticks)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(go)
                            .children(reset),
                    ),
            )
            .into_any_element(),
    )
}
