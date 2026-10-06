//! scope items for report reads.

use super::*;

/// Return the canonical "this row is closed" test for one aliased source.
///
/// The type check is load-bearing rather than decorative: SQLite orders TEXT above every number,
/// so a bare `closedate > 0` counts an unparseable timestamp as a close time. An unparseable
/// value is not a close time, so it reads as still open — the same judgement the traded-volume
/// eligibility test has always made, and this is now the ONE place either of them spells it.
///
/// Args:
///     cols: Columns available on this source.
///
/// Returns:
///     The predicate, or `None` when the source cannot express `closedate` at all.
pub(in crate::db) fn closed_row_predicate(
    cols: &std::collections::HashSet<String>,
) -> Option<String> {
    cols.contains("closedate")
        .then(|| closed_test_sql(CLOSEDATE))
}

/// Spell the closed test over one column expression.
///
/// Shared with the replica's open-rows partial index, whose `WHERE` must be this exact
/// expression for SQLite to use it.
///
/// Args:
///     col: The close-date column expression, aliased or bare.
///
/// Returns:
///     The parenthesised "has a numeric close time" test.
pub(crate) fn closed_test_sql(col: &str) -> String {
    format!("(typeof({col}) IN ('integer','real') AND {col} > 0)")
}

/// The close-date column as the row-scope predicates spell it.
pub(super) const CLOSEDATE: &str = "r.\"closedate\"";

/// The same column with SQLite's unary plus: identical value and type, but no longer a term the
/// planner may use as an index bound (see the disjunctions in [`append_row_scope`]).
const UNINDEXED_CLOSEDATE: &str = "+r.\"closedate\"";

/// Take every copy of the closed test's `closedate > 0` in a disjunction off the index.
///
/// Every offset group's branch carries the same `closedate > 0`; SQLite folds the identical terms
/// into one derived `closedate > 0` and may pick it as the index's lower bound over the real
/// window, walking the replica's whole history. The window terms stay indexable.
///
/// Args:
///     branches: The joined branches of one disjunction.
///
/// Returns:
///     The same predicate with the positivity test spelled on [`UNINDEXED_CLOSEDATE`].
fn close_test_off_index(branches: &str) -> String {
    branches.replace(
        &format!("{CLOSEDATE} > 0"),
        &format!("{UNINDEXED_CLOSEDATE} > 0"),
    )
}

/// Return the canonical "this row is still open" test for one aliased source.
///
/// Built as the literal negation of [`closed_row_predicate`] so the two partition every row
/// exactly once by construction rather than by two spellings agreeing. `typeof(NULL)` is
/// `'null'`, so the inner expression is FALSE rather than NULL for an absent close time and no
/// three-valued logic escapes into the surrounding `OR`.
///
/// Args:
///     cols: Columns available on this source.
///
/// Returns:
///     The predicate, or `None` when the source cannot express `closedate` at all.
pub(super) fn open_row_predicate(cols: &std::collections::HashSet<String>) -> Option<String> {
    closed_row_predicate(cols).map(|closed| format!("(NOT {closed})"))
}

/// Decide whether a report period reaches the present, and therefore admits open positions.
///
/// An open position has no `closedate`, so no date window can contain it as an event. What
/// decides its membership is whether the window reaches NOW: a period ending in the past is a
/// retrospective, where a still-running position would be a statement about a time it did not
/// hold. The period's LOWER bound is deliberately not consulted — a position opened last week and
/// still running belongs in "today" precisely because it is present state rather than history.
///
/// # Why the comparison is deliberately slack
///
/// `date_to` is resolved on the axis of the column it filters — the CORE's own wall clock — while
/// `now` is this machine's true UTC. `offset_secs` is what makes them comparable: the group's
/// cores read `now` as `now + offset_secs` on their own clocks, so that is the instant the bound
/// is tested against. A group with no measured offset passes `0` and lands exactly on the naive
/// comparison, which is correct for it.
///
/// This replaced a version that widened the comparison by the widest real time-zone offset in
/// BOTH directions, because it could not tell which core a row came from. That was deliberately
/// generous — the two ways to be wrong are not symmetric, since admitting an open row into a
/// window that had already ended shows a position the user can see is still running, while
/// DROPPING one silently removes money from a report that still looks complete. With the offset
/// known per group the generosity is no longer needed, and the slack it cost goes away.
///
/// Args:
///     date_to: Inclusive upper bound of the period, or `None` for an unbounded one.
///     now: Current Unix timestamp in seconds, on this machine's true UTC.
///     offset_secs: Seconds east of UTC on the clock of every core in this group.
///
/// Returns:
///     [`RowScope::ClosedAndOpen`] for a period still reaching the present on that clock,
///     [`RowScope::Closed`] for one that has demonstrably already ended there.
pub fn open_rows_for_bound(date_to: Option<i64>, now: i64, offset_secs: i32) -> RowScope {
    let ended = now.saturating_add(i64::from(offset_secs));
    match date_to {
        Some(to) if to < ended => RowScope::Closed,
        _ => RowScope::ClosedAndOpen,
    }
}

/// Append the row-scope predicate and the date window, which are ONE decision.
///
/// They are appended together because the window only ever applied to closed rows: an open
/// position carries no `closedate` to compare, so binding it to the window is what used to drop
/// it from every bounded period. Under [`RowScope::ClosedAndOpen`] the window therefore
/// constrains the closed side alone and the open side rides past it.
///
/// A source that cannot express `closedate` degrades per arm, and the asymmetry is deliberate.
/// `Closed` and `Open` both fail CLOSED — a source that cannot prove a row's state must not
/// assert it — while `ClosedAndOpen` emits nothing at all, which is exactly what an unset filter
/// did before this predicate existed and keeps a pre-schema replica showing its rows.
///
/// # The coarse range
///
/// With two or more offset groups and a bounded period on both sides, the closed branches share
/// one leading `closedate` range spanning the union of their shifted windows. It is a strict
/// SUPERSET of every branch's own window, so it admits no row those branches would not admit
/// themselves; what it buys is one index seek for the whole disjunction instead of one per
/// branch, measured at 1.94x over a 12-core half-million-row replica at four distinct zones.
///
/// It leads the CLOSED disjunct ALONE, never the whole predicate. An open position carries no
/// `closedate` at all, so a range in front of everything would drop every open row -- money
/// vanishing from a report that still looks complete, which is the exact failure the closed/open
/// asymmetry above exists to prevent.
///
/// Args:
///     sql: Predicate buffer being built.
///     params: Ordered bound values being built.
///     f: Complete Report filter.
///     cols: Columns available on this source.
pub(super) fn append_row_scope(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
) {
    if f.period_basis == PeriodBasis::OpenDate && (f.date_from.is_some() || f.date_to.is_some()) {
        append_open_basis_scope(sql, params, f, cols);
        return;
    }
    // Open rows carry no `closedate`, so neither the window nor the axis reaches them: this arm
    // is offset-independent and stays exactly the single-branch shape it always was.
    if f.rows == RowScope::Open {
        match open_row_predicate(cols) {
            Some(open) => sql.push_str(&format!(" AND {open}")),
            None => sql.push_str(" AND 1=0"),
        }
        return;
    }
    // Read once for the whole predicate, the same way the UI used to read it once for the whole
    // filter: two branches resolving "does this window still reach the present" against two
    // different instants would be a difference nothing on screen could explain.
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let groups = offset_groups(f, now);
    let mut parts: Vec<GroupPredicate> = Vec::new();
    for (offset, cores) in &groups {
        let Some(guard) = group_guard(cores, f) else {
            continue;
        };
        // The bounds are true-UTC instants and the column is core-local, so the group's offset is
        // added to the BOUND. Converting the column instead would be the same arithmetic and would
        // cost the index.
        let mut window = String::new();
        let mut bounds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut shifted_from: Option<i64> = None;
        let mut shifted_to: Option<i64> = None;
        if let Some(from) = f.date_from {
            let shifted = crate::db::ReportAxis::shift_bound(from, *offset);
            window.push_str(&format!(" AND {CLOSEDATE} >= ?"));
            bounds.push(Box::new(shifted));
            shifted_from = Some(shifted);
        }
        if let Some(to) = f.date_to {
            let shifted = crate::db::ReportAxis::shift_bound(to, *offset);
            window.push_str(&format!(" AND {CLOSEDATE} <= ?"));
            bounds.push(Box::new(shifted));
            shifted_to = Some(shifted);
        }
        let current = open_rows_for_bound(f.date_to, now, *offset) == RowScope::ClosedAndOpen;
        if f.rows == RowScope::OpenIfCurrent {
            // A group whose window has already ended contributes NO open rows, so it contributes
            // no branch at all. Dropping the branch rather than emitting a false one is what lets
            // the empty case below fail closed.
            if !current {
                continue;
            }
            // The open test itself is hoisted once after the loop; the group keeps its guard.
            parts.push(GroupPredicate {
                guard,
                ..GroupPredicate::default()
            });
            continue;
        }
        let resolved = match f.rows {
            RowScope::ClosedAndOpenIfCurrent if current => RowScope::ClosedAndOpen,
            RowScope::ClosedAndOpenIfCurrent => RowScope::Closed,
            other => other,
        };
        let mut part = GroupPredicate {
            guard,
            ..GroupPredicate::default()
        };
        match resolved {
            RowScope::Closed => match closed_row_predicate(cols) {
                Some(closed) => {
                    part.closed = Some(format!("{closed}{window}"));
                    part.bounds = bounds;
                    part.shifted_from = shifted_from;
                    part.shifted_to = shifted_to;
                }
                None => part.closed = Some("1=0".to_string()),
            },
            RowScope::ClosedAndOpen => {
                match (closed_row_predicate(cols), open_row_predicate(cols)) {
                    (Some(closed), Some(open)) => {
                        part.closed = Some(format!("{closed}{window}"));
                        part.bounds = bounds;
                        part.shifted_from = shifted_from;
                        part.shifted_to = shifted_to;
                        part.open = Some(open);
                    }
                    // Without the column there is no row state to test and no window to apply.
                    // Emitting nothing is what an unset filter always did and is what keeps a
                    // pre-schema replica showing its rows. Nothing has been bound yet, so this
                    // leaves both buffers exactly as it found them.
                    _ => return,
                }
            }
            // Every other variant was handled before this match: `Open` and `OpenIfCurrent`
            // returned or continued above, and `ClosedAndOpenIfCurrent` resolved into one of the
            // two arms here.
            RowScope::Open | RowScope::OpenIfCurrent | RowScope::ClosedAndOpenIfCurrent => return,
        }
        parts.push(part);
    }
    if f.rows == RowScope::OpenIfCurrent {
        // Every group carries the same open test and no bounds, so it is hoisted out of the
        // disjunction: `A AND g1 OR A AND g2` is `A AND (g1 OR g2)`, and only a top-level `A`
        // matches `idx_rep_open`'s WHERE. It is the plain `open_row_predicate` text, never passed
        // through `close_test_off_index`: a partial index is matched structurally.
        if parts.is_empty() {
            sql.push_str(" AND 1=0");
            return;
        }
        let open = open_row_predicate(cols).unwrap_or_else(|| "1=0".to_string());
        let guards = parts
            .iter()
            .map(|p| format!("({}1=1)", p.guard))
            .collect::<Vec<_>>();
        if guards.len() == 1 {
            sql.push_str(&format!(" AND {open} AND {}", guards[0]));
        } else {
            sql.push_str(&format!(" AND {open} AND ({})", guards.join(" OR ")));
        }
        return;
    }
    if let Some((from, to)) = coarse_range(&parts) {
        // Inside the disjunction the column is spelled `+r."closedate"`: same value, same type,
        // and every comparison on it sits behind the `typeof` test, so no row changes sides. What
        // changes is the plan. Each branch carries the same `closedate > 0` from the closed test,
        // SQLite folds the identical terms into one derived `closedate > 0`, and then picks THAT
        // as the index's lower bound instead of the coarse `>= ?` below -- every statement walked
        // the whole replica's history up to the window's end (18 ms for an empty day on 606k
        // rows, 30 days of the Mini App month = 0.7 s). The unary plus keeps the branch terms off
        // the index, so the coarse range is the only bound left to choose.
        let closed = parts
            .iter()
            .filter_map(|p| {
                p.closed
                    .as_ref()
                    .map(|c| format!("{}{}", p.guard, c.replace(CLOSEDATE, UNINDEXED_CLOSEDATE)))
            })
            .collect::<Vec<_>>()
            .join(") OR (");
        let open = parts
            .iter()
            .filter_map(|p| p.open.as_ref().map(|o| format!("{}{o}", p.guard)))
            .collect::<Vec<_>>()
            .join(") OR (");
        // The range is bound FIRST because it is emitted first, and the per-group bounds follow
        // in group order exactly as they did before this shape existed.
        params.push(Box::new(from));
        params.push(Box::new(to));
        for part in parts.iter_mut() {
            params.append(&mut part.bounds);
        }
        let closed_side = format!("{CLOSEDATE} >= ? AND {CLOSEDATE} <= ? AND (({closed}))");
        if open.is_empty() {
            sql.push_str(&format!(" AND ({closed_side})"));
        } else {
            sql.push_str(&format!(" AND (({closed_side}) OR (({open})))"));
        }
        return;
    }
    let branches = parts.iter().map(GroupPredicate::branch).collect::<Vec<_>>();
    for part in parts.iter_mut() {
        params.append(&mut part.bounds);
    }
    match branches.len() {
        // `OpenIfCurrent` returned above, so zero branches here means no predicate to apply.
        0 => {}
        1 => sql.push_str(&format!(" AND {}", branches[0])),
        // A one-sided or unbounded window with two or more groups: the branches keep their own
        // window bounds on the index, but not the shared positivity test (Mini App's "any row past
        // the window" read: 377 ms -> 2.6 ms on 606k rows).
        _ => sql.push_str(&format!(
            " AND (({}))",
            close_test_off_index(&branches.join(") OR ("))
        )),
    }
}

/// `core_uid` guard one offset group's branches lead with.
///
/// Args:
///     cores: The group's cores, or `None` for the catch-all group.
///     f: Complete Report filter.
///
/// Returns:
///     `None` when the group names no core and contributes nothing, otherwise the guard text
///     (empty for an unguarded catch-all group).
fn group_guard(cores: &Option<Vec<u64>>, f: &ReportFilter) -> Option<String> {
    let mut guard = String::new();
    if let Some(cores) = cores {
        if cores.is_empty() {
            return None;
        }
        let ids = cores
            .iter()
            .map(|uid| (*uid as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        // Leads with `core_uid` so this branch still opens `idx_rep_core_close` rather than
        // scanning: that index is what keeps the period filter at tens of milliseconds over a
        // half-million-row replica, and it is the whole reason the offset moves onto the
        // BOUND instead of wrapping the column in a conversion.
        guard.push_str(&format!("r.core_uid IN ({ids}) AND "));
    } else if let Some(excluded) = catch_all_exclusion(f) {
        guard.push_str(&format!("r.core_uid NOT IN ({excluded}) AND "));
    }
    Some(guard)
}

/// Append the row-scope predicate and a `buydate` window for [`PeriodBasis::OpenDate`].
///
/// Every row, open or closed, enters the window by its open time, so the "window still reaches
/// the present" resolution never applies and no coarse `closedate` range is factored out. A
/// source without `buydate` cannot place any row in the period and fails closed.
/// The clock is read only to group cores by UTC offset; no bound is derived from it.
///
/// Args:
///     sql: Predicate buffer being built.
///     params: Ordered bound values being built.
///     f: Complete Report filter with at least one period bound.
///     cols: Columns available on this source.
pub(super) fn append_open_basis_scope(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
) {
    if !cols.contains("buydate") {
        sql.push_str(" AND 1=0");
        return;
    }
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let resolved = match f.rows {
        RowScope::ClosedAndOpenIfCurrent => RowScope::ClosedAndOpen,
        RowScope::OpenIfCurrent => RowScope::Open,
        other => other,
    };
    // Same hoist as `append_row_scope`: one top-level open test lets `idx_rep_open` seek.
    if resolved == RowScope::Open {
        match open_row_predicate(cols) {
            Some(open) => sql.push_str(&format!(" AND {open}")),
            None => sql.push_str(" AND 1=0"),
        }
    }
    let mut branches: Vec<String> = Vec::new();
    for (offset, cores) in &offset_groups(f, now) {
        let Some(guard) = group_guard(cores, f) else {
            continue;
        };
        // Same core-local axis as `closedate`, so the offset moves onto the bound the same way.
        let mut window = String::new();
        if let Some(from) = f.date_from {
            window.push_str(" AND r.\"buydate\" >= ?");
            params.push(Box::new(crate::db::ReportAxis::shift_bound(from, *offset)));
        }
        if let Some(to) = f.date_to {
            window.push_str(" AND r.\"buydate\" <= ?");
            params.push(Box::new(crate::db::ReportAxis::shift_bound(to, *offset)));
        }
        let state = match resolved {
            RowScope::Closed => Some(closed_row_predicate(cols).unwrap_or_else(|| "1=0".into())),
            // Hoisted above the branches.
            RowScope::Open => None,
            _ => match (closed_row_predicate(cols), open_row_predicate(cols)) {
                (Some(closed), Some(open)) => Some(format!("({closed}) OR {open}")),
                // Without `closedate` there is no row state to test; the window alone applies.
                _ => None,
            },
        };
        branches.push(match state {
            Some(state) => format!("{guard}({state}){window}"),
            None => format!("{guard}1=1{window}"),
        });
    }
    match branches.len() {
        0 => sql.push_str(" AND 1=0"),
        1 => sql.push_str(&format!(" AND {}", branches[0])),
        _ => sql.push_str(&format!(" AND (({}))", branches.join(") OR ("))),
    }
}

/// One offset group's contribution to the row-scope predicate, held apart so the closed and open
/// sides can be composed either per group or factored under a shared coarse range.
#[derive(Default)]
struct GroupPredicate {
    /// `core_uid` guard this group's branches lead with; empty for an unguarded single group.
    guard: String,
    /// Closed-side predicate including this group's own shifted window, or `None` when the scope
    /// asks for open rows alone.
    closed: Option<String>,
    /// Values bound by `closed`, in the order they appear in it.
    bounds: Vec<Box<dyn rusqlite::types::ToSql>>,
    /// Open-side predicate, or `None` when this group contributes no open rows.
    open: Option<String>,
    /// This group's lower bound after the offset shift; `None` for an unbounded period.
    shifted_from: Option<i64>,
    /// This group's upper bound after the offset shift; `None` for an unbounded period.
    shifted_to: Option<i64>,
}

impl GroupPredicate {
    /// Compose this group as ONE self-contained branch, the shape used whenever the coarse range
    /// does not apply.
    ///
    /// Returns:
    ///     Branch text, guard included.
    fn branch(&self) -> String {
        let mut branch = self.guard.clone();
        match (&self.closed, &self.open) {
            (Some(closed), Some(open)) => branch.push_str(&format!("(({closed}) OR {open})")),
            (Some(closed), None) => branch.push_str(closed),
            (None, Some(open)) => branch.push_str(open),
            // Every push site sets at least one side, so a part with neither is never built.
            (None, None) => {}
        }
        branch
    }
}

/// Widest window every closed branch fits inside, when factoring one out is worth doing.
///
/// Args:
///     parts: Every group that contributed to this predicate.
///
/// Returns:
///     Shifted lower and upper bound spanning all closed branches, or `None` when there is only
///     one of them, when the period is unbounded on either side, or when any closed branch
///     carries no window to widen.
fn coarse_range(parts: &[GroupPredicate]) -> Option<(i64, i64)> {
    let closed = parts
        .iter()
        .filter(|p| p.closed.is_some())
        .collect::<Vec<_>>();
    if closed.len() < 2 {
        return None;
    }
    let mut from = i64::MAX;
    let mut to = i64::MIN;
    for part in closed {
        from = from.min(part.shifted_from?);
        to = to.max(part.shifted_to?);
    }
    Some((from, to))
}

/// Resolve which offset groups this filter's rows fall into.
///
/// A scoped read names its cores and groups exactly those. An UNBOUNDED read cannot name them, so
/// it takes one branch per MEASURED offset plus a catch-all carrying `None` -- every core with no
/// measurement, which converts as the identity.
///
/// # Known limitation: grouped at ONE instant
///
/// A core is placed in the group its offset occupies at `now`, and that single offset then shifts
/// BOTH bounds. A period spanning an offset transition therefore has its far bound shifted by the
/// wrong segment, so trades within one delta of that edge can be admitted or dropped. Bounded by
/// the delta -- an hour across DST -- and only ever at the edge.
///
/// The alternative is a sub-branch per segment per core, which multiplies the disjunction the
/// coarse range in [`append_row_scope`] exists to keep cheap. Left deliberately, recorded here so
/// the next reader does not mistake it for an oversight.
///
/// Args:
///     f: Complete Report filter.
///     now: Current Unix timestamp in seconds, on this machine's true UTC.
///
/// Returns:
///     Offset and the cores it applies to, or `None` for the unbounded catch-all. A fleet with no
///     measurements at all collapses to a single identity group, which reproduces the predicate
///     this function had before offsets existed.
fn offset_groups(f: &ReportFilter, now: i64) -> Vec<(i32, Option<Vec<u64>>)> {
    if !f.core_uids.is_empty() {
        return f
            .axis
            .groups(&f.core_uids, now)
            .into_iter()
            .map(|(offset, cores)| (offset, Some(cores)))
            .collect();
    }
    let mut groups: Vec<(i32, Option<Vec<u64>>)> = f
        .axis
        .measured_groups(now)
        .into_iter()
        .map(|(offset, cores)| (offset, Some(cores)))
        .collect();
    groups.push((0, None));
    groups
}

/// Inline core-uid list a catch-all branch must exclude, or `None` when nothing is measured.
///
/// Args:
///     f: Complete Report filter.
///
/// Returns:
///     Comma-separated measured core uids, or `None` when the catch-all covers every core and
///     needs no guard at all.
fn catch_all_exclusion(f: &ReportFilter) -> Option<String> {
    let measured = f.axis.measured_cores();
    if measured.is_empty() {
        return None;
    }
    Some(
        measured
            .iter()
            .map(|uid| (*uid as i64).to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Apply report predicates to one aliased source.
///
/// Before the core schema arrives, the replica may lack `closedate`, `coin`,
/// `isshort`, or `emulator`; filtering on an absent column would fail the entire
/// SELECT. A strategy predicate is different: a source without either identity column cannot
/// prove a match and therefore contributes zero rows.
///
/// Args:
///     f: Complete Report filter.
///     cols: Columns available on this source.
///     meta: Strategy metadata of this read: name readability and the resolved name mask.
///
/// Returns:
///     Parameterized SQL suffix and its ordered bound values.
pub(super) fn build_where(
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
    meta: &StrategyMeta,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let has = |n: &str| cols.contains(n);
    let mut sql = String::from(" WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !f.core_uids.is_empty() {
        // Inline numeric values safely; IN supports the multi-core selector.
        let ids = f
            .core_uids
            .iter()
            .map(|u| (*u as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND r.core_uid IN ({ids})"));
    }
    append_strategy_filter(&mut sql, f, cols, meta.names);
    append_strategy_name_mask(&mut sql, f, cols, meta);
    append_row_scope(&mut sql, &mut params, f, cols);
    let coin = f.coin.trim();
    if let Some(coins) = &f.exact_coins {
        if coins.is_empty() || !has("coin") {
            sql.push_str(" AND 1=0");
        } else {
            sql.push_str(" AND r.coin COLLATE NOCASE IN (");
            for (index, exact) in coins.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push('?');
                params.push(Box::new(exact.clone()));
            }
            sql.push(')');
        }
    } else if !coin.is_empty() && has("coin") {
        if report_coin_is_exact(&f.coin) {
            // The ticker itself, or the ticker followed by a contract tail (`SOL_RP`,
            // `SOL_0925`); `SOLV` is another coin. The tail pattern escapes the ticker, since
            // `_` and `%` are LIKE wildcards.
            let ticker = coin.to_uppercase();
            // The NOCASE range is a superset of every match (ASCII fold; '_' < '`'), so it gives
            // the coin index a seek while the unchanged residual keeps the result set. A coin
            // stored as BLOB sorts above all TEXT and would fall outside the range; accepted
            // because the replica writer stores coin as TEXT.
            sql.push_str(
                " AND r.coin COLLATE NOCASE >= ? AND r.coin COLLATE NOCASE < ? \
                 AND (r.coin COLLATE NOCASE = ? OR r.coin LIKE ? ESCAPE '\\')",
            );
            params.push(Box::new(ticker.clone()));
            params.push(Box::new(format!("{ticker}`")));
            params.push(Box::new(ticker.clone()));
            params.push(Box::new(format!("{}\\_%", escape_like(&ticker))));
        } else {
            sql.push_str(" AND r.coin LIKE ?");
            params.push(Box::new(format!("%{}%", coin.to_uppercase())));
        }
    }
    if has("isshort") {
        match f.side {
            SideFilter::All => {}
            SideFilter::Long => sql.push_str(" AND r.isshort = 0"),
            SideFilter::Short => sql.push_str(" AND r.isshort = 1"),
        }
    }
    if has("emulator") {
        match f.emulator {
            None => {}
            Some(true) => sql.push_str(" AND COALESCE(r.emulator, 0) = 1"),
            Some(false) => sql.push_str(" AND COALESCE(r.emulator, 0) = 0"),
        }
    }
    // Deleted-mode semantics live on `ReportFilter::deleted_only`; the `1=0` arm makes
    // a column-less source contribute nothing when only deleted rows are wanted.
    if has("deleted") {
        sql.push_str(if f.deleted_only {
            " AND COALESCE(r.deleted, 0) <> 0"
        } else {
            " AND COALESCE(r.deleted, 0) = 0"
        });
    } else if f.deleted_only {
        sql.push_str(" AND 1=0");
    }
    (sql, params)
}
