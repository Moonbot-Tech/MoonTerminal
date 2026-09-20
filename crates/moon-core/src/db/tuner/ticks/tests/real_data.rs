//! The model against a real data root — the phase-1 acceptance run, not a unit test.
//!
//! Ignored by default: it needs a data root and prints rather than asserts. Run it as
//! `MOON_TICKS_DATA_DIR=<exe dir> cargo test -p moon-core --target x86_64-pc-windows-msvc --lib
//! db::tuner::ticks::tests::real_data -- --ignored --nocapture`. It reads every deal of the
//! whole history through [`read_deals`] — the path the axis itself uses — takes each MoonShot
//! deal's prints through the worker's held-data query (`query_held`, the path the table's
//! coverage column uses) and its entry line from `order_traces.sqlite`, runs [`verify`] on the
//! parameters as of the buy, and prints one line per deal plus the ✓ share per group. The
//! environment variable is read HERE only, in a test a developer runs by hand; the application
//! never moves its data root on a variable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::super::exit::ExitModel;
use super::super::mshot::DEFAULT_LATENCY_MS;
use super::super::params::{StrategyValues, exit_params, mshot_params, param_keys};
use super::super::*;
use crate::config::paths;
use crate::db::analytics::Query;
use crate::db::order_traces::{TraceEntry, read_many};
use crate::db::tuner::strategy_values_at;
use crate::feed::report_traces::ArchivedLineKind;
use crate::market::trade_replay::{Coverage, TickQuery, query_held};
use crate::symbol::{coin_match_key, coin_of_market};

/// The tape must reach this far back before the buy for the corridor to have a run-up.
const RUN_UP_MS: i64 = 30_000;

/// The archived first point of an entry line and every point of an exit line.
type ArchivedLines = (Option<(i64, f64)>, Option<Vec<(i64, f64)>>);

/// The archived first point of the deal's own entry line and every point of its own exit
/// line, when the archive holds them.
fn archived_lines(deal: &Deal) -> ArchivedLines {
    let Ok(entries) = read_many(deal.core_uid, &[deal.report_uid]) else {
        return (None, None);
    };
    match entries.get(&deal.report_uid) {
        Some(TraceEntry::Lines(lines)) => {
            let entry = lines
                .iter()
                .find(|l| l.own && l.kind == ArchivedLineKind::Entry)
                .and_then(|l| l.points.first().map(|&(t, p)| (t as i64, p)));
            let exit = lines
                .iter()
                .find(|l| l.own && l.kind == ArchivedLineKind::Exit)
                .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
            (entry, exit)
        }
        _ => (None, None),
    }
}

/// The held prints for a deal under one `(exchange, market)` spelling, through the worker.
fn held_ticks(exchange_key: &str, market: &str, from_ms: i64, to_ms: i64) -> Vec<Tick> {
    let (reply, rx) = mpsc::channel();
    query_held(TickQuery {
        exchange_key: exchange_key.to_string(),
        market: market.to_string(),
        spans: Coverage::one((from_ms, to_ms)),
        reply,
    });
    rx.recv_timeout(Duration::from_secs(10))
        .map(|answer| answer.ticks)
        .unwrap_or_default()
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
    // The replica reader needs the process lease the application takes at start; a running
    // terminal holds it, and the probe then has no honest way in.
    let Some(_permit) = crate::db::report_recovery::prepare() else {
        eprintln!(
            "reports replica lease unavailable ({:?}): close the terminal on this data root first",
            crate::db::report_recovery::status()
        );
        return;
    };

    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = read_deals(&scope).expect("deals");
    eprintln!(
        "deals with ms stamps: {} · without: {}",
        read.deals.len(),
        read.without_ms
    );

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
    let margin_ms = crate::market::trade_replay::margin_ms();

    let keys = param_keys();
    let defaults = HashMap::new();
    let (mut entry_hits, mut entry_n, mut exit_hits, mut exit_n, mut with_tape) = (0, 0, 0, 0, 0);
    let mut kinds_seen: HashMap<String, usize> = HashMap::new();
    for mut deal in read.deals {
        *kinds_seen.entry(deal.kind.clone()).or_default() += 1;
        if !entry_model_for(&deal.kind) {
            continue;
        }
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            continue;
        };
        let coin_key = coin_match_key(&deal.coin);
        let mut ticks: Vec<Tick> = Vec::new();
        for (exchange, market) in pairs
            .iter()
            .filter(|(_, m)| coin_match_key(coin_of_market(m)) == coin_key)
        {
            ticks.extend(held_ticks(
                exchange,
                market,
                deal.buy_ms - margin_ms,
                deal.close_ms + margin_ms,
            ));
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
        let (entry_start, exit_points) = archived_lines(&deal);
        let sv = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        let entry = EntryParams::MoonShot(mshot_params(&sv, DEFAULT_LATENCY_MS));
        let exit = exit_params(&sv);
        // The modelled line beside the archive's moves, for the eye.
        if let (Some(fill), Some(moves)) = (
            simulate(&deal, &ticks, &entry, &exit, entry_start).fill,
            exit_points.as_deref().map(verify::archived_replacements),
        ) {
            let modelled = ExitModel::new(&exit).walk(&deal, &ticks, fill);
            let fmt = |pts: &[(i64, f64)]| -> String {
                pts.iter()
                    .take(6)
                    .map(|(t, p)| format!("{:+}ms {:.6}", t - deal.buy_ms, p))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mine: Vec<(i64, f64)> = modelled.points.iter().map(|p| (p.t_ms, p.price)).collect();
            eprintln!("    model line: {}", fmt(&mine));
            eprintln!("    archive   : {}", fmt(&moves));
        }
        let plain = verify(&deal, &ticks, &entry, &exit, None, None);
        let archived = verify(
            &deal,
            &ticks,
            &entry,
            &exit,
            entry_start,
            exit_points.as_deref(),
        );
        eprintln!(
            "{uid} {coin:<8} buy {buy:.6} | plain fill {fill:?} dev {dev:?} ✓{ok:?} | \
             archived start {start:?} fill {fill2:?} dev {dev2:?} ✓{ok2:?} | \
             exit {exit_kind:?} ✓{exit_ok:?} dev {exit_dev:?} line {line:?} | {reason} | ticks {n} step {tick:?}",
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
            line = archived.line_points,
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
