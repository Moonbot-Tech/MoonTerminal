//! The auto-pick for EVERY strategy of the tuner's list at once.
//!
//! One scan reads the period's trades for all strategies, grouped per strategy-list row key
//! (`strategyid@core_uid`) in memory, and each group is fitted with the very same
//! [`coin_pick_from_rows`] the per-strategy card runs. No second algorithm and no thresholds of
//! its own: a batch verdict is, by construction, what pressing "Pick" on that one strategy with
//! its saved lists would say.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{CoinPick, CoinPickParams, CoinTrade, coin_pick_from_rows};
use rusqlite::types::Value;

use crate::db::analytics::Query;
use crate::db::read_fail::read_fail_on;
use crate::db::{FailCode, FailKind, ReadFail, ReadResult};

/// The settings every strategy of a batch shares; each strategy brings its own lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickSettings {
    pub min_trades: u32,
    pub holdout_pct: u32,
    pub black: bool,
    pub white: bool,
}

impl PickSettings {
    /// The single-strategy parameters for one strategy's own lists.
    ///
    /// Args:
    ///     lists: The strategy's saved lists, or `None` when it has none to read.
    ///
    /// Returns:
    ///     Parameters equal to the ones the per-strategy card would pass for that strategy.
    pub fn params_for(&self, lists: Option<&ManualLists>) -> CoinPickParams {
        CoinPickParams {
            min_trades: self.min_trades,
            holdout_pct: self.holdout_pct,
            black: self.black,
            white: self.white,
            manual_black: lists.map(|l| l.black.clone()).unwrap_or_default(),
            manual_white: lists.map(|l| l.white.clone()).unwrap_or_default(),
        }
    }
}

/// One strategy's saved lists, as folded coin tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManualLists {
    pub black: HashSet<String>,
    pub white: HashSet<String>,
}

/// One closed trade tagged with the strategy-list row key that made it.
#[derive(Debug, Clone)]
pub struct KeyedTrade {
    pub key: String,
    pub trade: CoinTrade,
}

/// Shared between the caller and the background fit: how far it got, and a stop request.
///
/// The fit is CPU work after the scan has released its snapshot, so the SQLite interrupt that
/// cancels a read cannot reach it; the flag is checked between strategies instead.
#[derive(Debug, Default)]
pub struct BatchProgress {
    done: AtomicUsize,
    total: AtomicUsize,
    cancelled: AtomicBool,
}

impl BatchProgress {
    /// Strategies fitted so far and how many the batch holds (`0` until the scan has landed).
    pub fn counts(&self) -> (usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    /// Ask the running batch to stop at the next strategy.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether [`Self::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Fit every strategy's trades with its own lists.
///
/// Rows are grouped by key in their input order; [`coin_pick_from_rows`] sorts each group
/// itself, so the grouping cannot change any answer.
///
/// Args:
///     rows: Every trade of the batch, tagged with its strategy key.
///     lists: Each strategy's saved lists; a key without an entry has empty lists.
///     settings: The pick settings shared by the whole batch.
///     progress: Advanced once per fitted strategy; a cancel stops the loop.
///
/// Returns:
///     One pick per key that has trades, or `None` when the batch was cancelled midway.
pub fn coin_picks_from_rows(
    rows: Vec<KeyedTrade>,
    lists: &HashMap<String, ManualLists>,
    settings: PickSettings,
    progress: &BatchProgress,
) -> Option<HashMap<String, CoinPick>> {
    let mut groups: BTreeMap<String, Vec<CoinTrade>> = BTreeMap::new();
    for row in rows {
        groups.entry(row.key).or_default().push(row.trade);
    }
    progress.total.store(groups.len(), Ordering::Relaxed);
    let mut out = HashMap::with_capacity(groups.len());
    for (key, trades) in groups {
        if progress.is_cancelled() {
            return None;
        }
        let pick = coin_pick_from_rows(&trades, &settings.params_for(lists.get(&key)));
        out.insert(key, pick);
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    Some(out)
}

/// Read the period's trades and every strategy's saved lists in ONE snapshot, then fit.
///
/// Run on a background executor: this scans the whole query period once for all strategies.
///
/// Args:
///     q: The strategy list's scope and period (no strategy restriction, or the list's own).
///     settings: The pick settings shared by the whole batch.
///     progress: Progress and cancellation shared with the caller.
///
/// Returns:
///     One pick per strategy key with trades (`Ok(None)` when cancelled), or the read failure.
pub fn suggest_coins_by_strategy(
    q: &Query,
    settings: PickSettings,
    progress: &BatchProgress,
) -> ReadResult<Option<HashMap<String, CoinPick>>> {
    let (rows, lists) = super::super::read_tuner_rows(q, read_keyed)?;
    Ok(coin_picks_from_rows(rows, &lists, settings, progress))
}

/// Every trade of the period tagged with its strategy-list row key, plus each strategy's saved
/// lists, from one pinned connection.
///
/// Args:
///     conn: Report snapshot, the strategy database attached as `strat` when present.
///     q: Scope and period; `src` binds `?1`/`?2` to its bounds.
///     src: The tuner source expression, aliased `o`.
///
/// Returns:
///     The keyed trades and the lists, or the read failure (a failed list read included: a
///     verdict against an invented empty baseline would pass for a real one).
fn read_keyed(
    conn: &rusqlite::Connection,
    q: &Query,
    src: &str,
) -> ReadResult<(Vec<KeyedTrade>, HashMap<String, ManualLists>)> {
    let ctx = "tuner: suggest_coins_by_strategy";
    // The key expression is the strategy list's own (`analytics::groups`), so every pick lands
    // on the row the user is looking at.
    let sql = format!(
        "SELECT CAST(o.strategyid AS TEXT) || '@' || CAST(o.core_uid AS TEXT),
                o.strategyid, o.core_uid,
                o.coin, COALESCE(o.closedate,0), COALESCE(o.pnl,0)
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, ctx, e))?;
    let integer = |v: Value| match v {
        Value::Integer(i) => Some(i),
        _ => None,
    };
    let mapped = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                // Only a numeric identity has saved lists; a textual strategy id still gets
                // its pick, against no lists, exactly like its row in the strategy table.
                integer(r.get(1)?).zip(integer(r.get(2)?)),
                CoinTrade {
                    // Raw report name: `coin_pick_from_rows` owns the normalization.
                    token: r.get(3)?,
                    close: r.get(4)?,
                    profit: r.get(5)?,
                },
            ))
        })
        .map_err(|e| read_fail_on(conn, ctx, e))?;
    let mut rows = Vec::new();
    let mut pairs: HashMap<String, (i64, i64)> = HashMap::new();
    for row in mapped {
        let (key, pair, trade) = row.map_err(|e| read_fail_on(conn, ctx, e))?;
        // A trade without a strategy identity has no row in the list to answer for.
        let Some(key) = key else { continue };
        if let Some(pair) = pair {
            pairs.entry(key.clone()).or_insert(pair);
        }
        rows.push(KeyedTrade { key, trade });
    }
    drop(stmt);
    Ok((rows, saved_lists(conn, &pairs)?))
}

/// Every strategy's saved lists from the attached strategy database, in the read's snapshot.
///
/// An absent strategy database means no strategy has lists, as in the strategy table. A
/// present one that fails to answer is a read failure, not an empty baseline.
fn saved_lists(
    conn: &rusqlite::Connection,
    pairs: &HashMap<String, (i64, i64)>,
) -> ReadResult<HashMap<String, ManualLists>> {
    if pairs.is_empty() || !crate::db::analytics::strategies_attached(conn) {
        return Ok(HashMap::new());
    }
    let wanted: HashSet<(i64, i64)> = pairs.values().copied().collect();
    let Some(by_pair) = crate::db::analytics::read_metadata(conn, &wanted)?.groups else {
        return Err(ReadFail::failed(
            FailKind::Other,
            "strategy versions unreadable",
            "strategies.sqlite",
            "tuner: suggest_coins_by_strategy lists",
            FailCode::None,
        ));
    };
    Ok(pairs
        .iter()
        .filter_map(|(key, pair)| {
            let (black, white) = by_pair.get(pair)?.coin_lists();
            Some((key.clone(), ManualLists { black, white }))
        })
        .collect())
}

#[cfg(test)]
mod tests;
