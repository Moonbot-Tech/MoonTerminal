//! The station's own updates, in its section: a switch showing what the station runs with now,
//! read from its status, and changed through `station.toml` the way the tape window is
//! (`moon_remote::station::push_auto_update`).

use gpui::*;
use moon_core::station_api::Status;
use moon_remote::ssh::Target;
use moon_ui::{MoonCheckbox, MoonPalette, rgba_from, v_flex};
use rust_i18n::t;

use super::super::SettingsView;
use crate::backend::station::job::Job;
use crate::design;

/// What the switch can show for the station's last read status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoUpdateView {
    /// The station reported its switch.
    Known(bool),
    /// The station answers but is older than the switch: it must be updated first.
    OldStation,
    /// No status read: the station is stopped, starting or not reached yet.
    Unread,
}

impl AutoUpdateView {
    /// The view for a status as last read.
    pub(super) fn of(status: Option<&Status>) -> Self {
        match status {
            None => Self::Unread,
            Some(status) => status.auto_update.map_or(Self::OldStation, Self::Known),
        }
    }
}

impl SettingsView {
    /// The switch, live only while the station reported its value and no job runs.
    pub(super) fn server_auto_update_block(
        &self,
        target: &Target,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let backend = self.backend.read(cx);
        let busy = backend.station.busy();
        let view = AutoUpdateView::of(
            backend
                .station
                .bot
                .as_ref()
                .and_then(|state| state.station.as_deref()),
        );
        let note = match view {
            AutoUpdateView::Known(_) => None,
            AutoUpdateView::OldStation => Some(t!("telegram.server.auto_update_old")),
            AutoUpdateView::Unread => Some(t!("telegram.server.auto_update_unknown")),
        };
        let target = target.clone();
        let switch = MoonCheckbox::new("server-auto-update")
            .checked(view == AutoUpdateView::Known(true))
            .disabled(busy || !matches!(view, AutoUpdateView::Known(_)))
            .label(t!("telegram.server.auto_update").to_string())
            .description(t!("telegram.server.auto_update_hint").to_string())
            .on_change(cx.listener(move |this, on: &bool, _, cx| {
                let job = Job::AutoUpdate {
                    target: target.clone(),
                    on: *on,
                };
                this.server_bot_run(Ok(job), cx);
            }));
        let mut block = v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap(design::ui_px(cx, 6.0))
            .child(switch);
        if let Some(note) = note {
            block = block.child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(note.to_string()),
            );
        }
        block.into_any_element()
    }
}

#[cfg(test)]
mod tests;
