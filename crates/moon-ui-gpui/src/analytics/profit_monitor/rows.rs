//! Pure row grouping for the desktop Profit Monitor.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use moon_core::config::{CoreGroup, WorkspaceMode};
use moon_core::db::analytics::{ProfitMonitorCore, ProfitMonitorSummary};
use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

use crate::controls::venue_section_label;
use crate::workspace::scope_marker::ScopeMarker;

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
    /// Cores the user marked active, whether or not they are currently connected.
    ///
    /// The authority is `ServerConfig::active`, the same flag the connection table edits — never
    /// "a live session exists", which merely says a core is reachable right now. A core switched
    /// off deliberately must not come back as a zero row, and one that is active but offline must.
    pub(super) active: HashSet<CoreId>,
    /// The user's saved core groups, as the shared picker stores them.
    ///
    /// Carried in the context, not read at render time, for one reason: the monitor regroups from
    /// this struct, and a group edited in another window has to reach the table through the same
    /// 5-second sample that a renamed core does — as a [`super::ContextChange::Regroup`], with no
    /// SQLite read behind it.
    pub(super) core_groups: Vec<CoreGroup>,
    /// Preset this window displays under.
    ///
    /// The Profit Monitor is `DisplayOwner::Singleton`: it inherits the last focused group's
    /// preset (Auto or Classic), `None` while no group is focused. Carried so
    /// [`crate::workspace::scope_marker`] can build its facts from the same typed pair every other
    /// aggregate uses.
    pub(super) preset: Option<WorkspaceMode>,
    /// Configured cores before the membership filter ran.
    pub(super) configured_total: usize,
    /// Raw ids of every core `config.servers` names, before the membership filter ran.
    ///
    /// [`Self::core_names`], [`Self::core_order`] and [`Self::active`] are already filtered down to
    /// what the active preset shows; this is the unfiltered source they were built from, kept so
    /// [`super::model::scoped_query_core_ids`] can tell "not configured at all" apart from
    /// "configured but hidden" — a core absent here is a data-only core the membership filter never
    /// had authority to hide, and must not lose its money to it. Always `configured_total` long.
    pub(super) configured_core_ids: HashSet<CoreId>,
    /// Every core the header's own FLEET run cell may command, independent of the active preset's
    /// display narrowing.
    ///
    /// The same predicate as [`Self::active`], minus `Backend::core_displayed`: the preset is a READ
    /// narrowing only (the precedent is `analytics::mod::analytics_display_scope`'s docstring), and
    /// this cell commands the WHOLE table rather than one row, so scoping its authority through the
    /// same filter that legitimately narrows individual rows would silently narrow a COMMAND path
    /// too — the failure §4.5 of the goal spec exists to prevent.
    pub(super) action_core_ids: Vec<CoreId>,
}

impl LiveContext {
    /// Build this context's scope marker from its own membership-boundary counts.
    ///
    /// The one place both the scoped query ([`super::model::scoped_query_core_ids`]) and every
    /// render path ask "does the active preset hide anything", so the rows a query returns and the
    /// marker drawn beside them can never disagree about the same scope.
    ///
    /// Returns:
    ///     A marker driven by this context's own preset, shown count and configured total.
    pub(super) fn scope_marker(&self) -> ScopeMarker {
        ScopeMarker::new(self.preset, self.core_order.len(), self.configured_total)
    }
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
    /// `None` rather than zero when nothing closed: a ratio with no denominator is not a rate of
    /// zero, and a row for a core that traded nothing must not claim it lost every trade. The one
    /// definition, so the cell that draws it and the column that sorts by it cannot disagree.
    ///
    /// Returns:
    ///     Profitable trades divided by all trades, or `None` for a row with no closed trade.
    pub(super) fn win_rate(&self) -> Option<f64> {
        (self.trades > 0).then(|| self.wins as f64 * 100.0 / self.trades as f64)
    }

    /// Return the exact average positive order spend after grouping.
    ///
    /// `None` on an empty denominator, exactly as [`Self::win_rate`].
    ///
    /// Returns:
    ///     Positive spend divided by its contributing count, or `None` when none exists.
    pub(super) fn average_order(&self) -> Option<f64> {
        (self.positive_orders > 0).then(|| self.positive_spent / self.positive_orders as f64)
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
/// A Core row is exactly its own core — taken from `primary_core` rather than from the cores that
/// TRADED, which are the same list for every row a report produced and an empty one for an idle
/// row. An Exchange row is every CONFIGURED core reporting that exchange, not only the ones that
/// traded inside the period: the label names the exchange, and a filter that quietly dropped the
/// quiet cores would hide their open orders everywhere else. Configured order is preserved so the
/// payload is deterministic across refreshes.
///
/// Args:
///     row: Finished row, already carrying the cores that traded.
///     buckets: Configured cores per venue.
///     mode: Selected grouping axis.
///
/// Returns:
///     The row's click payload, never empty.
fn filter_cores(
    row: &MonitorRow,
    buckets: &HashMap<ExchangeId, Vec<CoreId>>,
    mode: GroupMode,
) -> Rc<[CoreId]> {
    if mode == GroupMode::Core {
        return Rc::from([row.primary_core].as_slice());
    }
    // The "unknown exchange" row groups cores no venue was reported for; a configured core that
    // reported nothing is not evidence it belongs there, so that row stays literal too.
    let bucket = row.venue.as_ref().and_then(|venue| buckets.get(&venue.id));
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

/// Fold a set of rows into the single row that states their combined values.
///
/// One definition for the table's grand-total footer and for a group's subtotal, so the two can
/// never answer the same question differently. Every field is additive except the newest trade, which is
/// the newest one AMONG the folded rows rather than a sum — the footer answers the same question
/// its rows do rather than mixing values from different instants. The `(close, core)` pair breaks a
/// tie deterministically: folding in list order would otherwise let the money change when the user
/// clicks a different sort column.
///
/// `primary_core`, `venue` and the filter payload are deliberately left at their defaults: a fold
/// is not one core and not one exchange, and the callers draw it without a logo or a click target.
///
/// Args:
///     rows: Rows to combine; an empty slice folds to an all-zero row.
///
/// Returns:
///     The combined row.
pub(super) fn fold_total(rows: &[MonitorRow]) -> MonitorRow {
    rows.iter().fold(MonitorRow::default(), |mut total, row| {
        total.profit += row.profit;
        total.trades += row.trades;
        total.wins += row.wins;
        total.positive_spent += row.positive_spent;
        total.positive_orders += row.positive_orders;
        if (row.last_close, row.last_core) > (total.last_close, total.last_core) {
            total.last_profit = row.last_profit;
            total.last_close = row.last_close;
            total.last_core = row.last_core;
        }
        total
    })
}

/// Build the zero rows of active cores the period holds no trade for.
///
/// Not a database question: the report simply has no row for a core that closed nothing, and the
/// list of cores that SHOULD be visible anyway is configuration. Only `active` cores qualify — a
/// core switched off in the connection table is not "quiet today", it is turned off, and a table
/// that listed it would grow with every core the user ever configured.
///
/// Args:
///     traded: Cores the snapshot already produced a row for.
///     live: Current configuration context.
///     labels: Localized fallback labels.
///
/// Returns:
///     One all-zero row per active configured core with no row yet, in canonical order.
fn idle_rows(
    traded: &HashSet<CoreId>,
    live: &LiveContext,
    labels: &RowLabels<'_>,
) -> Vec<MonitorRow> {
    live.core_order
        .iter()
        .copied()
        .filter(|core| live.active.contains(core) && !traded.contains(core))
        .map(|core| MonitorRow {
            name: core_row_name(core, live, None, labels),
            primary_core: core,
            venue: live
                .venues
                .get(&core)
                .filter(|venue| venue.is_nameable())
                .cloned(),
            // `cores` stays EMPTY on purpose: it is what traded, and it is what the arrival
            // highlight reads. A core with no trade in the period has nothing to light up for.
            ..MonitorRow::default()
        })
        .collect()
}

/// Resolve the visible label of one core row.
///
/// The configured name wins over the one the core reported: renaming a core in the connection table
/// is the user's own answer to "what is this", and a report row carrying its old name would take a
/// week of retention to catch up. A configured name that is BLANK is not an answer at all, so it is
/// discarded before the report name is considered — otherwise a core someone left unnamed would
/// fall past its perfectly good reported name to the numeric fallback.
///
/// Args:
///     core: The core's uid.
///     live: Current configuration context.
///     report: Name the report row carried, when it has one.
///     labels: Localized fallback labels.
///
/// Returns:
///     The configured name, the reported one, or the localized `Core <uid>` fallback.
fn core_row_name(
    core: CoreId,
    live: &LiveContext,
    report: Option<&str>,
    labels: &RowLabels<'_>,
) -> String {
    live.core_names
        .get(&core)
        .map(String::as_str)
        .filter(|name| !name.trim().is_empty())
        .or(report)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {core}", labels.core))
}

/// Group per-core report aggregates into visible Core or Exchange rows.
///
/// Args:
///     summary: Per-core additive database payload.
///     live: Current labels and canonical core order.
///     mode: Selected grouping axis.
///     include_idle: Whether active cores with no trade in the period get their zero row. Honoured
///         in Core mode only — an exchange row exists because a trade named that venue, and a quiet
///         core adds nothing to it but a name the row does not show.
///     labels: Localized fallback labels.
///
/// Returns:
///     Exact merged rows in canonical Core order or descending profit order for aggregate modes.
pub(super) fn grouped_rows(
    summary: &ProfitMonitorSummary,
    live: &LiveContext,
    mode: GroupMode,
    include_idle: bool,
    labels: RowLabels<'_>,
) -> Vec<MonitorRow> {
    let mut grouped = HashMap::<RowKey, MonitorRow>::new();
    for source in &summary.cores {
        let core = source.core_uid;
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
                    GroupMode::Core => core_row_name(core, live, report, &labels),
                    GroupMode::Exchange => venue_section_label(venue),
                },
                primary_core: core,
                venue: venue.cloned(),
                ..MonitorRow::default()
            })
            .push(source);
    }

    let mut rows = grouped.into_values().collect::<Vec<_>>();
    if include_idle && mode == GroupMode::Core {
        let traded: HashSet<CoreId> = rows.iter().map(|row| row.primary_core).collect();
        rows.extend(idle_rows(&traded, live, &labels));
    }
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
