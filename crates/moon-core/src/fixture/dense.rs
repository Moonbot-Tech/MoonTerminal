//! Generated chart history with a bounded retained ring and an explicitly advanced clock.
//! No market files or network are needed; both candles and crosses consume these same trades.

use std::collections::VecDeque;
use std::sync::Mutex;

use moonproto::{MoonTime, state::TradeHistoryRow};

use super::{ACTIVE, ChartFixture};
use crate::feed::{Side, Tick};
use crate::market::candles::{CandleSeries, ChartCandle, resample};

/// The default run starts full so its first appended batch also exercises ring eviction.
pub const CAPACITY: usize = 100_000;
/// Distinct catalog targets let the ordinary Main stack show both modes without bypassing dedup.
pub const MARKETS: [&str; 2] = ["DENSECANDLES", "DENSEHYBRID"];
/// Six hours fill the normal chart window with hundreds of candles and dense trades across its width.
const HISTORY_MS: i64 = 21_600_000;

/// One generated sequence, retained by the fixture rather than by any chart consumer.
pub struct DenseHistory {
    rows: VecDeque<TradeHistoryRow>,
    produced: u64,
    last_ms: i64,
    seed: u64,
    frozen: bool,
    candles: CandleSeries,
}

impl DenseHistory {
    /// Seed reproducible prices and volumes; only their absolute placement depends on `end_ms`.
    pub fn new(end_ms: i64, seed: u64, span_ms: i64) -> Self {
        let mut history = Self {
            rows: VecDeque::with_capacity(CAPACITY),
            produced: 0,
            last_ms: end_ms - span_ms,
            seed,
            frozen: false,
            candles: CandleSeries::default(),
        };
        history.candles.rebuild(60_000, &[], 60_000, &[]);
        history.advance(CAPACITY, span_ms);
        history
    }

    /// Append a bounded logical-time batch, independent of scheduler wake frequency.
    /// Calls are explicit so recovery can be checked without any replacement market data.
    pub fn advance(&mut self, count: usize, span_ms: i64) {
        assert!(count > 0 && span_ms > 0);
        let start = self.last_ms;
        for index in 0..count {
            self.seed = self
                .seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let seq = self.produced;
            // Integer waves, alternating sides and regular extremes expose lost ranges and tails.
            let price = 100.0 + (seq % 401) as f32 * 0.01 - 2.0
                + if seq.is_multiple_of(997) { 3.0 } else { 0.0 };
            let qty = 1.0 + ((self.seed >> 32) % 1000) as f32 * 0.01;
            let row = TradeHistoryRow {
                time: MoonTime::from_unix_millis(
                    start + (index + 1) as i64 * span_ms / count as i64,
                ),
                price,
                qty: if seq.is_multiple_of(2) { qty } else { -qty },
            };
            if self.rows.len() == CAPACITY {
                self.rows.pop_front();
            }
            self.rows.push_back(row);
            self.candles.push_trades(&[Tick {
                time_ms: row.unix_millis() as f64,
                price,
                qty,
                side: if row.is_buy() { Side::Buy } else { Side::Sell },
            }]);
            self.produced += 1;
        }
        self.last_ms += span_ms;
    }

    /// Number of generated trades, including those evicted from the ring.
    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// Retained rows actually available to consumers.
    pub fn retained(&self) -> usize {
        self.rows.len()
    }

    /// Current generated price for the synthetic book and header.
    pub fn last_price(&self) -> f32 {
        self.rows.back().map_or(100.0, |row| row.price)
    }

    /// Stop auxiliary synthetic book/news updates during the recovery oracle.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Whether the controlled scene must receive no new market payloads.
    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// Feed volume layers from the same trades without scanning the dense ring on every read.
    pub(super) fn candles(&self, tf_ms: i64, from_ms: i64, to_ms: i64) -> Vec<ChartCandle> {
        let tf_ms = tf_ms.max(60_000);
        let from_ms = from_ms.div_euclid(tf_ms) * tf_ms;
        let mut candles = Vec::new();
        resample(self.candles.candles(), tf_ms, &mut candles);
        candles.retain(|candle| {
            candle.t_open_ms >= from_ms as f64 && candle.t_open_ms <= to_ms as f64
        });
        candles
    }

    /// Completed candle history outlives trade-ring eviction, as it does for a live market.
    pub(crate) fn base_candles(&self) -> &[ChartCandle] {
        self.candles.candles()
    }

    /// Copy a consumer's delta, reporting eviction so it can rebuild from the retained rows.
    pub(crate) fn copy_since(&self, next: Option<u64>, out: &mut Vec<TradeHistoryRow>) -> bool {
        let oldest = self.produced - self.rows.len() as u64;
        let reset = next.is_none_or(|next| next < oldest || next > self.produced);
        let skip = if reset {
            0
        } else {
            (next.unwrap() - oldest) as usize
        };
        out.clear();
        out.extend(self.rows.iter().skip(skip).copied());
        reset
    }
}

/// Install a generated bench in a fresh throwaway root before any application worker starts.
pub(super) fn prepare() -> anyhow::Result<&'static ChartFixture> {
    let stamp = crate::util::now_unix_ms_i64();
    let root = std::env::temp_dir().join(format!(
        "moonterminal-fixture-dense-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir(&root)?;
    if !crate::config::paths::set_data_dir_override(root.clone()) {
        anyhow::bail!("data root was already selected before the dense fixture");
    }
    let history = DenseHistory::new(stamp - 1_000, 1, HISTORY_MS);
    let figures = MARKETS
        .into_iter()
        .map(|market| {
            super::figures::seed(
                1,
                market,
                super::figures::SeedWindow {
                    from_ms: (stamp - 1_000 - HISTORY_MS) as f64,
                    to_ms: (stamp - 1_000) as f64,
                    low: 98.0,
                    high: 105.0,
                },
            )
        })
        .sum();
    let fixture = ChartFixture {
        root,
        exchange: "200:".into(),
        market: MARKETS[1].into(),
        last_price: history.last_price(),
        shift_secs: 0,
        figures,
        served: Mutex::new(None),
        dense: Some(Mutex::new(history)),
    };
    ACTIVE
        .set(fixture)
        .map_err(|_| anyhow::anyhow!("fixture already installed"))?;
    Ok(ACTIVE.get().expect("fixture just installed"))
}

#[cfg(test)]
mod tests;
