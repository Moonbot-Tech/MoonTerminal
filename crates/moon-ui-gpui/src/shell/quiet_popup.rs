//! Content of the header's quiet-mode settings popover.
//!
//! Rendered as a [`Shell`] method because the three text fields are persistent `MoonInputState`
//! entities owned by Shell: `MoonPopover` takes its content eagerly and a free function receives
//! only `&App`, so a field built here would lose its cursor and its text on every repaint. Shell
//! seeds them when the popup opens and commits them on Enter/Blur and again when the popup closes
//! — clicking a checkbox inside the same popup does NOT blur a field, so the commit on close is
//! what keeps a typed-then-dismissed value. See `shell::quiet` for why Change is not one of them.
//!
//! The checkbox rows write straight through the backend, like the Core Status alert popup.

use gpui::*;
use moon_ui::{MoonCheckbox, MoonCheckboxSize, MoonInput, MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::config::quiet::QuietCfg;

use super::Shell;
use crate::design;
use crate::panels::{popup_close_button, popup_group, popup_title};

/// Popup content width in logical pixels: two time fields side by side, and one axis label per row
/// without wrapping in any of the three languages.
pub(crate) const CONTENT_W: f32 = 268.0;

/// Width of one `HH:MM` field.
const TIME_FIELD_W: f32 = 62.0;

/// The core-status axes offered as bypasses, as `(id, label key, getter, setter)`.
///
/// The labels are the Core Status popup's own axis names, reused rather than duplicated: two
/// spellings of "core dropout" in one product is how they drift apart.
type AxisAccess = (
    &'static str,
    &'static str,
    fn(&QuietCfg) -> bool,
    fn(&mut QuietCfg, bool),
);
const AXES: [AxisAccess; 7] = [
    (
        "conn",
        "core_status.warn_cfg.conn",
        |c| c.warn_bypass.conn,
        |c, v| c.warn_bypass.conn = v,
    ),
    (
        "cpu",
        "core_status.warn_cfg.cpu",
        |c| c.warn_bypass.cpu,
        |c, v| c.warn_bypass.cpu = v,
    ),
    (
        "mem",
        "core_status.warn_cfg.mem",
        |c| c.warn_bypass.mem,
        |c, v| c.warn_bypass.mem = v,
    ),
    (
        "ping",
        "core_status.warn_cfg.ping",
        |c| c.warn_bypass.ping,
        |c, v| c.warn_bypass.ping = v,
    ),
    (
        "exch",
        "core_status.warn_cfg.exch",
        |c| c.warn_bypass.exch,
        |c, v| c.warn_bypass.exch = v,
    ),
    (
        "api",
        "core_status.warn_cfg.api",
        |c| c.warn_bypass.api,
        |c, v| c.warn_bypass.api = v,
    ),
    (
        "api_quota",
        "core_status.warn_cfg.api_quota",
        |c| c.warn_bypass.api_quota,
        |c, v| c.warn_bypass.api_quota = v,
    ),
];

impl Shell {
    /// Build the quiet-mode settings content for the header's ⚙ popover.
    ///
    /// Args:
    ///     p: Active palette.
    ///     cx: Shell context used to read the backend and scale metrics.
    ///
    /// Returns:
    ///     The complete popup body.
    pub(crate) fn quiet_settings_content(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let cfg = self.backend.read(cx).quiet_cfg();
        v_flex()
            .id("quiet-settings-popup")
            // `w_full`, NOT a width computed from `CONTENT_W`: the popover has already set its box
            // to `content_width_font(CONTENT_W)`, which it resolves through `font_width` (a pure
            // multiply by the mono-body scale). Restating it here with `font_value` — the ADDITIVE
            // `font()` used for text SIZES — produced a content block ~47 px narrower than the
            // frame around it at the shipped +3 delta. One box, one width, set by its owner.
            .w_full()
            .gap(design::ui_px(cx, 6.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 4.0))
                    .child(popup_title(t!("quiet.title").to_string(), p, cx))
                    .child(popup_close_button(
                        "header-quiet-close",
                        cx.listener(|this, _ev, window, cx| {
                            this.set_quiet_settings_open(false, window, cx)
                        }),
                    )),
            )
            .child(self.quiet_schedule_group(&cfg, cx))
            .child(self.quiet_bypass_group(&cfg, p, cx))
            .into_any_element()
    }

    /// The schedule group: the on/off switch, which clock it runs on, and the two time fields.
    fn quiet_schedule_group(&self, cfg: &QuietCfg, cx: &Context<Self>) -> impl IntoElement {
        // The time the window is actually measured against, so the two fields below are readable
        // even when the header clock shows another city.
        let now = self.backend.read(cx).quiet_clock_hhmm();
        popup_group(
            "header-quiet-schedule",
            t!("quiet.schedule_group").to_string(),
        )
        .child(self.quiet_checkbox(
            "quiet-schedule",
            t!("quiet.schedule").to_string(),
            cfg.schedule_on,
            |c, v| c.schedule_on = v,
        ))
        // Beside the window it applies to, not in the bypass group: this says WHICH 23:00 the two
        // fields below mean, and reading it anywhere else would leave that question open.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(design::ui_px(cx, 6.0))
                .child(self.quiet_checkbox(
                    "quiet-local-time",
                    t!("quiet.local_time").to_string(),
                    cfg.use_local_time,
                    |c, v| c.use_local_time = v,
                ))
                .child(quiet_field_label(now, cx)),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(quiet_field_label(t!("quiet.from").to_string(), cx))
                .child(
                    div().w(design::font_w_px(cx, TIME_FIELD_W)).child(
                        MoonInput::new("quiet-from")
                            .state(&self.quiet_from_input)
                            .small(),
                    ),
                )
                .child(quiet_field_label(t!("quiet.to").to_string(), cx))
                .child(
                    div().w(design::font_w_px(cx, TIME_FIELD_W)).child(
                        MoonInput::new("quiet-to")
                            .state(&self.quiet_to_input)
                            .small(),
                    ),
                ),
        )
    }

    /// The bypass group: what still makes a sound while the terminal sleeps.
    fn quiet_bypass_group(
        &self,
        cfg: &QuietCfg,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mut group = popup_group("header-quiet-bypass", t!("quiet.bypass_group").to_string())
            .child(self.quiet_checkbox(
                "quiet-figure-alerts",
                t!("quiet.figure_alerts").to_string(),
                cfg.figure_alerts_bypass,
                |c, v| c.figure_alerts_bypass = v,
            ));
        for (id, label_key, get, set) in AXES {
            group = group.child(self.quiet_checkbox(id, t!(label_key).to_string(), get(cfg), set));
        }
        group.child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 3.0))
                .child(quiet_field_label(t!("quiet.charts").to_string(), cx))
                .child(
                    MoonInput::new("quiet-charts")
                        .state(&self.quiet_charts_input)
                        .placeholder(t!("quiet.charts_hint").to_string())
                        .small(),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("quiet.charts_tip").to_string()),
                ),
        )
    }

    /// One bypass/schedule checkbox writing a single field through the backend.
    fn quiet_checkbox(
        &self,
        id: &'static str,
        label: String,
        checked: bool,
        set: fn(&mut QuietCfg, bool),
    ) -> impl IntoElement {
        let backend = self.backend.clone();
        MoonCheckbox::new(SharedString::from(format!("header-{id}")))
            .label(label)
            .checked(checked)
            .size(MoonCheckboxSize::Compact)
            .mono(true)
            .on_change(move |value, _window, cx| {
                let value = *value;
                backend.update(cx, |b, bcx| {
                    let mut cfg = b.quiet_cfg();
                    set(&mut cfg, value);
                    b.set_quiet_cfg(cfg, bcx);
                });
            })
    }
}

/// A muted caption standing beside a field.
fn quiet_field_label(text: String, cx: &App) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(design::t_caption(cx))
        .text_color(rgb(MoonPalette::active(cx).text_muted))
        .child(text)
}
