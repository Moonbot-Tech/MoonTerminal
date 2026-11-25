//! Blacklist state shared by the parent row and the temporary-ban section.

use moon_core::session::CoreId;
use rust_i18n::t;

use super::places::CoinPlace;
use crate::display_text::fmt_ban_left;

/// Permanent places and the clicked core's temporary ban, if held.
pub(super) struct BlState {
    pub(super) listed: usize,
    pub(super) ban: Option<(u64, bool)>,
}

/// Count permanent core and strategy entries separately from temporary bans.
pub(super) fn permanent_count(places: &[CoinPlace]) -> usize {
    places
        .iter()
        .filter(|place| !matches!(place, CoinPlace::Temp { .. }))
        .count()
}

/// Format the muted state beside the submenu caret, if any.
pub(super) fn bl_right_label(state: &BlState) -> Option<String> {
    let listed = (state.listed > 0).then(|| t!("coin_menu.bl_in", n = state.listed).to_string());
    let ban = state.ban.map(|(ms, stale)| {
        let left = fmt_ban_coarse(ms);
        t!(
            if stale {
                "coin_menu.bl_ban_stale"
            } else {
                "coin_menu.bl_ban"
            },
            left = left
        )
        .to_string()
    });
    match (listed, ban) {
        (Some(listed), Some(ban)) => Some(format!("{listed} · {ban}")),
        (listed, ban) => listed.or(ban),
    }
}

/// Show the largest elapsed-time unit that fits on the compact parent row.
fn fmt_ban_coarse(ms: u64) -> String {
    match ms / 60_000 {
        minutes @ 1440.. => t!("coin_menu.tbl_days", n = minutes / 1440),
        minutes @ 60.. => t!("coin_menu.tbl_hours", n = minutes / 60),
        minutes => t!("coin_menu.tbl_minutes", n = minutes.max(1)),
    }
    .to_string()
}

/// Bans held on offered cores, with a countdown only for the clicked core.
#[derive(Default)]
pub(super) struct HeldBan {
    pub(super) cores: Vec<CoreId>,
    pub(super) here: Option<(u64, bool)>,
}

impl HeldBan {
    /// Offer to clear held targets without borrowing another core's countdown.
    pub(super) fn lift_label(&self) -> Option<String> {
        if self.cores.is_empty() {
            return None;
        }
        Some(match self.here {
            Some((ms, stale)) => {
                let left = fmt_ban_left(i64::try_from(ms).unwrap_or(i64::MAX));
                t!(
                    if stale {
                        "coin_menu.tbl_clear_stale"
                    } else {
                        "coin_menu.tbl_clear_left"
                    },
                    left = left
                )
                .to_string()
            }
            None => t!("coin_menu.tbl_clear").to_string(),
        })
    }
}

#[cfg(test)]
mod tests;
