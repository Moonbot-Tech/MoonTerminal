//! Core archive capture and tile persistence.

use super::*;

/// What a close-time capture copies: the stretches the trade's own window asks for as ticks
/// ([`ReplayWindow::focus_spans`]), so a long position files only its two neighbourhoods — the
/// hours between them, which no window serves, stay out of the store's tick ceiling and off the
/// disk. Before the settle pass the trail has not printed yet, so the stretches end at the exit.
///
/// Args:
///     request: The trade and its core.
///     settle: Whether this is the settle pass, which includes the trail after the exit.
///
/// Returns:
///     The stretches to copy, ascending; one or two.
pub(super) fn capture_spans(request: &CaptureRequest, settle: bool) -> Coverage {
    let margin = request.margin_ms.max(0);
    let window = ReplayWindow {
        from_ms: request.open_ms.saturating_sub(margin),
        to_ms: request.close_ms.saturating_add(margin),
        open_ms: request.open_ms,
        close_ms: request.close_ms,
        margin_ms: margin,
        long_position_ms: request.long_position_ms,
        over_budget: false,
    };
    let spans = window.focus_spans();
    match settle {
        true => spans,
        false => spans.clip(&Coverage::one((window.from_ms, request.close_ms))),
    }
}

/// What the settle pass of a capture copies and when it is due, or `None` when there is nothing
/// to settle: the settle spans reach past the exit only when the margin gives the trade a trail,
/// and a margin of zero does not — scheduling a pass that copied the same stretch again would
/// schedule itself forever.
///
/// Args:
///     request: The trade and its core.
///
/// Returns:
///     The settle spans and the true-UTC millisecond they are due at (the trail's end plus
///     [`CAPTURE_SETTLE_SLACK`]).
pub(super) fn settle_plan(request: &CaptureRequest) -> Option<(Coverage, i64)> {
    let settle = capture_spans(request, true);
    let trail_end = settle.hull().map(|hull| hull.1)?;
    if trail_end <= request.close_ms {
        return None;
    }
    let due_ms = trail_end.saturating_add(CAPTURE_SETTLE_SLACK.as_millis() as i64);
    Some((settle, due_ms))
}

/// Copy `span` of one market out of the closing core's retained archive into the tile store and
/// its disk, as a [`TileSource::Core`] tile over what the archive actually held.
///
/// Silent when the archive holds nothing there — a market the core does not follow, or a ring
/// that has already moved past the span; the next window pages the venue as it always did.
///
/// Args:
///     request: The trade and its core.
///     span: The stretch to copy, inclusive, true-UTC milliseconds.
///     tiles: The worker's tile store.
pub(super) fn capture_from_core(
    request: &CaptureRequest,
    span: (i64, i64),
    tiles: &Mutex<TickTileStore>,
) {
    // Overlap, not bracket: the ring is contiguous, so whatever it holds inside the span is
    // exhaustive whatever its edges, and the copy is clipped to that — a ring lagging behind
    // the span's end at close time files up to its last print, and the settle pass files the
    // rest.
    file_core_span(
        "capture",
        &request.address,
        &request.market,
        span,
        tiles,
        |span| {
            request.address.history.capture_core_span(
                &request.address,
                &request.market,
                span.0,
                span.1,
                None,
            )
        },
    );
}

/// File what the core's ring holds inside each stretch of `spans` into the tile store and its
/// disk, as [`TileSource::Core`] tiles — the close-time capture's copy, run for a request whose
/// requester reads the tiles ([`ReplayIntent::files_core`]) before the stage decides what is
/// left for the venue. Both stores file only what they do not hold, so a stretch the capture
/// or an earlier session already filed costs nothing here, and a stretch the archive brings
/// AFTER this read is filed by the next request for the same focus, which finds the store
/// short and reads the ring again.
///
/// The tile rests on the same assumption as the close-time capture: the ring is contiguous
/// between its first and last print, so what it holds inside the span is exhaustive there. A
/// ring with a hole in it — a reconnect the feed did not backfill — would file the hole as
/// covered, for this path and for the capture alike; the hole check lives with the ring, not
/// here.
///
/// Args:
///     address: The core's exchange addressing.
///     market: Exchange-native market name.
///     spans: The stretches asked for as ticks, inclusive, true-UTC milliseconds.
///     tiles: The worker's tile store.
///     read: The ring copy of one stretch — the seam a test feeds without a core.
///
/// Returns:
///     The stretches actually filed, one per span the ring held something of.
pub(super) fn file_core_into_tiles(
    address: &ReplayAddress,
    market: &str,
    spans: &Coverage,
    tiles: &Mutex<TickTileStore>,
    read: impl Fn((i64, i64)) -> Option<CoreReplayTicks>,
) -> Vec<(i64, i64)> {
    spans
        .spans()
        .iter()
        .filter_map(|&span| file_core_span("backfill", address, market, span, tiles, &read))
        .collect()
}

/// Copy one stretch of a market out of a core's retained ring into the tile store and its
/// disk, as a [`TileSource::Core`] tile over what the ring actually held.
///
/// Silent when the ring holds nothing there — a market the core does not follow, or a ring
/// that has already moved past the span; the venue is asked as it always was.
///
/// Args:
///     what: The log's word for who is filing — `capture` at close time, `backfill` from a
///         tiles reader's stage — so the two mechanisms read apart in the log.
///     address: The core's exchange addressing.
///     market: Exchange-native market name.
///     span: The stretch to copy, inclusive, true-UTC milliseconds.
///     tiles: The worker's tile store.
///     read: The ring copy of the stretch.
///
/// Returns:
///     The stretch filed, clipped to what the ring held, or `None` when nothing was.
pub(super) fn file_core_span(
    what: &str,
    address: &ReplayAddress,
    market: &str,
    span: (i64, i64),
    tiles: &Mutex<TickTileStore>,
    read: impl Fn((i64, i64)) -> Option<CoreReplayTicks>,
) -> Option<(i64, i64)> {
    if span.0 > span.1 {
        return None;
    }
    let Some(native) = read(span) else {
        log::debug!(
            "[x] trade-replay {what} {market} span={}..{}: the core archive holds nothing there",
            span.0,
            span.1
        );
        return None;
    };
    let (from_ms, to_ms) = native.covered;
    if from_ms > to_ms {
        return None;
    }
    log::info!(
        "[x] trade-replay {what} {market} span={}..{}: {} prints from the core archive, covered={from_ms}..{to_ms}",
        span.0,
        span.1,
        native.ticks.len()
    );
    let key: TileKey = (address.exchange_key.clone(), market.to_string());
    if let Some(cache) = super::trade_cache::handle() {
        cache.insert(
            &address.exchange_key,
            market,
            from_ms,
            to_ms,
            native.ticks.clone(),
            TileSource::Core,
        );
    }
    let gained = !native.ticks.is_empty();
    lock_tiles(tiles).insert(key, from_ms, to_ms, native.ticks, TileSource::Core);
    // After the tiles hold them, whatever became of the file write: held queries read the
    // tiles, so a reader that refused this market asks again (`filed_since`).
    if gained {
        super::super::trade_cache::note_filed(&address.exchange_key, market);
    }
    Some((from_ms, to_ms))
}

/// The worker's tile store, poison-tolerant like the outcome ring: nothing inside a tile can be
/// half-written by a panic, so a poisoned lock still holds a consistent store.
pub(super) fn lock_tiles(tiles: &Mutex<TickTileStore>) -> std::sync::MutexGuard<'_, TickTileStore> {
    tiles
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
