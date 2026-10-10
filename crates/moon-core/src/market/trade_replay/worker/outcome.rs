//! Replay series composition and outcome caching.

use super::*;

/// Build the frozen TICK series one tick stage answers with.
///
/// `ticks` must already be globally sorted ascending and clipped to the harvest's own
/// [`TickHarvest::covered`] range — this function does neither; [`serve_ticks`] does both before
/// calling it. `candles` is the EXCHANGE'S OWN klines carried forward from the candle stage that
/// ran first ([`TickStage::candles`]), never aggregated from `ticks`: the bar layer covers the
/// whole window even where the points, per `partial`, cover only part of it.
///
/// Args:
///     request: The request being served.
///     venue: Venue the ticks came from.
///     ticks: Trade points, ascending, already clipped to the harvest's covered range.
///     thinning: How [`fit_ticks_around`] fitted the points into the budget.
///     partial: Whether `ticks` covers only part of `request.window`.
///     covered: The walk's own exhaustive stretches, carried onto the series verbatim — the
///         chart withholds the bars lying inside them, and only this coverage knows that a
///         covered minute with no trade in it is still covered.
///     candles: The exchange klines to carry as the bar layer.
///
/// Returns:
///     The series to hand the chart.
///
/// `side_slots` is the per-second split of the SAME run before it was thinned
/// (`side_slots_of_ticks`), already merged and ascending; it is carried onto the series verbatim.
#[allow(clippy::too_many_arguments)]
pub(super) fn compose_ticks(
    request: &TradeReplayRequest,
    venue: crate::venue::Venue,
    ticks: Vec<Tick>,
    thinning: TickThinning,
    side_slots: Vec<crate::market::source::SideSlot>,
    partial: bool,
    covered: Coverage,
    candles: Vec<ChartCandle>,
) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::Ticks,
        venue,
        window: request.window,
        tf_ms: BAR_MS,
        candles,
        ticks,
        identity: request.identity,
        tick_status: TickStatus::Served,
        thinning,
        partial,
        side_slots,
        covered,
    }
}

/// Answer the rows one cache write may actually carry, keyed on the series it came from.
///
/// The SQLite isolation seam (acceptance criterion 7): a [`TradeReplaySource::Ticks`] series must
/// NEVER reach [`write_cached_bars`], because that table is the SHARED kline cache the live
/// recorder writes too. In practice no call site ever offers one this way — [`serve`] is the only
/// caller and always passes [`TradeReplaySource::Klines1m`], since [`serve_ticks`] writes nothing
/// back to SQLite at all — but the guard is keyed on the TYPE rather than on that fact, so the
/// invariant survives a future call site instead of depending on every one of them getting it
/// right by omission.
///
/// Args:
///     source: Which kind of series `rows` was built for.
///     rows: The candidate rows.
///
/// Returns:
///     `rows` unchanged for [`TradeReplaySource::Klines1m`]; an empty slice for
///     [`TradeReplaySource::Ticks`].
pub(crate) fn rows_for_cache(source: TradeReplaySource, rows: &[ChartCandle]) -> &[ChartCandle] {
    match source {
        TradeReplaySource::Klines1m => rows,
        TradeReplaySource::Ticks | TradeReplaySource::CoreTicks => &[],
    }
}

/// Build the frozen series one request answers with.
///
/// Args:
///     request: The request being served.
///     venue: Venue the rows came from.
///     rows: Bars in ascending open time.
///
/// Returns:
///     The series to hand the chart.
pub(super) fn compose(
    request: &TradeReplayRequest,
    venue: crate::venue::Venue,
    rows: Vec<ChartCandle>,
) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::Klines1m,
        venue,
        window: request.window,
        tf_ms: BAR_MS,
        candles: rows,
        ticks: Vec::new(),
        identity: request.identity,
        tick_status: TickStatus::Pending,
        thinning: TickThinning::Raw,
        partial: false,
        side_slots: Vec::new(),
        // No tick walk ran, so nothing is covered and the chart keeps every bar.
        covered: Coverage::none(),
    }
}

/// Look one window up in the in-memory outcome ring.
///
/// Args:
///     cache: The ring.
///     key: The question being asked.
///     identity: Discriminator the caller expects on the series it gets back.
///
/// Returns:
///     A ready series, or `None`.
pub(super) fn remember_lookup(
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    key: &OutcomeKey,
    identity: u64,
) -> Option<Remembered> {
    let cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hit = cache.iter().find(|(k, _)| k == key)?;
    Some(match hit.1.clone() {
        Remembered::Ready {
            mut series,
            ticks_settled,
        } => {
            // The identity belongs to the WINDOW that asked, not to the cached rows: two windows
            // on the same trade must not share a chart revision, or the second would be told
            // nothing changed and would draw nothing.
            series.identity = identity;
            Remembered::Ready {
                series,
                ticks_settled,
            }
        }
        Remembered::Empty => Remembered::Empty,
    })
}

/// Remember one answered window, evicting the oldest when full.
///
/// Two independent ceilings, both enforced oldest-first: [`OUTCOME_CACHE_LEN`] bounds the number
/// of entries, [`OUTCOME_CACHE_MAX_TICKS`] bounds their combined tick count. Neither ever evicts
/// the entry this call just inserted, so a single series alone can outrun the tick ceiling
/// without being immediately discarded.
///
/// A model's request is never remembered: the key names the window alone, so its answer —
/// walked lead-first and stopped on the page budget before the trail — would be served to the
/// next chart window on the same trade as settled, with no trail at all. Its prints are in the
/// tiles either way, which is where the next stage takes them from.
///
/// Args:
///     cache: The ring.
///     intent: Who asked; see [`ReplayIntent::reuses_answers`].
///     key: The question that was answered.
///     answer: What the venue said.
pub(super) fn remember_store(
    cache: &Mutex<VecDeque<(OutcomeKey, Remembered)>>,
    intent: ReplayIntent,
    key: OutcomeKey,
    answer: Remembered,
) {
    if !intent.reuses_answers() {
        return;
    }
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(k, _)| *k != key);
    cache.push_back((key, answer));
    while cache.len() > OUTCOME_CACHE_LEN {
        cache.pop_front();
    }
    while cache.len() > 1 && total_ticks(&cache) > OUTCOME_CACHE_MAX_TICKS {
        cache.pop_front();
    }
}

/// Sum the ticks carried by every remembered entry.
///
/// Args:
///     cache: The ring.
///
/// Returns:
///     Combined tick count across every entry.
pub(super) fn total_ticks(cache: &VecDeque<(OutcomeKey, Remembered)>) -> usize {
    cache
        .iter()
        .map(|(_, answer)| match answer {
            // The slots ride the same entry and are not bounded by the tick budget, so they
            // count toward the same cap.
            Remembered::Ready { series, .. } => series.ticks.len() + series.side_slots.len(),
            Remembered::Empty => 0,
        })
        .sum()
}

/// Read the window's bars from the shared kline cache, when it covers the window.
///
/// Args:
///     cache: The open cache, if the terminal supplied one.
///     request: The request being served.
///
/// Returns:
///     Bars covering the whole window, or `None` to fall through to the network.
pub(super) fn read_cached_bars(
    cache: Option<&KlineCache>,
    request: &TradeReplayRequest,
) -> Option<Vec<ChartCandle>> {
    let cache = cache?;
    // `read_range` answers `None` for a TIMEOUT and `Some(vec![])` for an authoritative empty, and
    // the two must never be conflated: folding a timeout into "the cache holds nothing" would send
    // a window to the network that the cache could have answered. One retry, then fall through.
    let rows = match cache.read_range(
        &request.address.exchange_key,
        &request.market,
        1,
        request.window.from_ms,
        request.window.to_ms,
    ) {
        Some(rows) => rows,
        None => {
            std::thread::sleep(Duration::from_millis(300));
            cache.read_range(
                &request.address.exchange_key,
                &request.market,
                1,
                request.window.from_ms,
                request.window.to_ms,
            )?
        }
    };
    match super::cache_covers(&rows, request.window, BAR_MS, MAX_GAP_BARS) {
        true => Some(rows),
        false => None,
    }
}

/// Merge freshly fetched bars into the shared kline cache.
///
/// Written under the REAL exchange key rather than a private one: these are genuine exchange
/// one-minute bars, indistinguishable from the recorder's, so every core on that venue benefits
/// and the second open of this trade costs no request even after a restart.
///
/// Args:
///     cache: The open cache, if the terminal supplied one.
///     request: The request being served.
///     rows: Bars to store; an empty set writes nothing.
pub(super) fn write_cached_bars(
    cache: Option<&KlineCache>,
    request: &TradeReplayRequest,
    rows: &[ChartCandle],
) {
    let (Some(cache), false) = (cache, rows.is_empty()) else {
        return;
    };
    cache.merge_batch(vec![MergeItem {
        exchange: request.address.exchange_key.clone(),
        market: request.market.clone(),
        kind_min: 1,
        rows: rows.to_vec(),
    }]);
}
