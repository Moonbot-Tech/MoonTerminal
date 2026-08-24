//! The Analytics strategy-NAME mask: one resolved value, and the SQL term it produces.

use rusqlite::Connection;

use super::super::super::name_fold::{install_unicode_casefold, strategy_name_casefold};
use super::Query;

/// One Analytics strategy-name mask, resolved against a connection before any SQL is built.
///
/// A RESOLVED value rather than the raw text on [`Query`], because building the predicate needs
/// two things the SQL builder cannot do: register the Unicode folding on the connection, and ask
/// whether the strategy metadata is readable at all. Making the term unobtainable without that
/// value is what stops a future caller of [`Query::where_branches`] from emitting a predicate that
/// names a function nobody installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::db::analytics) enum StrategyMask {
    /// Empty or whitespace-only text: no predicate, no probe, no registration, no cost.
    Off,
    /// A mask was asked for, but the strategy metadata cannot answer it. Fails CLOSED.
    Unavailable,
    /// Ready predicate. The payload is the already-folded, already-escaped SQL string literal,
    /// quotes included.
    Match(String),
}

impl StrategyMask {
    /// Resolve one query's mask text against the connection its read will run on.
    ///
    /// An empty mask returns before touching the connection at all, so an unmasked Analytics read
    /// keeps exactly the cost it had: no probe, no scalar function, no predicate.
    ///
    /// A non-empty mask needs `strat.strategies`. When that database is absent or unreadable the
    /// mask resolves to [`Self::Unavailable`] rather than to an error: it is the same policy
    /// [`super::strategies_attached`] already applies for liquidation attribution, and the same one
    /// `report_read::append_strategy_name_mask` applies for the Report's own mask. Failing the
    /// whole read instead would sink the window over optional metadata, which this codebase has
    /// already been bitten by once.
    ///
    /// Args:
    ///     conn: Open report reader or pinned snapshot the masked read will use.
    ///     q: Analytics query carrying the raw user text.
    ///
    /// Returns:
    ///     The resolved mask.
    ///
    /// Errors:
    ///     Returns SQLite's registration error when the folding function cannot be installed.
    pub(in crate::db::analytics) fn resolve(
        conn: &Connection,
        q: &Query,
    ) -> rusqlite::Result<Self> {
        let mask = q.strategy_name_mask.trim();
        if mask.is_empty() {
            return Ok(Self::Off);
        }
        if !mask_columns_readable(conn) {
            log::debug!("analytics: strategy-name mask set but strategies.sqlite is unreadable");
            return Ok(Self::Unavailable);
        }
        install_unicode_casefold(conn)?;
        // NUL is dropped BEFORE escaping. The mask reaches SQL as literal text rather than as a
        // bound value (a bind here would have to travel through `unified_from_mode`'s returned
        // string, which eleven call sites already number `?1`/`?2` themselves), and SQLite's
        // parser stops reading a statement at the first NUL byte whatever length it was handed —
        // so one pasted NUL would truncate the whole UNION ALL mid-literal instead of failing.
        // `sql_str_list` doubles apostrophes and nothing else, which is the complete escape for
        // every other byte. A strategy name cannot contain a NUL, so dropping it removes no match.
        let folded: String = strategy_name_casefold(mask)
            .chars()
            .filter(|c| *c != '\0')
            .collect();
        Ok(Self::Match(crate::db::tuner::sql_str_list(&[folded])))
    }

    /// The SQL this mask adds to one already-built branch predicate.
    ///
    /// A NON-CORRELATED row-value subquery, deliberately not the correlated `EXISTS` the Report
    /// uses (`report_read::append_strategy_name_mask`): SQLite materializes this one into an
    /// ephemeral index, so the Rust folding callback runs once per STRATEGY instead of once per
    /// report ROW. The Report pages a table; Analytics scans whole periods and `groups()` embeds
    /// the source twice, so the per-row form is the wrong shape here even though the semantics are
    /// identical.
    ///
    /// `instr` rather than `LIKE` is what makes the mask LITERAL: `%`, `_` and `\` carry no
    /// wildcard meaning, matching the Report exactly. The pair is `(core_uid, strategy_id)` because
    /// a strategy name repeats across cores; dropping `core_uid` would pass every string test and
    /// silently mix two cores' trades.
    ///
    /// Args:
    ///     value_expr: SQL yielding the row's strategy id, already `COALESCE`d where the caller
    ///         wants liquidation attribution applied.
    ///     core_uid_col: The row's core column, qualified by the caller's alias.
    ///
    /// Returns:
    ///     The predicate to append, or `None` when this mask constrains nothing.
    pub(super) fn term(&self, value_expr: &str, core_uid_col: &str) -> Option<String> {
        match self {
            Self::Off => None,
            // Never reached from `where_branches`, which short-circuits the whole source. Kept so
            // that the ONE thing a caller can do with an unavailable mask is match no rows.
            Self::Unavailable => Some(" AND 1=0".to_string()),
            Self::Match(literal) => Some(format!(
                " AND ({core_uid_col}, {value_expr}) IN \
                 (SELECT core_uid, strategy_id FROM strat.strategies \
                 WHERE instr(mt_unicode_casefold(name), {literal}) > 0)"
            )),
        }
    }
}

/// Can the attached strategy table answer a NAME mask?
///
/// Deliberately NOT [`super::strategies_attached`], which asks `SELECT 1` and therefore approves a
/// table that exists but carries none of the three columns this predicate names. That is harmless
/// for the liquidation attribution it was written for, whose expression degrades on its own; here
/// it would let the mask's subquery fail to PREPARE, and a metadata problem the whole design calls
/// optional would surface as a failed Analytics read instead of the fail-closed empty result.
///
/// It still probes only ONE row: corruption further into the table cannot be ruled out without
/// reading all of it, and paying a full scan on every masked read to catch that is the wrong trade.
///
/// Args:
///     conn: Open report reader or pinned snapshot carrying the `strat` attachment.
///
/// Returns:
///     Whether `core_uid`, `strategy_id` and `name` can all be read.
fn mask_columns_readable(conn: &Connection) -> bool {
    match conn.query_row(
        "SELECT core_uid, strategy_id, name FROM strat.strategies LIMIT 1",
        [],
        |_| Ok(()),
    ) {
        Ok(()) => true,
        // An EMPTY table is usable: the mask simply matches nothing, which is a real answer.
        Err(rusqlite::Error::QueryReturnedNoRows) => true,
        Err(error) => {
            log::debug!(
                "analytics: strategy name columns unreadable, mask matches nothing: {error}"
            );
            false
        }
    }
}
