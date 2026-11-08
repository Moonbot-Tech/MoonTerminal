//! Reading the filtered trades once, as compact values the tools fold in Rust.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use rusqlite::types::Value;

use super::filter::{self, Reason, Residual, TradeFilter};
use super::{Scope, read_error};

/// Most trades one call may hold; past it the filter must be narrowed. A year of the largest
/// known replica is about 220 thousand.
const MAX_TRADES: usize = 400_000;

/// One closed trade, with what every tool reads from it.
#[derive(Debug, Clone)]
pub struct Trade {
    pub core: u64,
    /// The replica's `newrecid`; 0 on a legacy row.
    pub rec: i64,
    /// Effective strategy id, bit for bit the stored `i64` read as `u64`.
    pub sid: u64,
    pub coin: Arc<str>,
    /// The coin with contract tails removed, so `JCT_RP` and `JCT` group together.
    pub coin_key: Arc<str>,
    pub short: bool,
    pub lev: f64,
    /// Entry and close, UTC milliseconds; a core without a measured clock offset reads on its own
    /// clock and is listed in the answer.
    pub open_ms: Option<i64>,
    pub close_ms: i64,
    pub hold_sec: Option<i64>,
    pub currency: Option<&'static str>,
    /// Settled result in the trade's own currency.
    pub profit_native: f64,
    /// Result in USDT; `None` when no rate is known, which is never read as zero.
    pub profit_usdt: Option<f64>,
    /// Return on what was spent, percent.
    pub profit_pct: Option<f64>,
    /// Entry notional in USDT, under the Report's volume rules.
    pub volume_usdt: Option<f64>,
    pub reason: Reason,
    /// Current name, resolved through the live list, the archive, the row, then `#id`.
    pub name: Arc<str>,
    /// The name the row recorded when the trade happened.
    pub name_at_trade: Option<Arc<str>>,
    pub buy_price: Option<f64>,
    pub sell_price: Option<f64>,
    /// Displayed report id.
    pub record_id: Option<i64>,
    /// Raw texts, read only when an answer shows them.
    pub sell_reason: Option<String>,
    pub channel_name: Option<String>,
    pub comment: Option<String>,
    /// Entry-condition values in the order the read asked for them.
    pub entry: Vec<Option<f64>>,
}

#[cfg(test)]
impl Trade {
    /// A valued long trade for unit tests, closed at `close_ms` with `profit` USDT.
    pub fn sample(core: u64, sid: u64, coin: &str, close_ms: i64, profit: f64) -> Self {
        Trade {
            core,
            rec: close_ms,
            sid,
            coin: Arc::from(coin),
            coin_key: Arc::from(crate::symbol::coin_match_key(coin).as_str()),
            short: false,
            lev: 1.0,
            open_ms: Some(close_ms - 30_000),
            close_ms,
            hold_sec: Some(30),
            currency: Some("USDT"),
            profit_native: profit,
            profit_usdt: Some(profit),
            profit_pct: Some(profit),
            volume_usdt: Some(100.0),
            reason: Reason::SellPrice,
            name: Arc::from(format!("#{sid}").as_str()),
            name_at_trade: None,
            buy_price: Some(1.0),
            sell_price: Some(1.0),
            record_id: None,
            sell_reason: None,
            channel_name: None,
            comment: None,
            entry: Vec::new(),
        }
    }
}

/// What a read keeps besides the numbers.
#[derive(Default)]
pub struct Want {
    /// Keep the raw sell reason, channel name and comment.
    pub text: bool,
    /// Entry columns to read, tuner field names.
    pub entry: Vec<&'static str>,
}

/// The filtered trades, oldest close first, with what the read could not value.
pub struct Loaded {
    pub trades: Vec<Trade>,
    pub cores_without_clock_offset: BTreeSet<u64>,
    pub from_ms: i64,
    pub to_ms: i64,
}

/// Columns the read projects, in this order; entry columns follow.
const COLUMNS: &[&str] = &[
    "closedate",
    "buydate",
    "coin",
    "isshort",
    "lev",
    "channelname",
    "sellreason",
    "basecurrency",
    "profitbtc",
    "valuation_profit_usdt",
    "profitpct",
    "buyprice",
    "sellprice",
    "id",
    "comment",
    crate::db::NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN,
    crate::db::MINI_ENTRY_VOLUME_RATE_COLUMN,
];

/// Every entry field a filter, a group or a column may name: the tuner's whitelist.
pub fn entry_field(name: &str) -> Option<&'static str> {
    crate::db::tuner::FIELDS
        .iter()
        .find(|field| field.col == name)
        .map(|field| field.col)
}

/// Read the trades `filter` selects on `scope`'s cores, then run `extra` on the same snapshot.
///
/// Args:
///     scope: Authorized cores and their live strategy names.
///     filter: The client's filter; its window is required.
///     want: Texts and entry columns to keep.
///     extra: Further reads that must see the same snapshot as the trades.
///
/// Errors:
///     A bad filter, a read failure, a read past its time budget, or more than [`MAX_TRADES`].
pub fn load<T>(
    scope: &Scope,
    filter: &TradeFilter,
    want: &Want,
    extra: impl FnOnce(&rusqlite::Connection) -> Result<T>,
) -> Result<(Loaded, T)> {
    let (from_ms, to_ms) = filter::window(filter)?;
    let mut entry_fields = want.entry.clone();
    for bound in &filter.entry {
        let Some(field) = entry_field(&bound.field) else {
            bail!("entry field {} is not a tuner field", bound.field);
        };
        if !entry_fields.contains(&field) {
            entry_fields.push(field);
        }
    }
    let residual = Residual::new(filter, &entry_fields)?;
    let sids = filter::strategy_ids(filter)?;
    let conn = crate::db::open_reader().map_err(read_error)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    conn.progress_handler(10_000, Some(move || Instant::now() >= deadline))?;
    let snapshot = crate::db::read_snapshot(&conn).map_err(read_error)?;
    let axis = crate::db::ReportAxis::load(&snapshot, chrono_tz::UTC).map_err(read_error)?;
    let archive = crate::db::archive_strategy_names(&snapshot).map_err(read_error)?;
    let report = crate::db::ReportFilter {
        core_uids: scope.cores.clone(),
        // Rows carry whole seconds and the Report's upper bound is inclusive: the second `s` is
        // in `[from_ms, to_ms)` exactly when `s * 1000` is, so adjacent windows never share one.
        date_from: Some(from_ms.saturating_add(999).div_euclid(1000)),
        date_to: Some((to_ms - 1).div_euclid(1000)),
        emulator: if filter.include_emulator {
            None
        } else {
            Some(false)
        },
        side: match filter.side {
            None => crate::db::SideFilter::All,
            Some(filter::Side::Long) => crate::db::SideFilter::Long,
            Some(filter::Side::Short) => crate::db::SideFilter::Short,
        },
        period_basis: match filter.date_field {
            filter::DateField::Close => crate::db::PeriodBasis::CloseDate,
            filter::DateField::Buy => crate::db::PeriodBasis::OpenDate,
        },
        strategies: (!sids.is_empty()).then(|| {
            scope
                .cores
                .iter()
                .flat_map(|core| {
                    sids.iter().map(|sid| crate::db::ReportStrategyKey {
                        core_uid: *core,
                        strategy_id: *sid,
                    })
                })
                .collect()
        }),
        rows: crate::db::RowScope::Closed,
        axis: axis.clone(),
        ..Default::default()
    };
    let mut cols: Vec<String> = COLUMNS.iter().map(|c| c.to_string()).collect();
    cols.extend(entry_fields.iter().map(|c| c.to_string()));
    let mut builder = Builder {
        scope,
        archive: &archive,
        axis: &axis,
        want,
        entry_len: entry_fields.len(),
        interned: HashMap::new(),
    };
    // Shared by the visit and the reset: a retry after a corrupt valuation cache starts over.
    let collected = std::cell::RefCell::new((Vec::new(), false));
    let mut no_offset = BTreeSet::new();
    let mut visit = |core: u64, rec: i64, sid: i64, cells: &[Value]| {
        if axis.offset_secs(core, to_ms / 1000).is_none() {
            no_offset.insert(core);
        }
        let Some(trade) = builder.build(core, rec, sid as u64, cells) else {
            return true;
        };
        if !residual.keeps(&trade) {
            return true;
        }
        let mut collected = collected.borrow_mut();
        if collected.0.len() >= MAX_TRADES {
            collected.1 = true;
            return false;
        }
        collected.0.push(trade);
        true
    };
    let mut reset = || *collected.borrow_mut() = (Vec::new(), false);
    crate::db::scan_closed_trades(&snapshot, &report, &cols, &mut visit, &mut reset)
        .map_err(read_error)?;
    let (mut trades, overflow) = collected.into_inner();
    if overflow {
        bail!("more than {MAX_TRADES} trades match; narrow the window or the cores");
    }
    trades.sort_by_key(|t| (t.close_ms, t.core, t.rec));
    let more = extra(&snapshot)?;
    Ok((
        Loaded {
            trades,
            cores_without_clock_offset: no_offset,
            from_ms,
            to_ms,
        },
        more,
    ))
}

/// Turns projected cells into trades, interning the strings that repeat.
struct Builder<'a> {
    scope: &'a Scope,
    archive: &'a HashMap<(u64, i64), String>,
    axis: &'a crate::db::ReportAxis,
    want: &'a Want,
    entry_len: usize,
    interned: HashMap<String, Arc<str>>,
}

impl Builder<'_> {
    /// One string shared by every trade that carries it.
    fn intern(&mut self, text: &str) -> Arc<str> {
        if let Some(found) = self.interned.get(text) {
            return found.clone();
        }
        let shared: Arc<str> = Arc::from(text);
        self.interned.insert(text.to_string(), shared.clone());
        shared
    }

    /// The trade of one row, or `None` for a row without a close time.
    fn build(&mut self, core: u64, rec: i64, sid: u64, cells: &[Value]) -> Option<Trade> {
        let num = |i: usize| match cells.get(i) {
            Some(Value::Integer(v)) => Some(*v as f64),
            Some(Value::Real(v)) => Some(*v),
            _ => None,
        };
        let int = |i: usize| match cells.get(i) {
            Some(Value::Integer(v)) => Some(*v),
            Some(Value::Real(v)) => Some(*v as i64),
            _ => None,
        };
        let text = |i: usize| match cells.get(i) {
            Some(Value::Text(v)) => Some(v.as_str()),
            _ => None,
        };
        let close = int(0).filter(|s| *s > 0)?;
        let open = int(1).filter(|s| *s > 0);
        let close_ms = self.axis.to_utc(close, core).saturating_mul(1000);
        let open_ms = open.map(|s| self.axis.to_utc(s, core).saturating_mul(1000));
        let coin = text(2).unwrap_or_default().to_string();
        let coin_key = crate::symbol::coin_match_key(&coin);
        let channel = text(5);
        let at_trade = channel.and_then(name_in_channel).map(str::to_string);
        let reason_text = text(6).unwrap_or_default();
        let name = self
            .scope
            .live_names
            .get(&(core, sid))
            .cloned()
            .or_else(|| self.archive.get(&(core, sid as i64)).cloned())
            .or_else(|| at_trade.clone())
            // Ids repeat across cores, so a nameless strategy keeps its core in the label.
            .unwrap_or_else(|| format!("#{sid}@{core}"));
        let volume = num(15).zip(num(16)).map(|(native, rate)| native * rate);
        let trade = Trade {
            core,
            rec,
            sid,
            coin: self.intern(&coin),
            coin_key: self.intern(&coin_key),
            short: int(3).is_some_and(|v| v != 0),
            lev: num(4).unwrap_or(0.0),
            open_ms,
            close_ms,
            hold_sec: open.map(|o| close - o).filter(|h| *h >= 0),
            currency: int(7)
                .and_then(crate::db::QuoteCurrency::from_report_ordinal)
                .map(|c| c.ticker()),
            profit_native: num(8).unwrap_or(0.0),
            profit_usdt: num(9),
            profit_pct: num(10),
            volume_usdt: volume,
            reason: Reason::classify(reason_text),
            name: self.intern(&name),
            name_at_trade: at_trade.map(|n| self.intern(&n)),
            buy_price: num(11),
            sell_price: num(12),
            record_id: int(13),
            sell_reason: self.want.text.then(|| reason_text.to_string()),
            channel_name: self
                .want
                .text
                .then(|| channel.map(str::to_string))
                .flatten(),
            comment: self
                .want
                .text
                .then(|| text(14).map(str::to_string))
                .flatten(),
            entry: (0..self.entry_len)
                .map(|i| num(COLUMNS.len() + i))
                .collect(),
        };
        Some(trade)
    }
}

/// The strategy name a row's `channelname` recorded: the text in its last `<…>`.
///
/// Two shapes are known, `(strategy <NAME>)` on 99.9 % of a year's rows and `JoinKey: 17 <NAME>`.
pub fn name_in_channel(channel: &str) -> Option<&str> {
    let open = channel.rfind('<')?;
    let close = open + channel[open..].find('>')?;
    let name = channel[open + 1..close].trim();
    (!name.is_empty()).then_some(name)
}
