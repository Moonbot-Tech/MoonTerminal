//! Shared market picker for typed search and cached empty-field suggestions.
//!
//! The list is a two-level tree: exchange sections (drawn only when there is more than one), then
//! ONE ROW PER COIN, with the cores that offer it as child rows underneath. A coin row opens on the
//! core the host is addressing ([`pick_core`]); its `@server` names that core so the choice is
//! never hidden, and child rows expose a clipped core name in a tooltip. A group of more than
//! [`COIN_GROUP_AUTO_EXPAND`] cores starts collapsed — on a fifty-six-core config the flat form was
//! fifty-six identical rows of one coin. The widget does not define selection behavior; its owner
//! supplies `on_pick`, `on_toggle` and `on_expand`, and owns the expanded-row set the same way it
//! owns the multi-select one.
//!
//! Chart tabs open a market and may show Recent and Top 24h volatility sections, while the header
//! rate ticker and Report token filter remain query-only consumers.
//!
//! Consumers are the chart-tab strip and detached windows through the
//! [`crate::chart_tabs::coin_search`] shim, the header rate ticker in `shell/ticker.rs`, and the
//! Report token filter in `panels/report`.

use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonDisclosure, MoonDisclosureDirection,
    MoonInputState, MoonPalette, h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;
use moon_core::session::core_order::{self, ExchangeSection};
use moon_core::venue::CoreVenue;

mod in_trade;
mod ranking;
mod tabs;

mod group;
mod popup;
mod search;
mod sections;
mod suggest;

pub(crate) use group::*;
pub(crate) use popup::*;
pub(crate) use search::*;
pub(crate) use sections::*;
pub(crate) use suggest::*;

pub(crate) use tabs::{CoinActRow, CoinTab, CoinTabsCfg, banned, favorites};

use ranking::{
    MOVER_VOL_REF, Mover, SUGGEST_ROW_CAP, merge_ranked_heads, mover_score,
    neutralize_blind_provider, turnover_usd,
};

/// Maximum number of MoonProto search results requested per core.
pub(crate) const COIN_SEARCH_LIMIT: usize = 8;

/// Results per core for "the same instrument on another exchange", where the list is filtered by
/// identity afterwards rather than shown.
///
/// Far wider than the popup's, and it has to be: a live Bybit core lists BTC under ten expiries
/// beside the perpetual, and at eight rows the perpetual can fall outside the answer entirely —
/// the comparison would then open a dated contract while claiming to show the coin.
pub(crate) const COIN_MATCH_LIMIT: usize = 32;

/// Maximum logical height before the coin list starts scrolling.
const COIN_LIST_RAW_CAP: f32 = 340.0;

/// Logical height of the continuation fade over an overflowing coin list.
const COIN_LIST_FADE_H: f32 = 12.0;

/// Returns the cores whose market universes feed this token field. None searches the full group,
/// the same as a shared bucket.
///
/// Returned in canonical order: the search popup groups its hits per core and renders them
/// in the order given, so this order is what the user reads as `COIN — Server` rows.
fn cores_for(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoreId> {
    let group_cores = || {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .filter(|s| b.core_displayed_in_group(group, s.id))
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    let order = core_order::CoreOrder::new(&b.config);
    let mut ids = match bucket {
        None | Some(ChartBucket::Shared) => group_cores(),
        // Already the caller's own resolved bucket — not an enumeration, so it stays unfiltered.
        Some(ChartBucket::Core(id)) => vec![*id],
        Some(ChartBucket::Bundle(name)) => {
            let split = b.config.charts_split_by_core;
            b.session
                .sessions()
                .iter()
                .filter(|s| s.group == group)
                .filter(|s| b.core_displayed_in_group(group, s.id))
                .filter(|s| {
                    b.config
                        .servers
                        .iter()
                        .find(|sv| sv.id == s.id)
                        .map(|sv| sv.chart_bucket(split) == ChartBucket::Bundle(name.clone()))
                        .unwrap_or(false)
                })
                .map(|s| s.id)
                .collect()
        }
    };
    order.sort_by(&mut ids, |id| *id);
    ids
}

/// Returns the fixed height shared by every direct child of the scrolling result list.
///
/// Args:
///     cx: Application context used to resolve the font-scaled design height.
///
/// Returns:
///     The row height in logical pixels.
fn coin_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 20.0, 12.0, 4.0)
}

/// Returns the visible whole-row count for a raw viewport cap.
///
/// Args:
///     raw_cap: Maximum viewport height in logical pixels.
///     row_h: Fixed direct-child row height in logical pixels.
///
/// Returns:
///     The floored integral count when a row fits, or one slot to keep a scaled list visible; the
///     latter minimum can exceed `raw_cap`.
fn whole_row_slots(raw_cap: f32, row_h: f32) -> usize {
    (raw_cap / row_h).floor().max(1.0) as usize
}

/// Returns a viewport cap composed of complete result rows.
///
/// Args:
///     raw_cap: Maximum viewport height in logical pixels.
///     row_h: Fixed direct-child row height in logical pixels.
///
/// Returns:
///     The largest integral cap at or below `raw_cap` when one row fits, or one full row so the
///     list cannot collapse; that minimum can exceed `raw_cap`.
fn whole_row_cap(raw_cap: f32, row_h: f32) -> f32 {
    whole_row_slots(raw_cap, row_h) as f32 * row_h
}

/// Applies a recorded toggle as an inversion of the size-based default.
///
/// `toggled` holds the groups the user has FLIPPED away from their default, not the open ones, so a
/// freshly opened popup needs no seeding and a small group is open without ever being recorded.
///
/// Args:
///     key: Identity of the coin row.
///     members: How many cores it holds, which decides its default.
///     toggled: Groups the host has recorded a click on.
///
/// Returns:
///     Whether its child rows are rendered.
pub(crate) fn group_is_open(
    key: &CoinGroupKey,
    members: usize,
    toggled: &HashSet<CoinGroupKey>,
) -> bool {
    group_starts_expanded(members) != toggled.contains(key)
}

/// Avoids redundant exchange chrome when every row belongs to one venue.
///
/// One exchange needs no heading — every row is on it, and the caption would be a line of chrome
/// repeating what the scope already says.
pub(crate) fn shows_sections(sections: &[CoinSection]) -> bool {
    sections.len() > 1
}

/// Counts the fixed-height direct children one grouped list adds to the scrolling list.
///
/// The viewport cap and the overflow fade are both derived from a COUNT of fixed-height direct
/// children ([`whole_row_slots`], [`whole_row_cap`]), so every row type this list can draw has to
/// be counted here or the last visible row is cut and the fade lies about what is below it.
///
/// Args:
///     sections: The grouped list, as [`group_hits`] produced it.
///     toggled: Groups the host has recorded a click on.
///
/// Returns:
///     Exchange headings, coin rows and the child rows of OPEN groups.
pub(crate) fn direct_row_count(sections: &[CoinSection], toggled: &HashSet<CoinGroupKey>) -> usize {
    let headings = if shows_sections(sections) {
        sections.len()
    } else {
        0
    };
    headings
        + sections
            .iter()
            .flat_map(|section| section.groups.iter())
            .map(|group| {
                // The `members > 1` half is NOT redundant, and dropping it is the bug this guard
                // exists for: a one-core group has no caret (`push_section` builds it under the
                // same condition), so it can never enter `toggled`, so `group_is_open` is
                // unconditionally true for it -- while the renderer skips its child row anyway.
                // Counting 2 where 1 is drawn inflates the total on the COMMON case and paints the
                // continuation fade over a list with nothing below it.
                1 + if group.members.len() > 1
                    && group_is_open(&group.key, group.members.len(), toggled)
                {
                    group.members.len()
                } else {
                    0
                }
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests;
