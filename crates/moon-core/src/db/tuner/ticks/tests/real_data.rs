//! The model against a real data root — the phase-1 acceptance run, not a unit test.
//!
//! Ignored by default: it needs a data root and prints rather than asserts. Run it as
//! `MOON_TICKS_DATA_DIR=<exe dir> cargo test -p moon-core --target x86_64-pc-windows-msvc --lib
//! db::tuner::ticks::tests::real_data -- --ignored --nocapture`. It reads every deal of the
//! whole history through [`read_deals`] — the path the axis itself uses — takes each MoonShot
//! deal's prints through the worker's held-data query (`query_held`, the path the table's
//! coverage column uses) and its entry line from `order_traces.sqlite`, runs [`verify`] on the
//! parameters as of the buy, and prints one line per deal plus the ✓ share per group. The
//! environment variables are read HERE only, in a test a developer runs by hand (`MOON_TICKS_COIN`
//! narrows the run to one coin); the application never moves its data root on a variable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::super::calibrate;
use super::super::exit::ExitModel;
use super::super::mshot::DEFAULT_LATENCY_MS;
use super::super::params::{StrategyValues, exit_params, mshot_params, param_keys};
use super::super::*;
use crate::config::paths;
use crate::db::analytics::Query;
use crate::db::order_traces::{TraceEntry, read_many};
use crate::db::tuner::strategy_values_at;
use crate::feed::report_traces::ArchivedLineKind;
use crate::market::kline_cache::KlineCache;
use crate::market::trade_replay::{Coverage, TickQuery, long_position_ms, query_held};
use crate::symbol::{coin_match_key, coin_of_market};

/// Every point of an archived entry line and of an exit line, and whether the core answered
/// for the deal with lines at all.
type ArchivedLines = (Option<Vec<(i64, f64)>>, Option<Vec<(i64, f64)>>, bool);

/// The points of the deal's own entry line and of its own exit line, when the archive holds
/// them, and whether it answered with lines — as the axis reads them (`load.rs::ArchivedLines`).
fn archived_lines(deal: &Deal) -> ArchivedLines {
    let Ok(entries) = read_many(deal.core_uid, &[deal.report_uid]) else {
        return (None, None, false);
    };
    match entries.get(&deal.report_uid) {
        Some(TraceEntry::Lines(lines)) => {
            let entry = lines
                .iter()
                .find(|l| l.own && l.kind == ArchivedLineKind::Entry)
                .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
            let exit = lines
                .iter()
                .find(|l| l.own && l.kind == ArchivedLineKind::Exit)
                .map(|l| l.points.iter().map(|&(t, p)| (t as i64, p)).collect());
            (entry, exit, true)
        }
        _ => (None, None, false),
    }
}

/// The held prints of one market spelling inside the spans, through the worker, and its
/// coverage of them.
fn held_ticks(exchange_key: &str, market: &str, spans: &Coverage) -> (Vec<Tick>, Coverage) {
    let (reply, rx) = mpsc::channel();
    query_held(TickQuery {
        exchange_key: exchange_key.to_string(),
        market: market.to_string(),
        spans: spans.clone(),
        reply,
    });
    rx.recv_timeout(Duration::from_secs(10))
        .map(|answer| (answer.ticks, answer.covered))
        .unwrap_or_default()
}

/// One deal as the verdict saw it, for an analysis outside the probe: a JSON line in
/// `<dir>/deals.jsonl` (the row, the strategy's raw values, the held walk's points, the archived
/// Entry and Exit lines, the entry order's creation, placement and saved corridor, the MoonShot
/// bounds) and its prints as `t,price,qty,side` in `<dir>/ticks/<uid>.csv`.
#[allow(clippy::too_many_arguments)]
fn dump_deal(
    dir: &str,
    deal: &Deal,
    values: &HashMap<String, String>,
    ticks: &[Tick],
    held: &super::super::line::LineWalk,
    exit_points: Option<&[(i64, f64)]>,
    entry_points: Option<&[(i64, f64)]>,
    entry: &EntryParams,
) {
    use std::io::Write;
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(dir.join("ticks"));
    let row = serde_json::json!({
        "uid": deal.report_uid,
        "core": deal.core_name,
        "coin": deal.coin,
        "kind": deal.kind,
        "short": deal.is_short,
        "buy_ms": deal.buy_ms,
        "close_ms": deal.close_ms,
        "buy": deal.buy_price,
        "sell": deal.sell_price,
        "reason": deal.sell_reason,
        "tick": deal.tick,
        "values": values,
        "held_exit": [held.exit.t_ms, held.exit.price, format!("{:?}", held.exit.kind)],
        "held_points": held.points.iter().map(|p| (p.t_ms, p.price)).collect::<Vec<_>>(),
        "archive": exit_points,
        "entry": entry_points,
        "buy_set_ms": deal.buy_set_ms,
        "order_open_ms": deal.order_open_ms(),
        "entry_placed": deal.entry_placed,
        "corridor": deal.corridor,
        "mshot": match entry {
            EntryParams::MoonShot(p) => {
                let (near, far) = p.bounds_pct(&deal.deltas_at(deal.buy_ms));
                serde_json::json!({
                    "near": near,
                    "far": far,
                    "use_price": format!("{:?}", p.use_price),
                    "raise_wait_s": p.raise_wait_s,
                    "replace_delay_s": p.replace_delay_s,
                    "minus_satoshi": p.minus_satoshi,
                    "fast_algo": p.fast_algo,
                    "latency_ms": p.latency_ms,
                })
            }
            _ => serde_json::Value::Null,
        },
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("deals.jsonl"))
    {
        let _ = writeln!(f, "{row}");
    }
    let mut csv = String::with_capacity(ticks.len() * 32);
    for t in ticks {
        let side = if t.side == crate::feed::types::Side::Buy {
            'B'
        } else {
            'S'
        };
        csv.push_str(&format!(
            "{},{},{},{side}\n",
            t.time_ms as i64, t.price, t.qty
        ));
    }
    let _ = std::fs::write(
        dir.join("ticks").join(format!("{}.csv", deal.report_uid)),
        csv,
    );
}

/// Each core's exchange key — `<code>:<dex as 8 hex digits>`, the tape's own spelling — off the
/// `core N «name» identity: … -> ExchangeId { code: C, dex: D }` lines the application logs when
/// a core connects, since the report does not carry the venue and no core is connected here.
/// Reading a coin under every exchange that stores it instead mixed venues into one tape (AKE
/// sat under five), and judged a deal of a venue without a tape of its own — BB1 is Bybit, and
/// its tape is not in the store — on another venue's prints. Guessing the venue from where the
/// entry fill printed does not work either: a liquid coin prints the same price on Binance and
/// Bybit within a second, and BB1 "voted" Binance 49 to 36.
fn core_venues() -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir(paths::logs_dir_no_create()) else {
        return out;
    };
    for entry in dir.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text
            .lines()
            .filter(|l| l.contains("identity: exchange_code="))
        {
            let core = line
                .split_once("core ")
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .and_then(|n| n.parse::<u64>().ok());
            let id = line
                .split_once("ExchangeId { code: ")
                .and_then(|(_, rest)| {
                    let (code, rest) = rest.split_once(", dex: ")?;
                    let dex = rest.split_once(' ')?.0;
                    Some((code.parse::<u8>().ok()?, dex.parse::<u32>().ok()?))
                });
            if let (Some(core), Some((code, dex))) = (core, id) {
                out.insert(core, format!("{code}:{dex:08x}"));
            }
        }
    }
    out
}

/// Each core's PriceDown step lag off its own archived Exit lines, the way the axis calibrates
/// it (`calibrate::step_lag_samples` over the deals it loaded, the median per core).
fn core_step_lags(
    deals: &[Deal],
    keys: &[String],
    defaults: &HashMap<String, f64>,
) -> HashMap<u64, f64> {
    let mut samples: HashMap<u64, Vec<i64>> = HashMap::new();
    for deal in deals {
        if !is_tunable(&deal.kind, &deal.sell_reason) {
            continue;
        }
        let (_, Some(points), _) = archived_lines(deal) else {
            continue;
        };
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, keys)
        else {
            continue;
        };
        let exit = exit_params(&StrategyValues {
            values: &values,
            defaults,
        });
        samples
            .entry(deal.core_uid)
            .or_default()
            .extend(calibrate::step_lag_samples(deal, &exit, &points));
    }
    samples
        .into_iter()
        .filter_map(|(core, mut v)| calibrate::median_step_lag(&mut v).map(|lag| (core, lag)))
        .collect()
}

fn round3(v: Option<f64>) -> Option<f64> {
    v.map(|d| (d * 1000.0).round() / 1000.0)
}

/// BTC's market on every exchange the kline cache holds bars for — the BTC deltas are read off
/// it, as the table reads them off the market the catalog names (`FetchResolver`). The cache's
/// own spelling, since no core is connected here: a market whose coin is BTC, a USDT one first.
fn btc_markets() -> HashMap<String, String> {
    let Ok(db) =
        Connection::open_with_flags(paths::klines_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return HashMap::new();
    };
    let Ok(mut stmt) = db.prepare("SELECT DISTINCT exchange, market FROM chunks_v2") else {
        return HashMap::new();
    };
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let btc = coin_match_key("BTC");
    let mut out: HashMap<String, String> = HashMap::new();
    for (exchange, market) in pairs {
        if coin_match_key(coin_of_market(&market)) != btc {
            continue;
        }
        let usdt = market.to_ascii_uppercase().contains("USDT");
        match out.get(&exchange) {
            Some(held) if held.to_ascii_uppercase().contains("USDT") || !usdt => {}
            _ => {
                out.insert(exchange, market);
            }
        }
    }
    out
}

/// The delta summary the table shows (`deltas::summarize`), printed.
fn print_delta_quality(tracks: &[std::sync::Arc<deltas::DeltaTrack>], no_track: usize) {
    let quality = deltas::summarize(tracks.iter().map(|t| t.as_ref()));
    eprintln!(
        "live deltas: {} tracks, {} deals without one (no stamp the tape reaches); at the stamp, before the anchor:",
        quality.tracks, no_track
    );
    for field in &quality.fields {
        eprintln!(
            "  {:10} live {:4} · window covered {:>5} · within 0.1 pp {:4} of {:4} · median |err| {} pp",
            field.field.column(),
            field.live,
            field
                .coverage_median
                .map_or("—".to_string(), |c| format!("{:.0}%", c * 100.0)),
            field.reproduced,
            field.checked,
            field
                .error_median
                .map_or("—".to_string(), |e| format!("{e:.4}")),
        );
    }
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
        "NOTE: no core schema here, so `defaults` is empty — a field the strategy dump omits          (it is at the core default) reads as the model's own fallback, not as the core's.          The application passes the live schema (`strategy_field_defaults`)."
    );
    eprintln!(
        "deals with ms stamps: {} · without: {}",
        read.deals.len(),
        read.without_ms
    );

    // Every (exchange, market) pair the tape holds — the deal's market spelling is not in the
    // report, so a coin is tried under each market of its core's exchange that stores it.
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
    let venue_of_core = core_venues();
    eprintln!("core venues (from the identity lines of the logs): {venue_of_core:?}");

    let keys = param_keys();
    let defaults = HashMap::new();
    let core_lags = core_step_lags(&read.deals, &keys, &defaults);
    // The live deltas' history bars (`deltas::track_for`), off this data root's kline cache —
    // opened on a COPY of the data root: opening prunes past the retention, as the app does.
    // `MOON_TICKS_SNAPSHOT_DELTAS=1` runs the model on the report's snapshot, for the A/B.
    let snapshot_only = std::env::var_os("MOON_TICKS_SNAPSHOT_DELTAS").is_some();
    let klines = KlineCache::open(paths::klines_db_path());
    eprintln!(
        "deltas: {}",
        if snapshot_only {
            "the report's snapshot"
        } else if klines.is_some() {
            "live track"
        } else {
            "no kline cache — snapshot"
        }
    );
    let btc_of_exchange = btc_markets();
    eprintln!("BTC markets: {btc_of_exchange:?}");
    let mut tracks: Vec<std::sync::Arc<deltas::DeltaTrack>> = Vec::new();
    let mut no_track = 0usize;
    eprintln!("PriceDown step lag per core: {core_lags:?}");
    let (mut entry_hits, mut entry_n, mut exit_hits, mut exit_n, mut with_tape) = (0, 0, 0, 0, 0);
    let mut kinds_seen: HashMap<String, usize> = HashMap::new();
    let mut unfit: HashMap<String, usize> = HashMap::new();
    // Deals whose core no identity line named: skipped, and counted, so an empty `logs/` folder
    // reads as that in the summary rather than as a scope without tape.
    let mut no_venue = 0usize;
    let (mut fit_n, mut own_n, mut own_close) = (0usize, 0usize, 0usize);
    // MoonShot entries replayed from the order's creation (`mshot`, `Deal::entry_placed`), and
    // how many of those the model reproduced.
    let (mut created_n, mut created_hits, mut stamped) = (0usize, 0usize, 0usize);
    // MoonShot trades whose saved corridor (`Deal::corridor`) the model's band matches in width.
    let (mut corridor_n, mut corridor_hits) = (0usize, 0usize);
    let (mut own_sum, mut fact_sum) = (0.0f64, 0.0f64);
    for mut deal in read.deals {
        *kinds_seen.entry(deal.kind.clone()).or_default() += 1;
        // Every kind the axis takes, as the table does: a kind without an entry model replays
        // its exit from the factual entry.
        if !is_tunable(&deal.kind, &deal.sell_reason) {
            continue;
        }
        // One coin, when a single deal is under the glass: `MOON_TICKS_COIN=ARX`.
        if let Ok(only) = std::env::var("MOON_TICKS_COIN") {
            if deal.coin != only {
                continue;
            }
        }
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            continue;
        };
        let coin_key = coin_match_key(&deal.coin);
        // The same window and the same gate the axis applies (`load.rs::replay_row`): the
        // worker's coverage must include `required_spans`, whatever the prints say.
        let Some(window) = model_window(&deal, margin_ms, long_position_ms()) else {
            continue;
        };
        let spans = window.focus_spans();
        let mut ticks: Vec<Tick> = Vec::new();
        let mut covered = Coverage::none();
        let mut address: Option<(String, String)> = None;
        // The core's own venue only: a coin the tape holds under several exchanges is not one
        // tape, and the axis reads the deal's own (`RowAddress::exchange_key`). A core the logs
        // never named is skipped rather than replayed on a mixture.
        let Some(venue) = venue_of_core.get(&deal.core_uid) else {
            no_venue += 1;
            continue;
        };
        for (exchange, market) in pairs
            .iter()
            .filter(|(e, m)| e == venue && coin_match_key(coin_of_market(m)) == coin_key)
        {
            let (held, held_covered) = held_ticks(exchange, market, &spans);
            if address.is_none() && !held.is_empty() {
                address = Some((exchange.clone(), market.clone()));
            }
            ticks.extend(held);
            for &span in held_covered.spans() {
                covered.add(span);
            }
        }
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        ticks.dedup_by(|a, b| a.time_ms == b.time_ms && a.price == b.price && a.qty == b.qty);
        if ticks.is_empty() || !covered.covers(&required_spans(&window)) {
            continue;
        }
        // What the verdict comes to on a shorter tape: `MOON_TICKS_CLIP_MS=5000` keeps only that
        // much before the window's open and past the close, as a margin setting that low would.
        if let Some(clip_ms) = std::env::var("MOON_TICKS_CLIP_MS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
        {
            let (from_ms, to_ms) = (window.open_ms - clip_ms, window.close_ms + clip_ms);
            ticks.retain(|t| (from_ms..=to_ms).contains(&(t.time_ms as i64)));
            if ticks.is_empty() {
                continue;
            }
        }
        with_tape += 1;
        deal.tick = infer_tick(&ticks);
        let (entry_line, exit_points, answered) = archived_lines(&deal);
        let sv = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        let entry = if entry_model_for(&deal.kind) {
            // `MOON_TICKS_LATENCY_MS=<ms>` replays the entry with another replacement latency.
            let latency = std::env::var("MOON_TICKS_LATENCY_MS")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(DEFAULT_LATENCY_MS);
            EntryParams::MoonShot(mshot_params(&sv, latency))
        } else {
            EntryParams::Fact
        };
        let exit = exit_params(&sv);
        deal.step_lag_ms = core_lags.get(&deal.core_uid).copied().unwrap_or(0.0);
        if let (Some(cache), Some((exchange, market))) = (klines.as_ref(), address.as_ref()) {
            let btc = btc_of_exchange.get(exchange).map(String::as_str);
            let track = deltas::track_for(cache, exchange, market, btc, &deal, &ticks, &covered);
            match &track {
                Some(track) => tracks.push(track.clone()),
                None => no_track += 1,
            }
            if !snapshot_only {
                deal.delta_track = track;
            }
        }
        prepare_deal(
            &mut deal,
            &entry,
            &exit,
            OwnLines {
                entry: entry_line.as_deref(),
                exit: exit_points.as_deref(),
                answered,
            },
        );
        // The corridor the core saved against the model's band, `near` … `2 · far − near` off one
        // reference (`mshot`): the ratio of its edges is the band's width whatever the reference.
        if let (Some((down, up)), EntryParams::MoonShot(params)) = (deal.corridor, &entry) {
            let (near, far) = params.bounds_pct(&deal.deltas_at(deal.buy_ms));
            let (a, b) = (near / 100.0, (2.0 * far - near) / 100.0);
            let predicted = if deal.is_long() {
                (1.0 - a) / (1.0 - b)
            } else {
                (1.0 + b) / (1.0 + a)
            };
            corridor_n += 1;
            corridor_hits +=
                usize::from((down.max(up) / down.min(up) / predicted - 1.0).abs() <= 0.0005);
        }
        // The modelled line beside the archive's moves, for the eye.
        if let (Some(fill), Some(moves)) = (
            simulate(&deal, &ticks, &entry, &exit, entry_line.as_deref()).fill,
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
        // The walk the verdict itself judges — the factual entry, the archived take, the sell
        // held through the close — with every archived move beside the nearest modelled one,
        // so a miss on one point shows WHICH point and by how much.
        {
            let fact_exit = ExitParams {
                take_from_archive: true,
                ..exit.clone()
            };
            let fact_fill = verify::fact_sell_start(&deal, &exit, exit_points.as_deref());
            let held = ExitModel::new(&fact_exit).walk_held(
                &super::super::record::unanchored(&deal),
                &ticks,
                fact_fill,
                deal.close_ms,
            );
            if let Ok(dir) = std::env::var("MOON_TICKS_DUMP") {
                dump_deal(
                    &dir,
                    &deal,
                    &values,
                    &ticks,
                    &held,
                    exit_points.as_deref(),
                    entry_line.as_deref(),
                    &entry,
                );
            }
            eprintln!(
                "    held exit {:?} at {:+}ms of close · stop {:.3}% · model pts {}",
                held.exit.kind,
                held.exit.t_ms - deal.close_ms,
                super::super::exit::stop_pct(&exit, &deal, deal.buy_ms),
                held.points.len()
            );
            if let Some(points) = exit_points.as_deref() {
                for (t, p) in verify::archived_replacements(points) {
                    let near = held
                        .points
                        .iter()
                        .min_by_key(|m| (m.t_ms - t).abs())
                        .map(|m| (m.t_ms - t, (m.price - p) / p * 100.0));
                    eprintln!(
                        "    arch {:+}ms {:.8} (close {:+}ms) near {:?}",
                        t - deal.buy_ms,
                        p,
                        t - deal.close_ms,
                        near.map(|(dt, dp)| (dt, (dp * 1000.0).round() / 1000.0))
                    );
                }
            }
        }
        let plain = verify(&deal, &ticks, &entry, &exit, None, None);
        let archived = verify(
            &deal,
            &ticks,
            &entry,
            &exit,
            entry_line.as_deref(),
            exit_points.as_deref(),
        );
        eprintln!(
            "{uid} {coin:<8} {kind:<8} buy {buy:.6} | plain fill {fill:?} dev {dev:?} ✓{ok:?} | \
             archived start {start:?} fill {fill2:?} dev {dev2:?} ✓{ok2:?} | \
             exit {exit_kind:?} ✓{exit_ok:?} dev {exit_dev:?} line {line:?} | {reason} | ticks {n} step {tick:?}              | hook depth {hook_depth:?} core {hook_stated:?} model {hook_model:?}",
            uid = deal.report_uid,
            coin = deal.coin,
            kind = deal.kind,
            buy = deal.buy_price,
            fill = plain.fill.map(|f| f.price),
            dev = round3(plain.entry_dev_pct),
            ok = plain.entry,
            start = entry_line.as_ref().and_then(|l| l.first()),
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
            hook_depth = round3(deal.hook_depth_pct),
            hook_stated = round3(deal.hook_stated_take_pct),
            // The formula against the core's own number, for the same trade — the check that
            // keeps `docs-internal/STRATEGY_FORMULAS/moonhook.md` honest over time.
            hook_model = round3(
                deal.hook_depth_pct
                    .map(|d| super::super::hook::hook_take_pct(d, exit.hook_sell_level_pct))
            ),
        );
        // Which trades the search may run on, and — for those — how the trade's own settings
        // replay against the fact, with everything the fact proves in hand: the check that a
        // variant column counts the same money the "Fact" column does.
        let fit = fit_for_search(&archived);
        let own = simulate(&deal, &ticks, &entry, &exit, entry_line.as_deref());
        let fact = profit_pct(&deal, deal.buy_price, deal.sell_price);
        eprintln!(
            "    fit {fit} · own {:?} {:?} at {:?} · fact {:?}",
            own.exit.map(|e| e.kind),
            round3(own.profit_pct),
            own.exit.map(|e| e.t_ms - deal.close_ms),
            round3(fact),
        );
        if fit {
            fit_n += 1;
            if let (Some(own), Some(fact)) = (own.profit_pct, fact) {
                own_sum += own;
                fact_sum += fact;
                own_close += usize::from((own - fact).abs() <= 0.05);
                own_n += 1;
            }
        } else {
            let why = if archived.entry == Some(false) {
                "entry ✗".to_string()
            } else if exit.unmodelled.is_some() {
                format!("rule not modelled ({:?})", exit.unmodelled)
            } else if archived.exit.is_none() {
                "exit not judged".to_string()
            } else {
                let reason = deal.sell_reason.trim();
                let reason = reason.get(..reason.len().min(22)).unwrap_or(reason);
                format!("exit ✗ {reason}")
            };
            *unfit.entry(why).or_default() += 1;
        }
        // Counted as the axis counts it (`load.rs::replay_row_with` passes both archived lines
        // whenever it has them): the entry off its archived line when there is one — without it
        // the two verdicts are the same call — and the exit ALWAYS against the archived Exit
        // line. Taking the plain verdict's exit for a deal without an Entry line judged it with
        // no archive at all, which is not what the table shows.
        let entry_verdict = if entry_line.is_some() {
            archived.entry
        } else {
            plain.entry
        };
        if let Some(ok) = entry_verdict {
            entry_n += 1;
            entry_hits += usize::from(ok);
            // Replayed from the creation: the record proved the placement and the tape reaches
            // back to it — the condition `mshot` starts the order there on.
            stamped += usize::from(deal.order_open_ms().is_some());
            let from_creation = deal.entry_placed.is_some()
                && deal
                    .order_open_ms()
                    .is_some_and(|created| (ticks[0].time_ms as i64) <= created);
            if from_creation {
                created_n += 1;
                created_hits += usize::from(ok);
            }
        }
        if let Some(ok) = archived.exit {
            exit_n += 1;
            exit_hits += usize::from(ok);
        }
    }
    eprintln!("kinds: {kinds_seen:?}");
    eprintln!(
        "with tape: {with_tape} · entry ✓ {entry_hits}/{entry_n} · exit ✓ {exit_hits}/{exit_n} · \
         skipped, core venue unknown: {no_venue}"
    );
    eprintln!(
        "entry from the order's creation: ✓ {created_hits}/{created_n} · stamped entries {stamped}"
    );
    eprintln!("saved corridors the model's band matches to 0.05 %: {corridor_hits}/{corridor_n}");
    print_delta_quality(&tracks, no_track);
    let mut unfit: Vec<(String, usize)> = unfit.into_iter().collect();
    unfit.sort_by_key(|u| std::cmp::Reverse(u.1));
    eprintln!("fit for the search: {fit_n} of {with_tape} · left out: {unfit:?}");
    eprintln!(
        "own settings replayed on the fit trades: {own_n} closed, mean {:.3} % against the fact's {:.3} %, within 0.05 pp on {own_close}",
        own_sum / own_n.max(1) as f64,
        fact_sum / own_n.max(1) as f64,
    );
}
