//! Which news belong on ONE chart: the per-coin news marks the chart draws along its bottom edge,
//! and the Ctrl-hover card reads. How they LOOK is the chart's business (`moon_chart::news_marks`);
//! this module only decides which news belong to a chart.
//!
//! Two consumers share this: the chart engine (marker geometry, needs only the times) and the chart
//! panel (hit-test + hover card, needs the whole item). Both must agree on the set, so the selection
//! rule — coin match, cross-core dedup, tag filter, ordering — lives HERE and nowhere else.
//!
//! Selection rule, in order:
//! 1. the item names the chart's coin in `coins` (`meta.coinsAuto`/`coinsSelf`), compared through
//!    [`crate::symbol::coin_match_key`] so `BTC_RP` and `btc` match one chart;
//! 2. the item passes the user's persisted tag filter — the same [`NewsTagSettings`] the News panel
//!    obeys, so hiding a topic there also clears its marks from the chart (and hiding every tag
//!    plus the tagless row is how a user turns the marks off wholesale);
//! 3. one row per `meta.id` across cores (the same news arrives from every subscribed core),
//!    preferring a translated copy exactly as the News panel does;
//! 4. sorted OLDEST first by mark time, which is the order the chart draws left to right.
//!
//! DELIBERATE difference from the News panel: this reads EVERY core, while the panel is scoped to
//! its window group. A chart's own core is usually a trading core with no news subscription at all,
//! so group scoping would leave most charts blank; news is one service-wide stream and the coin, not
//! the core, is what makes an item belong to a chart.

use std::collections::HashMap;

use crate::config::NewsTagSettings;
use crate::feed::news::NewsItem;
use crate::session::store::CoreStore;
use crate::symbol::{coin_match_key, coin_of_market, strip_contract_suffix};

/// Upper bound on marks handed to one chart. The per-core ring holds 50 frames and a coin's own news
/// is a small slice of that, so this only guards against a pathological feed: a chart cannot be
/// buried under marks, and the panel's hover scan stays trivially cheap.
pub const MAX_CHART_MARKS: usize = 64;

/// How far ahead of "now" a service timestamp may sit and still be trusted. The stamps are service-
/// controlled and a rescaled one (seconds multiplied by 1000 twice) lands centuries in the future,
/// where it would both draw off-chart and evict real marks at the [`MAX_CHART_MARKS`] cap. A minute
/// of tolerance covers ordinary clock skew between the news service and this machine.
const FUTURE_SKEW_MS: i64 = 60_000;

/// When the news existed, if that time is usable at all: [`mark_time_ms`], rejected when it sits
/// further ahead of `now_ms` than [`FUTURE_SKEW_MS`].
///
/// The feed's own clock: what orders the panel and drives the unread counters.
pub fn usable_time_ms(item: &NewsItem, now_ms: i64) -> Option<i64> {
    mark_time_ms(item).filter(|t| *t <= now_ms + FUTURE_SKEW_MS)
}

/// When the news became actionable, if that time is usable at all — the chart's clock, where a gem
/// is drawn.
///
/// The guard is applied to each CANDIDATE, not to the winner: a service that rescaled only its send
/// stamp should cost the item its precision, not its place on the chart, so the next stamp in the
/// chain takes over. Same [`FUTURE_SKEW_MS`] as the feed's clock, so a stamp centuries ahead is
/// rejected identically whichever question is being asked.
pub fn usable_delivery_time_ms(item: &NewsItem, now_ms: i64) -> Option<i64> {
    let horizon = now_ms + FUTURE_SKEW_MS;
    [item.send_time_ms, item.recv_time_ms, Some(item.time_ms)]
        .into_iter()
        .flatten()
        .find(|&t| t > 0 && t <= horizon)
}

/// When the news EXISTED, Unix ms, or `None` when the item carries no usable timestamp.
///
/// Publication (`meta.timeMs`) is the anchor: it is the moment the news appeared in the world. The
/// rest are fallbacks for items that arrive without one, in the order those moments happen. This is
/// the time the feed is ordered and counted by — what the news is, not when it reached anyone.
pub fn mark_time_ms(item: &NewsItem) -> Option<i64> {
    [
        Some(item.time_ms),
        item.recv_time_ms,
        item.send_time_ms,
        item.recv_terminal_ms,
    ]
    .into_iter()
    .flatten()
    .find(|&t| t > 0)
}

/// When the news became ACTIONABLE — the moment the service sent it out, Unix ms.
///
/// This is where the chart marks it, matching MoonBot, which marks news at the moment the bot got
/// it. Publication is when the world learned; this is the earliest a client here could have. The
/// gap between the two is the delivery delay, and drawing the mark at the later moment is what
/// makes that gap visible: the move starts left of the gem, and the distance is what the delay cost.
///
/// The terminal's own receipt is deliberately NOT in the chain, even last: it is stamped by this
/// PC's clock, and for news backfilled from the core's ring on connect it is the moment of connect,
/// which would park a mark hours away from its candle.
pub fn delivery_time_ms(item: &NewsItem) -> Option<i64> {
    [item.send_time_ms, item.recv_time_ms, Some(item.time_ms)]
        .into_iter()
        .flatten()
        .find(|&t| t > 0)
}

/// Whether an item passes the persisted tag filter: a tagged item shows unless EVERY one of its tags
/// is hidden; a tagless item shows unless the user hid tagless news. Shared with the News panel so
/// one filter drives both surfaces.
pub fn tag_visible(item: &NewsItem, settings: &NewsTagSettings) -> bool {
    if item.tags.is_empty() {
        !settings.hide_untagged()
    } else {
        item.tags
            .iter()
            .any(|t| !settings.is_hidden(&t.to_lowercase()))
    }
}

/// Whether a news item names `coin_key` (already a [`coin_match_key`]) among its tickers.
///
/// Compares without building a key per ticker: this is the first and least selective filter, so it
/// runs over every core's whole ring on each news change.
fn names_coin(item: &NewsItem, coin_key: &str) -> bool {
    item.coins
        .iter()
        .any(|c| strip_contract_suffix(c.trim()).eq_ignore_ascii_case(coin_key))
}

/// The news marks for `market` with their mark times, oldest FIRST, capped at [`MAX_CHART_MARKS`].
///
/// The time returned is the DELIVERY time ([`usable_delivery_time_ms`]) — where the gem is drawn —
/// while the item it comes with still carries its publication stamp, which is what the hover card
/// shows beside it. Item and time travel together so a caller cannot end up with two lists that
/// drifted apart: the chart indexes marks and hover text by the same position.
///
/// `now_ms` is the caller's clock, used only to reject stamps from the future (see
/// [`FUTURE_SKEW_MS`]).
pub fn collect(
    store: &CoreStore,
    market: &str,
    settings: &NewsTagSettings,
    now_ms: i64,
) -> Vec<(NewsItem, i64)> {
    let coin_key = coin_match_key(coin_of_market(market));
    if coin_key.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(NewsItem, i64)> = Vec::new();
    // Position of each accepted `meta.id` in `out`, so the cross-core merge stays linear instead of
    // rescanning the accumulated list for every item of every core.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (_id, core) in store.cores() {
        for item in &core.news.items {
            if !names_coin(item, &coin_key) || !tag_visible(item, settings) {
                continue;
            }
            let Some(time_ms) = usable_delivery_time_ms(item, now_ms) else {
                continue;
            };
            match seen.get(&item.id) {
                // Across cores the same news can be original on one and translated on another; keep
                // the translated copy, matching the News panel's merge.
                Some(&ix) => {
                    if out[ix].0.is_original && !item.is_original {
                        out[ix] = (item.clone(), time_ms);
                    }
                }
                None => {
                    seen.insert(item.id.clone(), out.len());
                    out.push((item.clone(), time_ms));
                }
            }
        }
    }
    // Oldest first: the chart draws left to right, and the cap must drop the OLDEST marks (the ones
    // scrolling off the left edge), not the newest.
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id)));
    if out.len() > MAX_CHART_MARKS {
        out.drain(..out.len() - MAX_CHART_MARKS);
    }
    out
}

/// Fold the cores' news revisions and the tag-filter revision into a change signature. The chart
/// rebuilds its marks only when this moves, so an ordinary market tick costs nothing.
///
/// The per-core fold is COMMUTATIVE because `store.cores()` walks a `HashMap`: an order-sensitive
/// fold would move the signature whenever a rehash reordered the cores, rebuilding marks that did
/// not change.
pub fn signature(store: &CoreStore, settings: &NewsTagSettings) -> u64 {
    let mut sig = settings.rev();
    for (id, core) in store.cores() {
        let core_sig = id
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(core.news_rev.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        sig = sig.wrapping_add(core_sig);
    }
    sig
}

#[cfg(test)]
mod tests;
