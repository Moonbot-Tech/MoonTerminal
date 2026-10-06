//! Wire candle conversion, retry gates and visible history fitting.

use super::*;

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
pub(super) fn wire_row_candle(
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
pub(super) fn deep_row_candle(
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
pub(super) fn snap5_row_candle(
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
pub(super) const HISTORY_RETRY_MIN_S: u32 = 30;
/// Upper bound of every history-retry delay in this file, in seconds.
pub(super) const HISTORY_RETRY_MAX_S: u32 = 600;
/// Minimum gap between two attempts at a cache read that timed out, in milliseconds.
///
/// The read itself gives up after 250 ms, and this block runs on the frame path, so an unthrottled
/// retry would ask a worker that is already busy again on the very next frame.
pub(super) const CACHE_RETRY_MS: u64 = 500;
/// Period of the core's automatic 5-minute snapshot ring, in milliseconds.
///
/// Named because it is used as a TIMESTAMP SHIFT rather than as a bucket width: rows in that ring
/// are stamped at the end of their period, so an open is one of these behind its stamp.
pub(super) const SNAP5_TF_MS: i64 = 300_000;
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
pub(super) const NATIVE_BACKFILL_MAX_ATTEMPTS: u32 = 5;

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
pub(super) fn native_backfill_due(state: Option<&NativeBackfillAttempt>, now: Instant) -> bool {
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
pub(super) struct NativeBackfillAttempt {
    /// When the claim was taken, which is when the request was about to be queued.
    pub(super) last_attempt: Instant,
    /// Seconds that must elapse before the key may be claimed again.
    pub(super) delay_s: u32,
    /// Claims taken so far for this key, against [`NATIVE_BACKFILL_MAX_ATTEMPTS`].
    pub(super) attempts: u32,
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
pub(crate) struct NativeBackfillGate {
    pub(super) claims: Mutex<HashMap<(CoreId, String, u32), NativeBackfillAttempt>>,
}

impl NativeBackfillGate {
    /// Take the send permit for `key`, or `None` when it is not due.
    ///
    /// Returns the backoff the gate will enforce before allowing a retry, so a failed-send
    /// diagnostic can name it. Claiming under the lock is what keeps N panels of one coin from all
    /// sending on the same frame: recording only the OUTCOME would leave every panel seeing an
    /// absent entry at once.
    pub(super) fn claim(&self, key: (CoreId, String, u32), now: Instant) -> Option<u32> {
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
    pub(crate) fn forget_provider(&self, provider: CoreId) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| *p != provider);
    }

    /// Drop the claims of every provider outside `keep`.
    pub(crate) fn retain_providers(&self, keep: &HashSet<CoreId>) {
        self.claims
            .lock()
            .expect("native backfill gate poisoned")
            .retain(|(p, _, _), _| keep.contains(p));
    }

    /// Drop every claim.
    pub(crate) fn clear(&self) {
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
pub(super) fn history_retry_next_delay_s(prev: Option<u32>) -> u32 {
    match prev {
        None => HISTORY_RETRY_MIN_S,
        Some(d) => d
            .max(HISTORY_RETRY_MIN_S)
            .saturating_mul(2)
            .min(HISTORY_RETRY_MAX_S),
    }
}

/// Map a CoinCard history timeframe in minutes to its MoonProto wire kind.
pub(super) fn deep_history_kind(tf_min: u32) -> DeepHistoryKind {
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
pub(super) fn visible_candle_fit(
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
pub(super) fn fixture_visible_fit(
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
