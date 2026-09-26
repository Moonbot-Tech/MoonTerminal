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
//! narrows the run to one coin; `MOON_TICKS_DUMP=<dir>` writes every deal and its prints for an
//! analysis outside; `MOON_TICKS_LATENCY_MS` replays with another latency and
//! `MOON_TICKS_LATENCY_BASE_MS` with that plus each core's archived round trip, the entry's only
//! unless `MOON_TICKS_LATENCY_EXIT` is set; `MOON_TICKS_PATH_DEBUG` prints each order's modelled
//! and archived path; `MOON_TICKS_VARIANT="Key=value,…"` replays each deal under those values laid
//! over its own and prints the line it walked; `MOON_TICKS_SEARCH=<kind>` runs the search over
//! that kind's fit deals, the Delta Modifiers section alone, and prints what it found); the
//! application never moves its data root on a variable.

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

mod money;
mod search;

/// One trade's d1m and d5m errors at the report's stamp (`StampCheck::error`).
type ShortErrors = (Option<f64>, Option<f64>);

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
/// bounds, the model's own take and the delta-modifier sum around the fill) and its prints as
/// `t,price,qty,side` in `<dir>/ticks/<uid>.csv`.
#[allow(clippy::too_many_arguments)]
fn dump_deal(
    dir: &str,
    deal: &Deal,
    values: &HashMap<String, String>,
    ticks: &[Tick],
    held: &super::super::exit::line::LineWalk,
    exit_points: Option<&[(i64, f64)]>,
    entry_points: Option<&[(i64, f64)]>,
    entry: &EntryParams,
    exit: &ExitParams,
) {
    use std::io::Write;
    // The stop as the verdict reads it (`verify::verify_stop`): the model's level off the fact's
    // buy, the level the core printed into the reason, and the activation — the archive's jump
    // past that level.
    let stop = super::super::exit::stops::stop_pct(exit, deal, deal.buy_ms);
    let level = super::super::exit::level_off_buy(deal.buy_price, stop, deal.is_long());
    let stated = verify::stated_stop_level(&deal.sell_reason);
    let activation = verify::stop_jump_level(deal, exit)
        .and_then(|jump_at| verify::archived_stop_jump(deal, jump_at, exit_points));
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(dir.join("ticks"));
    let sum_around_fill: Vec<(i64, f64)> = [
        -60_000i64, -10_000, -2_000, -500, 0, 250, 500, 1_000, 2_000, 5_000,
    ]
    .iter()
    .map(|dt| {
        (
            *dt,
            super::super::exit::delta_mods::modifier_sum(exit, deal, deal.buy_ms + dt),
        )
    })
    .collect();
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
        "gap": deal.gap.as_ref().map(|g| (g.from_ms, g.to_ms)),
        "held_points": held.points.iter().map(|p| (p.t_ms, p.price)).collect::<Vec<_>>(),
        "archive": exit_points,
        "entry": entry_points,
        "buy_set_ms": deal.buy_set_ms,
        "order_open_ms": deal.order_open_ms(),
        "entry_placed": deal.entry_placed,
        "corridor": deal.corridor,
        // The take the rules place off the fact's fill, modifiers and all, beside what they are
        // built from — the check against the archive's first point outside the probe.
        "model_take": ExitModel::new(exit).take_level(
            deal,
            ticks,
            Fill {
                t_ms: deal.buy_ms,
                price: deal.buy_price,
            },
        ),
        "modifier_sum": super::super::exit::delta_mods::modifier_sum(exit, deal, deal.buy_ms),
        // The same sum around the fill, for when the core actually reads the deltas: offsets in
        // ms from the fill. `tracked` says only that the deal HAS a live track — outside its
        // covered stretches the track answers with the snapshot, so an offset may still read it.
        "modifier_sum_at": sum_around_fill,
        "tracked": deal.delta_track.is_some(),
        // Whether the record held a reading of the core's sum — the sum above is then inside the
        // band that reading allows, the model's own where it already was.
        "fact_modifier": deal.fact_modifier.is_some(),
        "hook_depth": deal.hook_depth_pct,
        "hook_stated": deal.hook_stated_take_pct,
        "stop": {
            "pct": stop,
            "level": level,
            "stated": stated,
            "activation": activation,
            "fast": exit.fast_stop_loss,
            "ema": exit.stop_loss_ema,
            "delay_s": exit.stop_loss_delay_s,
        },
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
                    "latency_ms": p.model.latency_ms,
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
        let exit = exit_params(
            &StrategyValues {
                values: &values,
                defaults,
            },
            ModelSettings::default(),
        );
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

/// How much of the order's archived path the model walked: the archived moves after the creation
/// and before the fill, the ones the model also made (within a second, on the same price step),
/// and the model's moves the archive never shows.
#[derive(Default)]
struct PathTally {
    deals: usize,
    whole: usize,
    archived: usize,
    matched: usize,
    extra: usize,
    /// Archived moves with a model move within the second, whatever its price.
    timed: usize,
    /// For those, how far the nearest model move's level sat, per cent of the archived one —
    /// `[0]` strategies whose `MShotAdd*` move the corridor, `[1]` those whose do not.
    level_errors: [Vec<f64>; 2],
}

impl PathTally {
    fn add(&mut self, deal: &Deal, model: &[(i64, f64)], archived: &[(i64, f64)], moved: bool) {
        // Within a second, and within a step or 0.05 % — the reference the core read and the
        // print the model read sit a step apart on a spike.
        let step = deal.tick.unwrap_or(0.0);
        let same = |a: (i64, f64), b: (i64, f64)| {
            (a.0 - b.0).abs() <= verify::POINT_TIME_TOLERANCE_MS
                && (a.1 - b.1).abs() <= (step * 1.01).max(a.1.abs() * 5e-4)
        };
        let until = deal.buy_ms;
        // Past the creation's own point, and before the fill.
        let archived: Vec<(i64, f64)> = verify::archived_replacements(archived)
            .into_iter()
            .skip(1)
            .filter(|&(t, _)| t < until)
            .collect();
        let model: Vec<(i64, f64)> = model
            .iter()
            .skip(1)
            .copied()
            .filter(|&(t, _)| t < until)
            .collect();
        let matched = archived
            .iter()
            .filter(|&&a| model.iter().any(|&m| same(a, m)))
            .count();
        let extra = model
            .iter()
            .filter(|&&m| !archived.iter().any(|&a| same(a, m)))
            .count();
        for &(t, p) in &archived {
            if let Some(&(_, mp)) = model
                .iter()
                .filter(|&&(mt, _)| (mt - t).abs() <= verify::POINT_TIME_TOLERANCE_MS)
                .min_by(|a, b| (a.1 - p).abs().total_cmp(&(b.1 - p).abs()))
            {
                self.timed += 1;
                self.level_errors[usize::from(!moved)].push((mp - p) / p * 100.0);
            }
        }
        self.deals += 1;
        self.whole += usize::from(matched == archived.len() && extra == 0);
        self.archived += archived.len();
        self.matched += matched;
        self.extra += extra;
    }
}

/// The shifts of `MShotPrice` the two entry methods are compared on, per cent points.
const PRICE_SHIFTS: [f64; 5] = [-0.3, -0.15, 0.0, 0.15, 0.3];

/// The two ways to replay a MoonShot variant ([`EntryMethod`]), side by side for one shift of
/// `MShotPrice`.
#[derive(Default)]
struct MethodTally {
    deals: usize,
    /// Fills per method: model, shift.
    filled: [usize; 2],
    /// Deals the two agree on — both unfilled, or both filled within 0.05 %.
    agree: usize,
    /// Fill price against the fact's buy, per cent, summed over the filled, per method.
    dev_sum: [f64; 2],
    /// Fills landing on the fact's own buy (within 0.05 %), per method — the shift of 0 checks it.
    on_fact: [usize; 2],
}

impl MethodTally {
    fn add(&mut self, deal: &Deal, fills: [Option<Fill>; 2]) {
        let same = |a: Option<Fill>, b: Option<Fill>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => (a.price - b.price).abs() <= a.price.abs() * 5e-4,
            _ => false,
        };
        self.deals += 1;
        self.agree += usize::from(same(fills[0], fills[1]));
        for (i, fill) in fills.iter().enumerate() {
            if let Some(fill) = fill {
                self.filled[i] += 1;
                let dev = (fill.price - deal.buy_price) / deal.buy_price * 100.0;
                self.dev_sum[i] += if deal.is_long() { dev } else { -dev };
                self.on_fact[i] += usize::from(dev.abs() <= 0.05);
            }
        }
    }
}

/// One MoonShot fill as the cross-core check reads it.
struct CrossRow {
    venue: String,
    coin: String,
    short: bool,
    core: u64,
    buy_ms: i64,
    /// Where the order stood at the spike (`MshotEntry::fact_anchor`).
    level: f64,
    /// The far bound it stood at, per cent.
    far: f64,
    /// The spike's extreme on the order's side from the fact's last move to 2 s past the buy.
    extreme: f64,
    /// The corridor model's own fill of this trade under its own parameters, off the buy, per
    /// cent — what the model gets wrong on the same spike; `None` when it never filled.
    model_err: Option<f64>,
}

impl CrossRow {
    fn reference(&self) -> f64 {
        if self.short {
            self.level / (1.0 + self.far / 100.0)
        } else {
            self.level / (1.0 - self.far / 100.0)
        }
    }
}

/// Another core's MoonShot fill, to be predicted from this trade's tape: its entry parameters
/// with its corridor as it stood at its own fill (the modifiers folded in), and where it filled.
struct Partner {
    core: u64,
    venue: String,
    coin: String,
    short: bool,
    buy_ms: i64,
    buy_price: f64,
    params: MshotParams,
}

/// The two methods' predictions of partners' real fills.
#[derive(Default)]
struct PartnerTally {
    pairs: usize,
    /// Fills per method: model, shift.
    filled: [usize; 2],
    /// |fill − partner's buy| per cent, per method.
    errors: [Vec<f64>; 2],
}

/// Every MoonShot trade's partner record, where its core's venue and its strategy are known.
fn partners_of(
    deals: &[Deal],
    venues: &HashMap<u64, String>,
    keys: &[String],
    defaults: &HashMap<String, f64>,
) -> Vec<Partner> {
    deals
        .iter()
        .filter(|d| entry_model_for(&d.kind))
        .filter_map(|d| {
            let venue = venues.get(&d.core_uid)?.clone();
            let values = strategy_values_at(d.strategy_id, Some(d.core_uid), d.buy_ms, keys)?;
            let own = mshot_params(
                &StrategyValues {
                    values: &values,
                    defaults,
                },
                ModelSettings::default(),
            );
            let (near, far) = own.bounds_pct(&d.deltas);
            Some(Partner {
                core: d.core_uid,
                venue,
                coin: coin_match_key(&d.coin),
                short: d.is_short,
                buy_ms: d.buy_ms,
                buy_price: d.buy_price,
                params: MshotParams {
                    price_pct: far,
                    price_min_pct: near,
                    modifiers: Default::default(),
                    ..own
                },
            })
        })
        .collect()
}

/// Each core's replace round trip off its archived Entry lines
/// (`calibrate::replace_round_trip_samples`, the median per core).
fn core_round_trips(deals: &[Deal]) -> HashMap<u64, f64> {
    let mut samples: HashMap<u64, Vec<i64>> = HashMap::new();
    for deal in deals.iter().filter(|d| entry_model_for(&d.kind)) {
        let (Some(points), _, _) = archived_lines(deal) else {
            continue;
        };
        samples
            .entry(deal.core_uid)
            .or_default()
            .extend(calibrate::replace_round_trip_samples(&points));
    }
    samples
        .into_iter()
        .filter_map(|(core, mut v)| calibrate::median_step_lag(&mut v).map(|rt| (core, rt)))
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
    // report, so a coin is tried under each market of its core's exchange that stores it. Asked
    // of the cache's own worker through the same handle `query_held` reads the prints through
    // (`handle`, which honours `persist_trades`), so the list covers whatever tables the file
    // keeps them in (legacy `spans` and packed `packs`) and opens nothing the read would not.
    let pairs: Vec<(String, String)> = match crate::market::trade_replay::trade_cache::handle()
        .and_then(|cache| cache.inventory())
    {
        Some(Ok(inventory)) => inventory.keys,
        other => {
            eprintln!("trades.sqlite gave no market list ({other:?}); nothing to replay");
            return;
        }
    };
    eprintln!("markets with a held tape: {}", pairs.len());
    let margin_ms = crate::market::trade_replay::margin_ms();
    let venue_of_core = core_venues();
    eprintln!("core venues (from the identity lines of the logs): {venue_of_core:?}");

    let keys = param_keys();
    // `MOON_TICKS_DEFAULTS=selldelay=0,mshotsellatlastprice=1`: the strategy-field defaults the
    // app reads off the live schema (`strategy_field_defaults`), which a data root does not keep.
    let defaults: HashMap<String, f64> = std::env::var("MOON_TICKS_DEFAULTS")
        .map(|spec| {
            spec.split(',')
                .filter_map(|pair| pair.split_once('='))
                .filter_map(|(k, v)| Some((k.trim().to_ascii_lowercase(), v.trim().parse().ok()?)))
                .collect()
        })
        .unwrap_or_default();
    eprintln!("strategy-field defaults: {defaults:?}");
    let core_lags = core_step_lags(&read.deals, &keys, &defaults);
    // `MOON_TICKS_LATENCY_BASE_MS=<ms>`: each core's latency is that plus its own archived
    // replace round trip, instead of one number for every core.
    let round_trips = core_round_trips(&read.deals);
    let latency_base = std::env::var("MOON_TICKS_LATENCY_BASE_MS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok());
    eprintln!("replace round trip per core: {round_trips:?} · base {latency_base:?}");
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
    // Each core's d1m and d5m errors at the stamp — a core whose `DeltasByTrades` is off reads
    // one point per tick, not every print, and shows as its own error level.
    let mut per_core_short: HashMap<String, Vec<ShortErrors>> = HashMap::new();
    let mut no_track = 0usize;
    eprintln!("PriceDown step lag per core: {core_lags:?}");
    let (mut entry_hits, mut entry_n, mut exit_hits, mut exit_n, mut with_tape) = (0, 0, 0, 0, 0);
    let mut kinds_seen: HashMap<String, usize> = HashMap::new();
    let mut unfit: HashMap<String, usize> = HashMap::new();
    // Deals whose core no identity line named: skipped, and counted, so an empty `logs/` folder
    // reads as that in the summary rather than as a scope without tape.
    let mut no_venue = 0usize;
    let (mut fit_n, mut own_n, mut own_close) = (0usize, 0usize, 0usize);
    // Long positions whose tape has a hole, and those the trade's own settings leave in it.
    let (mut holes, mut own_in_gap) = (0usize, 0usize);
    // MoonShot entries replayed from the order's creation (`mshot`, `Deal::entry_placed`), and
    // how many of those the model reproduced.
    let (mut created_n, mut created_hits, mut stamped) = (0usize, 0usize, 0usize);
    // MoonShot trades whose saved corridor (`Deal::corridor`) the model's band matches in width.
    let (mut corridor_n, mut corridor_hits) = (0usize, 0usize);
    // The order's path from its creation against the archived entry line.
    let mut path = PathTally::default();
    // The three entry methods per shift of `MShotPrice`, and the cross-core rows.
    let mut methods: Vec<MethodTally> = PRICE_SHIFTS
        .iter()
        .map(|_| MethodTally::default())
        .collect();
    let mut cross: Vec<CrossRow> = Vec::new();
    let partners = partners_of(&read.deals, &venue_of_core, &keys, &defaults);
    let mut partner_tally = PartnerTally::default();
    let (mut own_sum, mut fact_sum) = (0.0f64, 0.0f64);
    let search_kind = std::env::var("MOON_TICKS_SEARCH").ok();
    let mut searched: Vec<search::PreparedDeal> = Vec::new();
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
        // `MOON_TICKS_LATENCY_MS=<ms>` replays the entry with another replacement latency.
        let latency = std::env::var("MOON_TICKS_LATENCY_MS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_LATENCY_MS);
        let core_latency = match (latency_base, round_trips.get(&deal.core_uid)) {
            (Some(base), Some(rt)) => base + rt,
            _ => latency,
        };
        let entry = if entry_model_for(&deal.kind) {
            EntryParams::MoonShot(mshot_params(
                &sv,
                ModelSettings {
                    latency_ms: core_latency,
                    ..ModelSettings::default()
                },
            ))
        } else {
            EntryParams::Fact
        };
        let mut exit = exit_params(&sv, ModelSettings::default());
        if std::env::var_os("MOON_TICKS_LATENCY_EXIT").is_some() {
            exit.model.latency_ms = core_latency;
        }
        deal.step_lag_ms = core_lags.get(&deal.core_uid).copied().unwrap_or(0.0);
        if let (Some(cache), Some((exchange, market))) = (klines.as_ref(), address.as_ref()) {
            let btc = btc_of_exchange.get(exchange).map(String::as_str);
            let history = deltas::track_for(cache, exchange, market, btc, &deal, &ticks, &covered);
            deal.bars = history.bars;
            let track = history.track;
            match &track {
                Some(track) => {
                    tracks.push(track.clone());
                    let error = |field: deltas::DeltaField| track.stamp().error[field.index()];
                    per_core_short
                        .entry(deal.core_name.clone())
                        .or_default()
                        .push((
                            error(deltas::DeltaField::D1m),
                            error(deltas::DeltaField::D5m),
                        ));
                }
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
            &covered,
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
        // The two ways to replay a variant, on shifts of the trade's own `MShotPrice` — through
        // the entry model as the search calls it. At no shift the model is the fact itself
        // (`simulate`); the shift is replayed anyway, as the check of its anchor.
        if let EntryParams::MoonShot(own) = &entry {
            for (shift, tally) in PRICE_SHIFTS.iter().zip(methods.iter_mut()) {
                let variant = |method| MshotParams {
                    price_pct: (own.price_pct + shift).max(own.price_min_pct),
                    model: ModelSettings {
                        entry_method: method,
                        ..own.model
                    },
                    ..own.clone()
                };
                let (model, shifted_params) =
                    (variant(EntryMethod::Model), variant(EntryMethod::Shift));
                let full = if *shift == 0.0 {
                    Some(Fill {
                        t_ms: deal.buy_ms,
                        price: deal.buy_price,
                    })
                } else {
                    MshotEntry::new(&model).fill(&deal, &ticks, entry_line.as_deref())
                };
                let shifted =
                    MshotEntry::new(&shifted_params).fill(&deal, &ticks, entry_line.as_deref());
                if *shift == 0.0 && std::env::var_os("MOON_TICKS_METHOD_DEBUG").is_some() {
                    let on_fact = |f: Option<Fill>| {
                        f.is_some_and(|f| (f.price - deal.buy_price).abs() <= deal.buy_price * 5e-4)
                    };
                    if !on_fact(shifted) {
                        let (since, level) = MshotEntry::fact_anchor(&deal, entry_line.as_deref());
                        let near: Vec<String> = ticks
                            .iter()
                            .filter(|t| {
                                let tt = t.time_ms as i64;
                                (deal.buy_ms - 300..=deal.buy_ms + 300).contains(&tt)
                            })
                            .take(8)
                            .map(|t| format!("{:+}:{}", t.time_ms as i64 - deal.buy_ms, t.price))
                            .collect();
                        eprintln!(
                            "    method miss {} {} buy {} anchor {:+}ms {} shifted {:?} prints {:?}",
                            deal.coin,
                            deal.core_name,
                            deal.buy_price,
                            since - deal.buy_ms,
                            level,
                            shifted.map(|f| (f.t_ms - deal.buy_ms, f.price)),
                            near,
                        );
                    }
                }
                tally.add(&deal, [full, shifted]);
            }
            // Other cores' fills on the same spike, predicted from this trade's tape both ways.
            if let Some((venue, _)) = address.as_ref() {
                for p in partners.iter().filter(|p| {
                    p.core != deal.core_uid
                        && &p.venue == venue
                        && p.coin == coin_key
                        && p.short == deal.is_short
                        && (p.buy_ms - deal.buy_ms).abs() <= 3_000
                }) {
                    let shift = MshotParams {
                        model: ModelSettings {
                            entry_method: EntryMethod::Shift,
                            ..p.params.model
                        },
                        ..p.params.clone()
                    };
                    let predictions = [
                        MshotEntry::new(&p.params).fill(&deal, &ticks, entry_line.as_deref()),
                        MshotEntry::new(&shift).fill(&deal, &ticks, entry_line.as_deref()),
                    ];
                    partner_tally.pairs += 1;
                    for (i, fill) in predictions.iter().enumerate() {
                        if let Some(fill) = fill {
                            partner_tally.filled[i] += 1;
                            partner_tally.errors[i]
                                .push(((fill.price - p.buy_price) / p.buy_price * 100.0).abs());
                        }
                    }
                }
            }
            let (since, level) = MshotEntry::fact_anchor(&deal, entry_line.as_deref());
            let (_, far) = own.bounds_pct(&deal.deltas_at(deal.buy_ms));
            let side = ticks
                .iter()
                .filter(|t| {
                    let tt = t.time_ms as i64;
                    tt >= since && tt <= deal.buy_ms + super::super::mshot::SHIFT_WINDOW_MS
                })
                .map(|t| f64::from(t.price));
            let extreme = if deal.is_long() {
                side.reduce(f64::min)
            } else {
                side.reduce(f64::max)
            };
            if let (Some(extreme), Some((venue, _))) = (extreme, address.as_ref()) {
                cross.push(CrossRow {
                    venue: venue.clone(),
                    coin: coin_key.clone(),
                    short: deal.is_short,
                    core: deal.core_uid,
                    buy_ms: deal.buy_ms,
                    level,
                    far,
                    extreme,
                    model_err: MshotEntry::new(own)
                        .fill(&deal, &ticks, entry_line.as_deref())
                        .map(|f| ((f.price - deal.buy_price) / deal.buy_price * 100.0).abs()),
                });
            }
        }
        // The order's path from its creation, where the model replays the whole of it.
        if let (EntryParams::MoonShot(params), Some(line)) = (&entry, entry_line.as_deref()) {
            let from_creation = deal.entry_placed.is_some()
                && deal
                    .order_open_ms()
                    .is_some_and(|created| (ticks[0].time_ms as i64) <= created);
            if from_creation {
                let (_, moves) =
                    super::super::mshot::MshotEntry::new(params).trace(&deal, &ticks, Some(line));
                let moved = params.modifiers.near_addition(&deal.deltas) != 0.0;
                path.add(&deal, &moves, line, moved);
                if std::env::var_os("MOON_TICKS_PATH_DEBUG").is_some() {
                    let created = deal.order_open_ms().unwrap_or(deal.buy_ms);
                    let fmt = |pts: &[(i64, f64)]| -> String {
                        pts.iter()
                            .take(14)
                            .map(|(t, p)| format!("{:+}ms {:.8}", t - created, p))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    eprintln!(
                        "    path {} {} buy {:+}ms fast {} rw {} rd {} near {:.3} far {:.3}\n      model  : {}\n      archive: {}",
                        deal.coin,
                        deal.core_name,
                        deal.buy_ms - created,
                        params.fast_algo,
                        params.raise_wait_s,
                        params.replace_delay_s,
                        params.bounds_pct(&deal.deltas_at(created)).0,
                        params.bounds_pct(&deal.deltas_at(created)).1,
                        fmt(&moves),
                        fmt(&verify::archived_replacements(line)),
                    );
                }
            }
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
                    &exit,
                );
            }
            eprintln!(
                "    held exit {:?} at {:+}ms of close · stop {:.3}% · model pts {}",
                held.exit.kind,
                held.exit.t_ms - deal.close_ms,
                super::super::exit::stops::stop_pct(&exit, &deal, deal.buy_ms),
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
            // The formula against the core's own number, for the same trade. The depth is the one
            // the stated take implies (`record::placed_hook_depth`), so the two agree wherever
            // the comment states a take; a gap left is a row without one.
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
        holes += usize::from(deal.gap.is_some());
        own_in_gap += usize::from(own.exit.is_some_and(|e| e.kind == ExitKind::InGap));
        // `MOON_TICKS_VARIANT="SellPrice=1.6,PriceDownTimer=3"` replays the deal under those values
        // laid over its own, the way a variant column does, and prints the line it walked.
        if let Ok(spec) = std::env::var("MOON_TICKS_VARIANT") {
            let mut laid = values.clone();
            for pair in spec.split(',') {
                if let Some((k, v)) = pair.split_once('=') {
                    laid.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
            let lsv = StrategyValues {
                values: &laid,
                defaults: &defaults,
            };
            let v_entry = if entry_model_for(&deal.kind) {
                EntryParams::MoonShot(mshot_params(&lsv, ModelSettings::default()))
            } else {
                EntryParams::Fact
            };
            let v_exit = exit_params(&lsv, ModelSettings::default());
            let out = simulate(&deal, &ticks, &v_entry, &v_exit, entry_line.as_deref());
            eprintln!(
                "    variant fill {:?} · exit {:?} · {:?}",
                out.fill,
                out.exit,
                round3(out.profit_pct)
            );
            if let Some(fill) = out.fill {
                let w = ExitModel::new(&v_exit).walk(&deal, &ticks, fill);
                let pts: Vec<String> = w
                    .points
                    .iter()
                    .map(|p| format!("+{}ms {:.6}", p.t_ms - fill.t_ms, p.price))
                    .collect();
                eprintln!("    variant line: {}", pts.join(", "));
            }
        }
        let fact = profit_pct(&deal, deal.buy_price, deal.sell_price);
        eprintln!(
            "    fit {fit} · own {:?} {:?} at {:?} · fact {:?}",
            own.exit.map(|e| e.kind),
            round3(own.profit_pct),
            own.exit.map(|e| e.t_ms - deal.close_ms),
            round3(fact),
        );
        // `MOON_TICKS_SEARCH=<kind>`: the fit deals of that kind go to a search after the loop.
        if fit && search_kind.as_deref() == Some(deal.kind.as_str()) {
            searched.push(search::prepared(
                &deal,
                &ticks,
                entry_line.as_deref(),
                &values,
            ));
        }
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
    if let Some(kind) = &search_kind {
        search::run(searched, kind, &defaults);
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
    eprintln!(
        "entry path from the creation: {} deals, whole path {} · archived moves matched {}/{} · model moves the archive lacks {}",
        path.deals, path.whole, path.matched, path.archived, path.extra
    );
    eprintln!(
        "entry path timing: {}/{} archived moves have a model move within the second",
        path.timed, path.archived
    );
    for (name, errors) in ["with MShotAdd*", "without"].iter().zip(&path.level_errors) {
        let mut abs: Vec<f64> = errors.iter().map(|e| e.abs()).collect();
        abs.sort_by(f64::total_cmp);
        let q = |f: f64| {
            abs.get(((abs.len() as f64 - 1.0) * f) as usize)
                .map(|v| (v * 1000.0).round() / 1000.0)
        };
        eprintln!(
            "entry path level, {name}: {} moves · |err| p25 {:?} p50 {:?} p75 {:?} · model above {}",
            errors.len(),
            q(0.25),
            q(0.5),
            q(0.75),
            errors.iter().filter(|e| **e > 0.0).count()
        );
    }
    for (shift, t) in PRICE_SHIFTS.iter().zip(&methods) {
        let mean = |i: usize| t.dev_sum[i] / t.filled[i].max(1) as f64;
        eprintln!(
            "entry methods, MShotPrice {shift:+.2} pp: {} deals · filled model {} shift {} · agree {} · mean fill vs fact (+ deeper) model {:.3} shift {:.3} % · on the fact's buy model {} shift {}",
            t.deals,
            t.filled[0],
            t.filled[1],
            t.agree,
            -mean(0),
            -mean(1),
            t.on_fact[0],
            t.on_fact[1],
        );
    }
    // Different cores on the same spike: one core's level at the spike, shifted by the other's
    // far bound, against where the other's order really stood — and whether the first core's
    // spike reached the predicted level (the other's order did fill).
    let (mut pairs, mut reached, mut ref_err, mut level_err) = (0usize, 0usize, vec![], vec![]);
    let (mut model_err, mut model_unfilled) = (vec![], 0usize);
    for a in &cross {
        for b in cross.iter().filter(|b| {
            b.core != a.core
                && b.venue == a.venue
                && b.coin == a.coin
                && b.short == a.short
                && (b.buy_ms - a.buy_ms).abs() <= 3_000
                && (b.far - a.far).abs() > 1e-9
        }) {
            pairs += 1;
            let predicted = if b.short {
                a.reference() * (1.0 + b.far / 100.0)
            } else {
                a.reference() * (1.0 - b.far / 100.0)
            };
            reached += usize::from(reaches(a.extreme, predicted, !b.short));
            ref_err.push(((a.reference() - b.reference()) / b.reference() * 100.0).abs());
            level_err.push(((predicted - b.level) / b.level * 100.0).abs());
            match b.model_err {
                Some(e) => model_err.push(e),
                None => model_unfilled += 1,
            }
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let within = |v: &[f64], tol: f64| v.iter().filter(|e| **e <= tol).count();
    eprintln!(
        "cross-core same spike: {pairs} pairs · the first core's spike reached the second's predicted level {reached} · reference |err| median {:?} % · shift from the first core: level |err| median {:?} %, within 0.1 % {} · the model on the second's own tape: fill |err| median {:?} %, within 0.1 % {}, unfilled {model_unfilled}",
        median(&mut ref_err),
        median(&mut level_err),
        within(&level_err, 0.1),
        median(&mut model_err),
        within(&model_err, 0.1),
    );
    for (i, name) in ["model", "shift"].iter().enumerate() {
        let errors = &mut partner_tally.errors[i];
        eprintln!(
            "cross-core fill of the other core, {name}: {} pairs · filled {} · |err| median {:?} % · within 0.05 % {} · within 0.1 % {} · within 0.3 % {}",
            partner_tally.pairs,
            partner_tally.filled[i],
            median(errors),
            within(errors, 0.05),
            within(errors, 0.1),
            within(errors, 0.3),
        );
    }
    print_delta_quality(&tracks, no_track);
    let mut cores: Vec<_> = per_core_short.into_iter().collect();
    cores.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (core, errors) in cores {
        // |error| median, how many within 0.1 pp, how many where the history saw the wider move.
        let median = |pick: fn(&ShortErrors) -> Option<f64>| {
            let signed: Vec<f64> = errors.iter().filter_map(pick).collect();
            let mut v: Vec<f64> = signed.iter().map(|e| e.abs()).collect();
            v.sort_by(f64::total_cmp);
            let within = v.iter().filter(|e| **e <= 0.1).count();
            let wider = signed.iter().filter(|e| **e > 0.1).count();
            let narrower = signed.iter().filter(|e| **e < -0.1).count();
            (
                v.get(v.len() / 2).copied(),
                within,
                v.len(),
                wider,
                narrower,
            )
        };
        let (d1m, d1m_in, d1m_n, d1m_w, d1m_nr) = median(|e| e.0);
        let (d5m, d5m_in, d5m_n, d5m_w, d5m_nr) = median(|e| e.1);
        eprintln!(
            "  core {core:12} d1m median {:?} within 0.1 {d1m_in}/{d1m_n} wider {d1m_w} narrower {d1m_nr} · d5m median {:?} within 0.1 {d5m_in}/{d5m_n} wider {d5m_w} narrower {d5m_nr}",
            d1m.map(|v| (v * 1000.0).round() / 1000.0),
            d5m.map(|v| (v * 1000.0).round() / 1000.0),
        );
    }
    let mut unfit: Vec<(String, usize)> = unfit.into_iter().collect();
    unfit.sort_by_key(|u| std::cmp::Reverse(u.1));
    eprintln!("fit for the search: {fit_n} of {with_tape} · left out: {unfit:?}");
    eprintln!("long positions with a hole: {holes} · own settings left in it: {own_in_gap}");
    eprintln!(
        "own settings replayed on the fit trades: {own_n} closed, mean {:.3} % against the fact's {:.3} %, within 0.05 pp on {own_close}",
        own_sum / own_n.max(1) as f64,
        fact_sum / own_n.max(1) as f64,
    );
}
