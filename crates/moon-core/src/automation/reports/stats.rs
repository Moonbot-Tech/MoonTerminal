//! Metrics of a set of trades, computed in Rust over the filtered rows.
//!
//! Win rate, profit factor and drawdown come from the shared `metrics` formulas, so a figure here
//! matches the Analytics window and Telegram on the same trades.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::filter::Reason;
use super::trade::Trade;
use crate::db::metrics::Tally;

/// Every metric a client may ask for, by name, with the rounding its output gets.
pub const METRICS: &[(&str, Unit)] = &[
    ("trades", Unit::Count),
    ("wins", Unit::Count),
    ("losses", Unit::Count),
    ("winrate_pct", Unit::Pct),
    ("profit_usdt_sum", Unit::Usdt),
    ("profit_factor", Unit::Ratio),
    ("stoploss_share", Unit::Ratio),
    ("profit_usdt_avg", Unit::Usdt),
    ("profit_usdt_median", Unit::Usdt),
    ("gross_win_usdt", Unit::Usdt),
    ("gross_loss_usdt", Unit::Usdt),
    ("profit_pct_avg", Unit::Pct),
    ("profit_pct_median", Unit::Pct),
    ("max_win_usdt", Unit::Usdt),
    ("max_loss_usdt", Unit::Usdt),
    ("stoploss_count", Unit::Count),
    ("liquidations", Unit::Count),
    ("funding_usdt", Unit::Usdt),
    ("avg_hold_sec", Unit::Count),
    ("median_hold_sec", Unit::Count),
    ("volume_usdt", Unit::Usdt),
    ("max_consecutive_losses", Unit::Count),
    ("max_drawdown_usdt", Unit::Usdt),
    ("first_trade_ms", Unit::Count),
    ("last_trade_ms", Unit::Count),
    ("unvalued", Unit::Count),
];

/// Metrics an answer carries when the client names none.
pub const DEFAULT_METRICS: &[&str] = &[
    "trades",
    "winrate_pct",
    "profit_usdt_sum",
    "profit_factor",
    "stoploss_share",
];

/// How a metric is rounded on output; comparisons always use the exact value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Count,
    Usdt,
    Pct,
    Ratio,
}

/// Check names a `having` or a `sort` compares: numeric metrics only, so `native_profit`, an
/// object per currency, is refused rather than silently dropping every group.
///
/// Errors:
///     An unknown or non-numeric metric.
pub fn numeric_metric(name: &str) -> Result<()> {
    if name == "native_profit" {
        bail!("native_profit is per currency and cannot be compared; use profit_usdt_sum");
    }
    metric_names(&[name.to_string()]).map(drop)
}

/// Check client metric names against [`METRICS`], `native_profit` included.
///
/// Errors:
///     An unknown name, listing the known ones so the client need not probe.
pub fn metric_names(asked: &[String]) -> Result<Vec<String>> {
    if asked.is_empty() {
        return Ok(DEFAULT_METRICS.iter().map(|m| m.to_string()).collect());
    }
    for name in asked {
        if name != "native_profit" && !METRICS.iter().any(|(m, _)| m == name) {
            let known: Vec<_> = METRICS.iter().map(|(m, _)| *m).collect();
            bail!(
                "unknown metric {name}; known: {}, native_profit",
                known.join(", ")
            );
        }
    }
    Ok(asked.to_vec())
}

/// The figures of one set of trades.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    /// Trades, Funding rows excluded: Funding is money, not a trade.
    pub trades: i64,
    pub wins: i64,
    /// Trades without a USDT rate; while any exist, drawdown is withheld.
    pub unvalued: i64,
    /// Valued trades in close order: USDT sums, profit factor, drawdown.
    pub tally: Tally,
    pub funding_usdt: f64,
    pub funding_rows: i64,
    /// Funding rows without a USDT rate: left out of the USDT sums; the `unvalued` METRIC adds
    /// them, while the field above, which withholds drawdown, counts trades only.
    pub funding_unvalued: i64,
    usdt: Vec<f64>,
    pct: Vec<f64>,
    hold: Vec<f64>,
    pub stoploss: i64,
    pub liquidations: i64,
    pub volume_usdt: f64,
    pub max_win: Option<f64>,
    pub max_loss: Option<f64>,
    pub max_losing_run: i64,
    run: i64,
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
    /// Result per currency in the trades' own money, Funding included.
    pub native: BTreeMap<&'static str, f64>,
}

impl Stats {
    /// Add one trade; trades must arrive in close order, as `load` returns them.
    pub fn push(&mut self, trade: &Trade) {
        if let Some(currency) = trade.currency {
            *self.native.entry(currency).or_default() += trade.profit_native;
        }
        if trade.reason == Reason::Funding {
            self.funding_rows += 1;
            // Unknown is never zero: an unvalued Funding row is counted, not summed.
            match trade.profit_usdt {
                Some(usdt) => self.funding_usdt += usdt,
                None => self.funding_unvalued += 1,
            }
            return;
        }
        self.trades += 1;
        // The sign is read in the trade's own money, so an unvalued trade still counts as a win
        // or a loss; zero is a loss, as in every other report aggregate.
        if trade.profit_native > 0.0 {
            self.wins += 1;
            self.run = 0;
        } else {
            self.run += 1;
            self.max_losing_run = self.max_losing_run.max(self.run);
        }
        match trade.profit_usdt {
            Some(usdt) => {
                self.tally.push(usdt);
                self.usdt.push(usdt);
                // The biggest win among the wins and the biggest loss among the losses: a set
                // that never lost has no largest loss.
                if usdt > 0.0 {
                    self.max_win = Some(self.max_win.map_or(usdt, |m| m.max(usdt)));
                } else {
                    self.max_loss = Some(self.max_loss.map_or(usdt, |m| m.min(usdt)));
                }
            }
            None => self.unvalued += 1,
        }
        if let Some(pct) = trade.profit_pct {
            self.pct.push(pct);
        }
        if let Some(hold) = trade.hold_sec {
            self.hold.push(hold as f64);
        }
        match trade.reason {
            Reason::Stoploss => self.stoploss += 1,
            Reason::Liquidation => self.liquidations += 1,
            _ => {}
        }
        self.volume_usdt += trade.volume_usdt.unwrap_or(0.0);
        self.first_ms = Some(
            self.first_ms
                .map_or(trade.close_ms, |f| f.min(trade.close_ms)),
        );
        self.last_ms = Some(
            self.last_ms
                .map_or(trade.close_ms, |l| l.max(trade.close_ms)),
        );
    }

    /// The exact value of one metric; `None` where it does not exist for this set.
    pub fn value(&self, metric: &str) -> Option<f64> {
        let n = self.trades;
        let ratio = |a: i64| (n > 0).then(|| a as f64 / n as f64);
        Some(match metric {
            "trades" => n as f64,
            "wins" => self.wins as f64,
            "losses" => (n - self.wins) as f64,
            "winrate_pct" => crate::db::metrics::winrate(self.wins, n),
            "profit_usdt_sum" => self.tally.profit + self.funding_usdt,
            "profit_factor" => self.tally.profit_factor(),
            "stoploss_share" => return ratio(self.stoploss),
            "profit_usdt_avg" => return (self.tally.n > 0).then(|| self.tally.avg()),
            "profit_usdt_median" => return median(&self.usdt),
            "gross_win_usdt" => self.tally.wsum,
            "gross_loss_usdt" => -self.tally.lsum,
            "profit_pct_avg" => {
                return (!self.pct.is_empty())
                    .then(|| self.pct.iter().sum::<f64>() / self.pct.len() as f64);
            }
            "profit_pct_median" => return median(&self.pct),
            "max_win_usdt" => return self.max_win,
            "max_loss_usdt" => return self.max_loss,
            "stoploss_count" => self.stoploss as f64,
            "liquidations" => self.liquidations as f64,
            "funding_usdt" => self.funding_usdt,
            "avg_hold_sec" => {
                return (!self.hold.is_empty())
                    .then(|| self.hold.iter().sum::<f64>() / self.hold.len() as f64);
            }
            "median_hold_sec" => return median(&self.hold),
            "volume_usdt" => self.volume_usdt,
            "max_consecutive_losses" => self.max_losing_run as f64,
            // The curve skips the unvalued trades, so its fall would be invented.
            "max_drawdown_usdt" => return (self.unvalued == 0).then_some(self.tally.max_dd),
            "first_trade_ms" => return self.first_ms.map(|v| v as f64),
            "last_trade_ms" => return self.last_ms.map(|v| v as f64),
            "unvalued" => (self.unvalued + self.funding_unvalued) as f64,
            _ => return None,
        })
    }

    /// One metric as an answer prints it, rounded by its unit.
    pub fn output(&self, metric: &str) -> Value {
        if metric == "native_profit" {
            return json!(self.native);
        }
        let unit = METRICS
            .iter()
            .find(|(m, _)| *m == metric)
            .map_or(Unit::Count, |(_, u)| *u);
        match self.value(metric) {
            None => Value::Null,
            Some(v) => match unit {
                Unit::Count if v.fract() == 0.0 => json!(v as i64),
                Unit::Count => json!(round(v, 0)),
                Unit::Usdt | Unit::Pct => json!(round(v, 2)),
                Unit::Ratio => json!(round(v, 3)),
            },
        }
    }
}

/// Median of the values, or `None` when there are none.
fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// Round half away from zero to `digits` decimals, for output only.
pub fn round(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    let rounded = (value * scale).round() / scale;
    // `-0.004` rounds to negative zero, which prints as `-0.0`.
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// Fold a set of trades, already in close order.
pub fn of<'a>(trades: impl IntoIterator<Item = &'a Trade>) -> Stats {
    let mut stats = Stats::default();
    for trade in trades {
        stats.push(trade);
    }
    stats
}

#[cfg(test)]
mod tests;
