//! The station's copy of the terminal's saved core groups, for the bot's report by groups.
//!
//! Groups are sent on the user's word, never on their own: several terminals may serve one station,
//! and one without groups must not wipe the set another sent. So the box compares what the station
//! answered with the terminal's own groups and, when they differ, offers to send them.

use gpui::*;
use moon_core::config::CoreGroup;
use moon_ui::{MoonButton, MoonPalette, rgba_from, v_flex};
use rust_i18n::t;

use super::super::SettingsView;
use super::server_bot::known_server;
use crate::backend::station::job::Job;
use crate::design;

impl SettingsView {
    /// The groups line under the station's bot menu: whether the station's groups are the
    /// terminal's, and the button that sends them when not. `None` while the station has not
    /// been read, and when neither side has any group.
    pub(super) fn station_groups_row(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let seen = self.telegram.server.access_seen()?;
        let backend = self.backend.read(cx);
        let ours = backend.config.core_groups.clone();
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let Some(theirs) = seen.groups.as_ref() else {
            // A station that predates the groups: it answers none and keeps none.
            return (!ours.is_empty()).then(|| {
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.groups_unsupported").to_string())
                    .into_any_element()
            });
        };
        if ours.is_empty() && theirs.is_empty() {
            return None;
        }
        if same_groups(&ours, theirs) {
            return Some(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.groups_same", names = names(&ours)).to_string())
                    .into_any_element(),
            );
        }
        let busy = backend.station.busy();
        Some(
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div().child(
                        t!(
                            "telegram.server.groups_differ",
                            station = names(theirs),
                            terminal = names(&ours)
                        )
                        .to_string(),
                    ),
                )
                .child(
                    MoonButton::new("server-groups-send")
                        .padding_x(12.0)
                        .label(t!("telegram.server.groups_send").to_string())
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(target) = known_server() else {
                                return;
                            };
                            let groups = this.backend.read(cx).config.core_groups.clone();
                            this.server_bot_run(Ok(Job::Groups { target, groups }), cx);
                        }))
                        .render(),
                )
                .into_any_element(),
        )
    }
}

/// Whether two group lists are the same set: names and members alike, in any order.
fn same_groups(a: &[CoreGroup], b: &[CoreGroup]) -> bool {
    let key = |groups: &[CoreGroup]| {
        let mut keyed: Vec<(String, Vec<u64>)> = groups
            .iter()
            .map(|group| {
                let mut cores = group.cores.clone();
                cores.sort_unstable();
                (group.name.clone(), cores)
            })
            .collect();
        keyed.sort();
        keyed
    };
    key(a) == key(b)
}

/// The groups' names, comma-separated; a dash for none.
fn names(groups: &[CoreGroup]) -> String {
    if groups.is_empty() {
        return "\u{2014}".to_string();
    }
    groups
        .iter()
        .map(|group| group.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
