//! Diagnostic channel for how deep each core's retained 5-minute candle ring actually is
//! (`channels.candles_5m` in `cfg/diagnostics.toml`), built to answer one question the sources
//! cannot: how many sealed 5-minute candles does a core really hold per market.
//!
//! moonproto keeps a `candles_5m` ring per market (capacity 500, about 41.6 hours). The core fills
//! it with a snapshot after `subscribe_all_trades`, for every market of the provider, and the
//! client extends it from the trade stream as periods seal. Two things about that snapshot are
//! runtime facts nothing in the code fixes:
//!
//! - **Depth.** The capacity is an upper bound. What a core actually sends — and whether a market
//!   it never charted gets anything at all — is only visible by reading the rings.
//! - **Shape.** A capture on 2026-07-12 found snapshot candles with `open == high` and
//!   `close == low` throughout, i.e. only the high and low carried, no real open/close. Whether
//!   that still holds is the `honest_oc` share below: a candle counts as honest when
//!   `open != high || close != low`. A genuinely monotone falling candle also reads dishonest, so
//!   the share is a signal across a whole ring, not a verdict on one candle.
//!
//! One line per elected provider on every pass of the kline recorder — about a minute apart, the
//! recorder's own loop; the first pass after start backfills and can take longer — appended to
//! `logs/candles_5m_diag.log`.
//! The scan only READS rings already in memory — it sends nothing to the core — and runs on the
//! kline recorder's own background thread, never on the UI thread. Each ring is folded under its
//! own read lock in one pass, without copying its rows out.

use moonproto::state::{Candle5mRow, SeqRingReader};

/// Milliseconds per hour, for the span figures.
const MS_PER_HOUR: f64 = 3_600_000.0;

/// Milliseconds per minute, for the age figures.
const MS_PER_MINUTE: f64 = 60_000.0;

/// Whether the channel is on at all. The off case is one relaxed atomic load.
pub fn enabled() -> bool {
    crate::diagnostics::candles_5m()
}

/// What one market's ring holds, accumulated candle by candle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingFacts {
    /// Candles in the ring.
    pub count: usize,
    /// Oldest candle stamp in unix milliseconds; meaningful only when `count > 0`.
    ///
    /// The minimum rather than the first row, so a ring that is not perfectly time-ordered still
    /// reports its true span.
    pub first_ms: i64,
    /// Newest candle stamp in unix milliseconds; meaningful only when `count > 0`.
    pub last_ms: i64,
    /// Candles whose open differs from the high or whose close differs from the low.
    pub honest: usize,
    /// Candles carrying a positive volume.
    pub with_volume: usize,
}

impl RingFacts {
    /// Fold one candle in.
    ///
    /// Args:
    ///     time_ms: The candle's stamp (sealed candles are stamped at the period END).
    ///     open, high, low, close, volume: The candle's fields as the ring holds them.
    pub fn add(&mut self, time_ms: i64, open: f32, high: f32, low: f32, close: f32, volume: f32) {
        if self.count == 0 {
            self.first_ms = time_ms;
            self.last_ms = time_ms;
        } else {
            self.first_ms = self.first_ms.min(time_ms);
            self.last_ms = self.last_ms.max(time_ms);
        }
        self.count += 1;
        if open != high || close != low {
            self.honest += 1;
        }
        if volume > 0.0 {
            self.with_volume += 1;
        }
    }

    /// Time covered by the ring, newest minus oldest stamp, in hours; zero for an empty ring.
    pub fn span_hours(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        (self.last_ms - self.first_ms) as f64 / MS_PER_HOUR
    }

    /// Age of the newest candle at `now_ms`, in minutes.
    ///
    /// Signed on purpose: a negative age is a core clock ahead of this machine's, which is worth
    /// seeing rather than clamping away.
    pub fn age_minutes(&self, now_ms: i64) -> f64 {
        (now_ms - self.last_ms) as f64 / MS_PER_MINUTE
    }
}

/// Read one ring's facts under a single read lock, folding rows in place.
///
/// The view is walked inside the reader's own closure, so no row is copied out and the lock is
/// held for one linear pass over at most the ring's capacity.
pub fn ring_facts(reader: &SeqRingReader<Candle5mRow>) -> RingFacts {
    reader.with_last(reader.capacity(), |view| {
        let mut facts = RingFacts::default();
        view.for_each(|c| {
            // moonproto stores `MoonTime::ZERO` for a row whose core time was unusable and keeps
            // the row in the ring; one such stamp would stretch the span and age to decades.
            if c.unix_millis() <= 0 {
                return;
            }
            facts.add(
                c.unix_millis(),
                c.open(),
                c.high(),
                c.low(),
                c.close(),
                c.volume(),
            );
        });
        facts
    })
}

/// Minimum, median, 90th percentile and maximum of a sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    pub min: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

/// Nearest-rank quantile of a sorted, non-empty sample: the smallest value with at least `q` of
/// the sample at or below it. Nearest-rank rather than interpolated, so every figure printed is a
/// value some market actually has.
fn nearest_rank(sorted: &[f64], q: f64) -> f64 {
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The spread of a sample, or `None` when it is empty.
///
/// Sorts `values` in place; NaN sorts last under `total_cmp` and so can only ever show as `max`.
pub fn spread(values: &mut [f64]) -> Option<Spread> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(Spread {
        min: values[0],
        median: nearest_rank(values, 0.5),
        p90: nearest_rank(values, 0.9),
        max: values[values.len() - 1],
    })
}

/// Share of `part` in `whole` as a percentage; zero for an empty whole.
pub fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    part as f64 * 100.0 / whole as f64
}

/// One provider's rings, accumulated market by market.
#[derive(Debug, Default)]
pub struct ProviderDepth {
    /// Every market of the provider's catalog.
    markets: usize,
    /// Markets whose history readers carry a 5-minute ring at all.
    with_ring: usize,
    /// Largest ring capacity seen, so the line says what "full" means.
    capacity: usize,
    /// Markets with a non-empty ring, by name.
    filled: Vec<(String, RingFacts)>,
}

impl ProviderDepth {
    /// Count a market whose readers carry no 5-minute ring (or no readers at all).
    pub fn add_without_ring(&mut self) {
        self.markets += 1;
    }

    /// Count a market with a ring.
    ///
    /// Args:
    ///     name: The market key.
    ///     capacity: The ring's capacity.
    ///     facts: What the ring holds.
    pub fn add_ring(&mut self, name: &str, capacity: usize, facts: RingFacts) {
        self.markets += 1;
        self.with_ring += 1;
        self.capacity = self.capacity.max(capacity);
        if facts.count > 0 {
            self.filled.push((name.to_string(), facts));
        }
    }

    /// Whether the provider's catalog was empty, which is "not arrived yet" rather than a reading.
    pub fn is_empty(&self) -> bool {
        self.markets == 0
    }

    /// The line's body, without the provider's caption.
    ///
    /// Args:
    ///     now_ms: Current unix time in milliseconds, for the age of the newest candle.
    pub fn summary(&self, now_ms: i64) -> String {
        let head = format!(
            "markets={} ring={} nonempty={} cap={}",
            self.markets,
            self.with_ring,
            self.filled.len(),
            self.capacity
        );
        if self.filled.is_empty() {
            return head;
        }
        let mut counts: Vec<f64> = self.filled.iter().map(|(_, f)| f.count as f64).collect();
        let mut spans: Vec<f64> = self.filled.iter().map(|(_, f)| f.span_hours()).collect();
        let mut ages: Vec<f64> = self
            .filled
            .iter()
            .map(|(_, f)| f.age_minutes(now_ms))
            .collect();
        let (candles, honest, with_volume) = self
            .filled
            .iter()
            .fold((0usize, 0usize, 0usize), |(n, h, v), (_, f)| {
                (n + f.count, h + f.honest, v + f.with_volume)
            });
        // Non-empty samples by construction; `filled` was checked above.
        let counts = spread(&mut counts).unwrap_or(ZERO_SPREAD);
        let spans = spread(&mut spans).unwrap_or(ZERO_SPREAD);
        let ages = spread(&mut ages).unwrap_or(ZERO_SPREAD);
        let examples = self
            .examples()
            .iter()
            .map(|(label, name, facts)| {
                format!(
                    "{label} {name} n={} span={:.1}h age={:.1}m honest={:.1}% vol={:.1}%",
                    facts.count,
                    facts.span_hours(),
                    facts.age_minutes(now_ms),
                    percent(facts.honest, facts.count),
                    percent(facts.with_volume, facts.count),
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "{head} candles={candles} count[min/med/p90/max]={:.0}/{:.0}/{:.0}/{:.0} \
             span_h[min/med/p90/max]={:.1}/{:.1}/{:.1}/{:.1} age_min[med/max]={:.1}/{:.1} \
             honest_oc={:.1}% vol>0={:.1}% || {examples}",
            counts.min,
            counts.median,
            counts.p90,
            counts.max,
            spans.min,
            spans.median,
            spans.p90,
            spans.max,
            ages.median,
            ages.max,
            percent(honest, candles),
            percent(with_volume, candles),
        )
    }

    /// The deepest, the median and the shallowest non-empty ring, by candle count.
    ///
    /// Ties are broken by span and then by name, so two sweeps over the same rings name the same
    /// markets and the examples can be followed across lines.
    fn examples(&self) -> Vec<(&'static str, &str, RingFacts)> {
        let mut order: Vec<&(String, RingFacts)> = self.filled.iter().collect();
        order.sort_by(|(an, af), (bn, bf)| {
            af.count
                .cmp(&bf.count)
                .then(af.span_hours().total_cmp(&bf.span_hours()))
                .then(an.cmp(bn))
        });
        let (Some(first), Some(last)) = (order.first(), order.last()) else {
            return Vec::new();
        };
        let mid = order[(order.len() - 1) / 2];
        vec![
            ("deepest", last.0.as_str(), last.1),
            ("median", mid.0.as_str(), mid.1),
            ("shallowest", first.0.as_str(), first.1),
        ]
    }
}

/// Placeholder for a spread over an empty sample; never printed, since the callers check first.
const ZERO_SPREAD: Spread = Spread {
    min: 0.0,
    median: 0.0,
    p90: 0.0,
    max: 0.0,
};

/// Write one provider's line.
///
/// Args:
///     caption: Provider id, venue and exchange key, already formatted.
///     depth: The accumulated reading.
///     now_ms: Current unix time in milliseconds.
pub fn record(caption: &str, depth: &ProviderDepth, now_ms: i64) {
    if !enabled() || depth.is_empty() {
        return;
    }
    crate::diagnostics::stamped_line(
        "candles_5m_diag.log",
        &format!("{caption} {}", depth.summary(now_ms)),
    );
}

/// Write one recorder pass's account of the core's rings kept in the candle cache: how many
/// markets offered rows, how many rows, and what the cache did with them — stored, left out under
/// a real candle, or already there. One line per pass, beside the per-provider depth lines; nothing
/// is written while the channel is off.
///
/// Args:
///     markets: Markets whose ring offered at least one row this pass.
///     rows: Rows offered in all.
///     fill: The cache's answer, summed over the pass's batches.
pub fn record_ring_fill(markets: usize, rows: usize, fill: crate::market::kline_cache::RangeFill) {
    if !enabled() {
        return;
    }
    crate::diagnostics::stamped_line(
        "candles_5m_diag.log",
        &format!(
            "ring->cache markets={markets} rows={rows} written={} kept_real={} unchanged={}",
            fill.written, fill.kept_real, fill.unchanged
        ),
    );
}

#[cfg(test)]
mod tests;
