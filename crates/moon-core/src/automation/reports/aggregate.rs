//! `reports_aggregate`: group the filtered trades, compute metrics, keep and order the groups.

use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};
use chrono::{Datelike, TimeZone, Timelike};
use serde::Deserialize;
use serde_json::{Value, json};

use super::filter::TradeFilter;
use super::stats::{self, Stats};
use super::trade::{self, Loaded, Trade, Want};
use super::{ANSWER_BUDGET, Scope};

/// Arguments of `reports_aggregate`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    pub filter: TradeFilter,
    #[serde(default)]
    pub group_by: Vec<GroupBy>,
    #[serde(default)]
    pub bucket_size: Option<BucketSize>,
    /// IANA zone for `bucket`, `hour_of_day` and `weekday`; UTC by default.
    #[serde(default)]
    pub tz: Option<String>,
    #[serde(default)]
    pub entry_quantile: Option<Quantile>,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub having: Vec<Having>,
    #[serde(default)]
    pub sort: Vec<Sort>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: usize,
}

/// One grouping dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    Core,
    Coin,
    Side,
    Currency,
    Reason,
    Lev,
    /// Core and strategy id together: ids repeat across cores.
    Strategy,
    /// The strategy name across cores: one name on 15 cores is one group.
    StrategyName,
    Bucket,
    HourOfDay,
    Weekday,
    HoldBucket,
    EntryQuantile,
}

impl GroupBy {
    /// The column name an answer gives the dimension.
    fn name(self) -> &'static str {
        match self {
            GroupBy::Core => "core",
            GroupBy::Coin => "coin",
            GroupBy::Side => "side",
            GroupBy::Currency => "currency",
            GroupBy::Reason => "reason",
            GroupBy::Lev => "lev",
            GroupBy::Strategy => "strategy",
            GroupBy::StrategyName => "strategy_name",
            GroupBy::Bucket => "bucket",
            GroupBy::HourOfDay => "hour_of_day",
            GroupBy::Weekday => "weekday",
            GroupBy::HoldBucket => "hold_bucket",
            GroupBy::EntryQuantile => "entry_quantile",
        }
    }
}

/// Width of a time bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketSize {
    Hour,
    Day,
    Week,
}

/// Quantile groups of one entry condition.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantile {
    pub field: String,
    #[serde(default = "default_bins")]
    pub bins: usize,
}

/// Five quantile groups unless asked otherwise.
fn default_bins() -> usize {
    5
}

/// A condition on a group's metric, applied to exact values.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Having {
    pub metric: String,
    pub op: Op,
    pub value: f64,
}

/// Comparison of a `having` condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Op {
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
}

impl Op {
    /// Whether `left op right` holds.
    fn holds(self, left: f64, right: f64) -> bool {
        match self {
            Op::Gt => left > right,
            Op::Ge => left >= right,
            Op::Lt => left < right,
            Op::Le => left <= right,
            Op::Eq => left == right,
            Op::Ne => left != right,
        }
    }
}

/// One ordering key: a metric or a group column.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    pub by: String,
    #[serde(default)]
    pub dir: Dir,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    #[default]
    Asc,
    Desc,
}

/// Run `reports_aggregate`.
///
/// Errors:
///     A bad argument (with the allowed values named), a read failure or a time-out.
pub fn run(scope: &Scope, args: Args) -> Result<Value> {
    let metrics = stats::metric_names(&args.metrics)?;
    let tz: chrono_tz::Tz = match args.tz.as_deref() {
        None => chrono_tz::UTC,
        Some(name) => name
            .parse()
            .map_err(|_| anyhow::anyhow!("unknown time zone {name}"))?,
    };
    let quantile_field = match (
        &args.entry_quantile,
        args.group_by.contains(&GroupBy::EntryQuantile),
    ) {
        (Some(q), true) => {
            ensure!(
                (2..=10).contains(&q.bins),
                "entry_quantile.bins must be 2..10"
            );
            Some(trade::entry_field(&q.field).ok_or_else(|| {
                anyhow::anyhow!("entry_quantile.field {} is not a tuner field", q.field)
            })?)
        }
        (None, true) => bail!("group_by entry_quantile needs entry_quantile {{field, bins}}"),
        _ => None,
    };
    ensure!(
        !args.group_by.contains(&GroupBy::Bucket) || args.bucket_size.is_some(),
        "group_by bucket needs bucket_size: hour, day or week"
    );
    for having in &args.having {
        stats::numeric_metric(&having.metric)?;
    }
    let want = Want {
        text: false,
        entry: quantile_field.into_iter().collect(),
    };
    let (loaded, ()) = trade::load(scope, &args.filter, &want, |_| Ok(()))?;
    let bounds = quantile_field.map(|_| {
        quantile_bounds(
            &loaded.trades,
            args.entry_quantile.as_ref().map_or(5, |q| q.bins),
        )
    });
    let keys = Keys {
        by: &args.group_by,
        tz,
        bucket: args.bucket_size.unwrap_or(BucketSize::Day),
        bounds: bounds.as_deref(),
    };
    let mut groups: BTreeMap<Vec<String>, Stats> = BTreeMap::new();
    for trade in &loaded.trades {
        groups.entry(keys.of(trade)).or_default().push(trade);
    }
    let mut kept: Vec<(Vec<String>, Stats)> = groups
        .into_iter()
        .filter(|(_, stats)| {
            args.having.iter().all(|h| {
                stats
                    .value(&h.metric)
                    .is_some_and(|v| h.op.holds(v, h.value))
            })
        })
        .collect();
    order(&mut kept, &args.sort, &args.group_by)?;
    let limit = args.limit.unwrap_or(50).clamp(1, 1000);
    let mut columns: Vec<&str> = args.group_by.iter().map(|g| g.name()).collect();
    columns.extend(metrics.iter().map(String::as_str));
    let mut rows = Vec::new();
    let mut size = 0;
    for (key, stats) in kept.iter().skip(args.offset).take(limit) {
        let mut row: Vec<Value> = key
            .iter()
            .zip(&args.group_by)
            .map(|(part, by)| key_value(*by, part))
            .collect();
        row.extend(metrics.iter().map(|m| stats.output(m)));
        let row = Value::Array(row);
        size += row.to_string().len() + 1;
        if size > ANSWER_BUDGET && !rows.is_empty() {
            break;
        }
        rows.push(row);
    }
    let next = args.offset + rows.len();
    let totals = stats::of(&loaded.trades);
    let mut answer = json!({
        "columns": columns,
        "rows": rows,
        "total_groups": kept.len(),
        "has_more": next < kept.len(),
        "next_offset": next,
        "totals": metrics.iter().map(|m| (m.clone(), totals.output(m))).collect::<serde_json::Map<_, _>>(),
        "coverage": coverage(&loaded, &totals),
    });
    if let Some(bounds) = bounds {
        answer["entry_quantile_bounds"] = json!(bounds);
    }
    Ok(answer)
}

/// What the figures stand on: trades read, valued, and the caveats only when they apply.
pub fn coverage(loaded: &Loaded, totals: &Stats) -> Value {
    let mut coverage = json!({
        "trades": totals.trades,
        "unvalued": totals.unvalued + totals.funding_unvalued,
        "funding_rows": totals.funding_rows,
        "window_utc_ms": [loaded.from_ms, loaded.to_ms],
        "window_is": "from_ms inclusive, to_ms exclusive, at whole seconds",
    });
    if totals.unvalued + totals.funding_unvalued > 0 {
        coverage["unvalued_means"] = json!(
            "no USDT rate: left out of USDT sums, never read as zero; drawdown is withheld while any trade (not Funding) is unvalued"
        );
    }
    if !loaded.cores_without_clock_offset.is_empty() {
        coverage["cores_on_own_clock"] = json!(
            loaded
                .cores_without_clock_offset
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
        );
    }
    coverage
}

/// Group keys of a trade under the requested dimensions.
struct Keys<'a> {
    by: &'a [GroupBy],
    tz: chrono_tz::Tz,
    bucket: BucketSize,
    bounds: Option<&'a [f64]>,
}

impl Keys<'_> {
    /// One key part per dimension, as text that also sorts sensibly.
    fn of(&self, trade: &Trade) -> Vec<String> {
        let local = self.tz.timestamp_millis_opt(trade.close_ms).single();
        self.by
            .iter()
            .map(|by| match by {
                GroupBy::Core => trade.core.to_string(),
                GroupBy::Coin => trade.coin_key.to_string(),
                GroupBy::Side => if trade.short { "short" } else { "long" }.to_string(),
                GroupBy::Currency => trade.currency.unwrap_or("unknown").to_string(),
                GroupBy::Reason => trade.reason.name().to_string(),
                GroupBy::Lev => format!("{:08.2}", trade.lev),
                GroupBy::Strategy => format!("{}:{}", trade.core, trade.sid),
                GroupBy::StrategyName => trade.name.to_string(),
                GroupBy::Bucket => local.map_or_else(String::new, |t| match self.bucket {
                    BucketSize::Hour => t.format("%Y-%m-%d %H:00").to_string(),
                    BucketSize::Day => t.format("%Y-%m-%d").to_string(),
                    BucketSize::Week => {
                        let monday = t.date_naive()
                            - chrono::Duration::days(i64::from(t.weekday().num_days_from_monday()));
                        monday.format("%Y-%m-%d").to_string()
                    }
                }),
                GroupBy::HourOfDay => local.map_or(0, |t| t.hour()).to_string(),
                GroupBy::Weekday => local
                    .map_or(0, |t| t.weekday().number_from_monday())
                    .to_string(),
                GroupBy::HoldBucket => hold_bucket(trade.hold_sec).to_string(),
                GroupBy::EntryQuantile => {
                    match (self.bounds, trade.entry.first().copied().flatten()) {
                        (Some(bounds), Some(v)) => {
                            format!(
                                "q{}",
                                bounds
                                    .iter()
                                    .filter(|b| v > **b)
                                    .count()
                                    .min(bounds.len().saturating_sub(1))
                                    + 1
                            )
                        }
                        _ => "none".to_string(),
                    }
                }
            })
            .collect()
    }
}

/// Holding-time band of a trade.
fn hold_bucket(hold: Option<i64>) -> &'static str {
    match hold {
        // Every label carries a sort prefix that the answer strips, this one included.
        None => "z_unknown",
        Some(s) if s < 10 => "a_under_10s",
        Some(s) if s < 60 => "b_10s_1m",
        Some(s) if s < 600 => "c_1m_10m",
        Some(s) if s < 3600 => "d_10m_1h",
        Some(_) => "e_over_1h",
    }
}

/// Upper bounds of the quantile groups over the trades that carry the value; the last group is
/// open-ended. Returned in the answer, so a group label can be read back as a range.
fn quantile_bounds(trades: &[Trade], bins: usize) -> Vec<f64> {
    let mut values: Vec<f64> = trades
        .iter()
        .filter_map(|t| t.entry.first().copied().flatten())
        .collect();
    if values.is_empty() {
        return Vec::new();
    }
    values.sort_by(f64::total_cmp);
    (1..=bins)
        .map(|i| {
            values[(values.len() * i / bins)
                .saturating_sub(1)
                .min(values.len() - 1)]
        })
        .collect()
}

/// A key part as the answer prints it: numbers where the dimension is numeric.
fn key_value(by: GroupBy, part: &str) -> Value {
    match by {
        GroupBy::Lev => part.trim().parse::<f64>().map_or(Value::Null, |v| json!(v)),
        GroupBy::HourOfDay | GroupBy::Weekday => {
            part.parse::<i64>().map_or(Value::Null, |v| json!(v))
        }
        GroupBy::HoldBucket => json!(&part[2..]),
        _ => json!(part),
    }
}

/// Order the groups by the requested keys; without any, most trades first.
///
/// Errors:
///     A key that is neither a metric nor a requested group column.
fn order(groups: &mut [(Vec<String>, Stats)], sort: &[Sort], by: &[GroupBy]) -> Result<()> {
    enum Key {
        Metric(String),
        Group(usize),
    }
    let mut keys = Vec::new();
    for s in sort {
        let key = if let Some(index) = by.iter().position(|g| g.name() == s.by) {
            Key::Group(index)
        } else {
            stats::numeric_metric(&s.by)?;
            Key::Metric(s.by.clone())
        };
        keys.push((key, s.dir));
    }
    if keys.is_empty() {
        keys.push((Key::Metric("trades".into()), Dir::Desc));
    }
    groups.sort_by(|(ka, a), (kb, b)| {
        for (key, dir) in &keys {
            let ordering = match key {
                Key::Group(i) => key_order(by[*i], &ka[*i], &kb[*i]),
                // A missing value sorts last whichever the direction.
                Key::Metric(m) => match (a.value(m), b.value(m)) {
                    (Some(x), Some(y)) => {
                        let o = x.total_cmp(&y);
                        if *dir == Dir::Desc { o.reverse() } else { o }
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
            };
            let ordering = match key {
                Key::Group(_) if *dir == Dir::Desc => ordering.reverse(),
                _ => ordering,
            };
            if ordering.is_ne() {
                return ordering;
            }
        }
        ka.iter()
            .zip(kb)
            .zip(by)
            .map(|((x, y), dimension)| key_order(*dimension, x, y))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(())
}

/// Order two key parts of one dimension.
///
/// Numbers only for the dimensions whose every key is a number (a quantile by its index, with
/// `none` before them as text): a rule that tried numbers wherever both keys happened to parse
/// would not be a total order over names like `5`, `10` and `15m`, and the sort may panic on one.
fn key_order(by: GroupBy, a: &str, b: &str) -> std::cmp::Ordering {
    let number = |key: &str| match by {
        GroupBy::Core | GroupBy::HourOfDay | GroupBy::Weekday | GroupBy::Lev => {
            key.trim().parse::<f64>().ok()
        }
        GroupBy::EntryQuantile => key.strip_prefix('q').and_then(|n| n.parse::<f64>().ok()),
        _ => None,
    };
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests;
