//! What the feed shows, what it hides, and what it says when it shows nothing.
//!
//! Four decisions, all of them pure and all of them read from two places at once — ingestion and
//! presentation — which is exactly why they are functions rather than conditions written twice.

use std::collections::HashSet;

use moon_core::session::CoreId;
use rust_i18n::t;

use crate::Backend;

use crate::workspace::scope_marker::{self, ScopeMarker};

/// Pick the sentence an EMPTY detection feed states, in the house precedence.
///
/// A feed with no visible card is several different facts, and they must not share a string:
/// only the no-cores one asks the user to go and connect something.
///
/// Two orderings here are load-bearing, and both were wrong in this function's first draft.
///
/// **The empty UNIVERSE is checked first.** A card outlives the session that produced it — it
/// stays in the queue for its whole `KeepAlert` and [`Self::ingest`] never evicts one whose core
/// disappeared — so a disconnect mid-`KeepAlert` leaves cards retained, nothing visible, and no
/// core available. Asking about the retained cards first would blame the scope for hiding
/// detects when the cores behind them are simply gone. The PARTIAL version of that state — one
/// core of several goes away while its siblings sit idle — is settled by the caller instead,
/// which counts only cards whose core is still available (see `retained` below); the count and
/// this ordering are two halves of one rule. `available` counts
/// `WorkspaceCoreAvailability::is_available` (`workspace.rs:251`), which is group and core
/// activation plus a live session and a live window — hence "available", never "connected".
///
/// **The shared hidden-by-preset sentence applies ONLY when data really was excluded.**
/// [`scope_marker::scope_empty_text`] switches on membership alone, and its contract is that the
/// data exists and the preset is what withholds it. A Detects feed can be empty with a full
/// scope-hiding preset simply because nothing ever fired, and there the shared sentence would
/// send the user to widen a preset that is hiding nothing. So it is reached only under
/// `retained > 0`.
///
/// Args:
///     marker: This group's scope marker, built from the same membership counts presentation
///         filters on.
///     retained: Cards this panel holds whose core is STILL AVAILABLE — the ones a preset change
///         could actually bring back. Nonzero with nothing visible means detects DID arrive and
///         presentation is what hides them. A card whose core has gone away is deliberately NOT
///         counted: it is unreachable rather than hidden, and no scope change reveals it.
///     available: Group cores that survived availability — the universe the feed could ever
///         show. Zero means nothing here can detect at all.
///
/// Returns:
///     One localized sentence, already resolved; never empty.
pub(super) fn empty_feed_text(marker: &ScopeMarker, retained: usize, available: usize) -> String {
    if available == 0 {
        t!("detects.empty_no_cores").to_string()
    } else if retained > 0 {
        scope_marker::scope_empty_text(Some(marker), t!("detects.empty_filtered").to_string())
    } else {
        t!("detects.empty").to_string()
    }
}

/// Whether the crowd's card for `coin` would be saying the same thing a core card already says.
///
/// A core's detection and the crowd's rule land on the same coin more often than not: the crowd is
/// trading it BECAUSE something is happening there, which is also why a strategy fired. Two cards
/// then report one event, and the core's is the better of the two — it names a strategy, an
/// exchange and a market, and its chart is its own rather than borrowed from whichever core
/// happened to trade the ticker. So the crowd's card yields.
///
/// WHICH exchange the core card came from is deliberately not part of the question: the crowd has
/// no exchange, and "this coin is already on screen" is the whole of what makes the second card
/// redundant.
///
/// Compared on `MarketLabel::identity` — the CROSS-EXCHANGE key, which is the core's own
/// `market_currency_canonic` — and not on a fold of the name. That distinction is the whole of
/// "whatever exchange it is on": no rule over a name can tell Bybit's `1kBONKPERP` from Binance's
/// spelling of the same coin, and `match_key` deliberately keeps the two apart because a coin list
/// and a report must. Only the catalog knows, and the arbitrage column borrows quotes by the same
/// field.
///
/// Args:
///     identity: The crowd card's cross-exchange identity, borrowed from the market it took its
///         chart from.
///     cored: Identities of the core cards this pass is actually going to draw.
///
/// Returns:
///     `true` when the crowd's card should stand down.
pub(super) fn crowd_card_yields(identity: &str, cored: &HashSet<&str>) -> bool {
    cored.contains(identity)
}

/// Return whether a retained detection card belongs to the current presentation scope.
///
/// A card with no core is always visible, and that is not an oversight. The scope selects among
/// THIS GROUP's cores; the crowd is not one of them, so there is no preset under which it could be
/// the wrong one — and hiding it would leave the reader adjusting a preset that does not address it.
///
/// Args:
///     core: Core attached to one retained card, or `None` for a crowd detection.
///     visible: Effective workspace core ids.
///
/// Returns:
///     `true` only when presentation may render the retained card.
pub(super) fn detection_core_visible(core: Option<CoreId>, visible: &[CoreId]) -> bool {
    core.is_none_or(|core| visible.contains(&core))
}

/// Return whether a detection has outlived its `KeepAlert` window.
///
/// One rule for both readers: `prune` drops the cards that reach it, and ingestion refuses to build
/// a card — or pay for its market snapshot — for a row that would be dropped on the same pass.
///
/// Args:
///     now_ms: Wall-clock time of this pass, in Unix milliseconds.
///     born_ms: When the core reported the detection.
///     ttl_ms: `KeepAlert` for that detection, in milliseconds.
///
/// Returns:
///     `true` when the detection may no longer occupy the feed.
pub(super) fn detect_expired(now_ms: f64, born_ms: f64, ttl_ms: f64) -> bool {
    now_ms - born_ms >= ttl_ms
}

/// Return whether a retained card survives the AddToChart setting.
///
/// Ingestion already applies the same rule, so this only matters for cards taken in while the
/// setting was on: turning it off must clear them immediately rather than leave them for the rest
/// of their `KeepAlert`.
///
/// Args:
///     add_to_chart: The card's `AddToChart` tab number, `0` when the detect opens no tab.
///     show_add_to_chart: Whether this group displays chart-routed detects in the feed.
///
/// Returns:
///     `true` only when presentation may render the retained card.
pub(super) fn detection_route_visible(add_to_chart: u32, show_add_to_chart: bool) -> bool {
    add_to_chart == 0 || show_add_to_chart
}

/// Signature that makes the panel re-ingest: a hash of every group core's detect revision, PLUS the
/// AddToChart setting as its own value. The setting belongs here because flipping it changes which
/// rows the feed accepts, and this is what wakes EVERY panel of the group — the one whose checkbox
/// was clicked notifies itself, but a second, detached panel learns of it only through this.
///
/// The setting is a separate field rather than a seed folded into the hash: at `31 * flag + rev`,
/// a core whose revision advanced by exactly 31 in the same flush would cancel the flip out, and
/// the panel would never see it.
pub(super) fn detects_sig(b: &Backend, group: &str) -> (u64, bool) {
    let store = b.session.store();
    let revs = b
        .session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.detects_rev));
    (revs, b.detects_view.shows_add_to_chart(group))
}
