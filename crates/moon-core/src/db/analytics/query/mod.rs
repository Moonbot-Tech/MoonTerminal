//! Selection filters and the unified report source (`Query`, `unified_from`).

use rusqlite::Connection;

use super::super::{ProfitMetric, ReadResult, SideFilter};

/// Selection filters shared by all Analytics tabs.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// UTC Unix seconds; `from < 0` means all history. `to` is exclusive.
    pub from: i64,
    pub to: i64,
    /// Selected cores (multi-select, as in Orders); empty means all cores.
    pub cores: Vec<u64>,
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
    /// Which quantity every profit figure is measured in: absolute money (`Usdt`) or
    /// return on spent capital (`Percent`, the report's `Profit` column). Applied once in
    /// [`unified_from`]'s projected `pnl` column, so every reader below shares the choice.
    pub metric: ProfitMetric,
}

impl Query {
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

    /// Build the period-and-filter WHERE clause for ONE source, referencing only columns
    /// that source HAS (like the Report window's `build_where`; filtering on a missing
    /// column would fail the entire SELECT). Placeholders ?1/?2 are from/to; cores, side,
    /// and emulator are integer literals from configuration, so injection is impossible.
    pub(super) fn where_sql(&self, cols: &std::collections::HashSet<String>, sid: &str) -> String {
        self.where_with(WHERE_PERIOD, cols, sid)
    }

    /// The same filters under a DIFFERENT row predicate.
    ///
    /// Split out because one reader asks the opposite question of every other: which rows the
    /// period predicate throws away (see [`undated_closes`]). Sharing the filter tail is what
    /// keeps that count describing the same cores, side and emulator setting the figures
    /// beside it were computed under.
    /// `sid` is the SQL that yields a row's strategy id — plain `COALESCE(strategyid,0)`, or
    /// the liquidation-aware form. It is passed in rather than built here so the scope filter
    /// and the projected column cannot disagree about which strategy a row belongs to: a row
    /// attributed in the projection but not in the filter would show up in a strategy's list
    /// and vanish from its own detail.
    pub(super) fn where_with(
        &self,
        period: &str,
        cols: &std::collections::HashSet<String>,
        sid: &str,
    ) -> String {
        let has = |n: &str| cols.contains(n);
        let mut w = String::from(period);
        if has("deleted") {
            w.push_str(" AND COALESCE(deleted,0) = 0");
        }
        if !self.cores.is_empty() {
            let list = self
                .cores
                .iter()
                .map(|c| (*c as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            w.push_str(&format!(" AND core_uid IN ({list})"));
        }
        if has("isshort") {
            match self.side {
                SideFilter::All => {}
                SideFilter::Long => w.push_str(" AND COALESCE(isshort,0) = 0"),
                SideFilter::Short => w.push_str(" AND COALESCE(isshort,0) = 1"),
            }
        }
        if has("emulator") {
            match self.emulator {
                None => {}
                Some(false) => w.push_str(" AND COALESCE(emulator,0) = 0"),
                Some(true) => w.push_str(" AND COALESCE(emulator,0) = 1"),
            }
        }
        // Scope of the SELECTED strategies: rows matching any of the (strategy × its core)
        // pairs. One row can satisfy only one pair, so no aggregate double-counts.
        if !self.strategies.is_empty() {
            if has("strategyid") {
                let terms: Vec<String> = self
                    .strategies
                    .iter()
                    .map(|(want, core)| match core {
                        Some(c) => {
                            format!("(COALESCE({sid},0) = {want} AND core_uid = {})", *c as i64)
                        }
                        None => format!("COALESCE({sid},0) = {want}"),
                    })
                    .collect();
                w.push_str(&format!(" AND ({})", terms.join(" OR ")));
            } else {
                // This source cannot say which strategy a row belongs to, so it cannot
                // satisfy a strategy-scoped query: it contributes nothing, rather than
                // every row it holds.
                w.push_str(" AND 1=0");
            }
        }
        w
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
];

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
pub(super) fn effective_sid_expr(
    alias: &str,
    cols: &std::collections::HashSet<String>,
    on: bool,
) -> String {
    let plain = format!("{alias}.\"strategyid\"");
    // EVERY column the expression names. `core_uid` belongs here as much as the rest: a legacy
    // source can lack it (the projection right below emits `NULL AS "core_uid"` for exactly
    // that case), and naming it anyway makes the branch fail to PREPARE — sinking the whole
    // window the moment the switch is turned on.
    const NEEDED: [&str; 3] = ["strategyid", "channelname", "core_uid"];
    if !on || NEEDED.iter().any(|c| !cols.contains(*c)) {
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
pub(in crate::db) fn unified_from(conn: &Connection, q: &Query) -> ReadResult<Option<String>> {
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
    // The Report window deliberately does NOT follow this: it reads through its own queries,
    // and the two panels are allowed to disagree here. Anyone "fixing" that divergence should
    // know it was chosen.
    //
    // Attachment is PROBED rather than passed down: the callers that attach the strategy
    // database do so on this same connection, and the tests deliberately do not. Not to be
    // confused with `super::super::summary`'s own `has_strat_names`, which uses the same probe
    // independently to resolve strategy display names.
    let has_names = strategies_attached(conn);
    // Percent mode measures each trade as profit ÷ spent, so a trade without a positive
    // `spentbtc` has no percent at all.
    let pct = q.metric == ProfitMetric::Percent;
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
        let proj = cols
            .iter()
            .map(|c| {
                if *c == "strategyid" && src.cols.contains(*c) {
                    // The attributed value is published UNDER THE ORIGINAL NAME, so every
                    // query outside this source keeps reading `o.strategyid` and gets it.
                    format!("{sid} AS \"strategyid\"")
                } else if src.cols.contains(*c) {
                    format!("r.\"{c}\"")
                } else {
                    format!("NULL AS \"{c}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        // The active profit metric, projected ONCE here as `pnl` so every aggregation below and
        // the tuner sweep read one column. `Usdt` is the raw money; `Percent` is the report's
        // `Profit` column (profit ÷ spent × 100 = return on spent capital). The `spentbtc > 0`
        // filter appended below (percent mode) guarantees the ratio is finite and non-NULL, so
        // COUNT, SUM, wins, streaks and the top list all describe the SAME set of trades. Sign is
        // preserved (spent > 0), so win/loss, profit factor and best/worst stay correct on `pnl`.
        let pnl = if pct {
            "r.\"profitbtc\" / r.\"spentbtc\" * 100.0 AS \"pnl\""
        } else {
            "r.\"profitbtc\" AS \"pnl\""
        };
        let mut where_sql = q.where_sql(&src.cols, &sid);
        if pct {
            where_sql.push_str(" AND spentbtc > 0");
        }
        branches.push(format!(
            "SELECT {proj}, {pnl} FROM {} r WHERE {where_sql}",
            src.table
        ));
    }
    if branches.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({}) o", branches.join(" UNION ALL "))))
    }
}

const WHERE_PERIOD: &str = "closedate >= ?1 AND closedate < ?2 AND closedate > 0";

/// Rows a period can never contain: the close date is absent or non-positive.
pub(super) const WHERE_UNDATED: &str = "(closedate IS NULL OR closedate <= 0)";

/// Is the strategy database available on this connection?
///
/// Probed rather than tracked: `open_reader` attaches it, but a caller may hand us a
/// connection it opened itself, and a second ATTACH under the same alias fails. Asking the
/// connection is the only answer that cannot go stale.
pub(super) fn strategies_attached(conn: &Connection) -> bool {
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
