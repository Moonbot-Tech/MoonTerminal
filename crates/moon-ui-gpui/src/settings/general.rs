//! General-tab editor for personal and machine settings in `settings.toml`.
//! Changes remain in `Backend.preview`. UI mode/font, separate control zones, and the Main-window
//! idle timeout are consumed live from that draft and roll back when Settings closes unsaved;
//! other settings take effect after saving and reconciling the relevant runtime state.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState,
    MoonMenuSize, MoonPalette, MoonSelect, MoonSlider, MoonSliderEvent, MoonSliderState,
    MoonToggle, StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::UiThemeMode;

/// One bold caption beside a MoonUI select, the General tab's shape for an enum setting.
///
/// The width is passed once and reaches both the trigger box and the menu: written out per row,
/// those two drift, and the menu ends up narrower or wider than the control that opened it.
///
/// Args:
///     label: Localization key for the caption.
///     state: Select state driving the dropdown.
///     width: Trigger and menu width in unscaled pixels.
///     cx: Context used to scale the menu width with the UI font.
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
        .gap(px(10.0))
        .items_center()
        .child(div().font_bold().child(t!(label).to_string()))
        .child(
            div().w(px(width)).child(
                MoonSelect::new(state)
                    .trigger_size(MoonButtonSize::Action)
                    .menu_width(design::font_w(cx, width))
                    .menu_size(MoonMenuSize::Compact),
            ),
        )
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
        let btn = |suffix: &'static str, label: &'static str, delta: i32| {
            MoonButton::new(SharedString::from(format!("{id}{suffix}")))
                .ghost()
                .size(MoonButtonSize::Micro)
                .width(28.0)
                .label(label)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| adjust(this, delta, cx)))
                .render()
        };
        h_flex()
            .gap(design::ui_px(cx, 4.0))
            .items_center()
            .child(btn("-large", "<<", -large))
            .child(btn("-small", "<", -small))
            .child(
                div()
                    .w(design::font_w_px(cx, 72.0))
                    .text_center()
                    .text_color(color)
                    .child(value_text),
            )
            .child(btn("+small", ">", small))
            .child(btn("+large", ">>", large))
    }

    /// Store `ui_font_delta` in the draft and reinstall the MoonUI theme for a live preview.
    /// Returns whether the value changed so callers can synchronize the paired control without
    /// emitting redundant notifications or redraws for a no-op.
    pub(super) fn set_ui_font_delta(&mut self, v: f32, cx: &mut Context<Self>) -> bool {
        let changed = self.backend.update(cx, |b, bcx| {
            let Some(p) = b.preview.as_mut() else {
                return false;
            };
            if p.ui_font_delta == v {
                return false;
            }
            p.ui_font_delta = v;
            crate::install_moon_theme_for_config(p, bcx);
            bcx.notify();
            true
        });
        if changed {
            cx.notify();
        }
        changed
    }

    /// Build the General-tab UI-font control: a slider with integer marks and an exact input.
    /// The explanatory hint below replaces a separate label. The slider and marks share the
    /// `track_w` column so each tick aligns with the slider thumb center.
    pub(super) fn font_delta_control(&self, cx: &Context<Self>) -> impl IntoElement {
        let track_w = design::ui_value(cx, 210.0);
        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                v_flex()
                    .w(px(track_w))
                    .gap(design::ui_px(cx, 2.0))
                    .child(
                        div().w(px(track_w)).child(
                            MoonSlider::new(&self.ui_font).height(design::ui_value(cx, 22.0)),
                        ),
                    )
                    .child(font_delta_marks(cx, track_w)),
            )
            .child(
                div().w(design::font_w_px(cx, 56.0)).child(
                    MoonInput::new("ui-font-delta")
                        .state(&self.ui_font_input)
                        .small(),
                ),
            )
    }

    /// Build the General tab for UI mode/font, locale, chart grouping, control zones,
    /// Main-window idle closing, and file-log retention settings.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let (ui_theme_mode, split, scz, idle_secs, logf, ret) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.ui_theme_mode,
                d.charts_split_by_core,
                d.separate_control_zones,
                d.main_idle_close_secs,
                d.log_to_file,
                d.log_retention_days,
            )
        };
        let hint = |s: &str| div().text_color(muted).child(s.to_string());

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
            // UI mode and font are personal settings in settings.toml; the portable chart theme
            // is edited on the Interface tab and stored in theme.toml.
            .child(
                MoonToggle::new("ui-theme-mode")
                    .checked(ui_theme_mode == UiThemeMode::Light)
                    // Label the currently active mode instead of always describing the light mode.
                    .label(
                        if ui_theme_mode == UiThemeMode::Light {
                            t!("iface.light_theme")
                        } else {
                            t!("iface.dark_theme")
                        }
                        .to_string(),
                    )
                    .on_change(cx.listener(|this, checked: &bool, _window, cx| {
                        let mode = if *checked {
                            UiThemeMode::Light
                        } else {
                            UiThemeMode::Dark
                        };
                        let changed = this.backend.update(cx, |b, bcx| {
                            let Some(p) = b.preview.as_mut() else {
                                return false;
                            };
                            if p.ui_theme_mode == mode {
                                return false;
                            }
                            p.ui_theme_mode = mode;
                            crate::install_moon_theme_for_config(p, bcx);
                            bcx.notify();
                            true
                        });
                        if changed {
                            cx.notify();
                        }
                    })),
            )
            .child(hint(&t!("iface.light_theme_hint")))
            .child(self.font_delta_control(cx))
            .child(hint(&t!("iface.font_delta_hint")))
            .child(super::separator(p, cx))
            // Interface locale selector.
            .child(labeled_select("general.language", &self.lang, 220.0, cx))
            .child(hint(&t!("general.language_hint")))
            .child(super::separator(p, cx))
            // Which rate converts quote money to USDT. The hint explains the two limitations of
            // the current-rate conversion at the point where the application-wide choice is made.
            .child(labeled_select(
                "general.valuation_mode",
                &self.valuation,
                260.0,
                cx,
            ))
            .child(hint(&t!("general.valuation_mode_hint")))
            .child(super::separator(p, cx))
            // Place each core in a separate chart tab.
            .child(
                self.draft_checkbox(cx, "split", split, |p, v| {
                    if p.charts_split_by_core != v {
                        p.charts_split_by_core = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.charts_split_by_core").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.charts_split_by_core_hint")))
            .child(super::separator(p, cx))
            // Restrict order and line controls to the order-book control zone.
            .child(
                self.draft_checkbox(cx, "separate-zones", scz, |p, v| {
                    if p.separate_control_zones != v {
                        p.separate_control_zones = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.separate_control_zones").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.separate_control_zones_hint")))
            .child(super::separator(p, cx))
            // Close Main charts after window inactivity; zero disables the timeout.
            .child(
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
                .label(t!("general.main_idle_close").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(
                h_flex()
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
            .child(hint(&t!("general.main_idle_close_hint")))
            .child(super::separator(p, cx))
            // Stack layout is now configured per tab from the chart-tabs layout popup.
            // File logging and retention period.
            .child(
                self.draft_checkbox(cx, "logf", logf, |p, v| {
                    if p.log_to_file != v {
                        p.log_to_file = v;
                        true
                    } else {
                        false
                    }
                })
                .label(t!("general.log_to_file").to_string())
                .size(MoonCheckboxSize::Normal),
            )
            .child(hint(&t!("general.log_to_file_hint")))
            // Retention controls are enabled only while file logging is enabled; otherwise the
            // buttons are disabled and the value and labels are muted.
            .child(
                h_flex()
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
            .child(hint(&t!("general.log_retention_hint")))
            // Launch and servers.enc passwords. Last on the tab: it is the only block that can lock
            // the user out of their own cores, so it should not be the first thing a hand lands on.
            .child(super::separator(p, cx))
            .child(self.security_section(cx))
    }
}

/// UI-font delta range in logical pixels, shared by the slider, input clamp, and tick marks.
const FONT_DELTA_MIN: i32 = -2;
const FONT_DELTA_MAX: i32 = 6;

/// Format a font delta as its canonical integer-step text, normalizing negative zero.
fn font_delta_text(v: f32) -> String {
    (v.round() as i32).to_string()
}

/// Parse font-delta input after trimming whitespace and treating a comma as a decimal point.
/// Rejects incomplete, nonnumeric, and non-finite values before rounding; valid values are
/// rounded to the integer step, clamped to the supported range, and normalized from `-0.0`.
fn parse_font_delta(s: &str) -> Option<f32> {
    let v: f32 = s.trim().replace(',', ".").parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let v = v
        .round()
        .clamp(FONT_DELTA_MIN as f32, FONT_DELTA_MAX as f32);
    Some(if v == 0.0 { 0.0 } else { v })
}

/// Build slider marks at every integer in the font range and label the even values.
/// Marks use `(m - MIN) / span` across `track_w` to align with the full-width slider. Edge
/// labels are anchored to the track ends so they do not clip or overlap the input field.
fn font_delta_marks(cx: &App, track_w: f32) -> impl IntoElement {
    let span = (FONT_DELTA_MAX - FONT_DELTA_MIN) as f32;
    let p = MoonPalette::active(cx);
    let tick = rgba_from(p.border, 1.0);
    let label = rgba_from(p.text_muted, 1.0);
    let tick_h = design::ui_value(cx, 4.0);
    let label_w = design::font_w(cx, 20.0);
    let mut row = div()
        .relative()
        .w(px(track_w))
        .h(design::fit_h_px(cx, 16.0, 11.0, 1.0));
    for m in FONT_DELTA_MIN..=FONT_DELTA_MAX {
        let x = (m - FONT_DELTA_MIN) as f32 / span * track_w;
        row = row.child(
            div()
                .absolute()
                .left(px((x - 0.5).max(0.0)))
                .top(px(0.0))
                .w(px(1.0))
                .h(px(tick_h))
                .bg(tick),
        );
        if m % 2 == 0 {
            let left = if m == FONT_DELTA_MIN {
                0.0
            } else if m == FONT_DELTA_MAX {
                track_w - label_w
            } else {
                x - label_w / 2.0
            };
            row = row.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(tick_h + design::ui_value(cx, 1.0)))
                    .w(px(label_w))
                    .text_center()
                    .text_size(design::t_caption(cx))
                    .text_color(label)
                    .child(m.to_string()),
            );
        }
    }
    row
}

/// Build the bidirectionally synchronized UI-font slider and numeric input.
/// The range is -2 through 6 logical pixels in integer steps. Both subscriptions use
/// `subscribe_in` because updating the paired control requires `&mut Window`; field updates
/// suppress emitted events so synchronization does not form a feedback loop.
pub(super) fn build_font(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> (Entity<MoonSliderState>, Entity<MoonInputState>) {
    let cur = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).ui_font_delta
    };
    let slider = cx.new(|_| {
        MoonSliderState::new()
            .min(FONT_DELTA_MIN as f32)
            .max(FONT_DELTA_MAX as f32)
            .step(1.0)
            .default_value(cur)
    });
    let input = cx.new(|cx| MoonInputState::new(window, cx).default_value(font_delta_text(cur)));

    // Slider changes update the draft and mirror the canonical value into the input. The closure
    // deliberately obtains the paired input through `this` to avoid a strong-reference cycle.
    cx.subscribe_in(
        &slider,
        window,
        move |this, _slider, ev: &MoonSliderEvent, window, cx| {
            let MoonSliderEvent::Change(v) = ev else {
                return;
            };
            // Quantization in the negative subrange can produce IEEE -0.0; normalize it.
            let v = v.end();
            let v = if v == 0.0 { 0.0 } else { v };
            if this.set_ui_font_delta(v, cx) {
                this.ui_font_input
                    .update(cx, |st, c| st.set_value(font_delta_text(v), window, c));
            }
        },
    )
    .detach();

    // Input changes update the draft and slider without rewriting text mid-entry. Blur or Enter
    // canonicalizes the text, falling back to the current value for invalid input. The closure
    // receives its emitter and obtains the slider through `this` to avoid a reference cycle.
    cx.subscribe_in(
        &input,
        window,
        move |this, field, ev: &MoonInputEvent, window, cx| match ev {
            MoonInputEvent::Change => {
                // End the field's immutable `cx` borrow before calling `set_ui_font_delta`.
                let parsed = parse_font_delta(&field.read(cx).value());
                if let Some(v) = parsed {
                    if this.set_ui_font_delta(v, cx) {
                        this.ui_font.update(cx, |st, c| st.set_value(v, window, c));
                    }
                }
            }
            MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. } => {
                let cur = {
                    let b = this.backend.read(cx);
                    b.preview.as_ref().unwrap_or(&b.config).ui_font_delta
                };
                field.update(cx, |st, c| st.set_value(font_delta_text(cur), window, c));
            }
            _ => {}
        },
    )
    .detach();

    (slider, input)
}

#[cfg(test)]
mod tests;
