//! One-shot measurement of what a merged chart archive actually contains.
//!
//! Two things about mini candles cannot be answered by reading code, and both decide whether they
//! are worth building a chart layer on:
//!
//! 1. **Reach.** Detailed trades and mini candles are separate rings filled from separate archive
//!    sections. If the minis reach further back than the trades, they are a history prefix worth
//!    drawing; if they do not, their only value is the buy/sell split.
//! 2. **Units.** `buy_vol` / `sell_vol` arrive as bare `f32` with no unit anywhere in the wire
//!    format. Base or quote changes every downstream calculation, and guessing it wrong is the kind
//!    of error that survives review because both readings produce plausible numbers.
//!
//! The first run answered reach and, in doing so, ruled out the obvious way to answer units: the
//! two rings do not overlap, so there is no shared window to sum over. Mini candles cover the
//! period before the detailed trades and meet them within seconds.
//!
//! That seam is NOT evidence of an archive-only prefix, and reading it that way was a mistake worth
//! recording. MoonProto keeps the mini ring flush against the trade ring for the whole session:
//! `MarketHistoryStore` buffers rows evicted from the trade ring and `compact_evicted_futures`
//! folds them into mini candles as they age out. The seam is therefore maintained continuously,
//! not delivered once.
//!
//! For the same reason the observed step is not the core "choosing" one. `MINI_CANDLE_SPLIT_MS` is
//! a fixed 5 seconds: a candle closes on the first trade more than 5 s after its anchor. A step
//! measured at 128 s means the market was quiet, not that the rows were coarsened — so the spacing
//! describes trade sparsity, and no statistic over it describes a grid.
//!
//! So units are settled by size-per-trade instead, which both sections carry independently. A mini
//! candle knows how many trades it covers (`cnt`) and the price range it covered them at, and the
//! detailed section gives the true average trade size nearby. Reading the mini volume as base and
//! as quote produces two candidate sizes; the one that lands near the real average wins. The two
//! readings differ by a factor of the price, so on any coin that is not priced near 1 the answer is
//! not close.
//!
//! Gated behind `MOON_ARCHIVE_PROBE=1`; off by default and separate from `MOON_MARKET_DIAG`, which
//! is already noisy enough that this line would be lost in it.

use moonproto::MoonClient;

/// Time span and row count of one retained ring.
struct Reach {
    rows: usize,
    first_ms: i64,
    last_ms: i64,
}

impl Reach {
    fn hours(&self) -> f64 {
        (self.last_ms - self.first_ms) as f64 / 3_600_000.0
    }
}

fn reach_of(times: &[i64]) -> Option<Reach> {
    let first = *times.first()?;
    let last = *times.last()?;
    Some(Reach {
        rows: times.len(),
        first_ms: first,
        last_ms: last,
    })
}

/// Where the retained 5-minute rows sit relative to the wall-clock 5-minute grid.
///
/// MoonProto stamps a sealed row at the moment it seals, not at a period boundary
/// (`history_store/derived.rs`), so nothing in the protocol guarantees these land on the grid. But
/// the ring is filled mostly by the core's snapshot, and whether THAT is grid-aligned decides
/// whether a chart can place these rows by subtracting a fixed period or has to treat their
/// position as approximate. The core is Delphi and unavailable to read, so the phase is measured:
/// a distribution clustered near 0 means a grid, a spread means a drifting anchor.
fn probe_candle_5m_phase(
    readers: &moonproto::state::MarketHistoryReaders,
    market: &str,
    core_label: &impl std::fmt::Display,
) {
    const PERIOD_MS: i64 = 300_000;
    let Some(reader) = readers.candles_5m.as_ref() else {
        return;
    };
    let mut rows = Vec::new();
    reader.copy_last(reader.capacity(), &mut rows);
    if rows.len() < 2 {
        return;
    }
    let mut on_grid = 0usize;
    let mut phase_sum = 0i64;
    let mut phase_min = i64::MAX;
    let mut phase_max = i64::MIN;
    let mut gaps_exactly_a_period = 0usize;
    let mut previous: Option<i64> = None;
    for row in &rows {
        let at = row.time().unix_millis();
        let phase = at.rem_euclid(PERIOD_MS);
        if phase == 0 {
            on_grid += 1;
        }
        phase_sum += phase;
        phase_min = phase_min.min(phase);
        phase_max = phase_max.max(phase);
        if let Some(prev) = previous {
            if at - prev == PERIOD_MS {
                gaps_exactly_a_period += 1;
            }
        }
        previous = Some(at);
    }
    log::info!(
        "core {core_label} 5m phase {market}: {} rows, {} exactly on the 5m grid, \
         phase min {} ms / mean {} ms / max {} ms, {} of {} gaps exactly one period",
        rows.len(),
        on_grid,
        phase_min,
        phase_sum / rows.len() as i64,
        phase_max,
        gaps_exactly_a_period,
        rows.len() - 1,
    );
}

/// Measure the merged archive for `market` and log one line.
///
/// `core_label` is taken as `Display` rather than a string so it keeps the lazy resolution
/// [`crate::feed::core_label`] was built for: the name is looked up when the line is formatted.
pub(super) fn probe(client: &MoonClient, market: &str, core_label: impl std::fmt::Display) {
    if std::env::var_os("MOON_ARCHIVE_PROBE").is_none() {
        return;
    }
    let Some(snapshot) = client.snapshot_versioned() else {
        return;
    };
    let Some(readers) = snapshot.market_history_readers(market) else {
        return;
    };
    probe_candle_5m_phase(&readers, market, &core_label);

    let mut trades = Vec::new();
    if let Some(reader) = readers.futures_trades.or(readers.spot_trades) {
        reader.copy_last(reader.capacity(), &mut trades);
    }
    let mut minis = Vec::new();
    if let Some(reader) = readers.mini_candles {
        reader.copy_last(reader.capacity(), &mut minis);
    }
    if trades.is_empty() || minis.is_empty() {
        log::info!(
            "core {core_label} archive probe {market}: trades {} minis {} — nothing to compare",
            trades.len(),
            minis.len()
        );
        return;
    }

    let trade_times: Vec<i64> = trades.iter().map(|row| row.time.unix_millis()).collect();
    let mini_times: Vec<i64> = minis.iter().map(|row| row.time.unix_millis()).collect();
    let (Some(trade_reach), Some(mini_reach)) = (reach_of(&trade_times), reach_of(&mini_times))
    else {
        return;
    };

    // The true average trade size, from the section that states quantity unambiguously.
    let mut trade_qty = 0.0f64;
    let mut price_sum = 0.0f64;
    let mut priced = 0usize;
    for row in &trades {
        trade_qty += f64::from(row.qty.abs());
        if row.price.is_finite() && row.price > 0.0 {
            price_sum += f64::from(row.price);
            priced += 1;
        }
    }
    let avg_trade_qty = trade_qty / trades.len() as f64;
    let trade_avg_price = if priced > 0 {
        price_sum / priced as f64
    } else {
        f64::NAN
    };

    // The same quantity as the mini candles would report it, under each reading. Prices come from
    // each candle's own range, so a coin that moved over the archived period is still weighted
    // correctly rather than against one global average.
    let mut mini_vol = 0.0f64;
    let mut mini_cnt = 0i64;
    let mut mini_base_if_quote = 0.0f64;
    let mut mini_price_sum = 0.0f64;
    let mut mini_priced = 0usize;
    for row in &minis {
        let vol = f64::from(row.buy_vol) + f64::from(row.sell_vol);
        mini_vol += vol;
        mini_cnt += i64::from(row.cnt);
        let mid = (f64::from(row.min_price) + f64::from(row.max_price)) / 2.0;
        if mid.is_finite() && mid > 0.0 {
            mini_base_if_quote += vol / mid;
            mini_price_sum += mid;
            mini_priced += 1;
        }
    }
    let mini_avg_price = if mini_priced > 0 {
        mini_price_sum / mini_priced as f64
    } else {
        f64::NAN
    };
    let size_if_base = mini_vol / mini_cnt.max(1) as f64;
    let size_if_quote = mini_base_if_quote / mini_cnt.max(1) as f64;

    // Compare on a log scale: the two readings differ by a factor of the price, and "which is
    // closer" only means something as a ratio, not as a difference.
    let distance = |candidate: f64| -> f64 {
        if candidate > 0.0 && avg_trade_qty > 0.0 {
            (candidate / avg_trade_qty).ln().abs()
        } else {
            f64::INFINITY
        }
    };
    let (d_base, d_quote) = (distance(size_if_base), distance(size_if_quote));
    let units = if !d_base.is_finite() && !d_quote.is_finite() {
        "unknown"
    } else if d_base < d_quote {
        "BASE (mini volume is coin quantity)"
    } else {
        "QUOTE (mini volume is quantity * price)"
    };

    // Where the two sections meet. A near-zero gap confirms they are consecutive rather than
    // overlapping, which is what makes them a single continuous history.
    let seam_s = (trade_reach.first_ms - mini_reach.last_ms) as f64 / 1000.0;
    log::info!(
        "core {core_label} archive probe {market}: \
         minis {} rows / {:.2}h then trades {} rows / {:.2}h = {:.2}h total, seam {:+.1}s, \
         mini step ~{:.1}s; avg trade size {:.6} (price {:.6}); mini size if base {:.6} \
         (x{:.3}), if quote {:.6} (x{:.3}), mini price {:.6} -> units {units}",
        mini_reach.rows,
        mini_reach.hours(),
        trade_reach.rows,
        trade_reach.hours(),
        mini_reach.hours() + trade_reach.hours(),
        seam_s,
        mini_reach.hours() * 3600.0 / mini_reach.rows.max(1) as f64,
        avg_trade_qty,
        trade_avg_price,
        size_if_base,
        size_if_base / avg_trade_qty,
        size_if_quote,
        size_if_quote / avg_trade_qty,
        mini_avg_price,
    );
}
