//! The axis' report rows: every closed trade of the tuner scope, as [`Deal`]s.
//!
//! Read through the same unified source the other axes scan (`read_tuner_rows`), so the
//! "Fact" column and the tape replay describe the SAME trades — period, cores, strategies,
//! emulator and side filters included. Three kinds of row are counted rather than dropped
//! silently, and the caption and the load log print the counts: a row without a millisecond
//! stamp cannot be replayed (the tape is sub-second); a SERVICE row — funding, a liquidation, a
//! joined sell, no strategy behind it — is not a trade the tape explains ([`scope`]); and a
//! trade the tuner cannot be run on — a container kind, an unresolved one, a manual exit. The
//! "Fact" column keeps them all: it is the scope's money, and this file decides only what the
//! model reads.

use std::collections::HashMap;

use rusqlite::Connection;

use super::hook::parse_hook_detect;
use super::scope::{is_service_row, is_tunable, sold_more_than_bought};
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
    /// Service rows with stamps — funding, liquidations, joined sells, no strategy, and a sale
    /// that moved more coins than the entry bought — left out; see [`scope`].
    pub service: usize,
    /// Trades with stamps the tuner cannot be run on — a container or unresolved kind, a manual
    /// exit — left out; see [`is_tunable`].
    pub untunable: usize,
}

/// The delta columns in the order [`Deltas`] is filled below; every one is a `FIELDS` column,
/// so the unified source projects it (NULL when the replica lacks it).
const DELTA_COLS: [&str; 13] = [
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
    "dbtc1m",
];

/// Read the scope's closed trades as deals: the trades the tuner can be run on
/// ([`is_tunable`]), the rest counted in [`DealsRead::untunable`].
///
/// Args:
///     q: The tuner scope — period, cores, strategies, filters.
///
/// Returns:
///     The replayable deals with their kinds resolved, and the counts left out; `NotReady`
///     when no report source has the schema yet.
pub fn read_deals(q: &Query) -> ReadResult<DealsRead> {
    // The scan on the tuner's own source (the metric decides `pnl`), then the USDT money of
    // every row off the USDT source in the same snapshot — the table's profit column must not
    // change unit with the scope's quote, and the scan's `profitbtc` would.
    let mut read = crate::db::tuner::read_tuner_rows(q, |conn, q, src| {
        let mut read = read_on(conn, q, src)?;
        overlay_hook_detect(conn, q, &mut read.deals)?;
        match crate::db::tuner::tuner_source_usdt_on(conn, q)? {
            Some(usdt_src) => overlay_usdt_profit(conn, q, &usdt_src, &mut read.deals)?,
            None => log::info!(
                target: crate::diagnostics::TICKS_AXIS_TARGET,
                "[x] ticks deals: the scope's money cannot be valued in USDT, the profit column stays empty"
            ),
        }
        Ok(read)
    })?;
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
    let before = read.deals.len();
    read.deals
        .retain(|deal| is_tunable(&deal.kind, &deal.sell_reason));
    read.untunable = before - read.deals.len();
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
                o.\"spentbtc\", o.\"isshort\", o.\"sellreason\", COALESCE(o.pnl, 0), {deltas},
                o.\"core_name\", o.\"quantity\", o.\"boughtq\"
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
        let strategy_id = int(2)?;
        let sell_reason = r
            .get::<_, Option<String>>(10)
            .map_err(fail)?
            .unwrap_or_default();
        // Counted after the stamp gate on purpose: the caption's "without stamps" is the
        // scope's whole unreplayable history, the service count only what the stamps would
        // otherwise have admitted.
        if is_service_row(strategy_id, &sell_reason) {
            out.service += 1;
            continue;
        }
        // A sale the core topped up from the wallet balance moved coins this trade never
        // bought, so its price is an average of something else — counted with the service rows
        // rather than replayed (`scope::sold_more_than_bought`).
        let name_at = 12 + DELTA_COLS.len();
        if sold_more_than_bought(num(name_at + 1)?, num(name_at + 2)?) {
            out.service += 1;
            continue;
        }
        let mut deltas = Deltas::default();
        let slots: [&mut f64; 13] = [
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
            &mut deltas.btc1m,
        ];
        for (offset, slot) in slots.into_iter().enumerate() {
            *slot = num(12 + offset)?;
        }
        let report_uid = int(0)?;
        out.deals.push(Deal {
            report_uid,
            core_uid: int(1)? as u64,
            core_name: r
                .get::<_, Option<String>>(name_at)
                .map_err(fail)?
                .unwrap_or_default(),
            strategy_id,
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
            sell_reason,
            fact_pnl: num(11)?,
            // Filled by `overlay_usdt_profit` off the USDT source, when there is one.
            profit: None,
            deltas,
            tick: None,
            pre_spike_ask: None,
            archived_take: None,
            // Filled by `overlay_hook_detect` off the raw report row's comment.
            hook_depth_pct: None,
            hook_stated_take_pct: None,
            step_lag_ms: 0.0,
            // Filled with the model inputs, once the archive and the parameters are in.
            stop_anchor: None,
            own_entry: None,
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

/// Fill the hook numbers of every deal the core wrote a detect for — [`Deal::hook_depth_pct`]
/// and [`Deal::hook_stated_take_pct`], out of the report row's `comment`.
///
/// Read from the RAW report tables rather than through the unified source: `comment` is a long
/// text column, the unified projection is what every other axis scans, and widening it for one
/// rule of one kind would put that text into every analytics query. The scan is bounded by the
/// same period the deals were read with and by the `Depth:` marker, and only the uids already
/// scanned are kept, so nothing grows with the size of the replica but the rows the axis holds.
///
/// Keyed by `(core_uid, reportuid)`, never by the uid alone: a report uid is unique WITHIN a
/// core — the order-trace archive keys its own rows by the pair, and this scan can hold several
/// cores at once, where one core's detect would otherwise be pinned onto another core's trade.
/// A source without all three columns (the legacy table) contributes nothing, and so does a
/// failure to read one: a hook trade then simply has no depth, which the model reads as "the
/// take rule of this kind is unknown here" rather than guessing a level.
///
/// Args:
///     conn: The snapshot the scan ran in.
///     q: The floored query the scan ran with — its period bounds are the parameters.
///     deals: The scanned deals, filled in place.
fn overlay_hook_detect(conn: &Connection, q: &Query, deals: &mut [Deal]) -> ReadResult<()> {
    const CTX: &str = "tuner: ticks deals (hook detect)";
    if deals.is_empty() {
        return Ok(());
    }
    let wanted: std::collections::HashSet<(u64, i64)> =
        deals.iter().map(|d| (d.core_uid, d.report_uid)).collect();
    let mut found: HashMap<(u64, i64), super::HookDetect> = HashMap::new();
    for src in crate::db::read_sources_res(conn)? {
        if !src.cols.contains("reportuid")
            || !src.cols.contains("comment")
            || !src.cols.contains("core_uid")
        {
            continue;
        }
        let sql = format!(
            "SELECT \"core_uid\", \"reportuid\", \"comment\" FROM \"{}\"
             WHERE \"closedate\" BETWEEN ?1 AND ?2 AND \"comment\" LIKE '%Depth:%'",
            src.table
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
        let mut rows = stmt
            .query(rusqlite::params![q.from, q.to])
            .map_err(|e| read_fail_on(conn, CTX, e))?;
        while let Some(r) = rows.next().map_err(|e| read_fail_on(conn, CTX, e))? {
            let key = (
                r.get::<_, Option<i64>>(0)
                    .map_err(|e| read_fail_on(conn, CTX, e))?
                    .unwrap_or(0) as u64,
                r.get::<_, Option<i64>>(1)
                    .map_err(|e| read_fail_on(conn, CTX, e))?
                    .unwrap_or(0),
            );
            if !wanted.contains(&key) {
                continue;
            }
            let comment = r
                .get::<_, Option<String>>(2)
                .map_err(|e| read_fail_on(conn, CTX, e))?
                .unwrap_or_default();
            if let Some(detect) = parse_hook_detect(&comment) {
                found.entry(key).or_insert(detect);
            }
        }
    }
    for deal in deals.iter_mut() {
        if let Some(detect) = found.get(&(deal.core_uid, deal.report_uid)) {
            deal.hook_depth_pct = Some(detect.depth_pct);
            deal.hook_stated_take_pct = detect.stated_take_pct;
        }
    }
    log::info!(
        target: crate::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks deals: hook detect read for {} of {} deal(s)",
        found.len(),
        deals.len()
    );
    Ok(())
}

/// Fill [`Deal::profit`] of every deal with the row's `profitbtc` off the USDT-valued source
/// (`tuner_source_usdt_on`), keyed by `reportuid`. A deal the USDT source does not carry —
/// a row that joined the replica between the two scans of one snapshot cannot exist, so this
/// is a source that projects the row differently — stays unpriced and is counted in the log.
///
/// Args:
///     conn: The snapshot the scan ran in.
///     q: The floored query the scan ran with (its period bounds are the parameters).
///     usdt_src: The USDT `FROM` source.
///     deals: The scanned deals, filled in place.
fn overlay_usdt_profit(
    conn: &Connection,
    q: &Query,
    usdt_src: &str,
    deals: &mut [Deal],
) -> ReadResult<()> {
    const CTX: &str = "tuner: ticks deals (USDT money)";
    // Keyed by the pair, like the hook overlay above: a report uid is unique only WITHIN a
    // core, and this scan routinely holds several.
    let sql = format!(
        "SELECT o.\"core_uid\", o.\"reportuid\", COALESCE(o.\"profitbtc\", 0) FROM {usdt_src}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut rows = stmt
        .query(rusqlite::params![q.from, q.to])
        .map_err(|e| read_fail_on(conn, CTX, e))?;
    let mut money: HashMap<(u64, i64), f64> = HashMap::new();
    while let Some(r) = rows.next().map_err(|e| read_fail_on(conn, CTX, e))? {
        let key = (
            r.get::<_, Option<i64>>(0)
                .map_err(|e| read_fail_on(conn, CTX, e))?
                .unwrap_or(0) as u64,
            r.get::<_, Option<i64>>(1)
                .map_err(|e| read_fail_on(conn, CTX, e))?
                .unwrap_or(0),
        );
        let profit = r
            .get::<_, Option<f64>>(2)
            .map_err(|e| read_fail_on(conn, CTX, e))?
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        money.insert(key, profit);
    }
    let mut unpriced = 0usize;
    for deal in deals.iter_mut() {
        deal.profit = money.get(&(deal.core_uid, deal.report_uid)).copied();
        if deal.profit.is_none() {
            unpriced += 1;
        }
    }
    if unpriced > 0 {
        log::warn!(
            target: crate::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks deals: {unpriced} of {} deal(s) missing from the USDT source, profit left empty",
            deals.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
