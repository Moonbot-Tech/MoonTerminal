//! Frozen fixture history reads and window revisions.

use super::*;

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
pub(super) fn read_fixture_history(
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
pub(super) fn fixture_revision(tf_ms: i64, from_ms: i64, to_ms: i64) -> u64 {
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
