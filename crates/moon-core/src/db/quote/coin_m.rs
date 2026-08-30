//! Which cores own COIN-M rows, learned from the rows this process has not examined yet.
//!
//! A liquidation carries no market name on ANY core, so [`super::DENOMINATION_RULES`] cannot reach
//! it and the correction needs a third fact: whether the core owns rows that rule already
//! relabels. That fact lives in the replica and nowhere else — the protocol publishes a venue only
//! for a CONNECTED core, while the Report reads the history of cores that are long gone, and no
//! replica column carries the exchange. So it is measured here.
//!
//! The measurement is a scan over `fname`, which no index covers: 1 819 ms over 600k rows,
//! measured. The discovery path it hangs off runs 2-3 times per analytics query (see
//! `rep::table_cols_res`), so paying it per call cost a one-day Calendar seconds of scanning to
//! serve 2.5 ms of aggregate. It is paid per ROW RANGE instead: the primary key is
//! `(core_uid, newrecid)`, so "which cores exist and which ids do they span" is answered by
//! seeking (0.02 ms, measured) and only ids outside an examined span are ever scanned — 92 ms for
//! a whole new core, and nothing at all in the steady state.
//!
//! A span describes the ENDS of what was read, so it cannot notice a write landing between them —
//! and two of those exist: a catch-up re-fetching the pages under the local maximum, and a replica
//! wipe re-syncing the core from id 1. Neither is guessed at from row counts or timers: the writer
//! says so on commit through [`reexamine_core`] and [`forget_core`], which is the same authority
//! the rest of the replica is driven by.
//!
//! Short of those, knowledge only grows: a core that owned COIN-M rows still owns them in history,
//! so a proven core is never examined a second time.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use super::super::ReadSource;

/// One core present in a source, with the row ids it spans when the source numbers them.
type CorePresence = (i64, Option<(i64, i64)>);

/// Rows of one core this process has already examined.
#[derive(Clone, Copy)]
enum Span {
    /// Every row of the core, on a source that carries no row id to bound them with.
    All,
    /// The inclusive id range examined so far.
    Ids { low: i64, high: i64 },
}

/// What this process has established about COIN-M ownership.
#[derive(Default)]
struct Knowledge {
    /// Examined rows per core, whether or not that core turned out to be COIN-M.
    examined: BTreeMap<i64, Span>,
    /// Whether the legacy table has been swept. It is read-only and only ever shrinks, so it
    /// cannot acquire a core after the sweep and one pass answers for the session.
    legacy_swept: bool,
    /// Cores proven to own COIN-M rows.
    coin_m: BTreeSet<i64>,
}

/// Run `act` against the process-wide knowledge.
///
/// The lock is held across the SQL below on purpose: a second reader arriving mid-scan must WAIT
/// and then find the rows examined, rather than duplicate a two-second scan or — worse — build its
/// SQL from a half-learned set and value a liquidation at the wrong scale for one frame.
#[cfg(not(test))]
fn with<R>(act: impl FnOnce(&mut Knowledge) -> R) -> R {
    use std::sync::{Mutex, OnceLock};
    static STATE: OnceLock<Mutex<Knowledge>> = OnceLock::new();
    // A panicking scan leaves plain sets of ids that cannot be inconsistent, so recovering the
    // guard beats failing every later money read.
    let mut known = STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    act(&mut known)
}

/// Test builds keep the knowledge per THREAD.
///
/// Each test seeds its own fixture, and a verdict learned from one would silently correct
/// another's rows. libtest gives each test its own thread, which separates them by default;
/// `--test-threads=1` does not, so every test that DEPENDS on this knowledge calls [`reset`]
/// first. A test that merely reaches a reader can leave ids behind, and that is harmless only
/// because the tests reading them start from a clean slate.
#[cfg(test)]
fn with<R>(act: impl FnOnce(&mut Knowledge) -> R) -> R {
    use std::cell::RefCell;
    thread_local! {
        static STATE: RefCell<Knowledge> = RefCell::new(Knowledge::default());
    }
    STATE.with(|state| act(&mut state.borrow_mut()))
}

/// Forget everything learned, so a test's fixture is classified on its own evidence.
#[cfg(test)]
pub(in crate::db) fn reset() {
    with(|known| *known = Knowledge::default());
}

/// One thing the writer observed that the readers' knowledge cannot see for itself.
#[derive(Clone, Copy)]
enum Invalidation {
    /// The core's rows were wiped and re-synced from id 1: span and verdict are both stale.
    Forget(i64),
    /// A catch-up settled, which may have written between ids already examined.
    Reexamine(i64),
}

/// Run `act` against the pending invalidations.
///
/// A SECOND, tiny lock, and the reason it exists: [`with`] is held across a scan that measured
/// 1 819 ms, and the report WRITER publishes these signals from its own thread. Parking the writer
/// on a reader's scan would stall the catch-up page ACKs — and the corruption barrier behind them
/// — for the length of that scan. Nothing here ever holds this lock across SQL.
#[cfg(not(test))]
fn with_pending<R>(act: impl FnOnce(&mut Vec<Invalidation>) -> R) -> R {
    use std::sync::{Mutex, OnceLock};
    static PENDING: OnceLock<Mutex<Vec<Invalidation>>> = OnceLock::new();
    let mut queue = PENDING
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    act(&mut queue)
}

/// Per-thread queue, for the same reason [`with`] is per-thread in test builds.
#[cfg(test)]
fn with_pending<R>(act: impl FnOnce(&mut Vec<Invalidation>) -> R) -> R {
    use std::cell::RefCell;
    thread_local! {
        static PENDING: RefCell<Vec<Invalidation>> = const { RefCell::new(Vec::new()) };
    }
    PENDING.with(|queue| act(&mut queue.borrow_mut()))
}

/// Apply everything the writer has published since the last look.
///
/// Drained BEFORE a scan plans anything and never touched after, so a signal arriving mid-scan
/// stays queued for the next pass instead of being erased by the span that pass is about to write.
///
/// Args:
///     known: Knowledge to update.
fn drain_pending(known: &mut Knowledge) {
    let queue = with_pending(std::mem::take);
    for entry in queue {
        match entry {
            Invalidation::Forget(core) => {
                known.examined.remove(&core);
                known.coin_m.remove(&core);
                // The proof may have come from the legacy table, whose one-shot sweep would never
                // run again and could not restore it.
                known.legacy_swept = false;
            }
            Invalidation::Reexamine(core) => {
                known.examined.remove(&core);
            }
        }
    }
}

/// Forget one core entirely, after its replica rows were wiped.
///
/// `ReplicaRecreated` deletes the core's rows and re-syncs from id 1, so both halves of the
/// knowledge are stale at once: the examined span describes rows that no longer exist, and the
/// proof came from a database the core no longer serves.
///
/// Args:
///     core: Core whose replica was recreated.
pub(in crate::db) fn forget_core(core: u64) {
    with_pending(|queue| queue.push(Invalidation::Forget(core as i64)));
}

/// Re-examine one core after a catch-up settled.
///
/// A catch-up writes INSIDE the examined span, not only above it: `ReportStart::Checkpoint`
/// re-fetches the pages between the checkpoint and the local maximum, and a live upsert may
/// complete a row that arrived without its market name. Neither moves the span's ends, so the
/// span alone would never look at them — which is why the writer says so explicitly instead.
///
/// The proof is kept: a core that owned COIN-M rows still owns them in history, and re-proving it
/// would put the scan back on the path this module took it off.
///
/// Args:
///     core: Core whose sync just completed.
pub(in crate::db) fn reexamine_core(core: u64) {
    with_pending(|queue| queue.push(Invalidation::Reexamine(core as i64)));
}

/// Cores currently known to own COIN-M rows.
///
/// Returns:
///     The proven set, empty until [`learn`] finds one.
pub(super) fn cores() -> BTreeSet<i64> {
    with(|known| {
        // A wiped core must lose its verdict before the next statement is built, not merely before
        // the next scan: the SQL this feeds rewrites money.
        drain_pending(known);
        known.coin_m.clone()
    })
}

/// Examine whatever rows of this reader's sources have not been examined yet.
///
/// Failure is not fatal and not reported: an unexamined core corrects nothing, which is the same
/// answer this module gave before the correction existed. It is also not RECORDED — a span is
/// marked examined only when its scan actually completed, so a transient lock cannot cost a core
/// its correction for the rest of the session.
///
/// Args:
///     conn: Open report reader.
///     sources: Physical report sources with their discovered columns.
///     guards: Per-source COIN-M predicates over the alias `d`; empty when a source cannot
///         evidence the rule.
pub(super) fn learn(
    conn: &Connection,
    sources: &[ReadSource],
    guards: impl Fn(&ReadSource) -> Vec<String>,
) {
    with(|known| {
        drain_pending(known);
        for src in sources {
            if !src.cols.contains("core_uid") {
                continue;
            }
            let predicates = guards(src);
            if predicates.is_empty() {
                continue;
            }
            if src.legacy {
                learn_legacy(conn, src, &predicates, known);
            } else {
                learn_replica(conn, src, &predicates, known);
            }
        }
    });
}

/// Sweep the legacy table once per session.
///
/// It carries no row id to bound a scan with and is written by nobody — `SyncComplete` only purges
/// from it — so a second pass could not find a row the first one missed.
///
/// Args:
///     conn: Open report reader.
///     src: The legacy source.
///     predicates: COIN-M guards over the alias `d`.
///     known: Knowledge to extend.
fn learn_legacy(conn: &Connection, src: &ReadSource, predicates: &[String], known: &mut Knowledge) {
    if known.legacy_swept {
        return;
    }
    let Some(found) = sweep(conn, src.table, predicates, "1") else {
        return;
    };
    known.coin_m.extend(found);
    known.legacy_swept = true;
}

/// Examine the replica rows that fall outside every examined span.
///
/// A core already proven COIN-M is skipped entirely: the proof cannot be undone by later rows, and
/// re-reading them would put the expensive scan back on the hot path it was taken off.
///
/// Args:
///     conn: Open report reader.
///     src: The typed replica source.
///     predicates: COIN-M guards over the alias `d`.
///     known: Knowledge to extend.
fn learn_replica(
    conn: &Connection,
    src: &ReadSource,
    predicates: &[String],
    known: &mut Knowledge,
) {
    let bounded = src.cols.contains("newrecid");
    let Some(present) = cores_present(conn, src.table, bounded) else {
        return;
    };
    let mut scopes = Vec::new();
    let mut examined = Vec::new();
    for (core, ids) in present {
        if known.coin_m.contains(&core) {
            continue;
        }
        let (scope, span) = match (known.examined.get(&core).copied(), ids) {
            // Already read end to end, on a source that cannot say which rows are new.
            (Some(Span::All), _) | (Some(_), None) => continue,
            (None, None) => (vec![format!("d.core_uid = {core}")], Span::All),
            (None, Some((low, high))) => (
                vec![format!("d.core_uid = {core}")],
                Span::Ids { low, high },
            ),
            (Some(Span::Ids { low, high }), Some((first, last))) => {
                // BOTH ends: new trades arrive above the span, while a resumed history download
                // fills in below it. Each end is its OWN conjunction rather than one branch with
                // an inner `newrecid < a OR newrecid > b` — that inner OR costs the primary key
                // its range seek and reads the core's whole history instead (measured: 11.9 ms
                // over a 400k-row core against a seek that does not register).
                let mut ends = Vec::new();
                if first < low {
                    ends.push(format!("(d.core_uid = {core} AND d.newrecid < {low})"));
                }
                if last > high {
                    ends.push(format!("(d.core_uid = {core} AND d.newrecid > {high})"));
                }
                if ends.is_empty() {
                    continue;
                }
                (
                    ends,
                    Span::Ids {
                        low: low.min(first),
                        high: high.max(last),
                    },
                )
            }
        };
        scopes.extend(scope);
        examined.push((core, span));
    }
    if scopes.is_empty() {
        return; // The whole point: the steady state asks nothing further of the replica.
    }
    let Some(found) = sweep(
        conn,
        src.table,
        predicates,
        &format!("({})", scopes.join(" OR ")),
    ) else {
        return;
    };
    known.coin_m.extend(found);
    known.examined.extend(examined);
}

/// List the cores a source holds and the row ids each of them spans.
///
/// A LOOSE INDEX SCAN, the same shape `report_read::distinct_cores` uses for the same reason: this
/// runs on the discovery path of every money query, and reading 600k rows to answer a question
/// about twenty values is what this module exists to stop. Measured at 0.02 ms.
///
/// Args:
///     conn: Open report reader.
///     table: Physical source table.
///     bounded: Whether the source carries `newrecid` to bound a core's rows with.
///
/// Returns:
///     Present cores with their id span, or `None` when the walk failed.
fn cores_present(conn: &Connection, table: &str, bounded: bool) -> Option<Vec<CorePresence>> {
    // MIN and MAX of the second primary-key column under a fixed first one are two index seeks,
    // not a scan of that core's rows.
    let ids = if bounded {
        format!(
            "(SELECT MIN(newrecid) FROM {table} WHERE core_uid = cores.uid),
             (SELECT MAX(newrecid) FROM {table} WHERE core_uid = cores.uid)"
        )
    } else {
        "NULL, NULL".to_string()
    };
    let sql = format!(
        "WITH RECURSIVE cores(uid) AS (
             SELECT MIN(core_uid) FROM {table}
             UNION ALL
             SELECT (SELECT MIN(core_uid) FROM {table} WHERE core_uid > cores.uid)
             FROM cores WHERE cores.uid IS NOT NULL
         )
         SELECT uid, {ids} FROM cores WHERE uid IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, rusqlite::types::Value>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .ok()?;
    let mut out = Vec::new();
    for row in rows {
        let (uid, low, high) = row.ok()?;
        // A non-integer id belongs to no core the correction can name. Skipping the VALUE (rather
        // than the walk) keeps a dirty cell from hiding every core sorted after it.
        let rusqlite::types::Value::Integer(uid) = uid else {
            continue;
        };
        out.push((uid, low.zip(high)));
    }
    Some(out)
}

/// Find which cores own COIN-M rows within one scope.
///
/// Args:
///     conn: Open report reader.
///     table: Physical source table.
///     predicates: COIN-M guards over the alias `d`.
///     scope: Row scope to examine, `1` for the whole table.
///
/// Returns:
///     The owning cores, or `None` when any statement failed.
fn sweep(
    conn: &Connection,
    table: &str,
    predicates: &[String],
    scope: &str,
) -> Option<BTreeSet<i64>> {
    let mut out = BTreeSet::new();
    for guard in predicates {
        // The scope is built from i64 ids read back from this same column, never from user text,
        // so interpolating it cannot inject anything.
        let sql = format!(
            "SELECT DISTINCT d.core_uid FROM {table} d
              WHERE {guard} AND typeof(d.core_uid) = 'integer' AND {scope}"
        );
        let mut stmt = conn.prepare(&sql).ok()?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0)).ok()?;
        for row in rows {
            out.insert(row.ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
