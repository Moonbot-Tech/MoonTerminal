//! History floors, candle visibility and market window invalidation.

/// Whether the order-book instances must be rebuilt: new book data, a new view `render_range`
/// (compared bit for bit on the source value, so centre motion never jitters it), the view
/// leaving the window last emitted, or any move when the backend keeps no margin.
pub(super) fn book_instances_stale(
    last_rev: u64,
    last_emit: (f32, f32),
    last_render_range: f32,
    last_lo_hi: (f32, f32),
    rev: u64,
    render_range: f32,
    lo: f32,
    hi: f32,
    margin_price: f32,
) -> bool {
    last_rev != rev
        || last_render_range.to_bits() != render_range.to_bits()
        || !(lo >= last_emit.0)
        || !(hi <= last_emit.1)
        || (margin_price == 0.0 && last_lo_hi != (lo, hi))
}

/// Refit after a pixel of motion or any width/zoom change, without rescanning subpixel live motion.
pub(super) fn price_fit_window_changed(
    cached: Option<(f32, f32, f32)>,
    current: (f32, f32, f32),
) -> bool {
    cached.is_none_or(|(from, span, ppm)| {
        span != current.1 || ppm != current.2 || (from - current.0).abs() * current.2 >= 1.0
    })
}

/// Emergency candle kill switch. The presence of `MOON_CANDLES_OFF`, regardless of its value,
/// restores pure tick mode with crosses across the full window and an empty candle layer. Intended
/// for GPU/CPU A/B measurements.
pub(super) fn candles_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("MOON_CANDLES_OFF").is_some())
}

/// How often the arbitrage column re-reads its quotes, in milliseconds.
///
/// Not a rendering budget — the captions only repaint when a FORMATTED string changes — but a read
/// budget: the protocol hands the slots over one venue at a time, each behind the market lock, so a
/// column of twenty venues is twenty lock round trips. Four times a second is faster than the
/// reference terminal repaints the same column and far slower than a busy coin's revisions.
pub(super) const ARB_READ_PERIOD_MS: i64 = 250;

/// Whether a retained-history caption may read the source again.
///
/// The same clock as the arbitrage column and the volume captions: a busy coin revises several
/// times a second, and the derived snapshot walks retained trade buckets and the 5-minute candle
/// ring. A different market reads at once, so a switched coin does not keep the previous figures
/// for the rest of the period.
///
/// Args:
///     last_read_ms: Unix milliseconds of the previous read. Zero means never.
///     now_ms: Unix milliseconds of this sync.
///     last_market: Market the previous read was for.
///     market: Market this pane is showing.
///
/// Returns:
///     Whether the caller should call the source.
pub(super) fn retained_caption_due(
    last_read_ms: i64,
    now_ms: i64,
    last_market: &str,
    market: &str,
) -> bool {
    last_market != market || now_ms.saturating_sub(last_read_ms) >= ARB_READ_PERIOD_MS
}

/// One pane's window-delta read this sync.
pub(super) enum WindowPull<T> {
    /// The caption is off. Drop whatever the pane stored.
    Off,
    /// Inside the read period, on the same market. Keep the stored figures.
    Hold,
    /// A read just happened. `None` means the source had no snapshot.
    Read { market: String, value: Option<T> },
}

/// Store one window-delta pull.
///
/// A read replaces the stored figures, and a missing snapshot clears them even
/// when the market name did not change. Holding that `None` would leave the
/// previous percentages up after the provider dropped the coin. `Hold` is the
/// only arm that keeps the previous read.
///
/// Args:
///     figures: Stored readout. Cleared on `Off` and on a `None` read.
///     read_ms: Unix milliseconds of the previous read.
///     stored_market: Market that previous read was for.
///     pull: What this sync decided.
///     now_ms: Unix milliseconds of this sync.
pub(super) fn apply_window_pull<T>(
    figures: &mut Option<T>,
    read_ms: &mut i64,
    stored_market: &mut String,
    pull: WindowPull<T>,
    now_ms: i64,
) {
    match pull {
        WindowPull::Off => {
            if figures.is_some() || !stored_market.is_empty() {
                *figures = None;
                *read_ms = 0;
                stored_market.clear();
            }
        }
        WindowPull::Hold => {}
        WindowPull::Read { market, value } => {
            *figures = value;
            *read_ms = now_ms;
            *stored_market = market;
        }
    }
}

#[cfg(test)]
mod tests;

/// How often a pane asks the source for its horizontal-volume profile when nothing else moved.
///
/// The profile's window ends NOW, so on a quiet market rows slide out of it with no trade to
/// announce the change. Matched to the source's own slowest rebuild clock: asking faster costs a
/// lock and a compare for an answer that cannot differ.
pub(super) const HVOL_RECHECK_MS: i64 = 5_000;

/// Wall-clock span an opened chart asks back for, before the bar band clamps it.
const HISTORY_FLOOR_SPAN_MS: f64 = 2.0 * 86_400_000.0;
/// Fewest base candles the floor may resolve to. Keeps the coarse timeframes honest: two days is
/// two candles on a daily chart, which is the case where the symptom is worst.
const HISTORY_FLOOR_MIN_BARS: f64 = 120.0;
/// Most base candles the floor may resolve to. Keeps a 1-minute chart from rebuilding a
/// multi-thousand-bar series every time a bucket rolls over.
const HISTORY_FLOOR_MAX_BARS: f64 = 1500.0;

/// Minimum history span an open chart requests, regardless of camera zoom.
///
/// A chart opened at a few hours of zoom used to ask for only those few hours, so it started near
/// now and showed nothing of the history the local kline cache and the core's retained rings were
/// already holding; the user had to pan left before anything was fetched. The floor is really a BAR
/// COUNT — two days is only the tie-breaker inside the band — because a wall-clock span alone is
/// meaningless at both ends of the timeframe range.
///
/// It costs no exchange API weight. `from_rel_ms` reaches only the in-memory trade and candle
/// rings, the local SQLite prefix read and the series clip; every outbound `request_coin_card` is
/// gated on staleness, the effective-kind change, the per-panel backoff and the 30-second global
/// dedup, none of which reads it. A wider ask is the same one request.
///
/// Zero in pure tick mode, where there is no series to fill.
pub(super) fn chart_history_floor_ms(cfg: moon_core::market::CandleViewCfg) -> f32 {
    if candles_disabled() || cfg.mode == moon_core::market::candles::CANDLE_MODE_OFF {
        return 0.0;
    }
    let tf = cfg.tf_ms() as f64;
    HISTORY_FLOOR_SPAN_MS
        .max(HISTORY_FLOOR_MIN_BARS * tf)
        .min(HISTORY_FLOOR_MAX_BARS * tf) as f32
}

/// The shader boundary of the hide-candles zone, in milliseconds relative to the pane epoch.
///
/// Delegates to [`moon_core::market::candles::hide_zone_start_rel`], which owns the numeric
/// clamp and the Max step. Kept here so the chart sync and its tests share one name.
///
/// Args:
///     hide_candles: Hidden width, or the Max sentinel.
///     now_ms: Wall clock, Unix milliseconds.
///     tf_ms: Candle timeframe, milliseconds.
///     epoch_ms: Pane epoch, Unix milliseconds.
///     combo_left_rel: Oldest resident trade relative to the epoch, or NaN if none.
///
/// Returns:
///     Hide start relative to the epoch, or `f32::MAX` when candles stay everywhere.
pub(super) fn hide_start_rel(
    hide_candles: u16,
    now_ms: f64,
    tf_ms: i64,
    epoch_ms: f64,
    combo_left_rel: f32,
) -> f32 {
    moon_core::market::candles::hide_zone_start_rel(
        hide_candles,
        now_ms,
        tf_ms,
        epoch_ms,
        combo_left_rel,
    )
}
