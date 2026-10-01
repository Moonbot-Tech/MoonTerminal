//! The station's window around a trade, in its section: what the station records with now, read
//! from its status, and "Set" to change it (STATION.md §1 п. 14).
//!
//! The station's window is its own. The terminal never sends it behind the user's back — neither
//! at start nor when Settings -> Storage changes the terminal's window: a terminal on old values
//! would silently rewrite the station's, and several terminals may share one station. Only a new
//! station starts with the installing terminal's window.

use gpui::*;
use moon_core::config::storage as storage_cfg;
use moon_core::station_api::TapeWindow;
use moon_remote::ssh::Target;
use moon_ui::{MoonButton, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::super::SettingsView;
use crate::backend::station::job::Job;
use crate::design;

/// The window as the station reported it and as edited here.
#[derive(Default)]
pub(in crate::settings) struct TapeEd {
    /// The station's window as last read.
    seen: Option<TapeWindow>,
    /// The window as edited; follows `seen` while untouched.
    draft: Option<TapeWindow>,
}

impl SettingsView {
    /// A new read of the station's window: an untouched draft follows it, an edited one stays. A
    /// read without one (the station restarting, stopped, or older than the report) changes
    /// nothing: the last known window and the edit stay.
    pub(super) fn server_tape_sync(&mut self, cx: &App) {
        let read = self
            .backend
            .read(cx)
            .station
            .bot
            .as_ref()
            .and_then(|state| state.tape);
        let ed = &mut self.telegram.server.tape;
        if read.is_none() || read == ed.seen {
            return;
        }
        if ed.draft == ed.seen {
            ed.draft = read;
        }
        ed.seen = read;
    }

    fn server_tape_margin(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(draft) = self.telegram.server.tape.draft.as_mut() {
            draft.margin_s = storage_cfg::step_trade_margin_s(draft.margin_s, delta);
            cx.notify();
        }
    }

    fn server_tape_long(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(draft) = self.telegram.server.tape.draft.as_mut() {
            draft.long_position_min =
                storage_cfg::step_long_position_min(draft.long_position_min, delta);
            cx.notify();
        }
    }

    /// Align the station's trade-window labels and visible steppers in a wrapping form.
    /// "Set" stays live only while the draft differs from the station's recorded window.
    pub(super) fn server_tape_block(&self, target: &Target, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let busy = self.backend.read(cx).station.busy();
        let ed = &self.telegram.server.tape;
        let hint = |key: &str| {
            div()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!(key).to_string())
        };
        let title = div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgba_from(p.text, 1.0))
            .child(t!("telegram.server.tape_title").to_string());
        let Some(draft) = ed.draft else {
            return v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(title)
                .child(hint("telegram.server.tape_unknown"))
                .into_any_element();
        };
        let row = |label: String, control: AnyElement| {
            h_flex()
                .w_full()
                .min_w(px(0.0))
                .flex_wrap()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(
                    div()
                        .w(design::font_w_px(cx, 150.0))
                        .max_w_full()
                        .flex_shrink_0()
                        .text_color(rgba_from(p.text_soft, 1.0))
                        .child(label),
                )
                .child(control)
        };
        let changed = ed.seen != Some(draft);
        let target = target.clone();
        v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 6.0))
            .child(title)
            .child(hint("telegram.server.tape_hint"))
            .child(row(
                t!("storage.trades_margin").to_string(),
                self.stepper_controls_styled(
                    cx,
                    "server-tape-margin",
                    !busy,
                    Self::trades_margin_label(draft.margin_s),
                    1,
                    3,
                    Self::server_tape_margin,
                    true,
                )
                .into_any_element(),
            ))
            .child(row(
                t!("storage.trades_long_position").to_string(),
                self.stepper_controls_styled(
                    cx,
                    "server-tape-long",
                    !busy,
                    t!("storage.trades_min", min = draft.long_position_min).to_string(),
                    1,
                    5,
                    Self::server_tape_long,
                    true,
                )
                .into_any_element(),
            ))
            .child(row(
                String::new(),
                MoonButton::new("server-tape-set")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("telegram.server.tape_set").to_string())
                    .disabled(busy || !changed)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let job = Job::Tape {
                            target: target.clone(),
                            tape: draft,
                        };
                        this.server_bot_run(Ok(job), cx);
                    }))
                    .render()
                    .into_any_element(),
            ))
            .into_any_element()
    }
}
