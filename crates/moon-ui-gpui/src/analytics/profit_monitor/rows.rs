//! Pure row grouping for the desktop Profit Monitor.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use crate::controls::venue_section_label;

/// User-selected grouping axis for the monitor table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum GroupMode {
    /// Keep one row per report core in canonical core order.
    #[default]
    Core,
    /// Merge every core connected to the same venue.
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
    /// Last known venue keyed by core.
    pub(super) venues: HashMap<CoreId, CoreVenue>,
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
    /// Venue of `primary_core`, used to pick the row's logo and to resolve its filter payload.
    ///
    /// Populated in BOTH modes — a Core row draws its own core's brand — and `None` when nothing
    /// can name the core's venue, whether because none was reported or because the one reported is
    /// not nameable. That is also what the unidentified Exchange row stands for.
    pub(super) venue: Option<CoreVenue>,
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

/// Localized fallback label used by the pure grouping pass.
///
/// Only the core prefix is injected. The exchange caption comes from
/// [`crate::controls::venue_section_label`], so the Profit Monitor cannot word an unidentified
/// venue differently from every picker that lists the same core.
pub(super) struct RowLabels<'a> {
    /// Prefix used when no usable core name exists.
    pub(super) core: &'a str,
}

/// Bucket every configured core by the venue it is connected to, keyed as the rows key it.
///
/// One pass, so resolving each row's click payload is a lookup rather than a scan of every core:
/// two hundred cores across eight exchange rows would otherwise be sixteen hundred comparisons on
/// each rebuild. The key is the venue identity, matching [`grouped_rows`] exactly — cores that
/// render as one row must therefore filter as one row.
///
/// Args:
///     live: Current configured core order and per-core venues.
///
/// Returns:
///     Configured cores per venue, each in configured order.
fn cores_by_exchange(live: &LiveContext) -> HashMap<ExchangeId, Vec<CoreId>> {
    let mut buckets: HashMap<ExchangeId, Vec<CoreId>> = HashMap::new();
    for core in &live.core_order {
        // The same `is_nameable` filter `grouped_rows` applies. Without it a core that renders on
        // the unidentified row would still be filtered in by a named row's click.
        if let Some(venue) = live.venues.get(core).filter(|venue| venue.is_nameable()) {
            buckets.entry(venue.id).or_default().push(*core);
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
///     buckets: Configured cores per venue.
///     mode: Selected grouping axis.
///
/// Returns:
///     The row's click payload, never empty for a row that has at least one traded core.
fn filter_cores(
    row: &MonitorRow,
    buckets: &HashMap<ExchangeId, Vec<CoreId>>,
    mode: GroupMode,
) -> Rc<[CoreId]> {
    // The "unknown exchange" row groups cores no venue was reported for; a configured core that
    // reported nothing is not evidence it belongs there, so that row stays literal too.
    let bucket = (mode == GroupMode::Exchange)
        .then_some(row.venue.as_ref())
        .flatten()
        .and_then(|venue| buckets.get(&venue.id));
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

/// Identity one displayed row merges its cores under.
///
/// Exchange rows key on the VENUE, not on its caption: two cores whose builds spell one venue
/// differently still merge, and the unidentified row is one explicit bucket rather than whatever
/// the localized fallback label happens to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    /// One row per report core.
    Core(CoreId),
    /// One row per venue; `None` collects every core with no reported identity.
    Exchange(Option<ExchangeId>),
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
    let mut grouped = HashMap::<RowKey, MonitorRow>::new();
    for source in &summary.cores {
        let core = source.core_uid;
        let configured = live
            .core_names
            .get(&core)
            .map(String::as_str)
            .filter(|name| !name.trim().is_empty());
        let report = (!source.report_name.trim().is_empty()).then_some(source.report_name.as_str());
        // A venue nothing can name groups with the cores that reported none at all, exactly as the
        // core pickers do — one "not identified" row, never two.
        let venue = live.venues.get(&core).filter(|venue| venue.is_nameable());
        let key = match mode {
            GroupMode::Core => RowKey::Core(core),
            GroupMode::Exchange => RowKey::Exchange(venue.map(|venue| venue.id)),
        };
        // The caption is built INSIDE the vacancy closure, never before the lookup: two hundred
        // cores merge into a handful of exchange rows, and formatting one label per core would
        // throw almost all of them away — on a pass `body()` runs every render.
        grouped
            .entry(key)
            .or_insert_with(|| MonitorRow {
                name: match mode {
                    GroupMode::Core => configured
                        .or(report)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{} {core}", labels.core)),
                    GroupMode::Exchange => venue_section_label(venue),
                },
                primary_core: core,
                venue: venue.cloned(),
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
        // Cached keys: the tiebreak lowercases a name, and doing that inside the comparator
        // allocates twice per comparison rather than once per row.
        rows.sort_by_cached_key(|row| row.name.to_lowercase());
        rows.sort_by(|a, b| b.profit.total_cmp(&a.profit));
    }
    rows
}
