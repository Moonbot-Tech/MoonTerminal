//! The station's window around a trade, in its section: what the station records with now, read
//! from its status, and "Write to the station" to change it (STATION.md §1 п. 14).
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

/// Shown in both steppers when the station has not reported a window.
const UNKNOWN_VALUE: &str = "—";

/// The window as the station reported it and as edited here.
#[derive(Default)]
pub(in crate::settings) struct TapeEd {
    /// The station's window as last read.
    seen: Option<TapeWindow>,
    /// The window as edited; follows `seen` while untouched.
    draft: Option<TapeWindow>,
}

/// Which one line sits beside "Write to the station".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TapeStatus {
    /// The draft matches the station's window.
    Clean,
    /// The draft differs from the station's window.
    Draft,
    /// A job the user waits on is running.
    Busy,
    /// No window has been read, and the service did answer a status without one.
    Older,
    /// No window has been read because the station did not answer.
    Down,
}

/// The facts the tape line is chosen from. Names keep the four booleans from swapping.
pub(super) struct TapeFacts {
    /// A window was read, so the steppers have numbers.
    pub known: bool,
    /// The draft differs from the station's window.
    pub dirty: bool,
    /// A job the user waits on is running.
    pub busy: bool,
    /// A status was read and it carried no window: the service predates the report.
    pub older_service: bool,
}

/// Pick the tape block's one status line.
///
/// Unknown values win over a running job, because the dashes need either the danger line or
/// the older-service sentence. A running job wins over an unsaved draft, because the button
/// is already disabled. An untouched draft that matches the station is clean.
///
/// Args:
/// * `facts`: What the block knows about the window and the running job.
///
/// Returns:
/// The line to show. Exactly one.
pub(super) fn tape_status(facts: TapeFacts) -> TapeStatus {
    if !facts.known {
        return if facts.older_service {
            TapeStatus::Older
        } else {
            TapeStatus::Down
        };
    }
    if facts.busy {
        return TapeStatus::Busy;
    }
    if facts.dirty {
        return TapeStatus::Draft;
    }
    TapeStatus::Clean
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

    /// The station's trade window: what it records, the two steppers, and whether the draft
    /// has been written. "Write to the station" stays live only while the draft differs from
    /// the station's recorded window.
    pub(super) fn server_tape_block(&self, target: &Target, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let backend = self.backend.read(cx);
        let busy = backend.station.busy();
        let job_name = backend
            .station
            .running_label
            .map(|label| label.text())
            .unwrap_or_default();
        let ed = &self.telegram.server.tape;
        let older = ed.draft.is_none()
            && backend
                .station
                .bot
                .as_ref()
                .is_some_and(|bot| bot.station.is_some() && bot.tape.is_none());
        let known = ed.draft.is_some();
        let changed = ed.draft.is_some_and(|draft| ed.seen != Some(draft));
        let margin_changed = ed
            .draft
            .zip(ed.seen)
            .is_some_and(|(draft, seen)| draft.margin_s != seen.margin_s);
        let long_changed = ed
            .draft
            .zip(ed.seen)
            .is_some_and(|(draft, seen)| draft.long_position_min != seen.long_position_min);
        let status = tape_status(TapeFacts {
            known,
            dirty: changed,
            busy,
            older_service: older,
        });
        let sentence = |text: String, color: u32| {
            div()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(color, 1.0))
                .child(text)
        };
        let (status_text, status_color) = match status {
            TapeStatus::Clean => (
                t!("telegram.server.tape_clean").to_string(),
                design::positive_color(p),
            ),
            TapeStatus::Draft => {
                let margin = ed
                    .seen
                    .map(|window| Self::trades_margin_label(window.margin_s))
                    .unwrap_or_else(|| UNKNOWN_VALUE.to_string());
                let long = ed
                    .seen
                    .map(|window| {
                        t!("storage.trades_min", min = window.long_position_min).to_string()
                    })
                    .unwrap_or_else(|| UNKNOWN_VALUE.to_string());
                (
                    t!("telegram.server.tape_draft", margin = margin, long = long).to_string(),
                    p.amber,
                )
            }
            TapeStatus::Busy => (
                t!("telegram.server.tape_busy", job = job_name).to_string(),
                p.text_muted,
            ),
            TapeStatus::Down => {
                let status_button = t!("telegram.server.status").to_string();
                (
                    t!("telegram.server.tape_down", status = status_button).to_string(),
                    design::danger_color(p),
                )
            }
            TapeStatus::Older => (t!("telegram.server.tape_unknown").to_string(), p.text_muted),
        };
        let margin_text = match ed.draft {
            Some(draft) => Self::trades_margin_label(draft.margin_s),
            None => UNKNOWN_VALUE.to_string(),
        };
        let long_text = match ed.draft {
            Some(draft) => t!("storage.trades_min", min = draft.long_position_min).to_string(),
            None => UNKNOWN_VALUE.to_string(),
        };
        let row = |label: String, control: AnyElement, explanation: String| {
            h_flex()
                .w_full()
                .min_w(px(0.0))
                .flex_wrap()
                .gap(design::ui_px(cx, 12.0))
                .items_start()
                .child(
                    div()
                        .w(design::font_w_px(cx, 190.0))
                        .max_w_full()
                        .flex_shrink_0()
                        .pt(design::ui_px(cx, 4.0))
                        .font_family(design::ui_font())
                        .text_color(rgba_from(p.text, 1.0))
                        .child(label),
                )
                .child(control)
                .child(
                    div()
                        .flex_1()
                        .min_w(design::font_w_px(cx, 220.0))
                        .max_w_full()
                        .pt(design::ui_px(cx, 4.0))
                        .child(sentence(explanation, p.text_muted)),
                )
        };
        let mut write = MoonButton::new("server-tape-set")
            .primary()
            .size(design::CONTROL_TIER)
            .label(t!("telegram.server.tape_set").to_string())
            .disabled(busy || !changed);
        if let Some(draft) = ed.draft {
            let target = target.clone();
            write = write.on_click(cx.listener(move |this, _, _, cx| {
                this.server_bot_run(
                    Ok(Job::Tape {
                        target: target.clone(),
                        tape: draft,
                    }),
                    cx,
                );
            }));
        }
        v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 10.0))
            .font_family(design::ui_font())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgba_from(p.text, 1.0))
                    .child(t!("telegram.server.tape_title").to_string()),
            )
            .child(sentence(
                t!("telegram.server.tape_hint").to_string(),
                p.text_muted,
            ))
            .child(row(
                t!("storage.trades_margin").to_string(),
                self.stepper_controls_styled(
                    cx,
                    "server-tape-margin",
                    known && !busy,
                    margin_text,
                    1,
                    3,
                    Self::server_tape_margin,
                    true,
                    margin_changed,
                )
                .into_any_element(),
                t!("telegram.server.tape_margin_hint").to_string(),
            ))
            .child(row(
                t!("storage.trades_long_position").to_string(),
                self.stepper_controls_styled(
                    cx,
                    "server-tape-long",
                    known && !busy,
                    long_text,
                    1,
                    5,
                    Self::server_tape_long,
                    true,
                    long_changed,
                )
                .into_any_element(),
                t!("telegram.server.tape_long_hint").to_string(),
            ))
            .child(
                h_flex()
                    .w_full()
                    .min_w(px(0.0))
                    .flex_wrap()
                    .items_center()
                    .gap(design::ui_px(cx, 12.0))
                    .pt(design::ui_px(cx, 8.0))
                    .border_t_1()
                    .border_color(rgba_from(p.border, 1.0))
                    .child(write.render())
                    .child(
                        div()
                            .flex_1()
                            .min_w(design::font_w_px(cx, 220.0))
                            .child(sentence(status_text, status_color)),
                    ),
            )
            .child(sentence(
                t!("telegram.server.tape_footnote").to_string(),
                p.text_muted,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
