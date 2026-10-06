//! types items for report reads.

use super::*;

/// Query result containing column names and generic value rows for every column.
///
/// `cols` is a RUNTIME list from `PRAGMA table_info`, so core-added fields appear
/// without code changes: known columns keep canonical order and new ones follow.
pub struct ReportTable {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// `core_uid` for each row, parallel to `rows`.
    ///
    /// This service column is absent from `cols` and `DISPLAY_COLUMNS`, but lets a
    /// report coin click open the chart ON THE CORE that made the trade
    /// (`core_uid` equals the runtime `CoreId`).
    pub core_uids: Vec<u64>,
    /// `newrecid` (the replica replication key) for each row, parallel to `rows`.
    ///
    /// Also a hidden service column. It is the id the soft-delete protocol addresses, so the
    /// Report panel actions read it to build `set_report_rows_deleted`. Legacy rows, which have no
    /// `newrecid` and cannot be soft-deleted, carry `0` — never a real rec id.
    pub rec_ids: Vec<i64>,
}

/// Everything one Report totals read states: realized money, and the open positions beside it.
///
/// The two are separate FIELDS rather than one merged figure because they answer different
/// questions and must never be added together — [`Self::quotes`] is settled history, while
/// [`Self::open`] is what the market is showing right now and will change before it is a fact.
/// They also live here rather than as a field on [`QuoteBreakdown`], which Analytics builds for
/// its own surfaces and would carry an eternally empty open tally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportTotals {
    /// Realized profit per known currency over CLOSED rows only, plus traded volume, entry-spend
    /// subtotals, and coverage.
    pub quotes: QuoteBreakdown,
    /// Unrealized money on the positions still running, counted apart from every figure above.
    pub open: crate::db::OpenPositions,
}

/// One durable closed trade projected for an exact chart core and market.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartTradeRecord {
    /// Stable report-replica record identity.
    pub record_id: i64,
    /// Runtime core identity that owns this trade.
    pub core_uid: u64,
    /// Coin identity stored by the originating core.
    pub coin: String,
    /// Entry timestamp, in seconds on the CORE's own wall clock — NOT true UTC. Lift it through
    /// `ReportAxis::to_utc(secs, core_uid)` before treating it as a Unix instant; see the
    /// `report_axis` module for why.
    pub buy_date: i64,
    /// Close timestamp, same core-local caveat as `buy_date` above.
    pub close_date: i64,
    /// Raw `buydatems` as the core stored it: core-local MILLISECONDS, same clock and same
    /// caveat as `buy_date` above.
    ///
    /// `None` when the source predates the column or the cell is NULL — never derived from
    /// `buy_date`, because an absence is the wire's own statement that the core had no
    /// millisecond to give. Resolve it against the seconds column with
    /// [`ReportStamp::resolve`]; never read it directly.
    pub buy_ms: Option<i64>,
    /// Raw `closedatems`, same caveats as [`Self::buy_ms`].
    ///
    /// A stored `0` is the wire's "this row is STILL OPEN" sentinel, not an instant;
    /// [`ReportStamp::resolve`] is what keeps it from being read as 1970.
    pub close_ms: Option<i64>,
    /// When the EXIT order was created (`SellSetDate`) — not when it filled, that is
    /// [`Self::close_date`]. Same core-local seconds and the same caveat as `buy_date`. Where the
    /// exit line of this trade begins when the core archived none: the line was placed here and
    /// filled at the close, and a line that never moved is still a line. `0` when the source
    /// predates the column, and the exit is then taken to have been placed at the entry.
    pub sell_set_date: i64,
    /// Raw `sellsetdatems`, same caveats as [`Self::buy_ms`].
    pub sell_set_ms: Option<i64>,
    /// Raw `buysetdatems` — when the core CREATED the entry order, core-local milliseconds like
    /// [`Self::buy_ms`]. Filed by cores since 2026-09-21 and never backfilled: `None` for an older
    /// row, a zero, or a source without the column. Where the entry line of this trade begins
    /// when the core archived none — it archives an entry line only once the order moved, so an
    /// order that stood at its price from creation has no line but still has a placement.
    pub buy_set_ms: Option<i64>,
    /// `buycorridordown` / `buycorridorup` — the entry corridor the core last saved for a MoonShot
    /// (or managed MoonHook) order, as absolute prices under their own names, whatever their
    /// numeric order. `None` unless both are positive prices: zero is the column's "unavailable".
    pub corridor: Option<(f64, f64)>,
    /// Entry price.
    pub buy_price: f64,
    /// Exit price.
    pub sell_price: f64,
    /// Filled quantity reported by the core.
    pub quantity: f64,
    /// Whether the trade is short.
    pub is_short: bool,
    /// Whether an EMULATOR order made this trade, rather than a live one.
    ///
    /// Carried per row so the chart's trade-kind checkboxes can hide marks at DRAWING time. The
    /// alternative — narrowing the query itself — would make a display toggle decide which rows were
    /// read, and since the row cap is applied after the filter, hiding emulator trades would free
    /// slots and surface OLDER REAL trades that had been truncated away. A checkbox must not change
    /// what the history contains.
    ///
    /// OPTIONAL at the source, exactly like [`Self::profit`]: a replica whose table predates the
    /// `emulator` column reports every trade as REAL. That direction is deliberate — hiding real
    /// trades on old data is the unrecoverable error, while showing an emulated one as real is
    /// visible and recoverable. On such a replica the "emulator trades" checkbox appears inert,
    /// which is the correct failure.
    pub emulator: bool,
    /// Realized profit as the row SETTLED it, or `None` when this source carries no profit column.
    ///
    /// Read through `quote::settled_amount_expr`, the same correction the Report grid and the
    /// footer apply, so a COIN-M liquidation is not off by its own entry price. An absence is
    /// never a zero: a legacy source without `profitbtc` still returns every trade, and the hover
    /// card says the figure is unknown rather than printing a profit of nothing.
    pub profit: Option<f64>,
    /// Currency [`Self::profit`] is denominated in, decided by `quote::effective_ordinal_expr`.
    ///
    /// The ONE place a row's currency is decided, and it is not derivable from the coin: COIN-M
    /// rows share a coin spelling with USD-M while settling in BTC. Carried beside the amount
    /// because a bare number labelled with the wrong ticker is worse than no number.
    pub quote: Option<QuoteCurrency>,
    /// Realized profit as a PERCENTAGE of the amount spent, or `None` when either leg is missing.
    ///
    /// Both legs are settled amounts, so a COIN-M liquidation divides like for like — the exact
    /// definition the Report's own profit-percent column already uses. Unitless, and therefore
    /// readable even where [`Self::quote`] could not be resolved.
    pub profit_pct: Option<f64>,
    /// The row's `ReportUID`: the core's own immutable identity for this trade, which survives a
    /// database copy and is the key its archived order traces are filed under.
    ///
    /// OPTIONAL at the source like [`Self::emulator`]: a replica whose table predates the column,
    /// a core too old to send it, or a row replicated before the core reported it (stored as 0,
    /// the replica's local default) all yield `None` — the honest answer "this trade cannot be
    /// asked about", never a zero to be sent as a key. Neither [`Self::record_id`] nor an order
    /// uid may stand in for it.
    pub report_uid: Option<i64>,
}

impl ChartTradeRecord {
    /// The entry stamp to use, preferring the millisecond column when the core supplied one.
    ///
    /// Returns:
    ///     The typed core-local stamp for this row's entry.
    pub fn buy_stamp(&self) -> ReportStamp {
        ReportStamp::resolve(self.buy_date, self.buy_ms)
    }

    /// The exit stamp to use, preferring the millisecond column when the core supplied one.
    ///
    /// Returns:
    ///     The typed core-local stamp for this row's exit.
    pub fn close_stamp(&self) -> ReportStamp {
        ReportStamp::resolve(self.close_date, self.close_ms)
    }

    /// The stamp of the exit order's CREATION, preferring the millisecond column when the core
    /// supplied one, and falling back to the entry when the source has no `SellSetDate` at all.
    ///
    /// Returns:
    ///     The typed core-local stamp for the moment this row's exit order was placed.
    pub fn sell_set_stamp(&self) -> ReportStamp {
        if self.sell_set_date > 0 {
            ReportStamp::resolve(self.sell_set_date, self.sell_set_ms)
        } else {
            self.buy_stamp()
        }
    }

    /// The stamp of the entry order's CREATION, when the core filed one (`buysetdatems`). No
    /// seconds counterpart exists, so there is nothing to fall back on: `None` means the row does
    /// not say where its entry was placed.
    ///
    /// Returns:
    ///     The typed core-local stamp, or `None` for a row without the column's value.
    pub fn buy_set_stamp(&self) -> Option<ReportStamp> {
        self.buy_set_ms
            .filter(|ms| *ms > 0)
            .map(ReportStamp::Millis)
    }
}

/// Bounded durable chart-history result with explicit truncation state.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartTradeHistory {
    /// Newest-first records in the exact requested scope.
    pub records: Vec<ChartTradeRecord>,
    /// Whether at least one older matching record was omitted by the cap.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Long,
    Short,
}

/// Which quantity every profit figure in the Analytics window is measured in.
///
/// `Quote` uses raw `profitbtc` only when every contributing row has one known quote.
/// `Percent` measures each trade as `profitbtc / spentbtc * 100` — the exact formula of the
/// MoonBot report's `Profit` column: return on the capital spent, independent of order size.
/// The choice is a per-`Query` lens, applied once in the source projection (see
/// `analytics::unified_from`), so every aggregation and the tuner sweep read the same metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfitMetric {
    /// Absolute money in one exact quote currency.
    #[default]
    Quote,
    /// Return on spent capital in percent — the report's `Profit` column.
    Percent,
}

/// Which trades one report query returns: closed, open, or both.
///
/// A trade is CLOSED once it carries a usable positive `closedate`, and OPEN until then. The two
/// are not the same kind of fact and that is why this is an enum rather than a pair of flags: a
/// closed trade is a historical event that a date window can contain, while an open one is the
/// present state of a position and belongs to no window at all. Representing both as booleans
/// would admit the meaningless "closed only, but include the open ones" state.
///
/// The default is [`Self::ClosedAndOpen`], which is what an unset filter has always meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowScope {
    /// Only trades with a usable positive `closedate` — Analytics' closed-trade universe.
    ///
    /// Every boundary that publishes durable history takes this arm: chart trade history, the
    /// strategy purge scan, and the Analytics-owned scoped Report window.
    Closed,
    /// Closed trades inside the date window, plus every open position regardless of the window.
    #[default]
    ClosedAndOpen,
    /// Only trades still running. The date window never applies to them.
    ///
    /// Reached through the Report's second row pass and its totals aggregate; a caller asking for
    /// the present state alone would set it too.
    Open,
    /// Closed trades inside the window, plus open positions only where the window still reaches
    /// the present ON THAT CORE'S OWN CLOCK.
    ///
    /// This is an INTENT, not a resolved answer, and it is a separate variant because the answer
    /// is no longer single-valued. `date_to` is compared against a CORE-LOCAL column while "now"
    /// is this machine's true UTC, so one window can have demonstrably ended for a core running
    /// four hours behind and still be current for one running three ahead. Resolving it in the UI
    /// — as this scope's predecessor did — forces one verdict onto a fleet that does not share
    /// one, which is why the decision moved down to [`append_row_scope`], the one place that
    /// knows both the axis and which cores are in play.
    ///
    /// The asymmetry that governs the per-group decision is unchanged: admitting an open row into
    /// a window that had already ended shows a position the user can see is still running, while
    /// DROPPING one silently removes money from a report that still looks complete.
    ClosedAndOpenIfCurrent,
    /// The OPEN half of [`Self::ClosedAndOpenIfCurrent`], for the two-pass row query alone.
    ///
    /// The row query runs open and closed as separate passes so the open block can carry its own
    /// newest-first order and its own guaranteed slots. That split needs a scope meaning "open
    /// rows, but only from the cores whose window still reaches the present" — which plain
    /// [`Self::Open`] cannot say, since it deliberately ignores the window entirely. No caller
    /// outside that splitter sets this.
    OpenIfCurrent,
}

/// Which timestamp a bounded Report period is measured against.
///
/// The default is [`Self::CloseDate`], which is what every Report query has always meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeriodBasis {
    /// The period bounds apply to `closedate`; open positions are admitted by the "window still
    /// reaches the present" rule of [`RowScope::ClosedAndOpenIfCurrent`].
    #[default]
    CloseDate,
    /// The period bounds apply to `buydate` for every row, open positions included: a position
    /// enters the window by when it was opened, like any closed trade.
    OpenDate,
}

/// Whether the Report coin field asks for an exact ticker rather than a substring.
///
/// A raw value ending in a space means "the ticker ends here": it matches that ticker and its
/// contract tails only. Only a TRAILING space counts; the check runs on the untrimmed value.
///
/// Args:
///     raw: The coin field exactly as typed, before any trim.
///
/// Returns:
///     `true` when the value names an exact ticker.
pub fn report_coin_is_exact(raw: &str) -> bool {
    raw.ends_with(' ') && !raw.trim().is_empty()
}

/// Escape the LIKE wildcards `%`, `_` and the escape character itself with a backslash.
pub(super) fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Complete filter shared by Report rows, totals, export, and strategy discovery.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportFilter {
    /// Selected cores for the multi-select filter; empty means all cores. A caller holding a
    /// scope that is PRESENT but EMPTY (every named core has been filtered out) must send
    /// [`crate::config::NO_MATCH_CORE_UID`] rather than an empty list, or the read broadens to
    /// every core instead of returning none.
    pub core_uids: Vec<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    /// Report coin field as typed; a trailing space asks for an exact ticker
    /// ([`report_coin_is_exact`]), otherwise a substring match.
    pub coin: String,
    /// Exact case-insensitive coin identities used by chart history.
    ///
    /// `None` keeps the Report substring filter above. `Some` replaces it with an exact set;
    /// an explicit empty set matches no rows so a lost market identity cannot widen the query.
    pub exact_coins: Option<Vec<String>>,
    pub side: SideFilter,
    /// Emulator orders: `None` selects all, `Some(false)` only real orders, and
    /// `Some(true)` only emulator orders. A NULL column value counts as real.
    pub emulator: Option<bool>,
    /// Soft-deleted trades (the core-supplied `deleted` column): `false` hides them,
    /// `true` shows ONLY them. A NULL column value counts as not deleted, matching
    /// the analytics filter; a source without the column holds no soft-deleted rows.
    pub deleted_only: bool,
    /// Which trades this query returns — see [`RowScope`].
    pub rows: RowScope,
    /// Which timestamp the period bounds apply to — see [`PeriodBasis`].
    pub period_basis: PeriodBasis,
    /// Time axis the replicated date columns are read on.
    ///
    /// Carried on the filter rather than loaded inside each query for the same reason
    /// [`Self::valuation`] is: the rows, the totals, the export and the RENDERED cell all take
    /// this one value, so a window built on one axis can never disagree with a timestamp printed
    /// on another. A default-constructed axis is the identity, which is what every caller that
    /// has not yet been given one already means.
    pub axis: crate::db::ReportAxis,
    /// Exact strategy identities; `None` selects all strategies, while `Some` remains constrained.
    ///
    /// The core is part of every key because strategy ids repeat across cores. An explicit empty
    /// collection intentionally matches no rows so a lost/stale selection cannot broaden a query.
    pub strategies: Option<Vec<ReportStrategyKey>>,
    /// Strategy-name query in the shared `moon_core::strategy_query` syntax (comma = OR, space =
    /// AND, `!word` = exclude, Unicode caseless), matched against the effective strategy name.
    ///
    /// Text that parses to an empty query adds no predicate. This stays independent of the exact
    /// strategy keys above, so using both filters narrows by their conjunction.
    pub strategy_name_mask: String,
    /// Which conversion the three USDT columns and the totals row apply.
    ///
    /// Carried on the filter rather than passed as a parameter because rows, totals and export all
    /// already receive this one value: a mode that reached the rows but not the totals would print
    /// a footer that does not sum the column above it.
    pub valuation: ValuationMode,
    /// Current names of the configured cores; the `core_name` column shows these and sorts by them.
    ///
    /// Carried on the filter for the reason [`Self::valuation`] is: the rows and the export both
    /// read this one value, so the file cannot name a core differently from the grid. Empty serves
    /// the stored names, which is what a caller without the configuration means.
    pub core_names: crate::db::CoreNames,
}

/// Exact report strategy identity across all connected cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportStrategyKey {
    /// Runtime core identity stored by the report replica.
    pub core_uid: u64,
    /// Delphi-signed strategy id stored in reports and `strategies.sqlite`.
    pub strategy_id: i64,
}

/// Strategy option shown by the Report filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportStrategy {
    /// Exact database identity used by the filter.
    pub key: ReportStrategyKey,
    /// Strategy name from `strategies.sqlite`, or the numeric id when metadata is unavailable.
    pub name: String,
}
