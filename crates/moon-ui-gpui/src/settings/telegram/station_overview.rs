//! The Station overview's head: tiles naming the station's state at a glance once one is known,
//! and the three install steps while none is.

use gpui::*;
use moon_core::telegram::TelegramStatus;
use moon_remote::station::bot::BotState;
use moon_ui::{MoonPalette, MoonProgress, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::super::{SettingsView, common};
use super::server_bot::bot_words;
use super::station_cores::diverging;
use crate::design;

/// One overview tile: a caption, a short value, an optional muted note under it and, where a
/// share means something, a bar.
pub(super) struct Tile {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) note: Option<String>,
    /// Percent, 0..=100.
    pub(super) progress: Option<f32>,
}

/// The install's three steps: server access, the server check, the install itself.
pub(super) fn install_labels() -> Vec<String> {
    [
        t!("station.install.steps.access"),
        t!("station.install.steps.verify"),
        t!("station.install.steps.install"),
    ]
    .into_iter()
    .map(|label| label.to_string())
    .collect()
}

/// Which install step the form stands at: access until the server's key was probed, the check
/// while that key awaits approval, the install while a job runs on an approved key.
pub(super) fn install_step(probed: bool, busy: bool) -> usize {
    match (probed, busy) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    }
}

/// The install steps indicator.
pub(super) fn install_steps_el(current: usize) -> AnyElement {
    div()
        .w_full()
        .debug_selector(|| "station-install-steps".into())
        .child(common::steps(
            "station-install-steps",
            install_labels(),
            current,
        ))
        .into_any_element()
}

/// The tiles in a row that wraps when narrow.
pub(super) fn tiles_el(tiles: Vec<Tile>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let mut row = h_flex()
        .id("station-tiles")
        .debug_selector(|| "station-tiles".into())
        .w_full()
        .min_w_0()
        .flex_wrap()
        // Every tile in a line is as tall as the tallest one.
        .items_stretch()
        .gap(design::ui_px(cx, 8.0));
    for (index, tile) in tiles.into_iter().enumerate() {
        row = row.child(
            v_flex()
                .debug_selector(move || format!("station-tile-{index}"))
                .flex_1()
                .min_w(design::font_w_px(cx, 130.0))
                .gap(design::ui_px(cx, 4.0))
                .p(design::ui_px(cx, 10.0))
                .border_1()
                .border_color(rgba_from(p.border, 1.0))
                .rounded(design::ui_px(cx, 6.0))
                // The mockup's tile: a small muted caption over a prominent mono value.
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(tile.label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(design::mono())
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba_from(p.text, 1.0))
                        .child(tile.value),
                )
                .children(tile.note.map(|note| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(note)
                }))
                .children(tile.progress.map(|percent| {
                    MoonProgress::new(("station-tile-bar", index))
                        .value(percent)
                        .height(design::ui_value(cx, 4.0))
                        .render()
                })),
        );
    }
    row.into_any_element()
}

/// Split a station version such as `v1.2.3 (abc123)` into the version and its build.
///
/// Args:
///     raw: The version the station reported.
///
/// Returns:
///     The version for the tile value and the build, when there is one, for its note.
pub(super) fn split_version(raw: &str) -> (String, Option<String>) {
    match raw.split_once(" (") {
        Some((version, build)) => (
            version.trim().to_string(),
            Some(build.trim_end_matches(')').trim().to_string()).filter(|b| !b.is_empty()),
        ),
        None => (raw.trim().to_string(), None),
    }
}

/// The bot tile's short value and note: a paired bot reads "running" over its chat count; any
/// other state is its usual sentence.
fn bot_tile_words(bot: &BotState) -> (String, Option<String>) {
    match bot.bot.as_ref().map(|b| &b.status) {
        Some(TelegramStatus::Paired { chat_count })
            if !bot.stopped && !bot.no_api && bot.has_token =>
        {
            (
                t!("station.over.bot_running").to_string(),
                Some(t!("station.over.bot_chats", count = chat_count).to_string()),
            )
        }
        _ => (bot_words(bot), None),
    }
}

impl SettingsView {
    /// The overview's tiles from what the station last told: whether it answers, its version,
    /// how many of its cores match the terminal's (with the share as a bar), and its bot.
    pub(super) fn station_tiles(&self, cx: &App) -> Vec<Tile> {
        let st = &self.backend.read(cx).station;
        let status = st.bot.as_ref().and_then(|bot| bot.station.as_ref());
        let mut tiles = vec![Tile {
            label: t!("station.over.status").to_string(),
            value: t!(if status.is_some() {
                "station.over.online"
            } else {
                "station.over.offline"
            })
            .to_string(),
            note: None,
            progress: None,
        }];
        if let Some(status) = status {
            let (version, build) = split_version(&status.station_version);
            tiles.push(Tile {
                label: t!("station.over.version").to_string(),
                value: version,
                note: build,
                progress: None,
            });
        }
        if matches!(st.cores_seen, Some(Some(_))) {
            let rows = &self.telegram.server.cores_cache.rows;
            let same = rows.len() - diverging(rows);
            tiles.push(Tile {
                label: t!("station.over.cores").to_string(),
                value: format!("{same} / {}", rows.len()),
                note: None,
                progress: (!rows.is_empty()).then(|| same as f32 * 100.0 / rows.len() as f32),
            });
        }
        if let Some(bot) = st.bot.as_ref() {
            let (value, note) = bot_tile_words(bot);
            tiles.push(Tile {
                label: t!("station.over.bot").to_string(),
                value,
                note,
                progress: None,
            });
        }
        tiles
    }
}

#[cfg(test)]
mod tests;
