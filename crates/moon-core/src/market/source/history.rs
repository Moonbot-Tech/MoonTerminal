//! Chart-history draining, retry, native backfill, and fixture-read implementation.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moonproto::DeepHistoryKind;

use crate::market::candles::ChartCandle;
use crate::session::CoreId;

use super::{
    CandleEmit, CandleReadParams, ChartHistoryBuffers, ChartHistoryCursor, ChartHistoryRead,
    MarketDataSource, drain_price_line, moon_time_from_rel_ms, price_rows_to_points, rows_to_ticks,
    trade_price_range,
};

/// Convert a bucket's OHLC plus a single wire volume figure into a chart candle, normalizing the
/// OHLC and splitting the volume into its base/quote pair. The ONLY place either adapter below
/// computes a volume pair, so `deep_row_candle` and `snap5_row_candle` cannot drift apart on it.
///
/// Args:
///     t_open_ms: Bucket open time, Unix milliseconds.
///     open, high, low, close: The bucket's OHLC, pre-normalization.
///     volume: The wire volume figure as the source reported it.
///     volume_is_quote: `true` when `volume` is quote-currency turnover (a Binance Futures row),
///         `false` when it is base-currency volume — see `crate::venue::deep_volume_is_quote`.
///
/// Returns:
///     The chart candle with normalized OHLC and both volume fields resolved.
fn wire_row_candle(
    t_open_ms: f64,
    open: f32,
    high: f32,
    low: f32,
    close: f32,
    volume: f32,
    volume_is_quote: bool,
) -> crate::market::candles::ChartCandle {
    let (open, high, low, close) = crate::market::candles::normalize_ohlc(open, high, low, close);
    let (volume, quote_volume) =
        crate::market::candles::split_wire_volume(volume, open, high, low, close, volume_is_quote);
    crate::market::candles::ChartCandle {
        t_open_ms,
        open,
        high,
        low,
        close,
        volume,
        quote_volume,
    }
}

/// Convert a retained CoinCard row into a chart candle.
///
/// Args:
///     r: The retained deep-history row.
///     volume_is_quote: Whether the core's `volume` slot for this row is quote-currency turnover
///         rather than base — resolved once per pass from the row's venue.
fn deep_row_candle(
    r: &moonproto::DeepPrice,
    volume_is_quote: bool,
) -> crate::market::candles::ChartCandle {
    wire_row_candle(
        r.unix_millis() as f64,
        r.open(),
        r.high(),
        r.low(),
        r.close(),
        r.volume(),
        volume_is_quote,
    )
}

/// Convert one 5-minute snapshot ring row into a chart candle.
///
/// Rows in this ring are stamped at the END of their period — moonproto seals a 5-minute candle
/// with the seal time, and the server's own snapshot is pushed in end-stamped as well. Shift back
/// one period so the open aligns with every other base, which is what `detect_snapshot` already
/// does for the same ring. Without it the whole snapshot layer sat one bucket late and its
/// boundary rows resampled into the wrong coarse bucket. This shift is unrelated to the volume
/// question below and must not move.
///
/// The ring row is a moonproto `DeepPrice` under a different wire shape (`Candle5mRow`, built by
/// `Candle5mRow::from_deep_price`): same Delphi record, same core, same `volume` field, so it
/// takes the same `volume_is_quote` flag as [`deep_row_candle`] rather than always being treated
/// as base.
///
/// Args:
///     r: The retained 5-minute snapshot ring row.
///     volume_is_quote: Whether this row's `volume` slot is quote-currency turnover.
fn snap5_row_candle(
    r: &moonproto::state::Candle5mRow,
    volume_is_quote: bool,
) -> crate::market::candles::ChartCandle {
    wire_row_candle(
        (r.time().unix_millis() - SNAP5_TF_MS) as f64,
        r.open(),
        r.high(),
        r.low(),
        r.close(),
        r.volume(),
        volume_is_quote,
    )
}

/// Lower bound of every history-retry delay in this file, in seconds.
///
/// Deliberately the same 30 seconds the effective-kind deep request starts from, so the two
/// request paths cannot beat each other into the core's API-limit auto-stop.
const HISTORY_RETRY_MIN_S: u32 = 30;
/// Upper bound of every history-retry delay in this file, in seconds.
const HISTORY_RETRY_MAX_S: u32 = 600;
/// Minimum gap between two attempts at a cache read that timed out, in milliseconds.
///
/// The read itself gives up after 250 ms, and this block runs on the frame path, so an unthrottled
/// retry would ask a worker that is already busy again on the very next frame.
const CACHE_RETRY_MS: u64 = 500;
/// Period of the core's automatic 5-minute snapshot ring, in milliseconds.
///
/// Named because it is used as a TIMESTAMP SHIFT rather than as a bucket width: rows in that ring
/// are stamped at the end of their period, so an open is one of these behind its stamp.
const SNAP5_TF_MS: i64 = 300_000;
/// Claims allowed per key before the backfill gives up for this client slot.
///
/// A budget is required, not merely tidy: the completion guard reads coarse depth out of the
/// snapshot and the cache, and a market that has no coarse depth to find — a young listing, or one
/// the core answers with an empty vector — can never satisfy it. Unbudgeted, such a key would keep
/// spending exchange weight at the 600-second cap forever. Five claims land at roughly 0, 30, 90,
/// 210 and 450 seconds, so the budget covers about seven and a half minutes of outage and the cap
/// is never actually reached on this path. That is sized against what a reconnect does rather than
/// against the outage alone: a dropped or replaced client slot clears the claims outright, so the
/// failure this exists for — a disconnected or momentarily busy core — gets a fresh budget exactly
/// when its cause clears.
const NATIVE_BACKFILL_MAX_ATTEMPTS: u32 = 5;

/// Whether a native backfill may be claimed now for a key in this state.
///
/// An absent entry has never been tried; a present one is due again once its own backoff elapses,
/// and never once it has spent its claim budget.
///
/// Success is deliberately NOT decided here: `request_coin_card` returns on QUEUEING, so no answer
/// at this call site can mean the backfill applied. The caller's `!have_native && !cache_covers`
/// guard is what observes the applied response and stops the requests for good; the budget is the
/// backstop for the markets that guard can never be satisfied for.
///
/// `now` is a parameter rather than an `Instant::now()` inside, so the decision is testable without
/// sleeping, and the comparison is saturating because the two instants come from separate clock
/// readings and a caller may legitimately hand back an earlier one.
fn native_backfill_due(state: Option<&NativeBackfillAttempt>, now: Instant) -> bool {
    match state {
        None => true,
        Some(a) => {
            a.attempts < NATIVE_BACKFILL_MAX_ATTEMPTS
                && now.saturating_duration_since(a.last_attempt)
                    >= Duration::from_secs(a.delay_s.max(HISTORY_RETRY_MIN_S) as u64)
        }
    }
}

/// One claimed native-backfill attempt; see [`NativeBackfillGate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeBackfillAttempt {
    /// When the claim was taken, which is when the request was about to be queued.
    last_attempt: Instant,
    /// Seconds that must elapse before the key may be claimed again.
    delay_s: u32,
    /// Claims taken so far for this key, against [`NATIVE_BACKFILL_MAX_ATTEMPTS`].
    attempts: u32,
}

/// Who may currently ask the core for a coarse-timeframe native backfill.
///
/// Shaped after [`super::archive::ArchiveGate`], which guards the analogous one-per-client archive
/// request: the state is private to the gate and the lifecycle points call methods on it rather
/// than reaching into a map and repeating the poison handling at each site.
///
/// A claim is taken BEFORE the request is queued, never after it lands. It cannot be the latter:
/// `request_coin_card` returns as soon as the request is queued, and whether it applied arrives
/// later as `CoinCardCandles::Updated` or `UpdateFailed`. So this gate only says "do not ask again
/// yet"; what says "stop asking" is the caller's own depth guard, which reads the applied response
/// out of the snapshot and the cache, backstopped by the claim budget for the markets that guard
/// can never be satisfied for.
///
/// The gate is PROCESS-GLOBAL rather than per panel — unlike [`super::ChartHistoryCursor`]'s
/// `deep_retry_delay_s` — because the request deliberately changes the core's shared timeframe
/// slot. A per-panel clock would multiply the attempt rate by the number of panels showing the
/// coin, which is exactly what the core's API-limit auto-stop punishes.
#[derive(Default)]
pub(super) struct NativeBackfillGate {
    claims: Mutex<HashMap<(CoreId, String, u32), NativeBackfillAttempt>>,
}

impl NativeBackfillGate {
    /// Take the send permit for `key`, or `None` when it is not due.
    ///
    /// Returns the backoff the gate will enforce before allowing a retry, so a failed-send
    /// diagnostic can name it. Claiming under the lock is what keeps N panels of one coin from all
    /// sending on the same frame: recording only the OUTCOME would leave every panel seeing an
    /// absent entry at once.
    fn claim(&self, key: (CoreId, String, u32), now: Instant) -> Option<u32> {
        let mut claims = self.claims.lock().expect("native backfill gate poisoned");
        let prev = claims.get(&key).copied();
        if !native_backfill_due(prev.as_ref(), now) {
            return None;
        }
        let delay_s = history_retry_next_delay_s(prev.map(|a| a.delay_s));
        claims.insert(
            key,
            NativeBackfillAttempt {
                last_attempt: now,
                delay_s,
                attempts: prev.map_or(1, |a| a.attempts + 1),
            },
        );
        Some(delay_s)
    }

    /// Drop every claim belonging to one provider, restoring its full budget.
    ///
    /// Called wherever the archive claims are forgotten: a replacement client slot has empty
    /// retained rings, so an old slot's backoff would only delay the first request the new one
    /// genuinely needs.
    pub(super) fn forget_provider(&self, provider: CoreId) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| *p != provider);
    }

    /// Drop the claims of every provider outside `keep`.
    pub(super) fn retain_providers(&self, keep: &HashSet<CoreId>) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| keep.contains(p));
    }

    /// Drop every claim.
    pub(super) fn clear(&self) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .clear();
    }
}

/// Next history-retry delay in seconds: 30 doubling to a 600 cap, floored on a fresh start.
///
/// ONE backoff shape for both history request paths in this file. The effective-kind deep request
/// and the coarse native backfill sit on different state — one per panel, one process-global — but
/// they answer to the same thing: the core spends exchange request weight on our behalf and stops
/// itself when it runs out. Two copies of this arithmetic drifted apart once already, the deep one
/// carrying bare literals where the named bounds belong.
///
/// The doubling SATURATES. Nothing in this file can currently reach a delay near `u32::MAX` — the
/// cap below is applied to every value that comes back out — but this is a total function now
/// serving two independent callers, and a plain `* 2` makes it panic in a debug build for an input
/// its own signature accepts. Saturating costs nothing here, because the cap discards the excess
/// either way.
fn history_retry_next_delay_s(prev: Option<u32>) -> u32 {
    match prev {
        None => HISTORY_RETRY_MIN_S,
        Some(d) => d
            .max(HISTORY_RETRY_MIN_S)
            .saturating_mul(2)
            .min(HISTORY_RETRY_MAX_S),
    }
}

/// Map a CoinCard history timeframe in minutes to its MoonProto wire kind.
fn deep_history_kind(tf_min: u32) -> DeepHistoryKind {
    match tf_min {
        1 => DeepHistoryKind::Min1,
        30 => DeepHistoryKind::Min30,
        60 => DeepHistoryKind::Hour1,
        240 => DeepHistoryKind::Hour4,
        1440 => DeepHistoryKind::Day1,
        _ => DeepHistoryKind::Min5,
    }
}

/// Fit the visible portion of retained fine and coarse candles, preserving any visible tick band.
/// History prefetch may contain extreme prices far off screen and must never reach this window.
fn visible_candle_fit(
    cursor: &ChartHistoryCursor,
    series_tf_ms: i64,
    window: (f64, f64),
    ticks: Option<(f32, f32)>,
) -> Option<(f32, f32)> {
    let mut range = ticks;
    let mut include = |lo: f32, hi: f32| {
        range = Some(match range {
            Some((a, b)) => (a.min(lo), b.max(hi)),
            None => (lo, hi),
        });
    };
    if let Some((lo, hi)) = cursor.candle_series.price_range(window.0, window.1) {
        include(lo, hi);
    }
    // `coarse_fill` is ascending by `t_open_ms` (`compose_with_coarse` rule 5), so only rows
    // from the first one whose widest possible bucket reaches the window up to the window's end
    // can intersect it.
    let fill = &cursor.coarse_fill;
    let start = cursor.coarse_fill_max_tf.map_or(0, |max_tf| {
        fill.partition_point(|(c, _)| c.t_open_ms + max_tf <= window.0)
    });
    let end = fill
        .partition_point(|(c, _)| c.t_open_ms <= window.1)
        .max(start);
    for (candle, tf) in &fill[start..end] {
        #[cfg(test)]
        VISIBLE_FIT_VISITED.with(|n| n.set(n.get() + 1));
        if f64::from(*tf) > series_tf_ms as f64
            && crate::market::candles::candle_intersects_window(
                candle.t_open_ms,
                f64::from(*tf),
                window.0,
                window.1,
            )
        {
            include(candle.low, candle.high);
        }
    }
    range
}

#[cfg(test)]
thread_local! {
    static VISIBLE_FIT_VISITED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Coarse-fill rows `visible_candle_fit` inspected on this thread since the last call; resets it.
#[cfg(test)]
#[allow(dead_code)] // read by the prover's before/after measurement
pub(crate) fn take_visible_fit_visited() -> u64 {
    VISIBLE_FIT_VISITED.with(|n| n.replace(0))
}

/// Fit the actual uploaded fixture bars, including partial boundary candles on cached reads.
fn fixture_visible_fit(
    candles: &[ChartCandle],
    tf_ms: i64,
    window: (f64, f64),
) -> Option<(f32, f32)> {
    candles
        .iter()
        .filter(|c| {
            c.low.is_finite()
                && c.high.is_finite()
                && c.high > 0.0
                && crate::market::candles::candle_intersects_window(
                    c.t_open_ms,
                    tf_ms as f64,
                    window.0,
                    window.1,
                )
        })
        .fold(None, |range: Option<(f32, f32)>, c| {
            Some(match range {
                Some((lo, hi)) => (lo.min(c.low), hi.max(c.high)),
                None => (c.low, c.high),
            })
        })
}

impl MarketDataSource {
    /// Read prefetched chart history while fitting prices only inside the independently supplied
    /// visible price window. `None` keeps the caller's cached fit without scanning retained rows.
    #[allow(clippy::too_many_arguments)]
    pub fn read_chart_history_into(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        from_rel_ms: f32,
        to_rel_ms: f32,
        force_reset: bool,
        price_window: Option<(f32, f32)>,
        candle_params: Option<&CandleReadParams>,
        cursor: &mut ChartHistoryCursor,
        out: &mut ChartHistoryBuffers,
    ) -> Option<ChartHistoryRead> {
        out.clear();
        // Fixture bench FIRST, and only then the live path. A bench process has no core and no
        // MoonProto client, so every guard below would decline and the chart would stay empty;
        // this branch cannot fire in a normal run because `fixture::active` is `None` unless the
        // process was started with `--fixture`, and even then only for the one market the bench
        // carries. The candle cache is the application's own — the bench copy IS the data root,
        // so opening a second one would mean a second connection and a second worker thread on
        // the very same file.
        if let Some(fixture) = crate::fixture::active() {
            if fixture.covers(market) {
                let cache = {
                    let inner = self.inner.read().expect("market source poisoned");
                    inner.kline_cache.clone()
                };
                let mut read = read_fixture_history(
                    fixture,
                    cache.as_ref(),
                    core,
                    epoch_ms,
                    from_rel_ms,
                    to_rel_ms,
                    candle_params,
                    out,
                );
                if read.candles_changed {
                    cursor.fixture_candles.clone_from(&out.candles);
                }
                read.tick_price_range = match (price_window, candle_params) {
                    (Some((from, to)), Some(params)) => fixture_visible_fit(
                        &cursor.fixture_candles,
                        params.tf_ms,
                        (epoch_ms + f64::from(from), epoch_ms + f64::from(to)),
                    ),
                    _ => None,
                };
                return Some(read);
            }
        }
        // The client's epoch is read HERE, under the guard that already resolves the client, and
        // carried down to the archive request: taking it again there would mean a second source
        // lock, a second client lookup and a third lock inside the slot, on the frame path.
        let (provider, client, client_epoch, archive) = {
            let inner = self.inner.read().expect("market source poisoned");
            let provider = inner.core_provider.get(&core).copied()?;
            let (client, epoch) = inner.clients.get(&provider)?.get_with_epoch()?;
            (provider, client, epoch, inner.archive.clone())
        };
        let snapshot = client.snapshot_versioned()?;
        let revision = client.snapshot_revision().unwrap_or(0);
        let readers = snapshot.market_history_readers(market)?;
        // This chart is open, so the core's accumulated archive for it is worth having. Asked for
        // AFTER the readers resolve, because the request is only legal for a market in the retained
        // trades scope — MoonProto re-checks that against its own fresh snapshot, so a refusal here
        // is still possible and is handled inside. One send per installed client; see `archive.rs`.
        archive.request(provider, market, &client, client_epoch);
        let from_time = moon_time_from_rel_ms(epoch_ms, from_rel_ms);
        let to_time = moon_time_from_rel_ms(epoch_ms, to_rel_ms.max(from_rel_ms + 1.0));
        // Read trade crosses and scans only from the last-K-candles display zone. INFINITY hides
        // trades entirely when K is zero; it does not constrain candle aggregation. Max
        // (`TRADES_FROM_UNBOUNDED`) is finite so trades stay on, and it is NOT raised to the
        // history window: the copy starts at the oldest retained row and the ring capacity
        // is the only clamp. The drawn edge is applied later, from the oldest row returned.
        let zone_unbounded = candle_params.is_some_and(|cp| {
            cp.trades_from_rel_ms == crate::market::candles::TRADES_FROM_UNBOUNDED
        });
        let display_trades = candle_params.map_or(true, |cp| cp.trades_from_rel_ms.is_finite());
        let trades_from_rel = if zone_unbounded {
            crate::market::candles::TRADES_FROM_UNBOUNDED
        } else {
            candle_params
                .map(|cp| cp.trades_from_rel_ms.max(from_rel_ms))
                .filter(|v| v.is_finite())
                .unwrap_or(from_rel_ms)
        };
        let zone_tf_ms = candle_params.map(|cp| cp.tf_ms).unwrap_or(1);
        let trades_from_time = moon_time_from_rel_ms(epoch_ms, trades_from_rel);
        let trades_limit = candle_params.map_or(usize::MAX, |cp| cp.trades_limit.max(1));
        let mut read = ChartHistoryRead {
            provider,
            revision,
            caught_up: true,
            ..ChartHistoryRead::default()
        };

        let trade_reader = readers.futures_trades.or(readers.spot_trades);
        if let Some(reader) = trade_reader.as_ref().filter(|_| display_trades) {
            read.combo_capacity = reader.capacity();
            let display_cap = reader.capacity().min(trades_limit);
            // Max follows the ring, not the clock. A full copy rebuilds the candle series
            // (`combo_reset` below), so it runs when the oldest retained trade leaves its
            // bucket or older history arrives — not on every live print.
            let mut reset = force_reset || cursor.trades.is_none();
            if !reset && zone_unbounded {
                reader.copy_from_cursor(
                    reader.cursor_from_oldest(),
                    1,
                    &mut cursor.scan_trade_rows,
                );
                let window_end_ms = to_time.unix_millis();
                let ring_oldest = cursor.scan_trade_rows.first().and_then(|row| {
                    let ms = row.unix_millis();
                    (ms < window_end_ms).then_some(ms)
                });
                if crate::market::candles::max_zone_recopy_due(
                    cursor.displayed_oldest_ms,
                    ring_oldest,
                    zone_tf_ms,
                ) {
                    reset = true;
                }
            }
            // Every full read parks the follow-up cursor at the first row PAST the window it just
            // copied, never at "now". The window's right edge sits behind now whenever the pane is
            // panned into the past — by hand, by wheel, or by a framing request — and a cursor
            // parked at now would skip every row between the two: the copy stops at `to_time`,
            // the drain starts at now, and nothing ever revisits the stretch in between, so the
            // chart shows a hole there until the next full read. Parking at `to_time` makes the
            // drain deliver that stretch itself; while following, `to_time` is ahead of now and
            // the two parkings coincide.
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    display_cap,
                    &mut cursor.trade_rows,
                );
                cursor.trades = Some(reader.cursor_at_or_after_time(to_time));
                read.combo_reset = true;
                read.caught_up = true;
                cursor.displayed_oldest_ms = cursor.trade_rows.first().map(|row| row.unix_millis());
                // A copy that filled its cap may have left rows out; the index then defers.
                cursor.price_fit.replace_from(
                    &cursor.trade_rows,
                    cursor.trade_rows.len() < display_cap,
                    trades_from_time.unix_millis(),
                );
            } else if let Some(cur) = cursor.trades.as_mut() {
                let meta = reader.drain_new_bounded(cur, display_cap, &mut cursor.trade_rows);
                read.clipped |= meta.clipped;
                read.caught_up &= meta.caught_up;
                if !meta.clipped {
                    // `trade_rows` holds only this drain's delta, and `copy_last` below may
                    // overwrite it, so the index takes it now.
                    cursor.price_fit.append(&cursor.trade_rows);
                    // Rows still waiting in the ring are rows the ring's copy would see.
                    if !meta.caught_up {
                        cursor.price_fit.mark_lagging();
                    } else if cursor.price_fit.needs_recopy() {
                        // Caught up again: one copy of everything since the window start, up to
                        // the ring's head (drains ran past `to_time`), re-arms the index.
                        reader.copy_time_range(
                            trades_from_time,
                            moonproto::MoonTime::from_unix_millis(i64::MAX),
                            display_cap,
                            &mut cursor.scan_trade_rows,
                        );
                        cursor.price_fit.replace_from(
                            &cursor.scan_trade_rows,
                            cursor.scan_trade_rows.len() < display_cap,
                            trades_from_time.unix_millis(),
                        );
                    }
                }
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
                        to_time,
                        display_cap,
                        &mut cursor.trade_rows,
                    );
                    cursor.trades = Some(reader.cursor_at_or_after_time(to_time));
                    read.combo_reset = true;
                    cursor.displayed_oldest_ms =
                        cursor.trade_rows.first().map(|row| row.unix_millis());
                    cursor.price_fit.replace_from(
                        &cursor.trade_rows,
                        cursor.trade_rows.len() < display_cap,
                        trades_from_time.unix_millis(),
                    );
                }
            }
            cursor.price_fit.cap_live(reader.capacity());
            cursor.price_fit.trim_before(trades_from_time.unix_millis());
            rows_to_ticks(&cursor.trade_rows, &mut out.ticks);
            read.combo_left_rel_ms = out
                .ticks
                .first()
                .map(|tick| (tick.time_ms - epoch_ms) as f32);
            if let Some(t) = out.ticks.last() {
                cursor.last_price = Some(t.price);
            } else if cursor.last_price.is_none() {
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
            if let Some((price_from, price_to)) = price_window {
                let from = moon_time_from_rel_ms(epoch_ms, price_from.max(trades_from_rel));
                let to = moon_time_from_rel_ms(epoch_ms, price_to);
                // The index answers while the window holds no more rows than the ring copy would
                // take; past the cap only the copy reproduces which rows it keeps.
                match cursor.price_fit.range(from.unix_millis(), to.unix_millis()) {
                    Some((range, n)) if n <= display_cap => read.tick_price_range = range,
                    _ => {
                        reader.copy_time_range(from, to, display_cap, &mut cursor.scan_trade_rows);
                        read.tick_price_range = trade_price_range(&cursor.scan_trade_rows);
                    }
                }
            }
        } else {
            cursor.trades = None;
            cursor.last_price = None;
            cursor.price_fit.clear();
            // Trades are hidden when K is zero, but the ring still supplies last_price. On reset,
            // combo_reset instructs the layer to clear its cross ring.
            if let Some(reader) = trade_reader.as_ref() {
                read.combo_capacity = reader.capacity();
                if force_reset {
                    read.combo_reset = true;
                }
                cursor.trade_rows.clear();
                reader.copy_last(1, &mut cursor.trade_rows);
                if let Some(row) = cursor.trade_rows.last() {
                    cursor.last_price = Some(row.price);
                }
            }
        }
        // Liquidations use a separate ring of the same type and stay synchronized with combo. A
        // full combo reset or first pass rereads the entire visible range; otherwise only the new
        // live edge is drained. The renderer tags them with side=2 for one shared color. Their
        // window matches normal trades: the last-K-candles zone.
        if let Some(reader) = readers.liquidations.as_ref().filter(|_| display_trades) {
            let reset = read.combo_reset || cursor.liquidations.is_none();
            if reset {
                reader.copy_time_range(
                    trades_from_time,
                    to_time,
                    reader.capacity(),
                    &mut cursor.liq_rows,
                );
                cursor.liquidations = Some(reader.cursor_at_or_after_time(to_time));
            } else if let Some(cur) = cursor.liquidations.as_mut() {
                let meta = reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.liq_rows);
                if meta.clipped {
                    reader.copy_time_range(
                        trades_from_time,
                        to_time,
                        reader.capacity(),
                        &mut cursor.liq_rows,
                    );
                    cursor.liquidations = Some(reader.cursor_at_or_after_time(to_time));
                }
            }
            rows_to_ticks(&cursor.liq_rows, &mut out.liquidations);
        } else {
            cursor.liquidations = None;
        }

        // The candle series combines the server's automatic 5-minute MoonProto snapshot, which
        // covers history before connection, with a local tail built from trades. Its own trade-ring
        // cursor keeps aggregation independent of the cross display zone. Only reset or timeframe
        // changes require a full rebuild; the live edge cheaply drains new rows.
        if let Some(cp) = candle_params {
            // CoinCard deep history applies only to timeframes of at least one minute. Sub-minute
            // candles are built from trades.
            let use_deep = cp.tf_ms >= 60_000;
            let native_kind_min =
                crate::market::candles::deep_kind_min_for_tf((cp.tf_ms / 60_000) as u32);
            // The core holds one candle timeframe per core, according to the MoonBot developer on
            // 2026-07-12. Alternating kinds between windows with different timeframes made the core
            // refetch exchange history for every request or subscription and could trigger API
            // limits. The provider's effective kind is the minimum live request because the kinds
            // divide into the chain 1|5|30|60|240|1440. Coarser panels resample the finer base at the
            // cost of depth; the core ring holds about 10,000 rows of the base timeframe.
            let deep_kind_min = if use_deep {
                let inner = self.inner.read().expect("market source poisoned");
                let mut wants = inner
                    .deep_kind_wants
                    .lock()
                    .expect("deep kind wants poisoned");
                let now_i = Instant::now();
                let m = wants.entry(provider).or_default();
                m.insert(native_kind_min, now_i);
                m.retain(|_, t| now_i.duration_since(*t) < Duration::from_secs(30));
                m.keys().copied().min().unwrap_or(native_kind_min)
            } else {
                native_kind_min
            };
            let deep_kind = deep_history_kind(deep_kind_min);
            // Pair the local kline-cache handle with the exchange key so cores on one exchange share
            // cached rows and provider election can change without changing the cache address.
            // CoreId itself is a stable uid since schema v11, but it identifies one core.
            let (kline_cache, exchange_key, deep_quote) = {
                let inner = self.inner.read().expect("market source poisoned");
                let exchange = inner.provider_exchange.get(&provider);
                (
                    inner.kline_cache.clone(),
                    exchange.map(|e| format!("{}:{:08x}", e.code, e.dex)),
                    exchange.is_some_and(|e| crate::venue::deep_volume_is_quote(e.code)),
                )
            };
            // Neither `cache_stale` nor `series_reset` below otherwise keys on the exchange key, so
            // a chart that first reads before the provider's `ExchangeId` is known — or whose
            // provider is later RE-ELECTED to a core on a different exchange key — would keep
            // reading/writing the STALE cache address forever, since `set_provider_exchanges` only
            // overwrites `provider_exchange` and invalidates nothing. Keyed on the ADDRESS
            // (`exchange_key`) rather than on the derived `deep_quote` bool: a bool cannot see a
            // spot provider's identity resolving (its `deep_quote` stays `false` throughout) or a
            // same-`deep_quote` provider election, both of which change the real cache address.
            // Comparing against the cursor's stored key turns either arrival into an explicit
            // transition: force a full cache re-read now, and fold the transition into
            // `series_reset` below so the composed series is rebuilt too. Note `None != Some(_)` is
            // also `true` on the very first pass — that reads as "changed" too, which is harmless
            // rather than incorrect: `cache_kind` is already `None` and `series_reset` is already
            // `true` via `!candle_series.is_valid()` on that same first pass.
            let exchange_key_changed = cursor.last_exchange_key != exchange_key;
            if exchange_key_changed {
                cursor.invalidate_cache_prefix();
            }
            // Subscribe to the core's live timeframe bars. Event::LiveCandle appends or replaces
            // the last retained tf_candles row. Without it, deep rows freeze at response time and
            // coarse-timeframe series can lag by hours. The subscription is global to the client
            // and the most recent kind wins, so a shared `(provider, market)` registry lets panels
            // refresh demand while entries stale for more than 60 seconds are unsubscribed.
            {
                let inner = self.inner.read().expect("market source poisoned");
                let mut subs = inner.candle_subs.lock().expect("candle subs poisoned");
                let now_i = Instant::now();
                if use_deep {
                    let entry = subs.entry((provider, market.to_string())).or_insert(
                        super::CandleSubState {
                            kind_min: deep_kind_min,
                            last_want: now_i,
                            subscribed: false,
                        },
                    );
                    if !entry.subscribed || entry.kind_min != deep_kind_min {
                        if client
                            .streams()
                            .subscribe_candles([market], deep_kind)
                            .is_ok()
                        {
                            entry.subscribed = true;
                            entry.kind_min = deep_kind_min;
                        }
                    }
                    entry.last_want = now_i;
                }
                // Remove stale subscriptions for this provider while its client is available.
                let stale: Vec<String> = subs
                    .iter()
                    .filter(|((p, _), s)| {
                        *p == provider
                            && s.subscribed
                            && now_i.duration_since(s.last_want) > Duration::from_secs(60)
                    })
                    .map(|((_, m), _)| m.clone())
                    .collect();
                for m in stale {
                    let _ = client.streams().unsubscribe_candles([m.as_str()]);
                    subs.remove(&(provider, m));
                }
            }
            // Load authoritative native klines from prior sessions as a local-cache prefix. Read
            // SQLite once per `(market, kind, left edge)` because pan and zoom reset frequently;
            // extending the window to the left triggers another read.
            // ONE left edge for the kline-cache read and the series build, with hysteresis so a pan
            // within a window span does not move it.
            let want_from_ms = (epoch_ms + (from_rel_ms - cp.tf_ms.max(0) as f32) as f64) as i64;
            let span_ms = ((to_rel_ms - from_rel_ms).max(0.0) as i64).max(cp.tf_ms.max(1));
            let series_floor_ms = series_floor(cursor.series_from_base_ms, want_from_ms, span_ms);
            if use_deep {
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                poll_cache_prefix(
                    cursor,
                    kline_cache.as_ref(),
                    exchange_key.as_deref(),
                    market,
                    native_kind_min,
                    series_floor_ms,
                    cp.tf_ms,
                    now_unix,
                );
            }
            // Backfill a coarse timeframe natively when the panel asks for a kind coarser than the
            // effective one-core timeframe and neither retained state nor the cache has native
            // depth. The core slot changes there and back as the freshness guard restores the
            // effective kind with backoff, while the response settles into the cache and avoids
            // future changes. Skip backfill without a cache because its result would be lost on
            // every restart while the slot changes remained.
            //
            // The request is deliberately NOT one-shot. It used to be, and a single disconnect then
            // killed that market's history for the whole process. It is instead claimed against a
            // bounded backoff with a claim budget, and stopped for good by the depth guard below
            // the moment the rows actually arrive.
            if use_deep && native_kind_min > deep_kind_min && kline_cache.is_some() {
                let native_kind = deep_history_kind(native_kind_min);
                let have_native = snapshot
                    .tf_candles(market, native_kind)
                    .map_or(false, |r| !r.is_empty());
                let cache_covers =
                    cursor.cache_rows_kind == native_kind_min && cursor.cache_rows.len() >= 30;
                if !have_native && !cache_covers {
                    let key = (provider, market.to_string(), native_kind_min);
                    let now_i = Instant::now();
                    // CLAIM the attempt under the locks, then send with both released. Two things
                    // ride on that order. The send is IPC to the core, and the sibling deep request
                    // below already refuses to make it under the source lock. And the claim is what
                    // keeps N panels of one coin from all sending on the same frame: recording only
                    // the OUTCOME would leave every panel seeing an absent entry at once, which is
                    // the herd the old unconditional insert prevented by accident.
                    let claimed = {
                        let inner = self.inner.read().expect("market source poisoned");
                        inner.native_backfill.claim(key, now_i)
                    };
                    // Nothing is written back after the send. `Ok` only means the core accepted the
                    // request into its queue; the outcome arrives asynchronously as a
                    // `CoinCardCandles` event, so marking the key done here would make an
                    // asynchronously failed or silently dropped request terminal. The guard above
                    // already stops the requests the moment the rows actually show up.
                    if let Some(delay_s) = claimed {
                        match client.candles().request_coin_card(market, native_kind) {
                            Ok(_) => log::log!(
                                super::SOURCE_TRACE_LEVEL,
                                "kline cache: native backfill queued {market} kind={native_kind:?}"
                            ),
                            Err(e) => super::market_diag(format!(
                                "native backfill request failed {market} kind={native_kind:?}: {e}; retrying in {delay_s}s"
                            )),
                        }
                    }
                }
            }
            // Native-backfill rows use the panel's native kind, which is coarser than the effective
            // kind, so they bypass the deep signature that tracks only the effective kind. Cache
            // them under a separate signature and force a prefix reread plus series rebuild so the
            // added depth appears immediately instead of in the next session.
            if use_deep && native_kind_min > deep_kind_min {
                if let (Some(cache), Some(ex)) = (kline_cache.as_ref(), exchange_key.as_ref()) {
                    let native_kind = deep_history_kind(native_kind_min);
                    if let Some(rows) = snapshot.tf_candles(market, native_kind) {
                        if !rows.is_empty() {
                            let sig = (rows.len() as u64).wrapping_mul(0x9e37_79b1)
                                ^ (rows.last().map_or(0, |r| r.unix_millis()) as u64);
                            if sig != cursor.cache_written_native_sig {
                                cursor.cache_written_native_sig = sig;
                                cache.merge(
                                    ex.clone(),
                                    market.to_string(),
                                    native_kind_min,
                                    rows.iter()
                                        .map(|r| deep_row_candle(r, deep_quote))
                                        .collect(),
                                );
                                // The merge enters the FIFO queue before the future read, so the
                                // prefix reread sees the new rows.
                                cursor.invalidate_cache_prefix();
                                cursor.candle_series.invalidate();
                            }
                        }
                    }
                }
            }
            // Check base freshness on every pass, not only reset. When K is zero and the trade zone
            // is disabled, no resets occur between configuration changes, so a lost or expired
            // response used to freeze the series forever. Stale means the current bucket has no
            // row; the live subscription must maintain it, and a gap means a missed response or
            // disconnect. The core fetches deep history from the exchange API and consumes request
            // weight, so retries without progress use exponential backoff from 30 seconds to 10
            // minutes. New rows or a kind change reset the delay. A fixed 30-second retry against a
            // silent core or exchange previously triggered the core's API-limit auto-stop.
            if use_deep {
                let base_tf_native_ms = deep_kind_min as i64 * 60_000;
                let rows = snapshot.tf_candles(market, deep_kind);
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let cur_bucket_ms =
                    crate::market::candles::bucket_open_ms(now_unix_ms, base_tf_native_ms);
                let deep_stale = rows
                    .and_then(|r| r.last())
                    .map_or(true, |r| (r.unix_millis() as f64) < cur_bucket_ms);
                let kind_changed = cursor.last_deep_kind != Some(deep_kind);
                let retry_delay =
                    Duration::from_secs(cursor.deep_retry_delay_s.max(HISTORY_RETRY_MIN_S) as u64);
                if deep_stale
                    && (kind_changed
                        || cursor
                            .last_deep_request
                            .map_or(true, |t| t.elapsed() > retry_delay))
                {
                    cursor.last_deep_request = Some(Instant::now());
                    cursor.last_deep_kind = Some(deep_kind);
                    // Add global deduplication above per-panel backoff. N panels for one coin share
                    // the retained response, so the application sends one `(coin, kind)` request
                    // every 30 seconds. A gated panel does not increase its backoff when another
                    // panel sent the request; its last_deep_request was already advanced above.
                    let gate_open = {
                        let inner = self.inner.read().expect("market source poisoned");
                        let mut gate = inner.deep_req_gate.lock().expect("deep req gate poisoned");
                        let key = (provider, market.to_string(), deep_kind_min);
                        let now_i = Instant::now();
                        match gate.get(&key) {
                            Some(t) if now_i.duration_since(*t) < Duration::from_secs(30) => false,
                            _ => {
                                gate.insert(key, now_i);
                                true
                            }
                        }
                    };
                    if gate_open {
                        cursor.deep_retry_delay_s = history_retry_next_delay_s(
                            (!kind_changed).then_some(cursor.deep_retry_delay_s),
                        );
                        if let Err(e) = client.candles().request_coin_card(market, deep_kind) {
                            super::market_diag(format!(
                                "coin-card request failed {market} kind={deep_kind:?}: {e}"
                            ));
                        }
                    }
                }
            }
            // Track a cheap deep-row fingerprint: row count plus the final timestamp. It detects a
            // new or advanced bucket, but not an in-place OHLC replacement at the same timestamp.
            // Such a replacement does not itself trigger rebuild or writeback; a trade-tail or
            // explicit reset can rebuild independently, while writeback waits for a later fingerprint
            // advance, normally a new bucket.
            let deep_rows_sig = if use_deep {
                snapshot.tf_candles(market, deep_kind).map_or(0u64, |rows| {
                    let last_ms = rows.last().map_or(0, |r| r.unix_millis());
                    (rows.len() as u64).wrapping_mul(0x9e37_79b1) ^ (last_ms as u64)
                })
            } else {
                0
            };
            if deep_rows_sig != cursor.last_deep_sig {
                // A response or live bar advanced the deep rows, so reset the request backoff.
                cursor.deep_retry_delay_s = HISTORY_RETRY_MIN_S;
            }
            let series_reset = series_reset_due(&SeriesResetInputs {
                params_reset: cp.series_reset,
                valid: cursor.candle_series.is_valid(),
                tf_changed: cursor.candle_series.tf_ms() != cp.tf_ms,
                trades_newly_available: cursor.candle_trades.is_none() && trade_reader.is_some(),
                deep_sig_changed: deep_rows_sig != cursor.last_deep_sig,
                exchange_key_changed,
                floor_moved: series_floor_ms != cursor.series_from_base_ms,
                cache_arrived: cursor.cache_generation != cursor.series_cache_generation,
            });
            if series_reset {
                cursor.series_from_base_ms = series_floor_ms;
                cursor.series_cache_generation = cursor.cache_generation;
                cursor.last_deep_sig = deep_rows_sig;
                cursor.last_exchange_key = exchange_key.clone();
                cursor.server_candle_rows.clear();
                cursor.server_candles.clear();
                // Keep the base's right edge at least at now rather than at the window's right edge.
                // A reset while scrolling into the past used to truncate the base at that window.
                // Returning live does not reset because it only extends left, leaving a gap in the
                // middle from the historical position to today's live bucket until another pan.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let to_ms = ((epoch_ms + to_rel_ms as f64) as i64).max(now_unix);
                // Base 1 is CoinCard deep history with authoritative OHLC for the effective
                // one-core timeframe. Base 2 is the free 5-minute range-only snapshot, used as a
                // prefix older than the deep part or as the entire base until deep history arrives.
                // The composite therefore has older range candles and newer authoritative candles.
                // Timeframes below five minutes cannot downsample the snapshot and do not use it.
                // One merge always converts the resulting base to the series timeframe.
                let base_tf_ms = cp.tf_ms;
                let mut deep_part: Vec<ChartCandle> = Vec::new();
                if let Some(rows) = snapshot.tf_candles(market, deep_kind).filter(|_| use_deep) {
                    deep_part.extend(
                        rows.iter()
                            .filter(|r| {
                                let t = r.unix_millis();
                                t >= series_floor_ms && t <= to_ms
                            })
                            .map(|r| deep_row_candle(r, deep_quote)),
                    );
                }
                let have_deep = !deep_part.is_empty();
                let use_snap5 = cp.tf_ms >= 300_000 && cp.tf_ms % 300_000 == 0;
                let mut snap_part: Vec<ChartCandle> = Vec::new();
                if use_snap5 {
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        let from5 = moonproto::MoonTime::from_unix_millis(series_floor_ms);
                        r5.copy_time_range(
                            from5,
                            moonproto::MoonTime::from_unix_millis(to_ms),
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        snap_part.extend(
                            cursor
                                .server_candle_rows
                                .iter()
                                .map(|r| snap5_row_candle(r, deep_quote)),
                        );
                        crate::market::candles::orient_range_rows(&mut snap_part);
                    }
                } else if cp.tf_ms < SNAP5_TF_MS {
                    // The same ring, kept as a COARSE FILL layer instead. A 1-minute series cannot
                    // resample a 5-minute bucket, so this ring contributes nothing to the base — but
                    // it is the only source that covers a stretch during which the CORE was running
                    // and the terminal was not, which is exactly the overnight hole a restart
                    // leaves. The local recorder cannot cover it: it aggregates from trade rings
                    // that only fill once this process is up.
                    cursor.ring_rows_5m.clear();
                    if let Some(r5) = readers.candles_5m.as_ref() {
                        r5.copy_time_range(
                            moonproto::MoonTime::from_unix_millis(series_floor_ms),
                            moonproto::MoonTime::from_unix_millis(to_ms),
                            r5.capacity(),
                            &mut cursor.server_candle_rows,
                        );
                        cursor.ring_rows_5m.extend(
                            cursor
                                .server_candle_rows
                                .iter()
                                .map(|r| snap5_row_candle(r, deep_quote)),
                        );
                        crate::market::candles::orient_range_rows(&mut cursor.ring_rows_5m);
                    }
                }
                // Use authoritative native klines from prior sessions as the visible cache portion,
                // and the finer cached kinds beside them to fill the native kind's holes.
                let in_window = |c: &&ChartCandle| {
                    let t = c.t_open_ms as i64;
                    t >= series_floor_ms && t <= to_ms
                };
                let cache_part: Vec<ChartCandle> = cursor
                    .cache_rows
                    .iter()
                    .filter(in_window)
                    .cloned()
                    .collect();
                let finer_part: Vec<ChartCandle> = cursor
                    .cache_rows_finer
                    .iter()
                    .filter(in_window)
                    .cloned()
                    .collect();
                // Write deep rows back to the cache without blocking when the cheap fingerprint
                // advances. Same-timestamp OHLC replacements do not advance it and remain unwritten
                // until a later bucket does. Persist the complete retained sequence rather than
                // `deep_part` clipped to the visible window; a narrow window used to save one
                // response candle and lose the remaining depth. Only the tail past the last written
                // bucket is converted unless the ring was replaced.
                if have_deep && deep_rows_sig != cursor.cache_written_sig {
                    match (kline_cache.as_ref(), exchange_key.as_ref()) {
                        (Some(cache), Some(ex)) => {
                            cursor.cache_written_sig = deep_rows_sig;
                            let rows = snapshot.tf_candles(market, deep_kind).unwrap_or_default();
                            let times: Vec<i64> = rows.iter().map(|r| r.unix_millis()).collect();
                            let same_key = cursor
                                .cache_written_key
                                .as_ref()
                                .is_some_and(|(k, m)| k == ex && *m == deep_kind_min);
                            let start = deep_writeback_start(
                                &times,
                                same_key,
                                cursor.cache_written_first_ms,
                                cursor.cache_written_last_ms,
                                cursor.cache_written_len,
                                cursor.cache_writebacks_since_full,
                            );
                            cursor.cache_writebacks_since_full = if start == 0 {
                                0
                            } else {
                                cursor.cache_writebacks_since_full.saturating_add(1)
                            };
                            let tail: Vec<ChartCandle> = rows[start..]
                                .iter()
                                .map(|r| deep_row_candle(r, deep_quote))
                                .collect();
                            cache.merge(ex.clone(), market.to_string(), deep_kind_min, tail);
                            cursor.cache_written_key = Some((ex.clone(), deep_kind_min));
                            cursor.cache_written_first_ms =
                                times.first().copied().unwrap_or(i64::MIN);
                            cursor.cache_written_last_ms =
                                times.last().copied().unwrap_or(i64::MIN);
                            cursor.cache_written_len = times.len();
                        }
                        (Some(_), None) => {
                            // The provider exchange identity is unavailable, so the cache cannot
                            // address these rows. Make this visible in the log once per panel. Keep
                            // the real signature unchanged, but use the initial marker to suppress
                            // repeated logging.
                            if cursor.cache_written_sig == 0 {
                                cursor.cache_written_sig = 1;
                                log::warn!(
                                    "kline cache: провайдер {provider} без ExchangeId — \
                                     ряды {market} не кэшируются"
                                );
                            }
                        }
                        _ => {}
                    }
                }
                // Merge every base source into the series timeframe in increasing priority:
                // 5-minute range-only snapshot < the native kind's holes filled from the finer
                // cached kinds < native cached klines < live, freshest deep history. The cached
                // parts carry the kind they were READ at, not this pass's native kind (see
                // `cache_rows_kind`). `merge_bases` skips a source whose timeframe is coarser
                // than or does not divide the target, such as a 5-minute snapshot for a
                // 1-minute series.
                {
                    use crate::market::candles::BasePart;
                    let cache_tf_ms = cursor.cache_rows_kind as i64 * 60_000;
                    crate::market::candles::merge_bases(
                        cp.tf_ms,
                        &[
                            BasePart {
                                rows: &snap_part,
                                tf_ms: SNAP5_TF_MS,
                            },
                            BasePart {
                                rows: &finer_part,
                                tf_ms: cache_tf_ms,
                            },
                            BasePart {
                                rows: &cache_part,
                                tf_ms: cache_tf_ms,
                            },
                            BasePart {
                                rows: &deep_part,
                                tf_ms: deep_kind_min as i64 * 60_000,
                            },
                        ],
                        &mut cursor.server_candles,
                    );
                }
                cursor.candle_trade_rows.clear();
                if let Some(reader) = trade_reader.as_ref() {
                    // Extend the series tail through now, which is already included in to_ms. The
                    // follow-up cursor starts at now, so the copied range must reach the same point
                    // or a permanent gap remains between them. The copy starts at the series floor
                    // so trade-built candles cover the same span as the base.
                    reader.copy_time_range(
                        moonproto::MoonTime::from_unix_millis(series_floor_ms),
                        moonproto::MoonTime::from_unix_millis(to_ms),
                        reader.capacity(),
                        &mut cursor.candle_trade_rows,
                    );
                    cursor.candle_trades = Some(
                        reader
                            .cursor_at_or_after_time(moonproto::MoonTime::from_unix_millis(to_ms)),
                    );
                } else {
                    cursor.candle_trades = None;
                }
                rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                cursor.candle_series.rebuild(
                    cp.tf_ms,
                    &cursor.server_candles,
                    base_tf_ms,
                    &cursor.candle_ticks,
                );
            } else if let (Some(reader), Some(cur)) =
                (trade_reader.as_ref(), cursor.candle_trades.as_mut())
            {
                let meta =
                    reader.drain_new_bounded(cur, reader.capacity(), &mut cursor.candle_trade_rows);
                if meta.clipped {
                    // The cursor fell behind the ring; force a full rebuild on the next pass.
                    cursor.candle_series.invalidate();
                } else if meta.copied > 0 {
                    rows_to_ticks(&cursor.candle_trade_rows, &mut cursor.candle_ticks);
                    cursor.candle_series.push_trades(&cursor.candle_ticks);
                }
            }
            read.candles_revision = cursor.candle_series.revision();
            let dirty = cursor.candle_series.take_dirty_from();
            // Compose the series with its cache-only coarser layers. The kind-5 layer comes from
            // the recorder and possible deep-history writeback, the daily layer from backfill and
            // cache; the retained `snap_part` separately feeds the main series. Fillers carry their
            // own timeframe for shader width and render muted against the selected one.
            //
            // Composed OUTSIDE the `candles_changed` branch on purpose: the auto-Y scan below runs
            // every frame while the upload runs only when the revision moved, so deriving the fill
            // in each of them is how the price scale and the drawn candles came to disagree. A
            // same-layout change is patched here too, so the auto-Y scan and the upload still share
            // one vector.
            let fill_key = (
                cursor.candle_series.layout_revision(),
                cursor.cache_generation,
            );
            if cursor.coarse_fill_key != Some(fill_key) {
                cursor.coarse_fill_key = Some(fill_key);
                recompose_coarse_fill(cursor, cp.tf_ms);
                cursor.candle_emit = CandleEmit::Full;
            } else if let Some(d) = dirty {
                match crate::market::candles::patch_composed_tail(
                    cursor.candle_series.candles(),
                    cp.tf_ms as f64,
                    d,
                    &mut cursor.coarse_fill,
                ) {
                    Some(fi) => {
                        cursor.candle_emit = match cursor.candle_emit {
                            CandleEmit::Full => CandleEmit::Full,
                            CandleEmit::Patch(p) => CandleEmit::Patch(p.min(fi)),
                            CandleEmit::Clean => CandleEmit::Patch(fi),
                        };
                    }
                    None => {
                        recompose_coarse_fill(cursor, cp.tf_ms);
                        cursor.candle_emit = CandleEmit::Full;
                    }
                }
            }
            read.candles_changed = cursor.candle_series.is_valid()
                && (read.candles_revision != cp.shipped_revision
                    || cursor.candle_emit != CandleEmit::Clean);
            if read.candles_changed {
                let from = match cursor.candle_emit {
                    CandleEmit::Patch(from)
                        if cursor.candles_emitted_rev == Some(cp.shipped_revision)
                            && from <= cursor.coarse_fill.len() =>
                    {
                        read.candles_patch_from = Some(from);
                        from
                    }
                    _ => {
                        read.candles_patch_from = None;
                        0
                    }
                };
                let shipped = &cursor.coarse_fill[from..];
                out.candles.reserve(shipped.len());
                out.candle_tf_ms.reserve(shipped.len());
                for (c, tf) in shipped {
                    out.candles.push(*c);
                    out.candle_tf_ms.push(*tf);
                }
                cursor.candle_emit = CandleEmit::Clean;
                cursor.candles_emitted_rev = Some(read.candles_revision);
                // Diagnose candle-to-now gaps. If the last candle is older than three timeframes,
                // log layer coverage once every 30 seconds so the exhausted layer is identifiable
                // as series, deep, cache, 5-minute, or 1-day instead of debugging screenshots.
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_millis() as f64);
                let last_ms = cursor
                    .coarse_fill
                    .last()
                    .map(|(c, _)| c.t_open_ms)
                    .unwrap_or(0.0);
                let warn_due = cursor
                    .last_gap_diag
                    .map_or(true, |t| t.elapsed() > Duration::from_secs(30));
                let scan_due = cursor
                    .last_gap_scan
                    .map_or(true, |t| t.elapsed() > Duration::from_secs(30));
                // Detect gaps inside the sequence where the next candle begins after the previous
                // one ends. This is where the scroll-to-history then return-to-live gap was hidden.
                let mut max_hole = 0.0f64;
                let mut hole_at = 0.0f64;
                if scan_due {
                    cursor.last_gap_scan = Some(Instant::now());
                    for w in cursor.coarse_fill.windows(2) {
                        let (prev, tf) = &w[0];
                        let prev_tf =
                            Some(*tf).filter(|t| *t > 0.0).unwrap_or(cp.tf_ms as f32) as f64;
                        let hole = w[1].0.t_open_ms - (prev.t_open_ms + prev_tf);
                        if hole > max_hole {
                            max_hole = hole;
                            hole_at = prev.t_open_ms + prev_tf;
                        }
                    }
                }
                if last_ms > 0.0
                    && (now_unix - last_ms > 3.0 * cp.tf_ms as f64
                        || max_hole > 3.0 * cp.tf_ms as f64)
                    && warn_due
                {
                    cursor.last_gap_diag = Some(Instant::now());
                    let ago_min = |ms: f64| ((now_unix - ms) / 60_000.0).round();
                    let span = |rows: &[ChartCandle]| match (rows.first(), rows.last()) {
                        (Some(f), Some(l)) => {
                            format!("{}м..{}м назад", ago_min(l.t_open_ms), ago_min(f.t_open_ms))
                        }
                        _ => "пусто".to_string(),
                    };
                    // The fill count is stated as "how many of the composed entries are NOT series
                    // candles", so a report distinguishes a fill that never ran from one that ran
                    // and fell short of the residual hole printed beside it.
                    let fill_n = cursor
                        .coarse_fill
                        .len()
                        .saturating_sub(cursor.candle_series.candles().len());
                    log::warn!(
                        "candle gap {market} tf={}с: последняя свеча {}м назад, макс. дыра \
                         {}м (кончается {}м назад); серия n={} \
                         [{}], заливка n={}, кэш kind{} n={} [{}], из finer n={} [{}], 5м n={} \
                         [{}], ринг5м n={} [{}], 1д n={} [{}]",
                        cp.tf_ms / 1000,
                        ago_min(last_ms),
                        (max_hole / 60_000.0).round(),
                        ago_min(hole_at + max_hole),
                        cursor.candle_series.candles().len(),
                        span(cursor.candle_series.candles()),
                        fill_n,
                        cursor.cache_rows_kind,
                        cursor.cache_rows.len(),
                        span(&cursor.cache_rows),
                        cursor.cache_rows_finer.len(),
                        span(&cursor.cache_rows_finer),
                        cursor.cache_rows_5m.len(),
                        span(&cursor.cache_rows_5m),
                        cursor.ring_rows_5m.len(),
                        span(&cursor.ring_rows_5m),
                        cursor.cache_rows_1d.len(),
                        span(&cursor.cache_rows_1d),
                    );
                }
            }
            if let Some((price_from, price_to)) = price_window {
                read.tick_price_range = visible_candle_fit(
                    cursor,
                    cp.tf_ms,
                    (
                        epoch_ms + f64::from(price_from),
                        epoch_ms + f64::from(price_to),
                    ),
                    read.tick_price_range,
                );
            }
        }

        if let Some(reader) = readers.last_prices {
            read.last_line = drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.last_prices,
                &mut cursor.last_price_rows,
                &mut out.last_points,
                &mut read,
                price_rows_to_points,
            );
        } else {
            cursor.last_prices = None;
        }

        if let Some(reader) = readers.mark_prices {
            read.mark_line = drain_price_line(
                &reader,
                from_time,
                to_time,
                force_reset,
                &mut cursor.mark_prices,
                &mut cursor.mark_price_rows,
                &mut out.mark_points,
                &mut read,
                price_rows_to_points,
            );
        } else {
            cursor.mark_prices = None;
        }

        read.last_price = cursor.last_price;
        Some(read)
    }
}

/// Serve one chart-history read from the frozen bench instead of a live core.
///
/// The bench has no trade ring, no price lines and no order book: it answers with the candle
/// series alone, which is what the marker, order-line and drawing-figure work needs to look at.
///
/// The revision is derived from what was ASKED FOR rather than from incoming data, because the
/// data never changes. A chart ships its last revision back in `shipped_revision` and expects an
/// empty series when nothing moved; with a constant revision, panning or switching the timeframe
/// would be answered with "nothing changed" and the chart would keep drawing its first window.
///
/// A repeat read of an already-served window still returns the price fields from memory. The
/// caller stores `last_price` and `tick_price_range` unconditionally, so answering it with empty
/// ones would wipe the chart's Y reference on the very next frame — and, because panning re-reads
/// every frame while the revision only moves once per timeframe bucket, that is the common case.
///
/// Args:
///     fixture: The bench installed for this process.
///     cache: The application's candle cache, already open on the bench copy.
///     core: Chart's core, echoed back as the provider so callers keep one identity.
///     epoch_ms: Chart epoch the relative bounds are measured from.
///     from_rel_ms: Visible-window start relative to the epoch.
///     to_rel_ms: Visible-window end relative to the epoch.
///     candle_params: Series request; `None` means the caller wants no candles.
///     out: Buffers to fill.
///
/// Returns:
///     A read describing the served window.
#[allow(clippy::too_many_arguments)]
fn read_fixture_history(
    fixture: &crate::fixture::ChartFixture,
    cache: Option<&crate::market::kline_cache::KlineCache>,
    core: CoreId,
    epoch_ms: f64,
    from_rel_ms: f32,
    to_rel_ms: f32,
    candle_params: Option<&CandleReadParams>,
    out: &mut ChartHistoryBuffers,
) -> ChartHistoryRead {
    let mut read = ChartHistoryRead {
        provider: core,
        caught_up: true,
        ..ChartHistoryRead::default()
    };
    let Some(params) = candle_params else {
        return read;
    };
    // A non-finite bound would convert to a saturated or zero timestamp and silently ask for the
    // wrong window; there is nothing sensible to draw for one, so decline it instead.
    if !epoch_ms.is_finite() || !from_rel_ms.is_finite() || !to_rel_ms.is_finite() {
        return read;
    }
    let from_ms = (epoch_ms + from_rel_ms as f64).round() as i64;
    let to_ms = (epoch_ms + to_rel_ms.max(from_rel_ms) as f64).round() as i64;
    let to_ms = to_ms.max(from_ms + 1);
    let revision = fixture_revision(params.tf_ms, from_ms, to_ms);
    read.revision = revision;
    read.candles_revision = revision;
    // Whether the CALLER already holds this series decides if the series is sent — not whether the
    // bench happens to remember serving it. A second pane, a new tab, or a candles off→on toggle
    // arrives with `shipped_revision` reset, and answering it from the bench's memory would hand it
    // "nothing changed" plus an empty series, leaving it with no candles at all.
    if params.shipped_revision == revision {
        if let Some((last_price, price_range)) = fixture.served_window(revision) {
            read.last_price = last_price;
            read.tick_price_range = price_range;
        }
        return read;
    }
    let Some(cache) = cache else {
        // Still claim the reset: `resident_left_rel` is stamped only inside the caller's
        // combo-reset branch, and without it every later frame forces a full history re-read.
        read.combo_reset = true;
        read.replace_price_lines();
        return read;
    };
    out.candles = fixture.candles(cache, params.tf_ms, from_ms, to_ms);
    read.candles_changed = true;
    // The caller stamps `resident_left_rel` — its "how far left is this pane covered" mark — ONLY
    // inside its combo-reset branch. Without this flag it stays NaN, which the caller reads as
    // "coverage unknown" and forces a full history reset on EVERY frame.
    read.combo_reset = true;
    read.replace_price_lines();
    read.last_price = out.candles.last().map(|c| c.close);
    // The chart's automatic Y fit is built from the TICK price range — candles do not feed it. A
    // bench has no trade ring, so leaving this empty collapses the scale onto the single last
    // price and the whole series sits off-screen, which reads as "the chart is empty". The served
    // window's own low/high is the honest equivalent of what the ticks in it would have spanned.
    read.tick_price_range = out
        .candles
        .iter()
        .filter(|c| c.low.is_finite() && c.high.is_finite() && c.high > 0.0)
        .fold(None, |acc: Option<(f32, f32)>, c| {
            Some(match acc {
                None => (c.low, c.high),
                Some((lo, hi)) => (lo.min(c.low), hi.max(c.high)),
            })
        });
    fixture.remember_window(revision, read.last_price, read.tick_price_range);
    // One line per process, and only for a bench run: "the chart is open" and "the bench actually
    // answered it" are different facts, and without this the difference is invisible from outside.
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();
    ANNOUNCED.call_once(|| {
        log::info!(
            "стенд {}: первая серия — {} свечей, ТФ {} мин, последняя цена {:?}",
            fixture.market(),
            out.candles.len(),
            params.tf_ms / 60_000,
            read.last_price
        );
    });
    read
}

/// Revision identifying one served bench window: timeframe plus the bucket-aligned bounds.
///
/// Aligning to the timeframe keeps the revision stable while a drag moves the window by less than
/// one candle, so an idle chart is not handed a fresh series on every frame.
fn fixture_revision(tf_ms: i64, from_ms: i64, to_ms: i64) -> u64 {
    let tf = tf_ms.max(1);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [tf, from_ms.div_euclid(tf), to_ms.div_euclid(tf)] {
        hash ^= value as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    // Zero is the default `shipped_revision` of a chart that has never been served; a window that
    // hashed to it would be answered with "nothing changed" on the very first read.
    hash.max(1)
}

#[cfg(test)]
mod tests;

/// Left edge the candle series should be built from. Keeps the current floor while the needed
/// edge stays inside `[floor, floor + 4 * span]`; otherwise re-anchors one span LEFT of the needed
/// edge, so a leftward pan re-anchors at most once per span of travel and a rightward pan never
/// does until the retained excess exceeds four spans.
pub(crate) fn series_floor(current: i64, want_from_ms: i64, span_ms: i64) -> i64 {
    if current != i64::MAX
        && want_from_ms >= current
        && want_from_ms.saturating_sub(current) <= span_ms.saturating_mul(4)
    {
        current
    } else {
        want_from_ms.saturating_sub(span_ms)
    }
}

/// Every reason the candle series is rebuilt. A camera pan or a trade/combo reset is not one.
pub(crate) struct SeriesResetInputs {
    pub params_reset: bool,
    pub valid: bool,
    pub tf_changed: bool,
    pub trades_newly_available: bool,
    pub deep_sig_changed: bool,
    pub exchange_key_changed: bool,
    pub floor_moved: bool,
    pub cache_arrived: bool,
}

/// Whether the candle series must be rebuilt this read.
pub(crate) fn series_reset_due(i: &SeriesResetInputs) -> bool {
    i.params_reset
        || !i.valid
        || i.tf_changed
        || i.trades_newly_available
        || i.deep_sig_changed
        || i.exchange_key_changed
        || i.floor_moved
        || i.cache_arrived
}

/// Tail-only deep writebacks between two full ones.
const DEEP_FULL_WRITEBACK_EVERY: u32 = 64;

/// Index of the first deep row still to write back. 0 = rewrite everything: nothing written yet
/// for this (exchange, kind), OLDER history arrived (the first row moved earlier), or fewer rows
/// than were written (a re-fetched response replaced rows). A ring that slid forward (first row
/// later, same or larger count) writes only the tail: the first row at or after the last written
/// bucket, which re-covers that bucket.
///
/// Accepted limit: a same-count correction of a middle row, or a merge the worker dropped, reaches
/// the cache only at the next full rewrite, at most `DEEP_FULL_WRITEBACK_EVERY` advances later.
pub(crate) fn deep_writeback_start(
    times: &[i64],
    same_key: bool,
    written_first: i64,
    written_last: i64,
    written_len: usize,
    since_full: u32,
) -> usize {
    if !same_key
        || since_full >= DEEP_FULL_WRITEBACK_EVERY
        || written_last == i64::MIN
        || times.first().map_or(true, |f| *f < written_first)
        || times.len() < written_len
    {
        0
    } else {
        times.partition_point(|t| *t < written_last)
    }
}

/// The one kline-cache prefix read a chart has in flight, with the key it was asked for.
pub(crate) struct PendingCacheRead {
    read: crate::market::kline_cache::PendingPrefixRead,
    exchange: String,
    market: String,
    kind: u32,
    need_from: i64,
}

/// Keeps the chart's kline-cache prefix current without ever waiting on the cache worker.
///
/// At most one read is in flight; its reply is installed on the frame that finds it. The rows held
/// stay drawn until then, so a read in flight never shows as an empty history. A read that did not
/// happen is retried no more often than every `CACHE_RETRY_MS`, as a timed-out read was.
#[allow(clippy::too_many_arguments)] // every argument is a separate read-key part; a struct would only rename them
pub(crate) fn poll_cache_prefix(
    cursor: &mut ChartHistoryCursor,
    cache: Option<&crate::market::kline_cache::KlineCache>,
    exchange: Option<&str>,
    market: &str,
    native_kind_min: u32,
    need_from: i64,
    tf_ms: i64,
    now_unix_ms: i64,
) {
    use crate::market::kline_cache::{PrefixPoll, PrefixReadRequest};

    // Rows of another exchange or market are wrong, not merely old: drop them at once.
    if let Some(ex) = exchange {
        let same = cursor
            .cache_identity
            .as_ref()
            .is_some_and(|(e, m)| e == ex && m == market);
        if !same {
            cursor.cache_want_from = None;
            cursor.drop_cache_rows();
            cursor.invalidate_cache_prefix();
            cursor.cache_identity = Some((ex.to_string(), market.to_string()));
        }
    }
    // A read of another kind: its reply would be discarded anyway; waiting only delays the right one.
    if cursor
        .cache_pending
        .as_ref()
        .is_some_and(|p| p.kind != native_kind_min)
    {
        cursor.cache_pending = None;
    }

    if let Some(pending) = cursor.cache_pending.as_ref() {
        match pending.read.poll() {
            PrefixPoll::Pending => {}
            PrefixPoll::Lost => {
                cursor.cache_pending = None;
                cursor.cache_retry_at = Some(Instant::now());
            }
            PrefixPoll::Ready(rows) => {
                let pending = cursor
                    .cache_pending
                    .take()
                    .expect("pending read polled above");
                let current = exchange == Some(pending.exchange.as_str())
                    && market == pending.market
                    && native_kind_min == pending.kind;
                if current {
                    // Where the native kind leaves a hole, the worker also read the finer kinds:
                    // the 1-minute deep-history rows and the recorder's 5-minute rows, over the
                    // same window, aggregated to the native kind right here. Every supported
                    // timeframe is divisible by both. Reading them only when the native kind was
                    // EMPTY left a holey native kind to the range-only snapshot, which draws as
                    // bodies without wicks (#634); a native kind without holes has nothing to fill
                    // and costs no extra read.
                    let native_tf_ms = pending.kind as i64 * 60_000;
                    cursor.cache_rows_finer.clear();
                    if !rows.finer.is_empty() {
                        let parts: Vec<crate::market::candles::BasePart<'_>> = rows
                            .finer
                            .iter()
                            .map(|(fk, r)| crate::market::candles::BasePart {
                                rows: r,
                                tf_ms: *fk as i64 * 60_000,
                            })
                            .collect();
                        crate::market::candles::merge_bases(
                            native_tf_ms,
                            &parts,
                            &mut cursor.cache_rows_finer,
                        );
                    }
                    cursor.cache_rows = rows.native;
                    cursor.cache_rows_kind = pending.kind;
                    // Cache-only coarser layers extending the historical prefix. Kind-5 rows come
                    // from the recorder and possible deep-history writeback; the retained 5-minute
                    // snapshot is merged separately through `snap_part`.
                    cursor.cache_rows_5m = rows.m5;
                    cursor.cache_rows_1d = rows.d1;
                    cursor.cache_generation = cursor.cache_generation.wrapping_add(1);
                    cursor.cache_kind = Some(pending.kind);
                    cursor.cache_from_ms = pending.need_from;
                    cursor.cache_retry_at = None;
                    if !cursor.cache_rows.is_empty() || !cursor.cache_rows_finer.is_empty() {
                        log::log!(
                            super::SOURCE_TRACE_LEVEL,
                            "kline cache: префикс {market} kind{}: {} рядов, из finer kinds: {}",
                            cursor.cache_rows_kind,
                            cursor.cache_rows.len(),
                            cursor.cache_rows_finer.len()
                        );
                    }
                }
            }
        }
    }

    let stale = cursor.cache_kind != Some(native_kind_min) || need_from < cursor.cache_from_ms;
    if !stale {
        return;
    }
    let want_from = cursor
        .cache_want_from
        .map_or(need_from, |w| w.min(need_from));
    cursor.cache_want_from = Some(want_from);
    // A read that did not happen must not be remembered as a completed one; retry it, but no more
    // often than every `CACHE_RETRY_MS` so a busy worker is not asked again on every frame.
    let retry_due = cursor.cache_retry_at.map_or(true, |t| {
        t.elapsed() >= Duration::from_millis(CACHE_RETRY_MS)
    });
    if cursor.cache_pending.is_some() || !retry_due {
        return;
    }
    match (cache, exchange) {
        (Some(cache), Some(ex)) => {
            let req = PrefixReadRequest {
                exchange: ex.to_string(),
                market: market.to_string(),
                native_kind: native_kind_min,
                from_ms: want_from,
                now_ms: now_unix_ms,
                want_5m: tf_ms < 300_000,
                want_1d: tf_ms < 86_400_000,
                done: cursor.cache_done.clone(),
            };
            match cache.request_prefix(req) {
                Some(read) => {
                    cursor.cache_pending = Some(PendingCacheRead {
                        read,
                        exchange: ex.to_string(),
                        market: market.to_string(),
                        kind: native_kind_min,
                        need_from: want_from,
                    });
                    cursor.cache_want_from = None;
                    // The coarse filler layers are drawn at their own timeframe, so ones held over
                    // from a finer chart must not fill this one's gaps while the read is in flight.
                    let mut dropped = false;
                    if tf_ms >= 300_000 && !cursor.cache_rows_5m.is_empty() {
                        cursor.cache_rows_5m.clear();
                        dropped = true;
                    }
                    if tf_ms >= 86_400_000 && !cursor.cache_rows_1d.is_empty() {
                        cursor.cache_rows_1d.clear();
                        dropped = true;
                    }
                    if dropped {
                        cursor.cache_generation = cursor.cache_generation.wrapping_add(1);
                    }
                }
                None => cursor.cache_retry_at = Some(Instant::now()),
            }
        }
        _ => {
            // No cache at all: nothing to retry, and the window IS loaded — as empty.
            cursor.drop_cache_rows();
            cursor.cache_kind = Some(native_kind_min);
            cursor.cache_from_ms = need_from;
            cursor.cache_want_from = None;
        }
    }
}

/// Recomposes the chart's series with its cache-only coarser layers into `coarse_fill`.
fn recompose_coarse_fill(cursor: &mut ChartHistoryCursor, series_tf_ms: i64) {
    let mut fill = std::mem::take(&mut cursor.coarse_fill);
    let mut layers: Vec<crate::market::candles::CoarseLayer<'_>> = Vec::new();
    // Order is PRIORITY: each layer's coverage is subtracted before the next is offered the
    // remainder. The local cache goes first because its rows are trade-derived with real OHLC; the
    // core's ring is range-only, so it fills what the cache could not — which after a restart is
    // most of the night.
    for (rows, tf) in [
        (&cursor.cache_rows_5m, 300_000.0f64),
        (&cursor.ring_rows_5m, 300_000.0f64),
        (&cursor.cache_rows_1d, 86_400_000.0f64),
    ] {
        // A layer finer than or equal to the series has nothing to add: those rows already reach
        // the series through `cache_part`/`snap_part` resampling, and re-adding them here would
        // draw every bucket twice.
        if (series_tf_ms as f64) >= tf {
            continue;
        }
        layers.push(crate::market::candles::CoarseLayer { rows, tf_ms: tf });
    }
    crate::market::candles::compose_with_coarse(
        cursor.candle_series.candles(),
        series_tf_ms as f64,
        &layers,
        &mut fill,
    );
    cursor.coarse_fill = fill;
    // A tail patch never changes which layers fed the fill, so only a recompose moves this bound.
    cursor.coarse_fill_max_tf = Some(
        layers
            .iter()
            // The fill stores each width as f32; bound by exactly what the filter reads.
            .map(|layer| f64::from(layer.tf_ms as f32))
            .fold(f64::from(series_tf_ms as f32), f64::max),
    );
}
