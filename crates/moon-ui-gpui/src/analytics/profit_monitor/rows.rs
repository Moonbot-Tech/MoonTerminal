//! Pure row grouping for the desktop Profit Monitor.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::session::CoreId;

use crate::controls::exchange_display_name_with_spot;

/// User-selected grouping axis for the monitor table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum GroupMode {
    /// Keep one row per report core in canonical core order.
    #[default]
    Core,
    /// Merge every core reporting the same exchange name.
    Exchange,
}

impl GroupMode {
    /// Restore a persisted group id.
    ///
    /// Args:
    ///     id: Stable layout value.
    ///
    /// Returns:
    ///     Matching mode, or `None` for unknown future values.
    pub(super) fn from_id(id: &str) -> Option<Self> {
        match id {
            "core" => Some(Self::Core),
            "exchange" => Some(Self::Exchange),
            _ => None,
        }
    }

    /// Return the stable layout id.
    ///
    /// Returns:
    ///     `core` or `exchange`.
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Exchange => "exchange",
        }
    }
}

/// Cached non-database context that can regroup an existing report snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LiveContext {
    /// Last known nonblank exchange names keyed by core.
    pub(super) exchange_names: HashMap<CoreId, String>,
    /// Current configured names keyed by core.
    pub(super) core_names: HashMap<CoreId, String>,
    /// Configured cores in canonical display order.
    pub(super) core_order: Vec<CoreId>,
}

/// One displayed row after the selected grouping axis has merged per-core data.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MonitorRow {
    /// Visible group label.
    pub(super) name: String,
    /// Unformatted identity used to preserve raw exchange-name sorting.
    pub(super) sort_name: String,
    /// Projected profit.
    pub(super) profit: f64,
    /// Closed-trade count.
    pub(super) trades: i64,
    /// Profitable-trade count.
    pub(super) wins: i64,
    /// Additive positive-spend total.
    pub(super) positive_spent: f64,
    /// Count contributing to `positive_spent`.
    pub(super) positive_orders: i64,
    /// Core identity used only for canonical Core-mode ordering.
    pub(super) primary_core: CoreId,
    /// Profit of the newest closed trade among this row's cores, when the period holds one.
    pub(super) last_profit: Option<f64>,
    /// Close date `last_profit` came from; zero when no dated trade exists.
    pub(super) last_close: i64,
    /// Core that supplied `last_profit`, used to break a tie deterministically when rows merge.
    pub(super) last_core: CoreId,
    /// Every core merged into this row, so the arrival highlight can follow any of them.
    ///
    /// Keying the highlight on `last_core` alone would miss a sibling core whose new trade carries
    /// an OLDER close date than the row's newest — cores stamp close dates from their own clocks,
    /// and a backfilled batch does the same thing.
    pub(super) cores: Vec<CoreId>,
    /// Cores this row stands for when it is clicked as a core filter, shared rather than copied.
    ///
    /// Not the same list as [`Self::cores`], which is only what TRADED in the period. An Exchange
    /// row labelled "Binance" has to filter the terminal to every configured Binance core, or a
    /// core that happened to close nothing this hour would silently vanish from Orders too. The
    /// `Rc` is what lets the table hand this to a click handler per row without copying it.
    pub(super) filter_cores: Rc<[CoreId]>,
    /// Raw exchange name reported for `primary_core`, used only to pick a logo.
    ///
    /// Kept unformatted: the Spot/Futures display policy belongs to the visible `name`, while the
    /// logo lookup wants the identity the core actually reported.
    pub(super) exchange: Option<String>,
}

impl MonitorRow {
    /// Merge one report-core aggregate into this displayed row.
    ///
    /// Args:
    ///     source: Per-core additive values from the database snapshot.
    fn push(&mut self, source: &ProfitMonitorCore) {
        self.profit += source.profit;
        self.trades += source.trades;
        self.wins += source.wins;
        self.positive_spent += source.positive_spent;
        self.positive_orders += source.positive_orders;
        self.cores.push(source.core_uid);
        // A merged row's "last trade" is the newest one across its cores, not the last core merged.
        // The database returns cores ordered by uid, so a tie resolves to the lowest uid and the
        // displayed value cannot flip between two refreshes that carry identical data. The close
        // date leads and the profit follows it: a trade whose projected profit is NULL still has a
        // real timestamp, and hiding the bracket is the honest answer there.
        if source.last_close > self.last_close {
            self.last_profit = source.last_profit;
            self.last_close = source.last_close;
            self.last_core = source.core_uid;
        }
    }

    /// Return the exact win percentage after grouping.
    ///
    /// Returns:
    ///     Profitable trades divided by all trades, or zero for an empty row.
    pub(super) fn win_rate(&self) -> f64 {
        if self.trades == 0 {
            0.0
        } else {
            self.wins as f64 * 100.0 / self.trades as f64
        }
    }

    /// Return the exact average positive order spend after grouping.
    ///
    /// Returns:
    ///     Positive spend divided by its contributing count, or zero when none exists.
    pub(super) fn average_order(&self) -> f64 {
        if self.positive_orders == 0 {
            0.0
        } else {
            self.positive_spent / self.positive_orders as f64
        }
    }
}

/// Localized fallback labels used by the pure grouping pass.
pub(super) struct RowLabels<'a> {
    /// Prefix used when no usable core name exists.
    pub(super) core: &'a str,
    /// One shared explicit label for missing exchange metadata.
    pub(super) unknown_exchange: &'a str,
    /// Localized suffix for a known spot exchange.
    pub(super) spot: &'a str,
}

/// Bucket every configured core by the exchange it reports, keyed as the row grouping keys it.
///
/// One pass, so resolving each row's click payload is a lookup rather than a scan of every core:
/// two hundred cores across eight exchange rows would otherwise be sixteen hundred comparisons on
/// each rebuild. The key is `to_lowercase()`, matching [`grouped_rows`] exactly — two spellings of
/// one exchange render as one row and must therefore filter as one row.
///
/// Args:
///     live: Current configured core order and reported exchange names.
///
/// Returns:
///     Configured cores per lowercased exchange name, each in configured order.
fn cores_by_exchange(live: &LiveContext) -> HashMap<String, Vec<CoreId>> {
    let mut buckets: HashMap<String, Vec<CoreId>> = HashMap::new();
    for core in &live.core_order {
        if let Some(exchange) = live.exchange_names.get(core) {
            buckets
                .entry(exchange.to_lowercase())
                .or_default()
                .push(*core);
        }
    }
    buckets
}

/// Resolve which cores one finished row stands for as a core filter.
///
/// A Core row is exactly its own core. An Exchange row is every CONFIGURED core reporting that
/// exchange, not only the ones that traded inside the period: the label names the exchange, and a
/// filter that quietly dropped the quiet cores would hide their open orders everywhere else.
/// Configured order is preserved so the payload is deterministic across refreshes.
///
/// Args:
///     row: Finished row, already carrying the cores that traded.
///     buckets: Configured cores per lowercased exchange name.
///     mode: Selected grouping axis.
///
/// Returns:
///     The row's click payload, never empty for a row that has at least one traded core.
fn filter_cores(
    row: &MonitorRow,
    buckets: &HashMap<String, Vec<CoreId>>,
    mode: GroupMode,
) -> Rc<[CoreId]> {
    // The "unknown exchange" row groups cores whose exchange nobody reported; a configured core
    // that reported nothing is not evidence it belongs there, so that row stays literal too.
    let bucket = (mode == GroupMode::Exchange)
        .then_some(row.exchange.as_deref())
        .flatten()
        .and_then(|exchange| buckets.get(&exchange.to_lowercase()));
    let Some(bucket) = bucket else {
        return Rc::from(row.cores.as_slice());
    };
    let mut cores = bucket.clone();
    // A core that traded but is no longer configured still belongs to the row the user can see.
    let configured: HashSet<CoreId> = bucket.iter().copied().collect();
    cores.extend(
        row.cores
            .iter()
            .copied()
            .filter(|core| !configured.contains(core)),
    );
    Rc::from(cores.as_slice())
}

/// Group per-core report aggregates into visible Core or Exchange rows.
///
/// Args:
///     summary: Per-core additive database payload.
///     live: Current labels and canonical core order.
///     mode: Selected grouping axis.
///     labels: Localized fallback labels.
///
/// Returns:
///     Exact merged rows in canonical Core order or descending profit order for aggregate modes.
pub(super) fn grouped_rows(
    summary: &ProfitMonitorSummary,
    live: &LiveContext,
    mode: GroupMode,
    labels: RowLabels<'_>,
) -> Vec<MonitorRow> {
    let mut grouped = HashMap::<String, MonitorRow>::new();
    for source in &summary.cores {
        let core = source.core_uid;
        let configured = live
            .core_names
            .get(&core)
            .map(String::as_str)
            .filter(|name| !name.trim().is_empty());
        let report = (!source.report_name.trim().is_empty()).then_some(source.report_name.as_str());
        let (key, name, sort_name) = match mode {
            GroupMode::Core => {
                let name = configured
                    .or(report)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {core}", labels.core));
                (format!("core:{core}"), name.clone(), name)
            }
            GroupMode::Exchange => match live.exchange_names.get(&core) {
                Some(exchange) => (
                    format!("exchange:{}", exchange.to_lowercase()),
                    exchange_display_name_with_spot(exchange, labels.spot),
                    exchange.clone(),
                ),
                None => {
                    let unknown = labels.unknown_exchange.to_string();
                    (
                        format!("exchange:{}", unknown.to_lowercase()),
                        unknown.clone(),
                        unknown,
                    )
                }
            },
        };
        grouped
            .entry(key)
            .or_insert_with(|| MonitorRow {
                name,
                sort_name,
                primary_core: core,
                exchange: live.exchange_names.get(&core).cloned(),
                ..MonitorRow::default()
            })
            .push(source);
    }

    let mut rows = grouped.into_values().collect::<Vec<_>>();
    let buckets = if mode == GroupMode::Exchange {
        cores_by_exchange(live)
    } else {
        HashMap::new()
    };
    for row in &mut rows {
        row.filter_cores = filter_cores(row, &buckets, mode);
    }
    if mode == GroupMode::Core {
        let rank = live
            .core_order
            .iter()
            .enumerate()
            .map(|(index, core)| (*core, index))
            .collect::<HashMap<_, _>>();
        rows.sort_by_key(|row| {
            (
                rank.get(&row.primary_core).copied().unwrap_or(usize::MAX),
                row.primary_core,
            )
        });
    } else {
        rows.sort_by(|a, b| {
            b.profit
                .total_cmp(&a.profit)
                .then_with(|| a.sort_name.to_lowercase().cmp(&b.sort_name.to_lowercase()))
        });
    }
    rows
}
