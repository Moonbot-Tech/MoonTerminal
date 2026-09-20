//! The model against a real data root — the phase-1 acceptance run, not a unit test.
//!
//! Ignored by default: it needs a data root and prints rather than asserts. Run it as
//! `MOON_TICKS_DATA_DIR=<exe dir> cargo test -p moon-core --target x86_64-pc-windows-msvc --lib
//! db::tuner::ticks::tests::real_data -- --ignored --nocapture`. It reads every MoonShot row
//! with millisecond stamps, takes its prints from `trades.sqlite` and its entry line from
//! `order_traces.sqlite`, runs [`verify`] on the parameters as of the buy, and prints one line
//! per deal plus the ✓ share per group. The environment variable is read HERE only, in a test a
//! developer runs by hand; the application never moves its data root on a variable.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

use super::super::mshot::DEFAULT_LATENCY_MS;
use super::super::params::{StrategyValues, exit_params, mshot_params, param_keys};
use super::super::*;
use crate::config::paths;
use crate::db::order_traces::{TraceEntry, read_many};
use crate::db::tuner::strategy_values_at;
use crate::feed::report_traces::ArchivedLineKind;
use crate::market::trade_replay::trade_cache::TradeCache;
use crate::symbol::{coin_match_key, coin_of_market};

/// The tape must reach this far back before the buy for the corridor to have a run-up.
const RUN_UP_MS: i64 = 30_000;

fn read_deals(reports: &Connection) -> Vec<Deal> {
    let mut rows = reports
        .prepare(
            "SELECT reportuid, core_uid, strategyid, coin, buydatems, closedatems,
                    buyprice, sellprice, spentbtc, isshort, sellreason,
                    d1m, d5m, d15m, d1h, d3h, d24h, dmark, pricebug, btc1hdelta, btc5mdelta,
                    exchange1hdelta
             FROM orders_rep
             WHERE buydatems > 0 AND closedatems > 0 AND deleted = 0
             ORDER BY buydatems",
        )
        .expect("query");
    rows.query_map([], |r| {
        let num = |i: usize| -> f64 { r.get::<_, Option<f64>>(i).ok().flatten().unwrap_or(0.0) };
        Ok(Deal {
            report_uid: r.get(0)?,
            core_uid: r.get::<_, i64>(1)? as u64,
            strategy_id: r.get(2)?,
            kind: String::new(),
            coin: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            buy_ms: r.get(4)?,
            close_ms: r.get(5)?,
            buy_price: num(6),
            sell_price: num(7),
            spent: num(8),
            is_short: r.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0,
            sell_reason: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            deltas: Deltas {
                d1m: num(11),
                d5m: num(12),
                d15m: num(13),
                d1h: num(14),
                d3h: num(15),
                d24h: num(16),
                dmark: num(17),
                pricebug: num(18),
                btc1h: num(19),
                btc5m: num(20),
                market1h: num(21),
            },
            tick: None,
        })
    })
    .expect("rows")
    .flatten()
    .collect()
}

/// The archived first point of the deal's own entry line, when the archive holds one.
fn archived_entry_start(deal: &Deal) -> Option<(i64, f64)> {
    let entries = read_many(deal.core_uid, &[deal.report_uid]).ok()?;
    match entries.get(&deal.report_uid)? {
        TraceEntry::Lines(lines) => lines
            .iter()
            .find(|l| l.own && l.kind == ArchivedLineKind::Entry)
            .and_then(|l| l.points.first().map(|&(t, p)| (t as i64, p))),
        TraceEntry::Empty { .. } => None,
    }
}

fn round3(v: Option<f64>) -> Option<f64> {
    v.map(|d| (d * 1000.0).round() / 1000.0)
}

#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_reproduction() {
    let Some(root) = std::env::var_os("MOON_TICKS_DATA_DIR") else {
        eprintln!("MOON_TICKS_DATA_DIR is not set; nothing to do");
        return;
    };
    assert!(paths::set_data_dir_override(PathBuf::from(root)));

    let reports =
        Connection::open_with_flags(paths::reports_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reports.sqlite");
    let deals = read_deals(&reports);
    eprintln!("deals with ms stamps: {}", deals.len());

    // Every (exchange, market) pair the tape holds — the deal's exchange key is not in the
    // report, so a coin is tried under each exchange and market spelling that stores it.
    let spans_db =
        Connection::open_with_flags(paths::trades_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("trades.sqlite");
    let pairs: Vec<(String, String)> = spans_db
        .prepare("SELECT DISTINCT exchange, market FROM spans")
        .expect("spans")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("pairs")
        .flatten()
        .collect();
    let cache = TradeCache::open(paths::trades_db_path()).expect("cache");
    let margin_ms = crate::market::trade_replay::margin_ms();

    let mut keys = param_keys();
    keys.push("SignalType".into());
    let defaults = HashMap::new();
    let (mut entry_hits, mut entry_n, mut exit_hits, mut exit_n, mut with_tape) = (0, 0, 0, 0, 0);
    let mut kinds_seen: HashMap<String, usize> = HashMap::new();
    for mut deal in deals {
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            continue;
        };
        deal.kind = values.get("SignalType").cloned().unwrap_or_default();
        *kinds_seen.entry(deal.kind.clone()).or_default() += 1;
        if !entry_model_for(&deal.kind) {
            continue;
        }
        let mut ticks: Vec<Tick> = Vec::new();
        let coin_key = coin_match_key(&deal.coin);
        for (exchange, market) in pairs
            .iter()
            .filter(|(_, m)| coin_match_key(coin_of_market(m)) == coin_key)
        {
            let Some(spans) = cache.read(
                exchange,
                market,
                deal.buy_ms - margin_ms,
                deal.close_ms + margin_ms,
            ) else {
                continue;
            };
            for span in spans {
                ticks.extend(span.ticks);
            }
        }
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        ticks.dedup_by(|a, b| a.time_ms == b.time_ms && a.price == b.price && a.qty == b.qty);
        let first = ticks.first().map(|t| t.time_ms as i64);
        let last = ticks.last().map(|t| t.time_ms as i64);
        if first.is_none_or(|f| f > deal.buy_ms - RUN_UP_MS)
            || last.is_none_or(|l| l < deal.close_ms)
        {
            continue;
        }
        with_tape += 1;
        deal.tick = infer_tick(&ticks);
        let entry_start = archived_entry_start(&deal);
        let sv = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        let entry = EntryParams::MoonShot(mshot_params(&sv, DEFAULT_LATENCY_MS));
        let exit = exit_params(&sv);
        let plain = verify(&deal, &ticks, &entry, &exit, None);
        let archived = verify(&deal, &ticks, &entry, &exit, entry_start);
        eprintln!(
            "{uid} {coin:<8} buy {buy:.6} | plain fill {fill:?} dev {dev:?} ✓{ok:?} | \
             archived start {start:?} fill {fill2:?} dev {dev2:?} ✓{ok2:?} | \
             exit {exit_kind:?} ✓{exit_ok:?} dev {exit_dev:?} | {reason} | ticks {n} step {tick:?}",
            uid = deal.report_uid,
            coin = deal.coin,
            buy = deal.buy_price,
            fill = plain.fill.map(|f| f.price),
            dev = round3(plain.entry_dev_pct),
            ok = plain.entry,
            start = entry_start,
            fill2 = archived.fill.map(|f| f.price),
            dev2 = round3(archived.entry_dev_pct),
            ok2 = archived.entry,
            exit_kind = archived.exit_kind,
            exit_ok = archived.exit,
            exit_dev = round3(archived.exit_dev_pct),
            reason = deal.sell_reason,
            n = ticks.len(),
            tick = deal.tick,
        );
        let best = if entry_start.is_some() {
            archived
        } else {
            plain
        };
        if let Some(ok) = best.entry {
            entry_n += 1;
            entry_hits += usize::from(ok);
        }
        if let Some(ok) = best.exit {
            exit_n += 1;
            exit_hits += usize::from(ok);
        }
    }
    eprintln!("kinds: {kinds_seen:?}");
    eprintln!(
        "with tape: {with_tape} · entry ✓ {entry_hits}/{entry_n} · exit ✓ {exit_hits}/{exit_n}"
    );
}
