//! The row a ⧉ press opens: WHICH kinds of tab it should reach, and what it will overwrite.
//!
//! The press used to be the button itself — one click, and the values went to every tab in the
//! group plus the global default. It now names its targets, because the three kinds of tab
//! ([`ChartTabKind`]) are exactly what a reader wants to keep apart: a dense main chart and sparse
//! torn-off windows is a normal thing to want, and one button could not express it.
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

use super::apply_all::{ApplyAll, KindTargets};
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
    fn apply_row_counts(&self, values: &[StackSetting], cx: &App) -> [usize; 3];
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
        false => [0; 3],
    };
    let entity = cx.entity();
    let mut ticks = h_flex().gap_2().flex_wrap();
    for kind in ChartTabKind::ALL {
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
    let go_entity = entity.clone();
    let pressed = ApplyAll {
        values,
        x_ppm,
        targets,
        as_default,
    };
    let go = MoonButton::new(SharedString::from(format!("{id_prefix}-apply-go")))
        .label(match as_default {
            true => t!("chart.defaults.set").to_string(),
            false => t!("chart.defaults.apply").to_string(),
        })
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Soft)
        .disabled(!targets.any())
        .on_click(move |_, _w, app| {
            let apply = pressed.clone();
            go_entity.update(app, |this, cx| {
                this.apply_press_mut().open = false;
                if apply.targets.any() {
                    this.perform_apply(apply, cx);
                }
                cx.notify();
            });
        })
        .render();
    Some(
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    // Two different sentences on purpose: setting a default also CLEARS what hides
                    // it, and a reader about to overwrite the tuning of several windows should read
                    // that in the popup rather than discover it afterwards.
                    .child(match as_default {
                        true => t!("chart.defaults.set_hint").to_string(),
                        false => t!("chart.defaults.apply_hint").to_string(),
                    }),
            )
            .child(h_flex().gap_2().items_center().child(ticks).child(go))
            .into_any_element(),
    )
}
