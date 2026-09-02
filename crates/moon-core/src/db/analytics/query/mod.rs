//! Selection filters and the unified report source (`Query`, `unified_from`).

use rusqlite::Connection;
use rusqlite::types::Value;

use super::super::report_axis::ReportAxis;
use super::super::valuation::ValuationMode;
use super::super::{ProfitMetric, QuoteBreakdown, ReadFail, ReadResult, SideFilter};

mod mask;

pub(in crate::db::analytics) use mask::StrategyMask;

/// Basis used to place Summary's immediately preceding comparison window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreviousPeriodBasis {
    /// Shift by the selected zone's civil clock, preserving calendar-day comparisons across DST.
    #[default]
    Civil,
    /// Shift by elapsed seconds, preserving custom range duration across ambiguous picker bounds.
    Elapsed,
}

/// Render core uids as an inline SQL list.
///
/// Inlined rather than bound because these are integers this process allocated itself
/// (`AppConfig.next_uid`), never user text, and because the offset grouping produces a variable
/// number of branches — binding them would make the parameter positions depend on how many time
/// zones the fleet happens to span.
///
/// Args:
///     cores: Core uids to render.
///
/// Returns:
///     Comma-separated list ready to drop inside `IN (...)`.
fn core_list(cores: &[u64]) -> String {
    cores
        .iter()
        .map(|uid| (*uid as i64).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Selection filters shared by all Analytics tabs.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// The time axis every replicated report timestamp in this read passes through.
    ///
    /// Carries BOTH halves of the projection, and is the single authority for either: the per-core
    /// correction from the core's own wall clock to true UTC, and the user-selected zone those
    /// corrected instants are finally displayed in ([`ReportAxis::zone`]). A separate zone field
    /// beside it would be a second authority that could silently disagree across the many places a
    /// `Query` is built, which is exactly how a replica column ends up converted twice.
    ///
    /// A reader that genuinely holds an already-UTC value — `now`, or this query's own `from`/`to`
    /// bounds — asks for the zone alone and must NOT route through the correction.
    pub axis: ReportAxis,
    /// How Summary places the previous comparison window.
    pub previous_period_basis: PreviousPeriodBasis,
    /// UTC Unix seconds; `from < 0` means all history. `to` is exclusive.
    pub from: i64,
    pub to: i64,
    /// Selected cores (multi-select, as in Orders); empty means all cores. A caller holding a
    /// scope that is PRESENT but EMPTY (every named core has been filtered out) must send
    /// [`crate::config::NO_MATCH_CORE_UID`] rather than an empty list, or the read broadens to
    /// every core instead of returning none.
    pub cores: Vec<u64>,
    // NOTE: `period_predicate` below is the ONLY place `from`/`to` meet the replica's date column.
    // Both bounds are true UTC while the column is core-local, so they cannot be compared without
    // the axis, and adding a second comparison elsewhere would be a second axis decision.
    pub side: SideFilter,
    /// `None` means all, `Some(false)` means real, and `Some(true)` means emulated.
    /// A NULL column value counts as real, as it does in the Report window.
    pub emulator: Option<bool>,
    /// Scope: the SELECTED strategies as `(strategyid, core_uid)`. The list is split per
    /// core, so `None` in the second slot means "this strategy on any core" (a legacy key
    /// without one). Empty = every strategy.
    ///
    /// A LIST rather than one id because the tuner has Ctrl multi-select: the KPI matrix,
    /// the histogram and the sweep must all describe the SAME set the user sees highlighted
    /// and that Save writes to. Scoping to the clicked row alone was the bug where
    /// "plan vs fact" compared one strategy while N were selected.
    pub strategies: Vec<(i64, Option<u64>)>,
    /// Literal, case-insensitive substring matched against the effective strategy NAME.
    ///
    /// Empty or whitespace-only text adds no predicate and costs nothing. Independent of the exact
    /// keys above, so setting both narrows by their CONJUNCTION — the same rule the Report states
    /// on `ReportFilter::strategy_name_mask`, and the reason this is raw user text here: trimming,
    /// folding and escaping happen once, in [`StrategyMask::resolve`], so Analytics and the Report
    /// cannot disagree about what "matches" means.
    pub strategy_name_mask: String,
    /// Which quantity every profit figure is measured in: absolute quote money (`Quote`) or
    /// return on spent capital (`Percent`, the report's `Profit` column). Applied once in
    /// [`unified_from`]'s projected `pnl` column, so every reader below shares the choice.
    pub metric: ProfitMetric,
    /// Which conversion turns quote money into USDT: per-trade historical rates, or the latest
    /// known ones. Carried on the query rather than passed down as a parameter so every Analytics
    /// reader — summary, calendar, coin groups, the tuner source — inherits one answer; a reader
    /// that missed the parameter would render a differently converted number under the same label.
    pub valuation: ValuationMode,
    /// Convert a single-quote scope to USDT instead of reporting it in its own currency.
    ///
    /// Without this, the unit follows whatever the PERIOD happens to contain: a BTC-quoted core
    /// reads in BTC for a month that holds only its trades, and flips to USDT for a year that also
    /// holds a USDT trade — same core, same screen, two scales. Setting it pins the scale to USDT
    /// whenever every row can be valued; it cannot invent a rate, so an unpriced scope still falls
    /// back to its native quote rather than showing a converted figure that is partly guessed.
    pub prefer_usdt: bool,
}

/// Latest instant a persisted or picked bound may name.
pub const ANALYTICS_HORIZON_SECS: i64 = 4_102_444_800; // 2100-01-01 UTC
/// Widest period Analytics will read. Not a taste limit: the span sets the number of SQL windows
/// and the number of time buckets, and both are unbounded without it.
pub const ANALYTICS_MAX_SPAN_SECS: i64 = 1_262_304_000; // 40 years

impl Query {
    /// Clamp this query's period to the readable range, refusing what it cannot safely read
    /// rather than silently narrowing or emptying it.
    ///
    /// Deliberately does NOT floor `from`: a floor here would turn a replica whose rows all
    /// predate the floor into a query with no rows and a SUCCESSFUL empty result — the exact
    /// "nothing happened" standing in for "your data is unreadable" this whole goal exists to
    /// close. `to` is capped to [`ANALYTICS_HORIZON_SECS`], and the resulting SPAN is capped at
    /// [`ANALYTICS_MAX_SPAN_SECS`] — refused, not narrowed, because silently pulling `from`
    /// forward to fit the cap is the same silent-empty trap: it would drop real rows and report
    /// success. A span that wide is never something a preset or the date picker can produce, so
    /// refusing it loses no genuine user intent.
    ///
    /// Deliberately does NOT bound `to` against the wall clock either — an earlier revision
    /// capped it at `now + 1 day`, and that turned a period lying WHOLLY in the future (a user
    /// picking `from 01.01.2027 to 01.02.2027` before that date) into a rejected read instead of
    /// the empty one it must be: SQL simply finds no future rows, and there is nothing left to
    /// bound anyway once `to` is already capped by the horizon and the span by
    /// `ANALYTICS_MAX_SPAN_SECS`. Reading real wall-clock time here would also make every caller
    /// time-dependent: a future period would first be rejected until a moving cap reached its
    /// start, then silently narrowed before eventually receiving its full range. A read must not
    /// change that way merely because time passed.
    ///
    /// Returns:
    ///     Nothing on a still-readable period, or a classified failure for one that is not.
    pub(in crate::db) fn clamp_period(&mut self) -> ReadResult<()> {
        self.to = self.to.min(ANALYTICS_HORIZON_SECS);
        if self.to <= self.from {
            return Err(ReadFail::PeriodOutOfRange);
        }
        if self.to.saturating_sub(self.from) > ANALYTICS_MAX_SPAN_SECS {
            return Err(ReadFail::PeriodOutOfRange);
        }
        Ok(())
    }

    /// Resolve the "all history" sentinel: a negative `from` means the whole replica, which
    /// every single-scan tuner reader (the KPI matrix, the histogram, the sweep, the coin
    /// groups) represents as `from = 1` — the earliest possible second, distinct from the
    /// `min_closedate` floor the summary and calendar use. ONE definition of the sentinel so
    /// the coin table and the KPI matrix beside it cannot resolve "all history" to two spans.
    pub(in crate::db) fn floor_all_history(&mut self) {
        if self.from < 0 {
            self.from = 1;
        }
    }

    /// Refresh this query's axis from the connection it is about to read through.
    ///
    /// The zone is the USER's and travels down from the panel; the offsets are the MACHINE's and
    /// must be as fresh as the rows they correct. Resolving them here rather than at the panel
    /// keeps both inside the caller's pinned snapshot, so every branch of one read — the period
    /// predicate, the bucketing scalars, the rows themselves — describes one state of the world.
    ///
    /// FAILS CLOSED through [`ReportAxis::load`]: an unreadable measurement stops the read instead
    /// of quietly becoming the identity axis, which on a skewed core is the wrong-money axis.
    ///
    /// Args:
    ///     conn: Open reader or pinned snapshot this read runs on.
    ///
    /// Returns:
    ///     The axis to use for this read, or a classified read failure.
    pub(in crate::db) fn resolved_axis(&self, conn: &Connection) -> ReadResult<ReportAxis> {
        ReportAxis::load(conn, self.axis.zone())
    }

    /// [`resolved_axis`](Self::resolved_axis) for a caller that returns `rusqlite::Result`.
    ///
    /// The two SQL-building passes report `rusqlite::Error` and are re-classified by the retry
    /// wrapper above them, so a [`crate::db::ReadFail`] cannot travel out of them intact. What
    /// matters is preserved exactly: the read still ABORTS. Falling back to the identity axis is
    /// the one outcome this seam exists to prevent, and mapping the error keeps that impossible;
    /// only the failure's granularity is rebuilt one level up instead of carried.
    ///
    /// Args:
    ///     conn: Open reader or pinned snapshot this read runs on.
    ///
    /// Returns:
    ///     The axis to use, or a SQLite-shaped error carrying the original reason.
    pub(in crate::db) fn resolved_axis_sql(
        &self,
        conn: &Connection,
    ) -> rusqlite::Result<ReportAxis> {
        self.resolved_axis(conn).map_err(|fail| {
            // Deliberately NOT `DatabaseCorrupt`: `writer_should_stop` treats that code as a
            // reason to halt the writer, and an unreadable offset table is a READ-side refusal,
            // not a corrupt database the process must stop for.
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::Unknown,
                    extended_code: 1,
                },
                Some(format!("report time axis: {fail}")),
            )
        })
    }

    /// Build the period predicate that compares this query's bounds against the replica's own
    /// date column.
    ///
    /// `from`/`to` are true-UTC instants; the column is CORE-LOCAL wall clock. The offset closes
    /// that gap, and it goes on the BOUND rather than around the column — `r.closedate >= ?1 +
    /// 10800` leaves the column bare, so `idx_rep_core_close` still opens the scan, while
    /// `mt_to_utc(r.closedate) >= ?1` would force a full pass over a half-million-row table for
    /// the same answer.
    ///
    /// Takes the axis rather than reading `self.axis` so the caller's ONE
    /// [`resolved_axis`](Self::resolved_axis) result governs every part of the statement it is
    /// building — a second read of the stale field here would put two axes in one query.
    ///
    /// Cores are grouped by the offset applying at this window's own upper bound rather than at
    /// "now": there is no present-tense question here, only which instants the stored values
    /// represent, so the window's own instant is the honest one to resolve against. A fleet with
    /// nothing measured produces exactly the single ungrouped predicate this method replaced.
    ///
    /// `?1` and `?2` stay bound to the raw true-UTC bounds at every one of the two dozen call
    /// sites that bind them, so nothing about the parameter plumbing moves.
    ///
    /// Args:
    ///     axis: Time axis already resolved from the connection this query will read through.
    ///     alias: Table alias every physical report column is qualified with.
    ///
    /// Returns:
    ///     A complete boolean expression, already parenthesised where it needs to be.
    pub(in crate::db) fn period_predicate_on(&self, axis: &ReportAxis, alias: &str) -> String {
        let column = format!("{alias}.closedate");
        let window = |offset: i32| {
            if offset == 0 {
                format!("{column} >= ?1 AND {column} < ?2")
            } else {
                let offset = i64::from(offset);
                format!("{column} >= ?1 + {offset} AND {column} < ?2 + {offset}")
            }
        };
        let mut branches: Vec<String> = Vec::new();
        if self.cores.is_empty() {
            for (offset, cores) in axis.measured_groups(self.to) {
                let list = core_list(&cores);
                branches.push(format!(
                    "({alias}.core_uid IN ({list}) AND {})",
                    window(offset)
                ));
            }
            let measured = axis.measured_cores();
            if measured.is_empty() {
                branches.push(window(0));
            } else {
                let list = core_list(&measured);
                branches.push(format!(
                    "({alias}.core_uid NOT IN ({list}) AND {})",
                    window(0)
                ));
            }
        } else {
            for (offset, cores) in axis.groups(&self.cores, self.to) {
                let list = core_list(&cores);
                branches.push(format!(
                    "({alias}.core_uid IN ({list}) AND {})",
                    window(offset)
                ));
            }
        }
        let window = match branches.len() {
            0 => window(0),
            1 => branches.remove(0),
            _ => format!("({})", branches.join(" OR ")),
        };
        // `closedate > 0` is a row-STATE test, not an instant: a zero or negative value means the
        // trade never closed. It stays on the raw column on both sides of every offset group.
        format!("{window} AND {column} > 0")
    }

    /// Build mutually exclusive filters for one physical report source.
    ///
    /// A strategy selection becomes separate raw `strategyid = value`, `strategyid = 0`, and
    /// `strategyid IS NULL` branches. This lets SQLite search the physical strategy key before
    /// evaluating the effective-id expression for liquidation attribution. Duplicate scopes and
    /// core-specific scopes shadowed by the same any-core key are removed before expansion. IDs
    /// sharing a core are folded into one `IN` predicate, keeping each source at three branches
    /// even when Ctrl/Shift selection contains hundreds of strategies.
    ///
    /// Args:
    ///     period: Caller-supplied period predicate.
    ///     cols: Columns available on the physical source.
    ///     sid: Effective strategy-id expression used for liquidation candidates.
    ///     alias: Optional table alias used for every physical report column.
    ///     attribution: Whether the expression can reassign a zero/NULL liquidation row.
    ///     mask: Strategy-NAME mask already resolved against this read's connection. It lands
    ///         ABOVE the raw-strategy branching, so every branch carries it; a mask that reached
    ///         only the `direct` branch would let liquidation-attributed rows through unmasked.
    ///         With a mask set and no exact selection, the single branch now evaluates the
    ///         attribution `CASE` per row where it previously did not — the price of matching by
    ///         the same effective identity the rest of Analytics groups by, and paid only while a
    ///         mask is typed.
    ///
    /// Returns:
    ///     One complete predicate per disjoint raw-strategy branch.
    pub(super) fn where_branches(
        &self,
        period: &str,
        cols: &std::collections::HashSet<String>,
        sid: &str,
        alias: Option<&str>,
        attribution: bool,
        mask: &StrategyMask,
    ) -> Vec<String> {
        let has = |n: &str| cols.contains(n);
        let column = |name: &str| match alias {
            Some(alias) => format!("{alias}.{name}"),
            None => name.to_string(),
        };
        let mut w = String::from(period);
        if has("deleted") {
            w.push_str(&format!(" AND COALESCE({},0) = 0", column("deleted")));
        }
        if !self.cores.is_empty() {
            if !has("core_uid") {
                w.push_str(" AND 1=0");
                return vec![w];
            }
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND {} IN ({list})", column("core_uid")));
        }
        if has("isshort") {
            match self.side {
                SideFilter::All => {}
                SideFilter::Long => {
                    w.push_str(&format!(" AND COALESCE({},0) = 0", column("isshort")))
                }
                SideFilter::Short => {
                    w.push_str(&format!(" AND COALESCE({},0) = 1", column("isshort")))
                }
            }
        }
        if has("emulator") {
            match self.emulator {
                None => {}
                Some(false) => w.push_str(&format!(" AND COALESCE({},0) = 0", column("emulator"))),
                Some(true) => w.push_str(&format!(" AND COALESCE({},0) = 1", column("emulator"))),
            }
        }
        // The strategy-NAME mask, before the raw-strategy branching below so every branch inherits
        // it. A mask the source cannot answer fails CLOSED: matching nothing is the only honest
        // answer, and it is what the Report already does (`report_read::append_strategy_name_mask`).
        match mask {
            StrategyMask::Off => {}
            StrategyMask::Unavailable => {
                w.push_str(" AND 1=0");
                return vec![w];
            }
            StrategyMask::Match(_) if !has("core_uid") || !has("strategyid") => {
                w.push_str(" AND 1=0");
                return vec![w];
            }
            StrategyMask::Match(_) => {}
        }
        // Which id the mask is matched against. With attribution the effective expression, so a
        // LIQUIDATION row booked under the strategy named in it matches that strategy's name;
        // without it the physical column, which is what `sid` already reduces to there.
        // `None` on an unmasked read, and NOTHING below it is built either: `term`'s own
        // short-circuit would still have made the caller format both of its arguments first, so an
        // empty mask has to stop here rather than inside the callee.
        let masked_sid = (!matches!(mask, StrategyMask::Off)).then(|| {
            if attribution {
                format!("COALESCE({sid}, 0)")
            } else {
                format!("COALESCE({}, 0)", column("strategyid"))
            }
        });
        if self.strategies.is_empty() {
            if let Some(masked_sid) = &masked_sid {
                if let Some(term) = mask.term(masked_sid, &column("core_uid")) {
                    w.push_str(&term);
                }
            }
            return vec![w];
        }
        if !has("strategyid") {
            w.push_str(" AND 1=0");
            return vec![w];
        }

        let raw_sid = column("strategyid");
        let core_uid = column("core_uid");
        let mut any_core = std::collections::BTreeSet::new();
        for &(want, core) in &self.strategies {
            if core.is_none() {
                any_core.insert(want);
            }
        }
        let mut by_core: std::collections::BTreeMap<u64, std::collections::BTreeSet<i64>> =
            std::collections::BTreeMap::new();
        if has("core_uid") {
            for &(want, core) in &self.strategies {
                if let Some(core) = core.filter(|_| !any_core.contains(&want)) {
                    by_core.entry(core).or_default().insert(want);
                }
            }
        }
        let scoped_predicate = |value: &str, keep: fn(i64) -> bool| {
            let render_ids = |ids: Vec<i64>| match ids.as_slice() {
                [] => None,
                [id] => Some(format!("{value} = {id}")),
                _ => Some(format!(
                    "{value} IN ({})",
                    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
                )),
            };
            let mut terms = Vec::new();
            if let Some(term) =
                render_ids(any_core.iter().copied().filter(|id| keep(*id)).collect())
            {
                terms.push(term);
            }
            for (core, ids) in &by_core {
                if let Some(term) = render_ids(ids.iter().copied().filter(|id| keep(*id)).collect())
                {
                    terms.push(format!("({core_uid} = {} AND {term})", *core as i64));
                }
            }
            match terms.as_slice() {
                [] => None,
                [term] => Some(term.clone()),
                _ => Some(format!("({})", terms.join(" OR "))),
            }
        };

        // The `direct` branch already constrains the RAW key to a non-zero id, where the effective
        // expression cannot differ from it (the CASE only rewrites a zero), so the mask matches on
        // the raw column there and the physical-key index seek survives.
        let direct_mask = masked_sid
            .as_ref()
            .and_then(|_| mask.term(&format!("COALESCE({raw_sid}, 0)"), &core_uid));
        let residual_mask = masked_sid
            .as_ref()
            .and_then(|masked_sid| mask.term(masked_sid, &core_uid));
        let mut branches = Vec::new();
        if let Some(direct) = scoped_predicate(&raw_sid, |want| want != 0) {
            branches.push(format!(
                "{w} AND {direct}{}",
                direct_mask.as_deref().unwrap_or("")
            ));
        }
        let residual = if attribution {
            scoped_predicate(&format!("COALESCE({sid}, 0)"), |_| true)
        } else {
            scoped_predicate(&format!("COALESCE({raw_sid}, 0)"), |want| want == 0)
        };
        if let Some(residual) = residual {
            let mask_sql = residual_mask.as_deref().unwrap_or("");
            branches.push(format!("{w} AND {raw_sid} = 0 AND {residual}{mask_sql}"));
            branches.push(format!(
                "{w} AND {raw_sid} IS NULL AND {residual}{mask_sql}"
            ));
        }
        if branches.is_empty() {
            vec![format!("{w} AND 1=0")]
        } else {
            branches
        }
    }
}

/// Base columns projected from both report sources. The tuner's market fields
/// (`db::tuner::FIELDS`) are chained AUTOMATICALLY, so a new tuner field cannot be
/// omitted from the projection and make its SQL fail silently.
const UNIFIED_COLS: &[&str] = &[
    "core_uid",
    "core_name",
    "coin",
    "isshort",
    "buydate",
    "closedate",
    "profitbtc",
    "strategyid",
    "emulator",
    "spentbtc",
    "basecurrency",
    // Execution inputs. Money columns above are REPLACED by the active projection; these are raw
    // quantities and prices, so they stay native and any figure built from them must be converted
    // with `quote_rate` below before it meets a projected sum. `lev` is NOT listed: it already
    // arrives through `tuner::FIELDS`, and naming it twice would project two identical columns.
    "boughtq",
    "buyprice",
    "sellprice",
    // Funding is booked as a pseudo-order (`buydate == closedate`, entry price == exit price,
    // `spentbtc == boughtq`). It carries real money, so it belongs in profit — but it is not a
    // trade, and counting it would inflate trade counts, turnover and win rate alike.
    "sellreason",
];

/// Money projection resolved by quote coverage before one analytical scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::db) enum ProjectionMode {
    /// Preserve native report money for comparable scopes and lens-neutral subgroup scans.
    Native,
    /// Replace profit, spend, quote identity, and active PnL with historical USDT values.
    Usdt,
    /// The same replacement, but every trade converted at the latest known rate.
    UsdtCurrent,
    /// Use per-trade return on spent capital without historical FX.
    Percent,
}

impl ProjectionMode {
    /// Which conversion this projection applies, if it converts at all.
    ///
    /// Every decision downstream asks this rather than comparing against one variant, so adding a
    /// conversion cannot silently fall through to native money in a branch nobody updated.
    ///
    /// Returns:
    ///     The valuation mode to build SQL for, or `None` for a projection that converts nothing.
    pub(in crate::db) const fn valuation(self) -> Option<ValuationMode> {
        match self {
            Self::Usdt => Some(ValuationMode::Historical),
            Self::UsdtCurrent => Some(ValuationMode::Current),
            Self::Native | Self::Percent => None,
        }
    }

    /// Whether this projection reports money in USDT.
    ///
    /// Returns:
    ///     True for either conversion.
    pub(in crate::db) const fn is_usdt(self) -> bool {
        self.valuation().is_some()
    }

    /// The USDT projection matching one requested valuation mode.
    ///
    /// Args:
    ///     mode: Conversion the caller asked for.
    ///
    /// Returns:
    ///     The projection that applies it.
    pub(in crate::db) const fn usdt_for(mode: ValuationMode) -> Self {
        match mode {
            ValuationMode::Historical => Self::Usdt,
            ValuationMode::Current => Self::UsdtCurrent,
        }
    }
}

/// Read safe raw-money totals for one Analytics query scope.
///
/// This query intentionally ignores the active profit lens: split totals always
/// use raw `profitbtc`, while the same period, core, side, emulator, and strategy
/// predicates as the analytical source keep the scope exact.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Fully resolved Analytics query with concrete period bounds.
///
/// Returns:
///     Known quote buckets, unknown and complete row counts, and optional complete USDT coverage.
///
/// Errors:
///     Returns a classified report read failure when source discovery or either aggregate pass
///     cannot complete.
pub(in crate::db) fn quote_breakdown_on(
    conn: &Connection,
    q: &Query,
) -> ReadResult<QuoteBreakdown> {
    let sources = super::super::read_sources_res(conn)?;
    let valuation_attached = super::super::valuation::is_attached(conn);
    match quote_breakdown_attempt(conn, q, &sources, valuation_attached) {
        Ok(totals) => Ok(totals),
        Err(error)
            if valuation_attached
                && super::super::valuation::prove_derived_corruption(conn, &error) =>
        {
            let _ = conn.execute(
                &format!("DETACH DATABASE {}", super::super::valuation::SCHEMA),
                [],
            );
            quote_breakdown_attempt(conn, q, &sources, false).map_err(|retry_error| {
                super::super::read_fail::read_fail(
                    "analytics: quote breakdown native retry",
                    retry_error,
                )
            })
        }
        // The guard above already performed schema attribution for this exact error.
        Err(error) => Err(super::super::read_fail::read_fail(
            "analytics: quote breakdown",
            error,
        )),
    }
}

/// Execute one complete Analytics quote-total pass with fresh accumulators.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     q: Fully resolved Analytics query.
///     sources: Physical report sources discovered from `main`.
///     include_valuation: Whether the historical mode may join the attached derived cache; the
///         current-rate mode does not depend on it.
///
/// Returns:
///     Exact quote totals, optionally carrying complete USDT coverage.
///
/// Errors:
///     Returns the underlying SQLite error from any physical-source aggregate.
fn quote_breakdown_attempt(
    conn: &Connection,
    q: &Query,
    sources: &[super::super::ReadSource],
    include_valuation: bool,
) -> rusqlite::Result<QuoteBreakdown> {
    let has_names = strategies_attached(conn);
    // Resolved ONCE for this whole pass, above the source loop: the rows pass and this totals pass
    // must narrow to the same trades, and re-resolving per source would put two chances to differ
    // where there is currently one value.
    let mask = StrategyMask::resolve(conn, q)?;
    let mut groups = Vec::new();
    let mut coverage = super::super::valuation::CoverageAggregate::default();
    // Loop-invariant: current-rate coverage is publishable without the cache; historical coverage
    // is publishable only while the derived cache is attached.
    let valuation_present =
        q.valuation == super::super::valuation::ValuationMode::Current || include_valuation;
    // Hoisted above the loop for the same reason the mask is: every source in this pass must be
    // narrowed on ONE axis, and re-resolving per source would put two chances to differ where
    // there is currently one value.
    let axis = q.resolved_axis_sql(conn)?;
    let period = q.period_predicate_on(&axis, "r");
    for src in sources {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue;
        }
        let sid = effective_sid_expr("r", &src.cols, has_names);
        let attribution = liquidation_attribution_available(&src.cols, has_names);
        let where_branches =
            q.where_branches(&period, &src.cols, &sid, Some("r"), attribution, &mask);
        let (quote, group_by) = super::super::quote::trusted_quote_group("r", &src.cols);
        let source = super::super::report_read::source_partition(src);
        let valuation = super::super::valuation::projection(
            q.valuation,
            include_valuation,
            "r",
            &src.cols,
            source,
        );
        let joins = valuation
            .as_ref()
            .map(|parts| parts.joins.as_str())
            .unwrap_or("");
        let coverage_columns = valuation
            .as_ref()
            .map(|parts| format!(", {}", parts.aggregate_columns()))
            .unwrap_or_default();
        let sql = where_branches
            .iter()
            .map(|where_sql| {
                format!(
                    "SELECT {quote}, COALESCE(SUM({settled_profit}), 0.0), COUNT(*)\
                     {coverage_columns} \
                     FROM {} r{joins} WHERE {where_sql}{group_by}",
                    src.table,
                    settled_profit =
                        super::super::quote::settled_amount_expr("r", &src.cols, "profitbtc"),
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![q.from, q.to])?;
        while let Some(row) = rows.next()? {
            let raw = row.get::<_, Value>(0)?;
            let ordinal = super::super::quote::report_ordinal_from_value(&raw);
            let profit = row.get::<_, f64>(1)?;
            let orders = row.get::<_, i64>(2)?;
            groups.push((ordinal, profit, orders));
            if valuation.is_some() {
                coverage.add_row(row, 3)?;
            }
        }
    }
    let totals = QuoteBreakdown::from_groups(groups);
    // Publish coverage whenever the selected mode can build a projection: always for current rates,
    // and only with an attached cache for historical rates.
    Ok(if valuation_present {
        totals.with_valuation(coverage.finish())
    } else {
        totals
    })
}

/// Decide whether a source can attribute liquidation rows through strategy metadata.
///
/// Args:
///     cols: Columns available on the physical report source.
///     on: Whether the strategy metadata database is attached.
///
/// Returns:
///     Whether every routing column and at least one owner-name column are available.
pub(super) fn liquidation_attribution_available(
    cols: &std::collections::HashSet<String>,
    on: bool,
) -> bool {
    const NEEDED: [&str; 3] = ["strategyid", "channelname", "core_uid"];
    on && NEEDED.iter().all(|column| cols.contains(*column))
        && ["signaltype", "comment"]
            .iter()
            .any(|column| cols.contains(*column))
}

/// A row's strategy id, with LIQUIDATION rows attributed to the strategy named in them.
///
/// THE ONE PLACE. It is applied inside [`unified_from`] — both in the projection and in the
/// scope filter — so the column every outer query already reads as `o.strategyid` carries the
/// attributed value, and none of them needed changing. Two readers cannot drift because there
/// is only one definition of "which strategy is this row".
///
/// A liquidation arrives with `strategyid = 0` and `channelname = 'LIQUIDATION'` exactly. The
/// exactness matters: a substring test matches 15 696 rows on the real database against 319
/// real ones, because a strategy is NAMED `Liquidations_Short_250620_184426_318` and its name
/// appears both in `channelname` and inside the `sellreason` text of its ordinary trades.
///
/// The owner's name is in `signaltype` (`MainShotS  ( MoonShot )`), with `comment` as the
/// fallback — measured at 288/319 and 304/319. Cutting at the first bracket is enough; no
/// regex, and it evaluates only for rows the CASE has already narrowed.
///
/// Returns the plain column whenever attribution is off, the strategy database is not
/// attached, or the source lacks a column this needs — a source that cannot answer must not
/// be made to guess.
///
/// Args:
///     alias: SQL alias of the report source.
///     cols: Columns available on that source.
///     on: Whether attached strategy metadata may be used.
///
/// Returns:
///     A SQL expression yielding the effective signed strategy id.
pub(in crate::db) fn effective_sid_expr(
    alias: &str,
    cols: &std::collections::HashSet<String>,
    on: bool,
) -> String {
    let plain = format!("{alias}.\"strategyid\"");
    // EVERY column the expression names. `core_uid` belongs here as much as the rest: a legacy
    // source can lack it (the projection right below emits `NULL AS "core_uid"` for exactly
    // that case), and naming it anyway makes the branch fail to PREPARE — sinking the whole
    // window the moment the switch is turned on.
    if !liquidation_attribution_available(cols, on) {
        return plain;
    }
    let mut sources = Vec::new();
    for c in ["signaltype", "comment"] {
        if cols.contains(c) {
            sources.push(format!("NULLIF({alias}.\"{c}\",'')"));
        }
    }
    if sources.is_empty() {
        return plain;
    }
    sources.push("''".to_string());
    let raw = format!("COALESCE({})", sources.join(", "));
    // "MainShotS  ( MoonShot )" -> "MainShotS". The WHOLE trimmed value is tried first, so a
    // strategy whose own name contains a bracket matches itself instead of being truncated to
    // its prefix — and then possibly matching a DIFFERENT strategy that is named that prefix.
    let whole = format!("trim({raw})");
    let cut = format!(
        "trim(substr({raw}, 1, CASE WHEN instr({raw},'(') > 0 \
         THEN instr({raw},'(') - 1 ELSE length({raw}) END))"
    );
    // A name that matches nothing (a deleted strategy, or no name at all) yields 0 and the row
    // stays in "Manual" — the same place it was, which is the honest answer.
    // Preference expressed as NESTED COALESCE, not as `IN (…) ORDER BY <match> DESC`: SQLite
    // rejects a correlated reference inside a subquery's ORDER BY ("no such column: r.comment"),
    // so that form compiled, passed every string-matching test, and would have failed at
    // runtime the first time the switch was turned on.
    let lookup = |n: &str| {
        format!(
            "(SELECT st.strategy_id FROM strat.strategies st \
             WHERE st.core_uid = {alias}.\"core_uid\" AND st.deleted = 0 AND st.name = {n})"
        )
    };
    format!(
        "CASE WHEN COALESCE({alias}.\"strategyid\",0) = 0 \
         AND upper(trim(COALESCE({alias}.\"channelname\",''))) = 'LIQUIDATION' \
         THEN COALESCE({}, {}, 0) ELSE {plain} END",
        lookup(&whole),
        lookup(&cut)
    )
}

/// Build the unified replica-and-legacy `FROM` source with filters inside each
/// branch so the `closedate` indexes remain usable.
///
/// Missing columns project as NULL. `Ok(None)` means no source has received the
/// required schema; a failed schema probe remains an error because opening a
/// database does not validate its schema b-tree.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Period, filters, and profit metric projected into each source branch.
///
/// Returns:
///     Unified filtered source SQL, `None` when no source is ready, or a classified failure.
pub(in crate::db) fn unified_from(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
    let mode = if q.metric == ProfitMetric::Percent {
        ProjectionMode::Percent
    } else {
        ProjectionMode::Native
    };
    unified_from_mode(conn, q, mode)
}

/// Build the unified source under one previously resolved money projection.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Period and row filters.
///     mode: Native, historical USDT, current-rate USDT, or percent projection established by
///         preflight.
///
/// Returns:
///     Unified filtered source SQL, no ready source, or a classified read failure.
pub(in crate::db) fn unified_from_mode(
    conn: &Connection,
    q: &Query,
    mode: ProjectionMode,
) -> ReadResult<Option<String>> {
    let cols: Vec<&str> = UNIFIED_COLS
        .iter()
        .copied()
        .chain(super::super::tuner::FIELDS.iter().map(|s| s.col))
        .collect();
    // Attribute LIQUIDATION rows to the strategy named in the row, whenever the strategy
    // database is attached.
    //
    // A liquidation arrives with `strategyid = 0` and `channelname = 'LIQUIDATION'`, so it
    // lands in "Manual (no strategy)" and the strategy that actually took the loss never sees
    // it. The name IS there — `signaltype`/`comment` carry `MainShotS  ( MoonShot )` — and
    // matching it against `strat.strategies` by `(core_uid, name)` recovers the owner. Rows
    // that do not attach (a deleted strategy, or no parseable name) stay in "Manual" rather
    // than being guessed at.
    //
    // The Report strategy filter reuses this same expression, so opening a strategy from
    // Analytics includes the liquidation rows that contributed to its summary.
    //
    // Attachment is PROBED rather than passed down: the callers that attach the strategy
    // database do so on this same connection, and the tests deliberately do not. Not to be
    // confused with `super::super::summary`'s own `has_strat_names`, which uses the same probe
    // independently to resolve strategy display names.
    let has_names = strategies_attached(conn);
    // Resolved ONCE above the source loop, for the same reason the totals pass does it: this
    // string is the rows half of a pair that must describe the same trades.
    let mask = StrategyMask::resolve(conn, q).map_err(|error| {
        super::super::read_fail::read_fail("analytics: strategy name mask", error)
    })?;
    // Percent mode measures each trade as profit ÷ spent, so a trade without a positive
    // `spentbtc` has no percent at all.
    let pct = mode == ProjectionMode::Percent;
    let mut branches = Vec::new();
    for src in super::super::read_sources_res(conn)? {
        if !src.cols.contains("closedate") || !src.cols.contains("profitbtc") {
            continue; // The core schema has not arrived yet, so there is nothing to aggregate.
        }
        // A source without `spentbtc` (e.g. the legacy `closed_sell_reports`) can form no
        // percent, so in percent mode it contributes NOTHING — not a column of NULLs that the
        // COUNT(*)/COALESCE(pnl,0) consumers below would miscount as zero-profit losing trades.
        if pct && !src.cols.contains("spentbtc") {
            continue;
        }
        // The branch table is aliased so the attribution's correlated subquery can name the
        // OUTER row explicitly. Unqualified `core_uid` inside it would bind to `strat.strategies`
        // and silently match every strategy of that name on any core.
        let sid = effective_sid_expr("r", &src.cols, has_names);
        let attribution = liquidation_attribution_available(&src.cols, has_names);
        let source = super::super::report_read::source_partition(&src);
        // `true` for attachment: a USDT projection is only ever chosen after `scope_decision_on`
        // proved coverage on this same snapshot, and the current-rate mode needs no cache at all.
        let valuation = mode.valuation().and_then(|valuation_mode| {
            super::super::valuation::projection(valuation_mode, true, "r", &src.cols, source)
        });
        let proj = cols
            .iter()
            .map(|c| {
                if *c == "strategyid" && src.cols.contains(*c) {
                    // The attributed value is published UNDER THE ORIGINAL NAME, so every
                    // query outside this source keeps reading `o.strategyid` and gets it.
                    format!("{sid} AS \"strategyid\"")
                } else if mode.is_usdt() && *c == "profitbtc" {
                    format!(
                        "{} AS \"profitbtc\"",
                        valuation
                            .as_ref()
                            .expect("USDT projection has coverage SQL")
                            .profit_usdt
                    )
                } else if mode.is_usdt() && *c == "spentbtc" {
                    format!(
                        "{} AS \"spentbtc\"",
                        valuation
                            .as_ref()
                            .expect("USDT projection has coverage SQL")
                            .spent_usdt
                    )
                } else if mode.is_usdt() && *c == "basecurrency" {
                    "1 AS \"basecurrency\"".to_string()
                } else if matches!(*c, "profitbtc" | "spentbtc") && src.cols.contains(*c) {
                    // A COIN-M liquidation stores its amount in a unit that is not its currency,
                    // so every reader of this source gets the corrected number under the same name.
                    format!(
                        "{} AS \"{c}\"",
                        super::super::quote::settled_amount_expr("r", &src.cols, c)
                    )
                } else if *c == "basecurrency" && src.cols.contains(*c) {
                    // Published under the original name, like the attributed strategy id above, so
                    // every consumer of this unified source — the group quote split, the summary
                    // stream — reads the quote the row's money is actually in.
                    format!(
                        "({}) AS \"basecurrency\"",
                        super::super::quote::effective_ordinal_expr("r", &src.cols)
                    )
                } else if src.cols.contains(*c) {
                    format!("r.\"{c}\"")
                } else {
                    format!("NULL AS \"{c}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        // The active profit metric, projected ONCE here as `pnl` so every aggregation below and
        // the tuner sweep read one column. `Quote` is raw money; `Percent` is the report's
        // `Profit` column (profit ÷ spent × 100 = return on spent capital). The `spentbtc > 0`
        // filter appended below (percent mode) guarantees the ratio is finite and non-NULL, so
        // COUNT, SUM, wins, streaks and the top list all describe the SAME set of trades. Sign is
        // preserved (spent > 0), so win/loss, profit factor and best/worst stay correct on `pnl`.
        let pnl = if pct {
            format!(
                "{profit} / {spent} * 100.0 AS \"pnl\"",
                profit = super::super::quote::settled_amount_expr("r", &src.cols, "profitbtc"),
                spent = super::super::quote::settled_amount_expr("r", &src.cols, "spentbtc"),
            )
        } else if let Some(parts) = &valuation {
            format!("{} AS \"pnl\"", parts.profit_usdt)
        } else {
            format!(
                "{} AS \"pnl\"",
                super::super::quote::settled_amount_expr("r", &src.cols, "profitbtc")
            )
        };
        // USDT paid for one unit of this row's quote, projected ONCE beside `pnl` so a consumer can
        // convert a figure the projection does not replace — the notional built from `boughtq` and
        // the entry/exit prices, which no valuation column covers. `1.0` under a native or percent
        // projection keeps every such expression written the same way in both worlds.
        //
        // `per_row.rate` reads the `v` value join only, and `CoverageSql::joins` below already
        // carries it. Its sibling `per_row.source` does NOT qualify: it needs the `ra` provenance
        // join, which aggregates deliberately omit.
        let quote_rate = match &valuation {
            Some(parts) => format!("{} AS \"quote_rate\"", parts.per_row.rate),
            None => "1.0 AS \"quote_rate\"".to_string(),
        };
        // Whether this row's prices are denominated in the same currency as its money. A consumer
        // that multiplies quantity by price must gate on this, or an inverse contract makes the
        // figure wrong by the price of a bitcoin. The rule itself lives in `quote`, which owns
        // every comparison against the persisted label.
        let prices_in_money_quote = format!(
            "({}) AS \"prices_in_money_quote\"",
            super::super::quote::prices_share_money_quote_expr("r", &src.cols)
        );
        let joins = valuation
            .as_ref()
            .map(|parts| parts.joins.as_str())
            .unwrap_or("");
        let axis = q.resolved_axis(conn)?;
        let period = q.period_predicate_on(&axis, "r");
        for mut where_sql in
            q.where_branches(&period, &src.cols, &sid, Some("r"), attribution, &mask)
        {
            if pct {
                where_sql.push_str(" AND r.spentbtc > 0");
            }
            if let Some(parts) = &valuation {
                where_sql.push_str(&format!(" AND ({})", parts.valued));
            }
            branches.push(format!(
                "SELECT {proj}, {pnl}, {quote_rate}, {prices_in_money_quote}
                 FROM {} r{joins} WHERE {where_sql}",
                src.table,
            ));
        }
    }
    if branches.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({}) o", branches.join(" UNION ALL "))))
    }
}

/// Rows a period can never contain: the close date is absent or non-positive.
pub(super) const WHERE_UNDATED: &str = "(closedate IS NULL OR closedate <= 0)";

/// Is the strategy database available on this connection?
///
/// Probed rather than tracked: `open_reader` attaches it, but a caller may hand us a
/// connection it opened itself, and a second ATTACH under the same alias fails. Asking the
/// connection is the only answer that cannot go stale.
///
/// Args:
///     conn: SQLite connection that may carry the `strat` attachment.
///
/// Returns:
///     Whether the attached strategy table can execute a read.
pub(in crate::db) fn strategies_attached(conn: &Connection) -> bool {
    // EXECUTED, not merely prepared. `prepare` validates the schema and nothing else, so a
    // corrupt or unreadable strategies.sqlite passed this check and then failed mid-scan —
    // and because the attribution subquery is baked into the unified source, that failure
    // sank the whole summary instead of degrading to "no attribution". Running the statement
    // is what turns a broken strategy database back into a silently absent one.
    match conn.query_row("SELECT 1 FROM strat.strategies LIMIT 1", [], |_| Ok(())) {
        Ok(()) => true,
        // An EMPTY table is still a usable one: nothing will match, every liquidation stays
        // in "Manual", and that is a correct answer rather than a failure.
        Err(rusqlite::Error::QueryReturnedNoRows) => true,
        Err(e) => {
            log::debug!("analytics: strategies.sqlite unreadable, attribution off: {e}");
            false
        }
    }
}

/// Attach the optional strategies database used to enrich strategy names.
pub(in crate::db) fn attach_strategies(conn: &Connection) -> bool {
    let path = crate::config::paths::strategies_db_path();
    if !path.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS strat",
        path.to_string_lossy().replace('\'', "''")
    );
    // An absent file is normal and silent; failure to attach an existing file
    // is logged as a real enrichment fault.
    match conn.execute(&sql, []) {
        Ok(_) => true,
        Err(e) => {
            // Logged ONCE. It now runs per reader (every Report refresh included), and a
            // permanently broken file would otherwise write a warn line forever.
            use std::sync::Once;
            static SAID: Once = Once::new();
            SAID.call_once(|| log::warn!("analytics: strategies.sqlite did not attach: {e}"));
            false
        }
    }
}

/// Strategy scope, liquidation-attribution expression, and the flag-wiring contract.
#[cfg(test)]
mod tests;
