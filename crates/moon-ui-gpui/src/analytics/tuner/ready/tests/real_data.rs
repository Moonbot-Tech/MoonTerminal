//! The readiness pass and the auto-pick against a real data root — the speed bench every change
//! of the pass is judged by, not a unit test.
//!
//! Ignored by default: it needs a data root and prints rather than asserts. Run it as
//! `MOON_TICKS_DATA_DIR=<copy of the exe dir> cargo test --release -p moon-ui-gpui --bin
//! moonterminal analytics::tuner::ready::tests::real_data::ready_real_data_bench -- --ignored
//! --exact --nocapture --test-threads=1`, on a COPY of the data root (opening the kline cache
//! prunes it). The root holds `data/`, `logs/` and `tape_archive/`.
//!
//! It reads the period's deals through `read_deals`, addresses them offline (each core's venue
//! off the `identity: exchange_code=` lines of `logs/`, the market off the tape store's own
//! inventory), calibrates the step lags once, then:
//! (a) times one whole readiness pass the way the app runs it, without the pause between
//! strategies, and says what that pause costs in production;
//! (c) times how long until the first 1, 10, 25 strategies are counted;
//! a second pass splits the time between the reads (`fetch_batch`) and the model
//! (`replay_fetched`);
//! (b) times the auto-pick on the largest and the smallest tunable strategy — the sample's
//! preparation and the search apart, the exit alone and both groups.
//!
//! `MOON_READY_BENCH_OUT=<file>` writes the parity dump: one line per strategy (class and
//! counts) and one per pick (the answer), sorted, so two runs — or a run before and after a
//! change — compare with `diff`. Environment: `MOON_READY_BENCH_DAYS` (30) the period before
//! `MOON_READY_BENCH_NOW_MS` (the newest close plus a day), `MOON_READY_BENCH_FIRST` (1,10,25),
//! `MOON_TICKS_DEFAULTS` (`key=value,…`, the schema defaults the app reads off a live core),
//! `MOON_READY_BENCH_SEED` (1), `MOON_READY_BENCH_RESTARTS` (10). The variables are read HERE
//! only; the application never moves its data root on one.

use crate::analytics::tuner::ticks::tail::Admission;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::super::super::ticks::ranges::auto_grids_for;
use moon_core::config::paths;
use moon_core::db::tuner::threshold_search::{SearchHandle, install_job};
use moon_core::db::tuner::ticks::params::param_keys;
use moon_core::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, SearchParams, SearchProfile, Suggested, suggest_with_candidate,
};
use moon_core::db::tuner::ticks::unmodelled::watched_keys;
use moon_core::db::tuner::ticks::{Deal, ModelSettings};
use moon_core::market::kline_cache::KlineCache;
use moon_core::symbol::{coin_match_key, coin_of_market};
use rusqlite::{Connection, OpenFlags};

use super::super::super::ticks::readiness::Readiness;
use super::super::super::ticks::state::{DealRow, RowAddress, TapeStatus, TicksData};
use super::super::super::ticks::{
    Fetched, PassReads, PendingDeal, archived_lines, calibrate_lags, fetch_batch, prepare_sample,
    replay_fetched, replay_timing,
};
use super::super::{
    ChunkLimits, ReadyClass, ReadyProgress, StrategyRows, count_strategies, group_by_strategy,
};

const DAY_MS: i64 = 86_400_000;

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Each core's exchange code and dex off the `core N «name» identity: … -> ExchangeId { code: C,
/// dex: D }` lines the application logs when a core connects — the report carries no venue and
/// no core is connected here. As moon-core's `ticks` real-data bench reads them.
fn core_exchanges() -> HashMap<u64, (u8, u32)> {
    let mut out = HashMap::new();
    let Ok(dir) = std::fs::read_dir(paths::logs_dir_no_create()) else {
        return out;
    };
    let mut files: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    // Later logs win, in a fixed order.
    files.sort();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
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
            if let (Some(core), Some(id)) = (core, id) {
                out.insert(core, id);
            }
        }
    }
    out
}

/// BTC's market per exchange key the kline cache holds bars for, a USDT one first — the BTC
/// deltas are read off it.
fn btc_markets() -> HashMap<String, String> {
    let Ok(db) =
        Connection::open_with_flags(paths::klines_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return HashMap::new();
    };
    let Ok(mut stmt) = db.prepare("SELECT DISTINCT exchange, market FROM chunks_v2") else {
        return HashMap::new();
    };
    let mut pairs: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    pairs.sort();
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

/// Where each `(core, coin)` of the deals lives, offline: the core's venue off the logs, the
/// coin's market off the tape store's inventory on that exchange key (a USDT market first, then
/// by name). A core the logs never named, or a coin the store holds no market for, stays
/// unaddressed — the app would resolve it off the catalog and could still address it.
fn addresses(
    deals: &[Deal],
    exchanges: &HashMap<u64, (u8, u32)>,
    inventory: &[(String, String)],
    btc: &HashMap<String, String>,
) -> HashMap<(u64, String), Option<Arc<RowAddress>>> {
    let mut out = HashMap::new();
    for deal in deals {
        out.entry((deal.core_uid, deal.coin.clone()))
            .or_insert_with(|| {
                let &(code, dex) = exchanges.get(&deal.core_uid)?;
                let venue = moon_core::venue::venue(code);
                let exchange_key = format!("{code}:{dex:08x}");
                let coin = coin_match_key(&deal.coin);
                let mut markets: Vec<&String> = inventory
                    .iter()
                    .filter(|(e, m)| {
                        *e == exchange_key && coin_match_key(coin_of_market(m)) == coin
                    })
                    .map(|(_, m)| m)
                    .collect();
                markets.sort_by_key(|m| (!m.to_ascii_uppercase().contains("USDT"), m.to_string()));
                let market = markets.first()?.to_string();
                Some(Arc::new(RowAddress {
                    core_uid: deal.core_uid,
                    venue,
                    btc_market: btc.get(&exchange_key).cloned(),
                    exchange_key,
                    market,
                }))
            });
    }
    out
}

/// One strategy's rows split as `count_strategies` splits them.
fn targets_of(rows: &[(Deal, Option<Arc<RowAddress>>)]) -> Vec<(Deal, Arc<RowAddress>)> {
    rows.iter()
        .filter_map(|(deal, address)| Some((deal.clone(), address.clone()?)))
        .collect()
}

/// The parity line of one strategy's class.
fn class_cells(class: ReadyClass) -> &'static str {
    match class {
        ReadyClass::Can { .. } => "can",
        ReadyClass::Few { .. } => "few",
        ReadyClass::NoTape => "notape",
        ReadyClass::Pending => "pending",
        ReadyClass::Unread => "unread",
    }
}

/// `text` with the number after every `label` replaced by `-`.
fn mask_count(text: &str, label: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(label) {
        let (head, tail) = rest.split_at(at + label.len());
        out.push_str(head);
        out.push('-');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// The process's CPU time so far, user and kernel, in seconds. NaN off Windows, where the bench
/// reports wall time only.
#[cfg(not(windows))]
fn process_cpu_s() -> f64 {
    f64::NAN
}

/// The process's CPU time so far, user and kernel, in seconds.
#[cfg(windows)]
fn process_cpu_s() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo-handle of this process and four FILETIMEs this frame owns.
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if read.is_err() {
        return 0.0;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (ticks(kernel) + ticks(user)) as f64 / 1e7
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// Measure complete offline readiness and picks, with uncancelled replays for parity output.
#[test]
#[ignore = "needs MOON_TICKS_DATA_DIR"]
fn ready_real_data_bench() {
    let Some(root) = std::env::var_os("MOON_TICKS_DATA_DIR") else {
        eprintln!("MOON_TICKS_DATA_DIR is not set; nothing to do");
        return;
    };
    assert!(paths::set_data_dir_override(PathBuf::from(root)));
    let Some(_permit) = moon_core::db::report_recovery::prepare() else {
        eprintln!(
            "reports replica lease unavailable ({:?}): close the terminal on this data root first",
            moon_core::db::report_recovery::status()
        );
        return;
    };
    SearchProfile::enable_timing();
    let days: i64 = env_or("MOON_READY_BENCH_DAYS", 30);
    let seed: u64 = env_or("MOON_READY_BENCH_SEED", 1);
    let restarts: usize = env_or("MOON_READY_BENCH_RESTARTS", 10);
    let mut firsts: Vec<usize> = std::env::var("MOON_READY_BENCH_FIRST")
        .unwrap_or_else(|_| "1,10,25".into())
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    firsts.sort_unstable();
    let defaults: HashMap<String, f64> = std::env::var("MOON_TICKS_DEFAULTS")
        .map(|spec| {
            spec.split(',')
                .filter_map(|pair| pair.split_once('='))
                .filter_map(|(k, v)| Some((k.trim().to_ascii_lowercase(), v.trim().parse().ok()?)))
                .collect()
        })
        .unwrap_or_default();
    let model = ModelSettings::default();

    // The whole history once, then the period before "now" cut out of it: the newest close
    // decides the default "now", so the period does not slide between two runs.
    let scope = moon_core::db::analytics::Query {
        from: -1,
        to: moon_core::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: moon_core::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let started = Instant::now();
    let read = moon_core::db::tuner::ticks::read_deals(&scope).expect("deals");
    let newest = read.deals.iter().map(|d| d.close_ms).max().unwrap_or(0);
    let now_ms: i64 = env_or("MOON_READY_BENCH_NOW_MS", newest + DAY_MS);
    let from_ms = now_ms - days * DAY_MS;
    let deals: Vec<Deal> = read
        .deals
        .into_iter()
        .filter(|d| d.close_ms >= from_ms && d.close_ms < now_ms)
        .collect();
    eprintln!(
        "[+] read_deals: {} deal(s) in {days} day(s) before now_ms {now_ms}, read in {}",
        deals.len(),
        ms(started.elapsed())
    );

    let inventory: Vec<(String, String)> =
        match moon_core::market::trade_replay::trade_cache::handle().and_then(|c| c.inventory()) {
            Some(Ok(inventory)) => inventory.keys,
            other => {
                eprintln!("[x] trades.sqlite gave no market list ({other:?}); nothing to replay");
                return;
            }
        };
    let exchanges = core_exchanges();
    let btc = btc_markets();
    let klines = KlineCache::open(paths::klines_db_path());
    eprintln!(
        "[+] markets with a held tape: {} | cores named by the logs: {} | BTC markets: {} | klines.sqlite: {} | order_traces.sqlite: {}",
        inventory.len(),
        exchanges.len(),
        btc.len(),
        if klines.is_some() { "open" } else { "absent" },
        if paths::order_traces_db_path().exists() {
            "present"
        } else {
            "absent"
        }
    );
    let addresses = addresses(&deals, &exchanges, &inventory, &btc);
    let long_position_ms = moon_core::market::trade_replay::long_position_ms();
    let min_tail_s = super::super::super::ticks::tail::effective_s();

    // The step lags once, off every addressed deal's archive — as the card's own load does
    // before the pass replays on what it found. The archives alone: no tape is read here, so
    // the pass below starts as cold as the app's.
    let started = Instant::now();
    let rows: Vec<DealRow> = deals
        .iter()
        .filter_map(|deal| {
            let address = addresses
                .get(&(deal.core_uid, deal.coin.clone()))?
                .clone()?;
            Some(DealRow {
                deal: deal.clone(),
                tape: TapeStatus::Missing,
                verdict: None,
                address: Some(address),
                ticks: None,
                entry_line: None,
                held: None,
            })
        })
        .collect();
    let traces = archived_lines(&rows);
    eprintln!(
        "[+] archived lines: {} of {} addressed deal(s) answered, {} with an entry line, {} with an exit line",
        traces.values().filter(|l| l.answered).count(),
        rows.len(),
        traces.values().filter(|l| l.entry_points.is_some()).count(),
        traces.values().filter(|l| l.exit_points.is_some()).count()
    );
    calibrate_lags(
        &Fetched {
            rows,
            tapes: HashMap::new(),
            traces,
        },
        &defaults,
        model,
    );
    eprintln!("[+] step lags calibrated in {}", ms(started.elapsed()));

    // Strategies by deal count, the biggest first; a tie by key.
    let mut sizes: HashMap<String, usize> = HashMap::new();
    for deal in &deals {
        *sizes
            .entry(format!("{}@{}", deal.strategy_id, deal.core_uid))
            .or_default() += 1;
    }
    let mut order: Vec<(String, usize)> = sizes.into_iter().collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let order: Vec<String> = order.into_iter().map(|(k, _)| k).collect();
    let groups: Vec<StrategyRows> = group_by_strategy(deals, &addresses, &order);
    let addressed: HashMap<String, (usize, usize)> = groups
        .iter()
        .map(|(key, rows)| {
            (
                key.clone(),
                (rows.len(), rows.iter().filter(|(_, a)| a.is_some()).count()),
            )
        })
        .collect();
    eprintln!(
        "[+] {} strateg(ies), {} deal(s), {} addressed",
        groups.len(),
        addressed.values().map(|v| v.0).sum::<usize>(),
        addressed.values().map(|v| v.1).sum::<usize>()
    );

    // (a) and (c): one whole pass, as the app runs it, with a sampler on its progress.
    let progress = ReadyProgress::default();
    let done = AtomicBool::new(false);
    let reads = PassReads::new();
    let cpu_before = process_cpu_s();
    // Every replayed row as the pass replayed it, for the per-row parity lines.
    let row_lines: std::sync::Mutex<Vec<(String, i64, String)>> = std::sync::Mutex::default();
    let record = |rows: &[DealRow]| {
        let mut out = row_lines.lock().expect("row lines");
        for row in rows {
            let key = format!("{}@{}", row.deal.strategy_id, row.deal.core_uid);
            let line = format!(
                "row\t{key}\t{}\t{:?}\tfit={}\tticks={}\tfetchable={}\theld={:?}\t{:?}",
                row.deal.report_uid,
                row.tape,
                row.fit(),
                row.ticks.is_some(),
                row.fetchable_with(&Admission::at(min_tail_s, now_ms)),
                row.held,
                row.verdict
            );
            out.push((key, row.deal.report_uid, line));
        }
    };
    let timing_before = (
        replay_timing::STRATEGY_VALUES.read(),
        replay_timing::KLINES.read(),
    );
    let started = Instant::now();
    let (counts, reached) = std::thread::scope(|s| {
        let sampler = s.spawn(|| {
            let mut reached: Vec<Option<Duration>> = vec![None; firsts.len()];
            loop {
                let (n, _) = progress.counts();
                for (i, &first) in firsts.iter().enumerate() {
                    if reached[i].is_none() && n >= first {
                        reached[i] = Some(started.elapsed());
                    }
                }
                if done.load(Ordering::Relaxed) {
                    return reached;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let counts = count_strategies(
            groups.clone(),
            |targets| fetch_batch(targets, long_position_ms),
            |fetched| {
                let rows = replay_fetched(
                    fetched,
                    &defaults,
                    model,
                    klines.as_ref(),
                    now_ms,
                    Some(&reads),
                    None,
                );
                record(&rows);
                rows
            },
            &progress,
            ChunkLimits::app(),
            now_ms,
            min_tail_s,
        );
        done.store(true, Ordering::Relaxed);
        (counts, sampler.join().expect("sampler"))
    });
    let pass = started.elapsed();
    let pass_cpu = process_cpu_s() - cpu_before;
    let counts = counts.expect("not cancelled");
    let can = counts
        .values()
        .filter(|r| matches!(ReadyClass::of(r), ReadyClass::Can { .. }))
        .count();
    eprintln!(
        "[+] (a) readiness pass: {} over {} strateg(ies), {can} can tune | chunks of at most {} | peak tape in flight {:.1} MiB",
        ms(pass),
        groups.len(),
        ChunkLimits::app().max_count,
        progress.peak_bytes() as f64 / (1024.0 * 1024.0)
    );
    eprintln!(
        "[+] (a) CPU {:.1} s over {:.1} s wall: {:.1} cores busy of {} pool thread(s)",
        pass_cpu,
        pass.as_secs_f64(),
        pass_cpu / pass.as_secs_f64().max(1e-9),
        moon_core::db::tuner::threshold_search::search_threads()
    );
    let (sv, kl) = (
        replay_timing::STRATEGY_VALUES.read(),
        replay_timing::KLINES.read(),
    );
    eprintln!(
        "[+] reads inside the pass's replay (summed over threads): strategy_values_at {} call(s) {:.1} s | deltas::track_for {} call(s) {:.1} s",
        sv.0 - timing_before.0.0,
        sv.1 - timing_before.0.1,
        kl.0 - timing_before.1.0,
        kl.1 - timing_before.1.1
    );
    for (first, at) in firsts.iter().zip(&reached) {
        eprintln!(
            "[+] (c) first {first} counted after {}",
            at.map_or("never".to_string(), ms)
        );
    }

    // The same pass again, the reads and the model timed apart (the tapes are warm now).
    let wanted: Vec<(String, usize)> = {
        let mut can: Vec<(String, usize)> = counts
            .iter()
            .filter_map(|(k, r)| match ReadyClass::of(r) {
                ReadyClass::Can { fit } => Some((k.clone(), fit)),
                _ => None,
            })
            .collect();
        can.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let largest = can
            .iter()
            .rev()
            .find(|(_, fit)| *fit >= 200)
            .or(can.last())
            .cloned();
        let smallest = can.first().cloned();
        let mut picks: Vec<(String, usize)> = Vec::new();
        for (label, pick) in [("largest", largest), ("smallest", smallest)] {
            if let Some((key, fit)) = pick {
                eprintln!("[+] (b) {label} can-tune strategy: {key}, {fit} fit");
                if !picks.iter().any(|(k, _)| *k == key) {
                    picks.push((key, fit));
                }
            }
        }
        picks
    };
    let wanted_keys: HashSet<&str> = wanted.iter().map(|(k, _)| k.as_str()).collect();
    let (mut fetch_t, mut replay_t, mut differs) = (Duration::ZERO, Duration::ZERO, 0usize);
    let mut kept: HashMap<String, Vec<DealRow>> = HashMap::new();
    let reads = PassReads::new();
    install_job(|| {
        for (key, rows) in &groups {
            let targets = targets_of(rows);
            if targets.is_empty() {
                continue;
            }
            let t = Instant::now();
            let fetched = fetch_batch(targets, long_position_ms);
            fetch_t += t.elapsed();
            let t = Instant::now();
            let replayed = replay_fetched(
                fetched,
                &defaults,
                model,
                klines.as_ref(),
                now_ms,
                Some(&reads),
                None,
            );
            replay_t += t.elapsed();
            let mut data = TicksData {
                rows: replayed
                    .into_iter()
                    .chain(
                        rows.iter()
                            .filter(|(_, a)| a.is_none())
                            .map(|(deal, _)| DealRow {
                                deal: deal.clone(),
                                tape: TapeStatus::NoAddress,
                                verdict: None,
                                address: None,
                                ticks: None,
                                entry_line: None,
                                held: None,
                            }),
                    )
                    .collect(),
                ..Default::default()
            };
            data.retain_within_cap();
            if Readiness::of_rows(&data.rows, now_ms, min_tail_s) != counts[key] {
                differs += 1;
            }
            if wanted_keys.contains(key.as_str()) {
                kept.insert(key.clone(), data.rows);
            }
        }
    });
    eprintln!(
        "[+] phase split (second pass): fetch_batch {} | replay_fetched {} | {differs} strateg(ies) counted differently than the first pass",
        ms(fetch_t),
        ms(replay_t)
    );

    // (b) the auto-pick on each wanted strategy: exit alone, then both groups.
    let mut keys = param_keys();
    for key in watched_keys() {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let mut pick_lines: Vec<String> = Vec::new();
    for (key, _) in &wanted {
        let Some(rows) = kept.remove(key) else {
            continue;
        };
        let mut own: HashMap<(i64, u64), Arc<HashMap<String, String>>> = HashMap::new();
        let mut pending: Vec<PendingDeal> = rows
            .iter()
            .filter(|r| r.fit() && r.ticks.is_some())
            .filter_map(|row| {
                let base = own
                    .entry((row.deal.strategy_id, row.deal.core_uid))
                    .or_insert_with(|| {
                        Arc::new(moon_core::db::tuner::strategy_current_values(
                            row.deal.strategy_id,
                            Some(row.deal.core_uid),
                            &keys,
                        ))
                    })
                    .clone();
                Some(PendingDeal {
                    deal: row.deal.clone(),
                    tape: row.ticks.clone()?,
                    entry_line: row.entry_line.clone(),
                    trail_ms: row.held.map(|(_, trail)| trail).unwrap_or(0),
                    own: base,
                })
            })
            .collect();
        pending.sort_by_key(|d| d.deal.close_ms);
        let Some(first) = pending.first() else {
            continue;
        };
        let kind = first.deal.kind.clone();
        let grids = auto_grids_for(&kind, &first.own, &defaults);
        let held = HashMap::new();
        let locked = HashSet::new();
        for (mode, vary_entry) in [("exit", false), ("both", true)] {
            let t = Instant::now();
            let sample = prepare_sample(pending.clone());
            let prepare = t.elapsed();
            let params = SearchParams {
                held: &held,
                defaults: &defaults,
                kind: &kind,
                vary_entry,
                vary_exit: true,
                locked: &locked,
                grids: &grids,
                restarts,
                min_n: None,
                seed: Some(seed),
                train_frac: 0.7,
                max_passes: DEFAULT_MAX_PASSES,
                model,
                keep_corridor: true,
                risk: Default::default(),
                screen_entry: true,
            };
            let before = SearchProfile::now();
            let cpu_before = process_cpu_s();
            let handle = SearchHandle::new();
            let t = Instant::now();
            // When each restart finished, off the handle, every 50 ms.
            let finished = AtomicBool::new(false);
            let (suggested, restart_done) = std::thread::scope(|s| {
                let watcher = s.spawn(|| {
                    let mut at: Vec<f64> = Vec::new();
                    while !finished.load(Ordering::Relaxed) {
                        while handle.completed() > at.len() {
                            at.push(t.elapsed().as_secs_f64());
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    at
                });
                let out = suggest_with_candidate(&sample, &params, &handle);
                finished.store(true, Ordering::Relaxed);
                (out, watcher.join().expect("watcher"))
            });
            let Suggested {
                result,
                candidate,
                searched,
            } = suggested;
            let search = t.elapsed();
            let profile = SearchProfile::now().since(&before);
            let cpu = process_cpu_s() - cpu_before;
            let threads = moon_core::db::tuner::threshold_search::search_threads();
            let capacity = search.as_secs_f64() * threads as f64;
            eprintln!(
                "[+] (b) profile {key} {mode}: replay cache {} hit(s), {} miss(es), {} uncached | time inside the restarts' replay calls {:.1} s, fill reads {:.1} s (wall per call, summed; a call fans out itself) | CPU {:.1} s of {threads} pool thread(s) x {:.1} s wall = {:.0}% busy | restarts done at {:?} s",
                profile.replay_hits,
                profile.replay_misses,
                profile.replay_uncached,
                profile.replay_nanos as f64 / 1e9,
                profile.fill_nanos as f64 / 1e9,
                cpu,
                search.as_secs_f64(),
                100.0 * cpu / capacity.max(1e-9),
                restart_done
                    .iter()
                    .map(|v| (v * 10.0).round() / 10.0)
                    .collect::<Vec<_>>()
            );
            let line = match &result {
                Ok(found) => {
                    let mut values = found.values.clone();
                    values.sort();
                    eprintln!(
                        "[+] (b) {key} {mode}: {} deal(s), prepare_sample {} | suggest_with_candidate {} | {} evaluation(s), {} on fills read before",
                        sample.len(),
                        ms(prepare),
                        ms(search),
                        found.stats.evaluations,
                        found.stats.fills_reused
                    );
                    format!(
                        "pick\t{key}\t{mode}\tfound\t{values:?}\t{found:?}\tcandidate={candidate:?}\tsearched={searched:?}\t{seed}"
                    )
                }
                Err(miss) => {
                    eprintln!(
                        "[+] (b) {key} {mode}: {} deal(s), prepare_sample {} | suggest_with_candidate {} | no answer {miss:?}",
                        sample.len(),
                        ms(prepare),
                        ms(search)
                    );
                    format!(
                        "pick\t{key}\t{mode}\tmiss {miss:?}\t[]\tcandidate={candidate:?}\tsearched={searched:?}\t{seed}"
                    )
                }
            };
            pick_lines.push(line);
        }
    }

    if let Ok(out) = std::env::var("MOON_READY_BENCH_OUT") {
        let mut keys: Vec<&String> = counts.keys().collect();
        keys.sort();
        let mut rows_of: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        for (key, uid, line) in row_lines.into_inner().expect("row lines") {
            rows_of.entry(key).or_default().push((uid, line));
        }
        let mut text = String::new();
        for key in keys {
            let ready = counts[key];
            let (n, with_address) = addressed.get(key).copied().unwrap_or_default();
            text.push_str(&format!(
                "{key}\t{}\t{}\t{}\t{}\t{n}\t{with_address}\n",
                class_cells(ReadyClass::of(&ready)),
                ready.fit,
                ready.covered,
                ready.fetchable
            ));
            let mut rows = rows_of.remove(key.as_str()).unwrap_or_default();
            rows.sort();
            for (_, line) in rows {
                text.push_str(&line);
                text.push('\n');
            }
        }
        pick_lines.sort();
        for line in pick_lines {
            // How many points hit the fill cache depends on how the parallel restarts happened
            // to interleave, not on the answer: masked, so two runs compare.
            let line = mask_count(&line, "fills_reused: ");
            text.push_str(&line);
            text.push('\n');
        }
        let path = PathBuf::from(out);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, text).expect("parity dump");
        eprintln!("[+] parity dump: {}", path.display());
    }
}
