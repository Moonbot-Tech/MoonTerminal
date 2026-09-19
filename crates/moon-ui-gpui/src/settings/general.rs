//! General-tab editor for personal and machine settings in `settings.toml`.
//! Changes remain in `Backend.preview`. UI theme, zoom, control zones and the Main-window
//! idle timeout are consumed live from that draft and roll back when Settings closes unsaved;
//! other settings take effect after saving and reconciling the relevant runtime state.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonPalette, MoonSelect, MoonSliderState,
    MoonTooltipView, StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};

/// Zoom endpoints shared by the slider state and its displayed captions.
///
/// The range the slider offers, not a bound on the setting: a hand-edited value outside it stays
/// valid (`repair_ui_scale` touches only values that cannot mean anything).
const UI_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.50..=2.00;

/// One bold caption beside a MoonUI select, the General tab's shape for an enum setting.
///
/// The width is passed once and reaches both the trigger box and the menu: written out per row,
/// those two drift, and the menu ends up narrower or wider than the control that opened it.
///
/// Args:
///     label: Localization key for the caption.
///     state: Select state driving the dropdown.
///     width: Trigger and menu width in design units.
///     cx: Context used to scale the trigger geometry and menu text width.
///
/// Returns:
///     The assembled row.
fn labeled_select<T: Clone + PartialEq + 'static>(
    label: &'static str,
    state: &Entity<moon_ui::MoonSelectState<T>>,
    width: f32,
    cx: &Context<SettingsView>,
) -> impl IntoElement {
    h_flex()
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(div().font_bold().child(t!(label).to_string()))
        .child(
            div().w(design::ui_px(cx, width)).child(
                MoonSelect::new(state)
                    .trigger_size(MoonButtonSize::density(cx))
                    .menu_width(design::font_w(cx, width)),
            ),
        )
}

/// Render a muted settings hint, shortening multi-sentence text to its first sentence.
///
/// A shortened hint retains its sentence-ending punctuation, adds an ellipsis, and exposes the
/// complete localized text in the standard wide settings tooltip. Text without a sentence
/// boundary is rendered unchanged and does not gain a tooltip.
///
/// Args:
///     key: Stable localization key used as the tooltip host ID.
///     text: Complete localized hint text.
///     muted: Muted text colour from the active MoonUI palette.
///
/// Returns:
///     The hint row, with a tooltip only when the visible text was shortened.
fn settings_hint(key: &'static str, text: &str, muted: Hsla) -> AnyElement {
    let (shown, full) = hint_parts(text);
    let Some(full) = full else {
        return div().text_color(muted).child(shown).into_any_element();
    };

    div()
        .id(key)
        .text_color(muted)
        .child(shown)
        .tooltip(hint_tooltip(full))
        .into_any_element()
}

/// Attach a settings hint to its checkbox as the checkbox's description.
///
/// The hint is shortened exactly as [`settings_hint`] shortens it. A shortened hint's full text
/// moves to the same wide tooltip, hosted on the whole control because the description is part of
/// the checkbox rather than an element of its own.
///
/// Args:
///     checkbox: The labelled checkbox the hint explains.
///     key: Stable localization key used as the tooltip host ID.
///     text: Complete localized hint text.
///
/// Returns:
///     The checkbox, wrapped in a tooltip host only when the visible text was shortened.
fn checkbox_with_hint(checkbox: MoonCheckbox, key: &'static str, text: &str) -> AnyElement {
    let (shown, full) = hint_parts(text);
    let checkbox = checkbox.description(shown);
    let Some(full) = full else {
        return checkbox.into_any_element();
    };

    div()
        .id(key)
        .child(checkbox)
        .tooltip(hint_tooltip(full))
        .into_any_element()
}

/// Split a hint into its visible text and, when that is shortened, the complete text.
///
/// Multi-sentence text is cut after its first sentence, keeping the sentence-ending punctuation and
/// adding an ellipsis. Text without a sentence boundary is returned unchanged with no full text.
fn hint_parts(text: &str) -> (String, Option<String>) {
    let sentence_end = [". ", "! ", "? "]
        .into_iter()
        .filter_map(|boundary| text.find(boundary).map(|index| index + 1))
        .min();
    match sentence_end {
        Some(end) => (format!("{} …", &text[..end]), Some(text.to_string())),
        None => (text.to_string(), None),
    }
}

/// Build the standard wide settings tooltip showing a hint's complete text.
fn hint_tooltip(full: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_window, cx| {
        cx.new(|_| MoonTooltipView::new(full.clone()).max_width(420.0))
            .into()
    }
}

impl SettingsView {
    /// Adjust the draft log-retention period, clamped to `0..=365` days.
    fn adjust_ret(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                let v = (p.log_retention_days as i32 + delta).clamp(0, 365) as u32;
                if p.log_retention_days != v {
                    p.log_retention_days = v;
                    bcx.notify();
                    changed = true;
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Fallback used when idle closing is re-enabled without a valid `idle_last_secs` value.
    /// A remembered valid timeout is restored before this 120-second default is considered.
    const IDLE_DEFAULT_SECS: u32 = 120;

    /// Adjust the draft Main-window idle-close timeout, clamped to `5..=3600` seconds.
    /// The adjustment is active only while idle closing is enabled (the value is nonzero).
    fn adjust_idle(&mut self, delta: i32, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                if p.main_idle_close_secs > 0 {
                    let v = (p.main_idle_close_secs as i32 + delta).clamp(5, 3600) as u32;
                    if p.main_idle_close_secs != v {
                        p.main_idle_close_secs = v;
                        bcx.notify();
                        changed = true;
                    }
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Keep equal-width arrows large enough for the body text at any zoom.
    /// Build a `<<  <  value  >  >>` stepper row with small and large adjustments.
    /// Shared by second/day counters and the Storage version limit; `adjust` owns clamping.
    pub(super) fn stepper_controls(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        enabled: bool,
        value_text: String,
        small: i32,
        large: i32,
        adjust: fn(&mut Self, i32, &mut Context<Self>),
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let color = if enabled {
            rgba_from(p.text, 1.0)
        } else {
            rgba_from(p.text_muted, 1.0)
        };
        // All four controls reserve the widest double-arrow label at the active tier.
        let font = design::BODY_TEXT;
        let label_w = ["<<", ">>"]
            .into_iter()
            .map(|label| design::ui_text_width_zoomed(cx, label, font, 400.0, false))
            .fold(0.0_f32, f32::max);
        let button_w = label_w
            + 2.0 * design::ui_value(cx, design::CONTROL_TIER.control_metrics().pad_x)
            + 2.0; // MoonUI draws a one-pixel border on each side, independent of zoom.
        let btn = |suffix: &'static str, label: &'static str, delta: i32| {
            MoonButton::new(SharedString::from(format!("{id}{suffix}")))
                .ghost()
                .width(button_w)
                .label(label)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| adjust(this, delta, cx)))
                .render()
        };
        h_flex()
            .flex_none()
            .gap(design::ui_px(cx, 4.0))
            .items_center()
            .child(btn("-large", "<<", -large))
            .child(btn("-small", "<", -small))
            .child(
                div()
                    .w(design::font_w_px(cx, 72.0))
                    .font_family(design::mono())
                    .text_center()
                    .text_color(color)
                    .child(value_text),
            )
            .child(btn("+small", ">", small))
            .child(btn("+large", ">>", large))
    }

    /// Build the General tab for UI theme and zoom, locale, chart grouping, control zones,
    /// Main-window idle closing, and file-log retention settings.
    ///
    /// Args:
    ///     cx: Settings context that supplies the active draft and palette.
    ///
    /// Returns:
    ///     The assembled General-tab content.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let (split, scz, idle_secs, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.charts_split_by_core,
                d.separate_control_zones,
                d.main_idle_close_secs,
                d.log_to_file,
                d.log_retention_days,
            )
        };
        // Remember the last valid enabled timeout and restore it when the checkbox is re-enabled.
        // The adjustment clamp keeps it at least 5; fall back to the default defensively.
        if idle_secs >= 5 {
            self.idle_last_secs.set(idle_secs);
        }
        let idle_restore = {
            let last = self.idle_last_secs.get();
            if last >= 5 {
                last
            } else {
                Self::IDLE_DEFAULT_SECS
            }
        };

        v_flex()
            .w_full()
            .gap_1()
            // UI theme and zoom are personal settings in settings.toml. The chart theme
            // is edited on the Interface tab and stored in theme.toml. The selector's own
            // behaviour -- live preview and rebuilding the per-mode editors -- lives with its
            // state in `settings/mod.rs`, beside every other dropdown on this tab.
            .child(labeled_select(
                "iface.theme_mode",
                &self.theme_mode,
                220.0,
                cx,
            ))
            .child(settings_hint(
                "iface.light_theme_hint",
                &t!("iface.light_theme_hint"),
                muted,
            ))
            .child(super::slider_row(
                &t!("iface.ui_zoom"),
                &self.ui_zoom,
                UI_ZOOM_RANGE,
                zoom_text,
                cx,
            ))
            .child(settings_hint(
                "iface.ui_zoom_hint",
                &t!("iface.ui_zoom_hint"),
                muted,
            ))
            .child(super::separator(p, cx))
            // Interface locale selector.
            .child(labeled_select("general.language", &self.lang, 220.0, cx))
            .child(settings_hint(
                "general.language_hint",
                &t!("general.language_hint"),
                muted,
            ))
            .child(super::separator(p, cx))
            // Which rate converts quote money to USDT. The hint explains the two limitations of
            // the current-rate conversion at the point where the application-wide choice is made.
            .child(labeled_select(
                "general.valuation_mode",
                &self.valuation,
                260.0,
                cx,
            ))
            .child(settings_hint(
                "general.valuation_mode_hint",
                &t!("general.valuation_mode_hint"),
                muted,
            ))
            .child(super::separator(p, cx))
            // Place each core in a separate chart tab.
            .child(checkbox_with_hint(
                self.draft_checkbox(cx, "split", split, |p, v| {
                    if p.charts_split_by_core != v {
                        p.charts_split_by_core = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.charts_split_by_core").to_string()),
                "general.charts_split_by_core_hint",
                &t!("general.charts_split_by_core_hint"),
            ))
            .child(super::separator(p, cx))
            // Restrict order and line controls to the order-book control zone.
            .child(checkbox_with_hint(
                self.draft_checkbox(cx, "separate-zones", scz, |p, v| {
                    if p.separate_control_zones != v {
                        p.separate_control_zones = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.separate_control_zones").to_string()),
                "general.separate_control_zones_hint",
                &t!("general.separate_control_zones_hint"),
            ))
            .child(super::separator(p, cx))
            // Close Main charts after window inactivity; zero disables the timeout. The hint defines
            // "idle" for the checkbox, so it is the checkbox's description, above the stepper.
            .child(checkbox_with_hint(
                self.draft_checkbox(cx, "idle-close", idle_secs > 0, move |p, v| {
                    // Enabling restores the last remembered value; disabling stores zero.
                    let want = if v { idle_restore } else { 0 };
                    // Preserve an already enabled timeout instead of resetting it.
                    let target = if v && p.main_idle_close_secs > 0 {
                        p.main_idle_close_secs
                    } else {
                        want
                    };
                    if p.main_idle_close_secs != target {
                        p.main_idle_close_secs = target;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.main_idle_close").to_string()),
                "general.main_idle_close_hint",
                &t!("general.main_idle_close_hint"),
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(if idle_secs > 0 {
                                rgba_from(p.text, 1.0)
                            } else {
                                muted
                            })
                            .child(t!("general.main_idle_close_secs").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "idle",
                        idle_secs > 0,
                        format!("{idle_secs} {}", t!("general.seconds")),
                        10,
                        100,
                        Self::adjust_idle,
                    )),
            )
            .child(super::separator(p, cx))
            // Stack layout is now configured per tab from the chart-tabs layout popup.
            // File logging and retention period.
            .child(checkbox_with_hint(
                self.draft_checkbox(cx, "logf", logf, |p, v| {
                    if p.log_to_file != v {
                        p.log_to_file = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.log_to_file").to_string()),
                "general.log_to_file_hint",
                &t!("general.log_to_file_hint"),
            ))
            // Retention controls are enabled only while file logging is enabled; otherwise the
            // buttons are disabled and the value and labels are muted.
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(if logf { rgba_from(p.text, 1.0) } else { muted })
                            .child(t!("general.log_retention").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "ret",
                        logf,
                        format!("{ret} {}", t!("general.days")),
                        1,
                        10,
                        Self::adjust_ret,
                    )),
            )
            .child(settings_hint(
                "general.log_retention_hint",
                &t!("general.log_retention_hint"),
                muted,
            ))
            // Launch and servers.enc passwords. Last on the tab: it is the only block that can lock
            // the user out of their own cores, so it should not be the first thing a hand lands on.
            .child(super::separator(p, cx))
            .child(self.security_section(cx))
    }
}

/// Format a UI scale as a whole percentage, hiding floating-point step noise.
fn zoom_text(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

/// Build the zoom slider with a 5% step and a draft theme preview applied only on release.
///
/// During drag, the slider state supplies the live percentage caption without rescaling the app.
/// Initializing the thumb does not rewrite an existing hand-edited scale outside the UI range.
pub(super) fn build_zoom(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).ui_scale
    };
    super::common::draft_slider_on(
        cx,
        *UI_ZOOM_RANGE.start(),
        *UI_ZOOM_RANGE.end(),
        0.05,
        cur,
        super::common::DraftSliderApplyOn::Release,
        |p, value, bcx| {
            if p.ui_scale == value {
                return false;
            }
            p.ui_scale = value;
            crate::install_moon_theme_for_config(p, bcx);
            true
        },
    )
}

#[cfg(test)]
mod tests;
