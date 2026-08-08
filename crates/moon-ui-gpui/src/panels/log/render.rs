//! Log rebuild signature, aggregation of live core logs, and line classification.
//!
//! [`super::LogPanel`] owns filtering, the virtual list, and follow-tail scrolling. Classification
//! (`line_list::classify_lower`) and coin detection ([`find_coin`]) run once per line, in
//! [`LineView::parse`], when that line arrives — never again. Parsing is expensive, while filtering
//! re-scans the whole buffer on every keystroke and the virtual list calls the row renderer in
//! [`super::row`] for every visible line on each frame; both only READ what parsing left behind.

use super::*;
use crate::panels::line_list::{self, Cat, Sev};
use moon_core::session::CoreStore;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::ops::Range;

/// Tracks whether backend log revisions should rebuild this panel.
///
/// Dock tabs toggle `active` through `Panel::set_active`; detached panels activate themselves on
/// their first render because their window host renders the view directly. Backend observations
/// only queue the newest signature; the actual render performs the expensive reload. This keeps an
/// outer dock hidden by its container idle even if the container does not call `set_active(false)`.
#[derive(Default)]
pub(super) struct RefreshGate {
    active: bool,
    loaded: bool,
    last_sig: u64,
    pending_sig: Option<u64>,
}

impl RefreshGate {
    /// Changes panel activity and returns whether activation requires an immediate reload.
    ///
    /// `sig` is the selected live source's current revision. Deactivation never reloads.
    pub(super) fn set_active(&mut self, active: bool, sig: u64) -> bool {
        self.active = active;
        if !active || self.loaded && self.last_sig == sig {
            return false;
        }
        self.record_reload(sig);
        true
    }

    /// Returns whether an observed backend revision should schedule an active panel render.
    pub(super) fn observe(&mut self, sig: u64) -> bool {
        if !self.active || self.loaded && self.last_sig == sig || self.pending_sig == Some(sig) {
            return false;
        }
        self.pending_sig = Some(sig);
        true
    }

    /// Records a reload caused directly by a source, file, or follow-mode action.
    pub(super) fn record_reload(&mut self, sig: u64) {
        self.loaded = true;
        self.last_sig = sig;
        self.pending_sig = None;
    }

    /// Queues a reload for the next actual render even when the source revision is unchanged.
    pub(super) fn request_reload(&mut self) {
        self.pending_sig.get_or_insert(self.last_sig);
    }

    /// Consumes a backend revision queued for the next actual panel render.
    pub(super) fn take_observed_reload(&mut self) -> bool {
        self.pending_sig.take().is_some()
    }

    /// Returns whether the panel currently participates in live backend refreshes.
    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    /// Returns whether the panel has ever loaded its source, so a catch-up can extend the buffer
    /// instead of replacing it.
    pub(super) fn has_loaded(&self) -> bool {
        self.loaded
    }
}

/// Precomputed render-ready line containing time, source, classification, flattened text, and an
/// optional coin-token byte range into `flat`.
///
/// One of these is built ONCE per log line, when the line arrives, and then lives in the panel's
/// buffer until the ring evicts it. Everything a filter or a frame needs is precomputed here
/// because both run far more often than lines arrive: the filters re-scan the whole buffer on every
/// keystroke, and the virtual list renders each visible row on every frame.
#[derive(Clone)]
pub(super) struct LineView {
    /// Full `YYYY-MM-DD HH:MM:SS.mmm` stamp, as the source spelled it.
    ///
    /// The whole stamp rather than the displayed clock: it is the key rows are merged and kept
    /// sorted by, and cores whose clocks disagree hand over rows that belong before ones already
    /// buffered — across midnight, a bare clock would sort those backwards.
    pub(super) ts: String,
    pub(super) target: String,
    pub(super) sev: Sev,
    pub(super) cat: Cat,
    /// Message with line breaks flattened for single-line rendering.
    pub(super) flat: String,
    /// Lowercase `flat`, kept rather than recomputed.
    ///
    /// It is what the text and coin filters match against, and what the row renderer highlights
    /// search hits from. Both used to allocate it per line per pass — the filters for the whole
    /// buffer on every rebuild, the renderer for every visible row on every frame.
    pub(super) lower: String,
    /// Detected coin token: its byte range in `flat` for highlighting and its base for
    /// click-filtering.
    ///
    /// For example, token `USDT-SPK` yields base `SPK`.
    pub(super) coin: Option<(Range<usize>, String)>,
}

impl LineView {
    /// Build a render-ready line, classifying and scanning it for a coin once.
    ///
    /// `known` contains coin bases learned from the lines seen so far, allowing bare tickers such as
    /// `SPK` to be highlighted; callers feed it the new lines' bases BEFORE parsing them, so a base
    /// stated by one line lights up its neighbours in the same batch.
    ///
    /// Classification reads the flattened text rather than the raw message. The two differ only in
    /// line breaks, which no keyword spans, and using one string for classification, filtering and
    /// highlighting is what keeps this to a single lowercase allocation per line.
    pub(super) fn parse(line: &LogLine, known: &HashSet<String>) -> Self {
        // Scan the flattened text because folding can change byte offsets.
        let flat = crate::display_text::flatten_lines(&line.msg);
        let lower = flat.to_lowercase();
        let cl = line_list::classify_lower(line.level, &lower);
        let coin = find_coin(&flat, known);
        Self {
            ts: line.ts.clone(),
            target: line.target.clone(),
            sev: cl.sev,
            cat: cl.cat,
            flat,
            lower,
            coin,
        }
    }

    /// Clock the row displays: the `HH:MM:SS.mmm` tail of its stamp.
    ///
    /// Borrowed from `ts` rather than stored separately — the row renderer asks for it on every
    /// frame, and a slice costs nothing.
    pub(super) fn time(&self) -> &str {
        self.ts.rsplit(' ').next().unwrap_or(self.ts.as_str())
    }
}

fn is_tick(b: u8) -> bool {
    // Lowercase letters belong to a token too: real coins carry them (`kPEPE`, `1kRATS`), and
    // without them `USDT-kPEPE` would break into two pieces neither of which is a market.
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Extract the base coin from a market-shaped token, or `None` for an ordinary word.
///
/// The shapes and the quote table live in `moon_core::symbol`, which is the single place that
/// knows how each exchange spells a market; this panel only decides WHERE to look. It used to
/// carry its own copy of both, which is how the panel and the Orders table came to disagree about
/// OKX names.
fn market_base(w: &str) -> Option<String> {
    moon_core::symbol::parse::market_in_text(w).map(str::to_string)
}

/// Longest coin token the core's line head may carry, in bytes.
///
/// Real names run to about ten characters (`1000000MOG`, `PUMPBTC`); the cap is what keeps a
/// sentence that merely LOOKS like the head shape from being read as a ticker.
const MAX_COIN_LEN: usize = 12;

/// Coin the core names at the head of its own log line, as a byte range into `msg`.
///
/// A trading core writes every task line as `HH:MM:SS  COIN<Dir> : [slot] (task) text` or
/// `HH:MM:SS  COIN: [slot] (task) text` — the coin is stated outright, in the core's own spelling.
/// Reading it here is what makes `CLO` and `1kRATS` coins: neither is market-SHAPED, and a name
/// carrying a lowercase letter (`kPEPE`, `1kRATS`, `preOPAI` — all real) cannot be recognized by
/// shape at all.
///
/// The tail is what makes this safe. The token counts only when a direction bracket or the slot
/// bracket follows it, so an ordinary prefixed line (`PumpDetection: ...`, `Starting new MoonShot
/// market: ...`) is not mistaken for one — measured against a day of real core logs, where the
/// strict form matches 150k lines of 211k and `PumpDetection` never matches.
fn coin_at_head(msg: &str) -> Option<Range<usize>> {
    let bytes = msg.as_bytes();
    // The core repeats its clock inside the message; step over it when present.
    let mut at = 0;
    if bytes.len() > 8 && bytes[2] == b':' && bytes[5] == b':' {
        at = 8;
    }
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    let start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphanumeric) {
        at += 1;
    }
    let token = msg.get(start..at)?;
    if !(2..=MAX_COIN_LEN).contains(&token.len()) || !token.bytes().any(|b| b.is_ascii_uppercase())
    {
        return None;
    }
    // `COIN<Short>`, or `COIN : [slot]` with the spaces the core puts in or leaves out.
    let mut tail = at;
    if bytes.get(tail) == Some(&b'<') {
        return Some(start..at);
    }
    while bytes.get(tail) == Some(&b' ') {
        tail += 1;
    }
    if bytes.get(tail) != Some(&b':') {
        return None;
    }
    tail += 1;
    while bytes.get(tail) == Some(&b' ') {
        tail += 1;
    }
    (bytes.get(tail) == Some(&b'[')).then_some(start..at)
}

/// Collect coin names that any line in the buffer states outright: at its head, or as a
/// market-shaped token.
///
/// This lets a later BARE mention be highlighted too, because `SPK` alone is structurally
/// indistinguishable from words such as `BUY` or `API`.
pub(super) fn collect_coin_bases(lines: &[LogLine]) -> HashSet<String> {
    let mut set = HashSet::new();
    for l in lines {
        if let Some(head) = coin_at_head(&l.msg) {
            set.insert(l.msg[head].to_string());
        }
        let bytes = l.msg.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !is_tick(bytes[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && is_tick(bytes[i]) {
                i += 1;
            }
            if let Some(base) = market_base(&l.msg[start..i]) {
                set.insert(base);
            }
        }
    }
    set
}

/// Find the first coin in a message with a purpose-built scanner rather than a regex.
///
/// The core's own line head answers first ([`coin_at_head`]) — it is the one place a coin is named
/// rather than inferred. Otherwise a market-shaped token takes its full byte range for highlighting
/// and yields a base for the click filter, and a bare ticker is accepted only when `known` holds it.
/// Return the byte range and base, or `None` when no coin is found.
pub(super) fn find_coin(msg: &str, known: &HashSet<String>) -> Option<(Range<usize>, String)> {
    if let Some(head) = coin_at_head(msg) {
        return Some((head.clone(), msg[head].to_string()));
    }
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_tick(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_tick(bytes[i]) {
            i += 1;
        }
        let word = &msg[start..i];
        if let Some(base) = market_base(word) {
            return Some((start..i, base));
        }
        if word.len() >= 2 && known.contains(word) {
            return Some((start..i, word.to_string()));
        }
    }
    None
}

/// Resolve live configured cores belonging to one exact exchange inside the panel scope.
///
/// Args:
///     configured: Configured core ids paired with their window-group names.
///     live: Core ids with current sessions.
///     exchange_names: Reported exchange names keyed by core id.
///     group: Panel window group, or empty for the global scope.
///     exchange: Exact reported exchange name selected by the user.
///
/// Returns:
///     Core ids present in every membership input.
fn exchange_members_from<'a>(
    configured: impl IntoIterator<Item = (CoreId, &'a str)>,
    live: &HashSet<CoreId>,
    exchange_names: &HashMap<CoreId, String>,
    group: &str,
    exchange: &str,
) -> HashSet<CoreId> {
    configured
        .into_iter()
        .filter(|(id, configured_group)| {
            (group.is_empty() || *configured_group == group)
                && live.contains(id)
                && exchange_names.get(id).is_some_and(|name| name == exchange)
        })
        .map(|(id, _)| id)
        .collect()
}

/// Resolve the current exchange membership shared by rows, refreshes, and chart actions.
///
/// Args:
///     b: Backend containing config, live sessions, and reported exchange identities.
///     group: Panel window group, or empty for the global scope.
///     exchange: Exact reported exchange name selected by the user.
///
/// Returns:
///     Live configured core ids belonging to the selected exchange and panel scope.
pub(super) fn exchange_core_ids(b: &Backend, group: &str, exchange: &str) -> HashSet<CoreId> {
    let live: HashSet<CoreId> = b
        .session
        .sessions()
        .iter()
        .map(|session| session.id)
        .collect();
    let exchange_names = b.session.market_source().core_exchange_names();
    exchange_members_from(
        b.config
            .servers
            .iter()
            .map(|server| (server.id, server.group.as_str())),
        &live,
        &exchange_names,
        group,
        exchange,
    )
}

/// Build one deterministic signature from exact core membership and log revisions.
///
/// Args:
///     store: Current per-core data store.
///     core_ids: Selected core membership.
///
/// Returns:
///     An order-independent signature that changes when either membership or a selected revision
///     changes.
fn selected_core_log_sig(store: &CoreStore, core_ids: &HashSet<CoreId>) -> u64 {
    let mut core_ids: Vec<CoreId> = core_ids.iter().copied().collect();
    core_ids.sort_unstable();
    core_ids.into_iter().fold(0u64, |signature, id| {
        signature
            .wrapping_mul(31)
            .wrapping_add(id)
            .wrapping_mul(31)
            .wrapping_add(store.core(id).map_or(0, |core| core.log_rev))
    })
}

/// Resolve chart-search candidates without leaving the selected exchange membership.
///
/// A row target matching a selected core narrows the search to that core. Otherwise every selected
/// exchange member remains a fallback candidate in configured order.
///
/// Args:
///     configured: Configured core ids paired with display names.
///     members: Current selected-exchange membership.
///     target: Source label attached to the clicked aggregate log row.
///
/// Returns:
///     One exact target or all selected members, never a core outside `members`.
pub(super) fn exchange_chart_candidates<'a>(
    configured: impl IntoIterator<Item = (CoreId, &'a str)>,
    members: &HashSet<CoreId>,
    target: &str,
) -> Vec<CoreId> {
    let candidates: Vec<(CoreId, &'a str)> = configured
        .into_iter()
        .filter(|(id, _)| members.contains(id))
        .collect();
    candidates
        .iter()
        .find(|(_, name)| *name == target)
        .map(|(id, _)| vec![*id])
        .unwrap_or_else(|| candidates.into_iter().map(|(id, _)| id).collect())
}

/// Builds the selected live source's log-rebuild signature.
///
/// Aggregate and Exchange include exact core membership plus each selected `log_rev`, Local reads
/// the application ring revision, and a Core source reads only that core. An unchanged unrelated
/// source therefore cannot rebuild the active panel.
///
/// Args:
///     b: Backend providing live sessions, exchange identities, and log revisions.
///     group: Panel window group, or empty for the global scope.
///     source: Currently selected live log source.
///
/// Returns:
///     A deterministic rebuild signature scoped to the selected source.
pub(super) fn log_sig(b: &Backend, group: &str, source: &LogSource) -> u64 {
    let store = b.session.store();
    match source {
        LogSource::Aggregate => {
            let scoped = !group.is_empty();
            let core_ids = b
                .session
                .sessions()
                .iter()
                .filter(|s| !scoped || s.group == group)
                .map(|session| session.id)
                .collect();
            selected_core_log_sig(store, &core_ids)
        }
        LogSource::Exchange(exchange) => {
            selected_core_log_sig(store, &exchange_core_ids(b, group, exchange))
        }
        LogSource::Local => applog::revision(),
        LogSource::Core(id) => store.core(*id).map_or(0, |core| core.log_rev),
    }
}

/// Merge live logs from selected scoped core sources in chronological order.
///
/// Each core ring is already chronological, so a newest-first k-way merge needs at most
/// `VIEW_LIMIT` heap pops and clones only the retained output. Equal timestamps preserve the old
/// stable-sort order: source order first, then original row order within each source.
///
/// Args:
///     store: Current per-core data store.
///     sources: Canonically ordered scoped source entries.
///     allowed: Optional selected core membership; `None` includes every core source.
///
/// Returns:
///     At most `VIEW_LIMIT` newest rows across the allowed core sources.
pub(super) fn aggregate(
    store: &CoreStore,
    sources: &[LogSourceItem],
    allowed: Option<&HashSet<CoreId>>,
) -> Vec<LogLine> {
    let mut tails = Vec::new();
    for item in sources {
        if let LogSource::Core(id) = item.source {
            if allowed.is_some_and(|allowed| !allowed.contains(&id)) {
                continue;
            }
            if let Some(c) = store.core(id) {
                tails.push((item.display.as_str(), c.log.iter().rev().take(AGG_PER_CORE)));
            }
        }
    }

    let mut heads = vec![None; tails.len()];
    let mut heap = BinaryHeap::with_capacity(tails.len());
    for (source_ix, (_, tail)) in tails.iter_mut().enumerate() {
        if let Some(line) = tail.next() {
            heads[source_ix] = Some(line);
            heap.push((line.ts.as_str(), source_ix));
        }
    }

    let mut selected = Vec::with_capacity(VIEW_LIMIT);
    while selected.len() < VIEW_LIMIT {
        let Some((_, source_ix)) = heap.pop() else {
            break;
        };
        let line = heads[source_ix]
            .take()
            .expect("aggregate heap entry must have a matching source head");
        selected.push((source_ix, line));
        if let Some(next) = tails[source_ix].1.next() {
            heads[source_ix] = Some(next);
            heap.push((next.ts.as_str(), source_ix));
        }
    }
    selected.reverse();
    selected
        .into_iter()
        .map(|(source_ix, line)| {
            let mut line = line.clone();
            line.target = tails[source_ix].0.to_string();
            line
        })
        .collect()
}

#[cfg(test)]
/// Tests for render-ready log-line construction.
mod tests;
