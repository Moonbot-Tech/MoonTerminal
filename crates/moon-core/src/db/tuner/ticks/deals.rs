//! The axis' report rows: every closed trade of the tuner scope, as [`Deal`]s.
//!
//! Read through the same unified source the other axes scan (`read_tuner_rows`), so the
//! "Fact" column and the tape replay describe the SAME trades — period, cores, strategies,
//! emulator and side filters included. A row without a millisecond stamp cannot be replayed
//! (the tape is sub-second) and is counted rather than dropped silently; the caption prints
//! the count.

use rusqlite::Connection;

use super::{Deal, Deltas};
use crate::db::analytics::Query;
use crate::db::read_fail::read_fail_on;
use crate::db::tuner::strategy_kinds;
use crate::db::{ReadFail, ReadResult};

/// The scope's rows split into what the axis can replay and what it cannot.
#[derive(Clone, Debug, Default)]
pub struct DealsRead {
    /// Rows with millisecond stamps, chronological by close.
    pub deals: Vec<Deal>,
    /// Rows the scope holds that carry no millisecond stamp — older replicas, or a core that
    /// predates the stamps. In the "Fact" column, not in the replay.
    pub without_ms: usize,
}

/// The delta columns in the order [`Deltas`] is filled below; every one is a `FIELDS` column,
/// so the unified source projects it (NULL when the replica lacks it).
const DELTA_COLS: [&str; 12] = [
    "d5s",
    "d1m",
    "d5m",
    "d15m",
    "d1h",
    "d3h",
    "d24h",
    "dmark",
    "pricebug",
    "btc1hdelta",
    "btc5mdelta",
    "exchange1hdelta",
];

/// Read the scope's closed trades as deals.
///
/// Args:
///     q: The tuner scope — period, cores, strategies, filters.
///
/// Returns:
///     The replayable deals with their kinds resolved, and the count left out; `NotReady` when
///     no report source has the schema yet.
pub fn read_deals(q: &Query) -> ReadResult<DealsRead> {
    let mut read = crate::db::tuner::read_tuner_rows(q, read_on)?;
    // The kind selects the entry model; resolved once per distinct strategy, off the replica's
    // snapshot, because it lives in strategies.sqlite.
    let mut pairs: Vec<(i64, u64)> = read
        .deals
        .iter()
        .map(|d| (d.strategy_id, d.core_uid))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let kinds = strategy_kinds(&pairs);
    for deal in &mut read.deals {
        if let Some(kind) = kinds.get(&(deal.strategy_id, deal.core_uid)) {
            deal.kind = kind.clone();
        }
    }
    Ok(read)
}

/// The scan itself, inside the pinned snapshot. Ordered in memory by the millisecond close —
/// the unified source has no total order to ask SQL for.
fn read_on(conn: &Connection, q: &Query, src: &str) -> ReadResult<DealsRead> {
    const CTX: &str = "tuner: ticks deals";
    let deltas = DELTA_COLS
        .iter()
        .map(|c| format!("o.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT o.\"reportuid\", o.\"core_uid\", o.\"strategyid\", o.\"coin\",
                o.\"buydatems\", o.\"closedatems\", o.\"buyprice\", o.\"sellprice\",
                o.\"spentbtc\", o.\"isshort\", o.\"sellreason\", COALESCE(o.pnl, 0), {deltas}
         FROM {src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut rows = stmt
        .query(rusqlite::params![q.from, q.to])
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut out = DealsRead::default();
    let mut order: Vec<(i64, i64)> = Vec::new();
    while let Some(r) = rows.next().map_err(|e| read_fail_on(conn, CTX, e))? {
        let fail = |e: rusqlite::Error| -> ReadFail { read_fail_on(conn, CTX, e) };
        let num = |i: usize| -> Result<f64, ReadFail> {
            Ok(r.get::<_, Option<f64>>(i)
                .map_err(fail)?
                .filter(|v| v.is_finite())
                .unwrap_or(0.0))
        };
        let int = |i: usize| -> Result<i64, ReadFail> {
            Ok(r.get::<_, Option<i64>>(i).map_err(fail)?.unwrap_or(0))
        };
        let buy_ms = int(4)?;
        let close_ms = int(5)?;
        if buy_ms <= 0 || close_ms <= 0 {
            out.without_ms += 1;
            continue;
        }
        let mut deltas = Deltas::default();
        let slots: [&mut f64; 12] = [
            &mut deltas.d5s,
            &mut deltas.d1m,
            &mut deltas.d5m,
            &mut deltas.d15m,
            &mut deltas.d1h,
            &mut deltas.d3h,
            &mut deltas.d24h,
            &mut deltas.dmark,
            &mut deltas.pricebug,
            &mut deltas.btc1h,
            &mut deltas.btc5m,
            &mut deltas.market1h,
        ];
        for (offset, slot) in slots.into_iter().enumerate() {
            *slot = num(12 + offset)?;
        }
        let report_uid = int(0)?;
        out.deals.push(Deal {
            report_uid,
            core_uid: int(1)? as u64,
            strategy_id: int(2)?,
            kind: String::new(),
            coin: r
                .get::<_, Option<String>>(3)
                .map_err(fail)?
                .unwrap_or_default(),
            buy_ms,
            close_ms,
            buy_price: num(6)?,
            sell_price: num(7)?,
            spent: num(8)?,
            is_short: int(9)? != 0,
            sell_reason: r
                .get::<_, Option<String>>(10)
                .map_err(fail)?
                .unwrap_or_default(),
            fact_pnl: num(11)?,
            deltas,
            tick: None,
        });
        order.push((close_ms, report_uid));
    }
    // Chronological by the millisecond close, ties broken by the row's identity so the order is
    // total.
    let mut index: Vec<usize> = (0..out.deals.len()).collect();
    index.sort_by_key(|&i| order[i]);
    out.deals = index.into_iter().map(|i| out.deals[i].clone()).collect();
    Ok(out)
}

#[cfg(test)]
mod tests;
