//! Per-core basis of the report's `spentbtc` column.
//!
//! `spentbtc` is NOT one quantity across cores. Measured on a live 567k-row replica: one Bybit
//! core writes the MARGIN it posted (notional ÷ leverage) in 193 774 of its 194 550 leveraged
//! rows, while every Binance/Hyperliquid/Gate core writes the full NOTIONAL. A second Bybit core
//! on the same venue writes notional — so this is a property of the CORE (its build or its
//! settings), never of the exchange, and it must be probed per core rather than mapped from a
//! venue name.
//!
//! Summing the column as-is understates a margin core's turnover by its per-trade leverage —
//! measured at 26% of one month's total volume across all cores. Everything here exists to give
//! the volume aggregate a `× leverage` factor for exactly those cores.
//!
//! Leverage is a per-ROW column: one core mixes 5×, 10×, 25× and 100× trades, and the notional
//! reconstruction holds for each of those buckets independently. So the probe decides only
//! WHETHER to multiply; the multiplier itself always comes from the trade.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::super::ReadResult;
use super::super::read_fail::read_fail_on;

/// How close `spentbtc × leverage` must sit to the notional to count as a margin row.
///
/// Wide on purpose: the two candidates differ by the leverage factor (5× at the least), so any
/// tolerance below that separates them completely, while a tight one would reject the fee already
/// baked into `spentbtc` and the rounding of a small position.
const MATCH_TOLERANCE: f64 = 0.02;

/// How long a probe result is trusted before the next reader re-measures.
///
/// A core's basis does not change while it runs, so this only bounds how long a NEWLY connected
/// core is valued with the conservative default.
const CACHE_TTL: Duration = Duration::from_secs(900);

/// One probe result: when it was taken, and the cores whose `spentbtc` is margin.
type Measured = (Instant, HashSet<u64>);

/// Cached probe result, absent until the first reader measures.
static MARGIN_CORES: OnceLock<Mutex<Option<Measured>>> = OnceLock::new();

/// Return the cores whose `spentbtc` holds margin rather than notional.
///
/// Cached for [`CACHE_TTL`], because the probe scans every leveraged row of the replica (measured
/// at 891 ms over 567k rows) while Calendar re-reads on every navigation. A core that connects
/// after the probe is absent from the set and is therefore valued as notional until the entry
/// expires — the conservative direction: it understates that one core's volume instead of
/// inflating everyone's by a leverage factor that does not apply.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///
/// Returns:
///     Core ids to multiply by per-trade leverage, or a classified read failure.
pub(in crate::db) fn margin_cores(conn: &Connection) -> ReadResult<HashSet<u64>> {
    // Tests bypass the cache entirely. Each seeds its own fixture, they run in parallel, and a
    // verdict measured from one fixture would silently multiply another's turnover — a failure
    // that appears only in some interleavings. Serializing them around a shared slot cannot fix
    // that, because an unguarded test can refill the slot between the guard and the probe.
    if cfg!(test) {
        return probe(conn);
    }
    let cell = MARGIN_CORES.get_or_init(|| Mutex::new(None));
    // A panicking prober would poison the lock; the cached value is a plain set that cannot be
    // left inconsistent, so recovering it is safe and beats failing every later volume read.
    let mut slot = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((taken, cores)) = slot.as_ref() {
        if taken.elapsed() < CACHE_TTL {
            return Ok(cores.clone());
        }
    }
    let cores = probe(conn)?;
    *slot = Some((Instant::now(), cores.clone()));
    Ok(cores)
}

/// Measure every core's `spentbtc` basis over the whole replica.
///
/// A core counts as margin only when MORE THAN HALF of its leveraged rows reconstruct the notional
/// through `spentbtc × leverage`. A bare "more margin-like than notional-like" test is not enough:
/// an inverse COIN-M core matches NEITHER candidate (its `boughtq` is in contracts), so a handful
/// of coincidental hits — 11 of 6 637 on a live core — would elect it margin and inflate its
/// volume by up to 50×.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///
/// Returns:
///     Core ids whose spend column is margin, or a classified read failure.
fn probe(conn: &Connection) -> ReadResult<HashSet<u64>> {
    const CTX: &str = "analytics: spent basis probe";
    const NEEDED: &[&str] = &[
        "core_uid",
        "spentbtc",
        "boughtq",
        "buyprice",
        "lev",
        "closedate",
    ];
    let mut out = HashSet::new();
    for src in super::super::read_sources_res(conn)? {
        if !NEEDED.iter().all(|c| src.cols.contains(*c)) {
            continue; // A source without the inputs cannot be classified, and defaults to notional.
        }
        // Funding rows carry `spentbtc = boughtq` (the position the funding was charged on), so
        // they match neither candidate and would only dilute the majority.
        let funding = if src.cols.contains("sellreason") {
            "AND COALESCE(sellreason,'') <> 'Funding'"
        } else {
            ""
        };
        let deleted = if src.cols.contains("deleted") {
            "AND COALESCE(deleted,0) = 0"
        } else {
            ""
        };
        let sql = format!(
            "SELECT core_uid,
                    SUM(CASE WHEN abs(spentbtc * lev / (boughtq * buyprice) - 1) < {tol}
                             THEN 1 ELSE 0 END),
                    COUNT(*)
               FROM {table}
              WHERE closedate > 0 AND lev > 1 AND boughtq > 0 AND buyprice > 0 AND spentbtc > 0
                    {funding} {deleted}
              GROUP BY core_uid",
            tol = MATCH_TOLERANCE,
            table = src.table,
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| read_fail_on(conn, CTX, e))?;
        for row in rows {
            let (core_uid, margin_like, total) = row.map_err(|e| read_fail_on(conn, CTX, e))?;
            if margin_like * 2 > total {
                out.insert(core_uid as u64);
            }
        }
    }
    Ok(out)
}

/// SQL factor turning one row's spend column into its traded notional.
///
/// Yields the per-trade leverage for a margin core and `1` everywhere else, so the caller writes
/// `SUM(spentbtc * <factor>)` once and gets turnover under either convention. Leverage is floored
/// at 1: the replica holds a row with `lev = 0`, and dividing turnover to zero would silently
/// erase it from the total.
///
/// Args:
///     alias: Table alias carrying `core_uid` and `lev`.
///     margin: Cores whose spend column is margin, from [`margin_cores`].
///
/// Returns:
///     A SQL expression evaluating to the notional multiplier.
pub(in crate::db) fn notional_factor_expr(alias: &str, margin: &HashSet<u64>) -> String {
    if margin.is_empty() {
        return "1".to_string();
    }
    let list = margin
        .iter()
        .map(|c| (*c as i64).to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "(CASE WHEN {alias}.core_uid IN ({list}) THEN MAX(COALESCE({alias}.lev, 1), 1) ELSE 1 END)"
    )
}

#[cfg(test)]
mod tests;
