//! Aggregate-only calibration of independent Drops hypotheses on identical held tapes.
//! Process-global cache readers, recovery lease and data-root override cannot be reset.
//! A parent process owns snapshot cleanup after the child releases all static readers.

use super::*;
use crate::db::tuner::ticks::drops::probe::{BEST, Probe, ticker_tape};
use crate::db::tuner::ticks::drops::{
    DropReading, DropsCadence, DropsEntry, DropsParams, DropsTrace, DropsVariant, KIND_DROPS,
    PRICE_TOLERANCE_PCT, SEARCH_BEFORE_MS,
};
use crate::db::tuner::ticks::params::drops_params;

mod fit;
mod heldout;
mod logs;

#[cfg(test)]
mod tests;

/// Own the isolated snapshot in the parent until its reader child has exited.
#[derive(Debug)]
struct ScratchDir(PathBuf, SnapshotMode);

/// Keep aggregate bench output anonymous while retaining the legacy diagnostic receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotMode {
    Diagnostic,
    Inventory,
    Heldout,
}

impl SnapshotMode {
    /// Return only fixed output labels, never data-derived text.
    fn prefix(self) -> &'static str {
        match self {
            Self::Diagnostic => "DIAG",
            Self::Inventory => "INV",
            Self::Heldout => "HO",
        }
    }
}

impl Drop for ScratchDir {
    /// Report cleanup failures rather than silently accumulating multi-gigabyte snapshots.
    fn drop(&mut self) {
        if std::fs::remove_dir_all(&self.0).is_err() {
            eprintln!("{} SNAPSHOT cleanup=0", self.1.prefix());
        } else {
            eprintln!("{} SNAPSHOT cleanup=1", self.1.prefix());
        }
    }
}

/// Create a unique OS-temporary directory with ownership established before any copy.
fn scratch_root() -> std::io::Result<ScratchDir> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("drops-timing-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&path)?;
    Ok(ScratchDir(path, SnapshotMode::Diagnostic))
}

/// Copy companions in one traversal; a changing live source may still yield a torn diagnostic copy.
fn copy_snapshot_tree(
    source: &std::path::Path,
    target: &std::path::Path,
    all: bool,
) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::other("snapshot symlink refused"));
        }
        let name = entry.file_name();
        let text = name.to_string_lossy();
        let all = all || text == "cfg" || text == "logs";
        if kind.is_dir() {
            copy_snapshot_tree(&entry.path(), &target.join(&name), all)?;
        } else if all
            || text.ends_with(".sqlite")
            || text.ends_with(".sqlite-wal")
            || text.ends_with(".sqlite-shm")
            || text.ends_with(".sqlite-journal")
        {
            std::fs::copy(entry.path(), target.join(&name))?;
        }
    }
    Ok(())
}

/// Install only the isolated copy, before any recovery lease or database reader can write.
#[cfg(test)]
fn snapshot_root(
    source: &std::path::Path,
    install: impl FnOnce(PathBuf) -> bool,
) -> std::io::Result<ScratchDir> {
    snapshot_root_named(source, install, SnapshotMode::Diagnostic)
}

/// Share snapshot isolation while keeping the inventory's entire output anonymous and prefixed.
fn snapshot_root_named(
    source: &std::path::Path,
    install: impl FnOnce(PathBuf) -> bool,
    mode: SnapshotMode,
) -> std::io::Result<ScratchDir> {
    let mut target = scratch_root()?;
    target.1 = mode;
    copy_snapshot_tree(source, &target.0, false)?;
    if !install(target.0.clone()) {
        return Err(std::io::Error::other("override unavailable"));
    }
    if mode != SnapshotMode::Diagnostic {
        eprintln!(
            "{} SNAPSHOT isolated=1 sqlite_companions_one_pass=1 source_refresh=0 live_atomic=0",
            mode.prefix()
        );
    } else {
        eprintln!(
            "DIAG snapshot isolated=true sqlite_companions=one_pass source_refresh=false live_atomic=false"
        );
    }
    Ok(target)
}

/// Run readers in a fresh process so Windows releases every static handle before removal.
fn snapshot_bench(name: &str, mode: SnapshotMode, body: impl FnOnce()) {
    if std::env::var_os("MOON_DROPS_CHILD").as_deref() == Some(std::ffi::OsStr::new("1")) {
        let root = std::env::var_os("MOON_DROPS_SNAPSHOT").expect("child snapshot root");
        assert!(
            paths::set_data_dir_override(PathBuf::from(root)),
            "child override unavailable"
        );
        body();
        return;
    }
    let source = std::env::var_os("MOON_TICKS_DATA_DIR").expect("snapshot source root");
    let snapshot =
        snapshot_root_named(&PathBuf::from(source), |_| true, mode).expect("snapshot copy");
    run_snapshot_child(snapshot, name, true);
}

/// Inherit both output streams and delete only the parent-owned snapshot after child exit.
fn run_snapshot_child(snapshot: ScratchDir, name: &str, ignored: bool) {
    // libtest names omit the crate component included by module_path!().
    let name = name
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .unwrap_or(name);
    let path = snapshot.0.clone();
    let mut command = std::process::Command::new(std::env::current_exe().expect("test executable"));
    command.args(["--exact", name]);
    if ignored {
        command.arg("--ignored");
    }
    let status = command
        .args(["--nocapture", "--test-threads=1"])
        .env("MOON_DROPS_CHILD", "1")
        .env("MOON_DROPS_SNAPSHOT", &path)
        .status();
    drop(snapshot);
    assert!(!path.exists(), "snapshot cleanup failed");
    assert!(
        status.expect("child process launch").success(),
        "snapshot child failed"
    );
}

/// A loaded deal keeps strategy defaults separate while sharing one immutable tape.
struct Replay {
    deal: Deal,
    ticks: Vec<Tick>,
    ui_ticks: Option<Vec<Tick>>,
    extended_covered: bool,
    current: DropsParams,
    legacy: Option<DropsParams>,
    values: HashMap<String, String>,
    dump_ms: Option<i64>,
    previous_ms: Option<i64>,
    covered: Coverage,
    ui_spans: Option<Coverage>,
}

/// Missing detections count as misses; signed timing medians use observed detections only.
#[derive(Default)]
struct Metrics {
    n: usize,
    detect: usize,
    price: usize,
    joint: usize,
    execution: usize,
    joint_limit: usize,
    placement_dt: Vec<f64>,
    xpd: usize,
    dt: Vec<f64>,
    errors: Vec<f64>,
    placement: usize,
    placement_joint: usize,
    candidate_refused: usize,
    candidate_degenerate: usize,
    detector: usize,
}

impl Metrics {
    /// The comment comparison is at factual buy_set, independent of the candidate's detect.
    fn observe_tape(&mut self, row: &Replay, ticks: &[Tick], p: &DropsParams, v: DropsVariant) {
        let mut deal = row.deal.clone();
        deal.tick = infer_tick(ticks);
        let trace = DropsEntry(p).trace_variant(&deal, ticks, v);
        self.observe_trace(
            row,
            trace,
            p.drop_at_variant(ticks, row.deal.buy_set_ms.expect("loaded buy_set"), v),
        );
    }

    /// Placement scores distinguish order delays from detector agreement.
    fn observe_trace(&mut self, row: &Replay, trace: DropsTrace, reading: Option<DropReading>) {
        self.n += 1;
        self.candidate_refused += usize::from(trace.refused);
        self.candidate_degenerate += usize::from(trace.degenerate);
        if trace.refused || trace.degenerate {
            return;
        }
        let stamp = row.deal.buy_set_ms.expect("loaded buy_set");
        let dt = trace.order_ms.map(|t| (t - stamp) as f64 / 1000.0);
        let detect = dt.is_some_and(|d| d.abs() <= 2.0);
        let price = trace
            .limit
            .is_some_and(|p| (p / row.deal.buy_price - 1.0).abs() * 100.0 <= PRICE_TOLERANCE_PCT);
        self.detect += usize::from(detect);
        self.detector += usize::from(trace.detect_ms.is_some_and(|t| (t - stamp).abs() <= 2000));
        self.price += usize::from(price);
        let execution = trace.fill.is_some_and(|f| {
            (f.price / row.deal.buy_price - 1.0).abs() * 100.0 <= PRICE_TOLERANCE_PCT
        });
        self.execution += usize::from(execution);
        let placed = trace.order_ms.is_some_and(|t| (t - stamp).abs() <= 2000);
        self.placement += usize::from(placed);
        self.placement_joint += usize::from(placed && execution);
        self.joint += usize::from(detect && execution);
        self.joint_limit += usize::from(detect && price && trace.fill.is_some());
        self.placement_dt
            .extend(trace.order_ms.map(|t| (t - stamp) as f64 / 1000.0));
        self.dt
            .extend(trace.detect_ms.map(|t| (t - stamp) as f64 / 1000.0));
        if let (Some(r), Some(comment)) = (reading, row.deal.drops_detect) {
            let error = (r.drop_pct - comment.x_price_delta_pct).abs();
            self.errors.push(error);
            self.xpd += usize::from(error <= 0.3);
        }
    }

    /// Emit one ASCII row without identifiers, market prices or strategy names.
    fn print(&self, name: &str, refused: usize, degenerate: usize, missing: usize) {
        eprintln!(
            "CAND {name} n={} detect2s={:.2} price01={:.2} joint={:.2} xpd03={:.2} median_dt_s={} median_abs_xpd_pp={} excluded-refused={} excluded-degenerate={} excluded-missing={} xpd-observed={} median_placement_dt_s={} execution01={:.2} joint_limit={:.2} placement2s={:.2} placement_joint={:.2} detector2s={:.2}",
            self.n,
            share(self.detect, self.n),
            share(self.price, self.n),
            share(self.joint, self.n),
            share(self.xpd, self.n),
            median(&self.dt),
            median(&self.errors),
            refused,
            degenerate,
            missing,
            self.errors.len(),
            median(&self.placement_dt),
            share(self.execution, self.n),
            share(self.joint_limit, self.n),
            share(self.placement, self.n),
            share(self.placement_joint, self.n),
            share(self.detector, self.n)
        );
        eprintln!(
            "DIAG candidate_{name} refused={} degenerate={} detector2s={:.2} selection=in_sample detect2s_basis=order_ms median_dt_basis=detect_ms",
            self.candidate_refused,
            self.candidate_degenerate,
            share(self.detector, self.n)
        );
    }
}

/// Zero populations cannot become positive evidence.
fn share(hits: usize, n: usize) -> f64 {
    if n == 0 {
        0.0
    } else {
        hits as f64 * 100.0 / n as f64
    }
}

/// Even populations use the midpoint; an absent observation stays explicit.
fn median(values: &[f64]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        "n/a".into()
    } else {
        format!(
            "{:.4}",
            (sorted[(sorted.len() - 1) / 2] + sorted[sorted.len() / 2]) / 2.0
        )
    }
}

/// Enumerate both endpoint formulas and all one-knob neighbours, including all cadences.
fn formulas() -> Vec<(&'static str, DropsVariant)> {
    let old = DropsVariant::default();
    let a = DropsVariant::ASTER;
    vec![
        ("today", old),
        ("aster", a),
        (
            "today_wall",
            DropsVariant {
                wall_slots: true,
                ..old
            },
        ),
        (
            "today_include",
            DropsVariant {
                include_current: true,
                ..old
            },
        ),
        (
            "today_rolling",
            DropsVariant {
                rolling_high: true,
                ..old
            },
        ),
        (
            "today_slot",
            DropsVariant {
                cadence: DropsCadence::Slot,
                ..old
            },
        ),
        (
            "today_print",
            DropsVariant {
                cadence: DropsCadence::Print,
                ..old
            },
        ),
        (
            "today_delay",
            DropsVariant {
                delay_slot: true,
                ..old
            },
        ),
        (
            "aster_relative",
            DropsVariant {
                wall_slots: false,
                ..a
            },
        ),
        (
            "aster_exclude",
            DropsVariant {
                include_current: false,
                ..a
            },
        ),
        (
            "aster_zone",
            DropsVariant {
                rolling_high: false,
                ..a
            },
        ),
        (
            "aster_second",
            DropsVariant {
                cadence: DropsCadence::Second,
                ..a
            },
        ),
        (
            "aster_slot",
            DropsVariant {
                cadence: DropsCadence::Slot,
                ..a
            },
        ),
        (
            "aster_delay",
            DropsVariant {
                delay_slot: true,
                ..a
            },
        ),
    ]
}

/// Resolve omitted fields explicitly; raw strategy values always win over either assumption.
fn defaults(last_ma: f64) -> HashMap<String, f64> {
    [
        ("dropsmaxtime", 60.0),
        ("dropspricema", 2.0),
        ("dropslastpricema", last_ma),
        ("dropspricedelta", 1.0),
        ("dropspriceislow", 0.0),
        ("dropsuselastprice", 0.0),
        ("buyprice", -2.0),
        ("buypricelasttrade", 0.0),
        ("use30secoldask", 0.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Query a single core-venue market, never merge competing market tapes into a synthetic one.
fn tape_for(
    deal: &Deal,
    venue: &str,
    pairs: &[(String, String)],
    spans: &Coverage,
) -> Option<(Vec<Tick>, Coverage)> {
    let selected = tape_pairs_for_deal(deal, venue, pairs);
    tape_for_deal_with(venue, &deal.coin, &selected, spans, held_ticks)
}

/// Preserve C2's empty-market fallback while sharing clipping and venue selection.
fn tape_for_deal_with(
    venue: &str,
    coin: &str,
    pairs: &[(String, String)],
    spans: &Coverage,
    mut read: impl FnMut(&str, &str, &Coverage) -> (Vec<Tick>, Coverage),
) -> Option<(Vec<Tick>, Coverage)> {
    logs::tape_for_coin_with(venue, coin, pairs, spans, |e, m, s| {
        let (ticks, covered) = read(e, m, s);
        if ticks.is_empty() {
            (ticks, Coverage::none())
        } else {
            (ticks, covered)
        }
    })
}

/// Keep the original deal cohort's market admission while log parsing accepts quote-first names.
fn tape_pairs_for_deal(
    deal: &Deal,
    venue: &str,
    pairs: &[(String, String)],
) -> Vec<(String, String)> {
    // Preserve the deal cohort's catalog-only selection before accepting log spellings.
    let coin = coin_match_key(&deal.coin);
    pairs
        .iter()
        .filter(|(e, m)| e == venue && coin_match_key(coin_of_market(m)) == coin)
        .cloned()
        .collect()
}

/// Load each eligible deal once, compare all hypotheses, then vary current MA under the winner.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_drops_calibration() {
    snapshot_bench(
        &format!("{}::real_data_drops_calibration", module_path!()),
        SnapshotMode::Diagnostic,
        || calibration(false),
    );
}

/// Batch two reuses the frozen loader but runs only paired UI-cohort input/timing probes.
#[test]
#[ignore = "needs the frozen snapshot in MOON_TICKS_DATA_DIR"]
fn real_data_drops_timing() {
    snapshot_bench(
        &format!("{}::real_data_drops_timing", module_path!()),
        SnapshotMode::Diagnostic,
        || calibration(true),
    );
}

/// Inventory logged detects on an isolated snapshot without scoring or opening the gate.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_drops_logs_inventory() {
    snapshot_bench(
        &format!("{}::real_data_drops_logs_inventory", module_path!()),
        SnapshotMode::Inventory,
        logs::inventory,
    );
}

/// Fit on earlier UTC dates and report a frozen rule on later dates without changing replay.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_drops_logs_heldout() {
    snapshot_bench(
        &format!("{}::real_data_drops_logs_heldout", module_path!()),
        SnapshotMode::Heldout,
        heldout::bench,
    );
}

/// Keep both batches on precisely the same snapshot loader and eligibility rules.
fn calibration(batch2: bool) {
    let Some(_permit) = crate::db::report_recovery::prepare() else {
        eprintln!("reports replica lease unavailable; GATE: FAIL (no eligible)");
        return;
    };
    let scope = Query {
        from: -1,
        to: crate::db::analytics::ANALYTICS_HORIZON_SECS,
        metric: crate::db::ProfitMetric::Percent,
        ..Default::default()
    };
    let read = match read_deals(&scope) {
        Ok(read) => read,
        Err(_) => {
            eprintln!("deals read failed; GATE: FAIL (no eligible)");
            return;
        }
    };
    let pairs = match crate::market::trade_replay::trade_cache::handle()
        .and_then(|cache| cache.inventory())
    {
        Some(Ok(inventory)) => inventory.keys,
        _ => {
            eprintln!("held tape inventory unavailable; GATE: FAIL (no eligible)");
            return;
        }
    };

    let current_defaults = defaults(2.0);
    let legacy_defaults = defaults(0.0);
    let empty = HashMap::new();
    let venues = core_venues();
    let mut keys = param_keys();
    keys.extend(EXTRA_KEYS.iter().map(|key| (*key).to_string()));
    let provenance = Connection::open_with_flags(
        paths::strategies_db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok();
    let mut previous = HashMap::new();
    let mut ordered: Vec<_> = read.deals.iter().filter(|d| d.kind == KIND_DROPS).collect();
    ordered.sort_by_key(|d| d.buy_set_ms.unwrap_or(d.buy_ms));
    let mut previous_by_deal = HashMap::new();
    for d in ordered {
        let stamp = d.buy_set_ms.unwrap_or(d.buy_ms);
        let key = (
            d.core_uid,
            d.strategy_id,
            coin_match_key(&d.coin).to_string(),
        );
        // Strictly earlier factual orders only; equal timestamps never suppress each other.
        let prior = previous
            .get(&key)
            .copied()
            .and_then(|(last, before)| if last < stamp { Some(last) } else { before });
        previous_by_deal.insert((d.core_uid, d.strategy_id, d.buy_ms, d.coin.clone()), prior);
        previous.insert(key, (stamp, prior));
    }
    let layout = crate::config::layout::WindowLayout::load(crate::config::ProfileAge::Established);
    let min_tail_s = layout
        .analytics_ticks
        .as_ref()
        .and_then(|t| t.min_tail_s)
        .unwrap_or(tail::DEFAULT_MIN_TAIL_S);
    let margin = crate::market::trade_replay::margin_ms();
    let tail_ms = tail::record_tail_ms(min_tail_s, margin);
    let mut rows = Vec::new();
    let mut missing = 0;
    let mut seen: usize = 0;
    for mut deal in read.deals {
        if deal.kind != KIND_DROPS || !deal.is_long() || deal.drops_detect.is_none() {
            continue;
        }
        let Some(stamp) = deal.buy_set_ms else {
            continue;
        };
        let Some(values) =
            strategy_values_at(deal.strategy_id, Some(deal.core_uid), deal.buy_ms, &keys)
        else {
            continue;
        };
        let Some(venue) = venues.get(&deal.core_uid) else {
            continue;
        };
        let current = drops_params(&StrategyValues {
            values: &values,
            defaults: &current_defaults,
        });
        let raw = StrategyValues {
            values: &values,
            defaults: &empty,
        };
        let dump_ms = provenance.as_ref().and_then(|conn| {
            conn.query_row(
                "SELECT valid_from FROM strategy_versions WHERE strategy_id=?1 AND core_uid=?2
                 ORDER BY CASE WHEN valid_from<=?3 AND (valid_to IS NULL OR valid_to>?3) THEN 0 ELSE 1 END,
                 CASE WHEN valid_from<=?3 AND (valid_to IS NULL OR valid_to>?3) THEN -valid_from ELSE valid_from END LIMIT 1",
                rusqlite::params![deal.strategy_id, deal.core_uid as i64, deal.buy_ms],
                |r| r.get::<_, i64>(0),
            ).ok()
        });
        let legacy = raw.num("DropsPriceMA", f64::NAN).is_finite().then(|| {
            drops_params(&StrategyValues {
                values: &values,
                defaults: &legacy_defaults,
            })
        });
        let mut history = current.clone();
        history.last_price_ma = history.last_price_ma.max(3);
        let extended = Coverage::one((
            stamp - SEARCH_BEFORE_MS - history.needed_history_ms() - 2000,
            deal.close_ms,
        ));
        let window = model_window(&deal, margin, long_position_ms());
        let ui_spans = window.as_ref().map(|w| tail::focus_with_tail(w, tail_ms));
        let mut spans = extended.clone();
        if let Some(ui) = &ui_spans {
            for &(from, to) in ui.spans() {
                spans.add((from, to));
            }
        }
        seen += 1;
        if seen.is_multiple_of(100) {
            eprintln!("calibration loaded={} missing={missing}", rows.len());
        }
        let Some((mut ticks, covered)) = tape_for(&deal, venue, &pairs, &spans) else {
            missing += 1;
            continue;
        };
        let ui_ticks = window
            .as_ref()
            .zip(ui_spans.as_ref())
            .and_then(|(window, spans)| {
                covered.covers(&required_spans(window)).then(|| {
                    ticks
                        .iter()
                        .filter(|tick| {
                            spans.spans().iter().any(|&(from, to)| {
                                tick.time_ms >= from as f64 && tick.time_ms <= to as f64
                            })
                        })
                        .cloned()
                        .collect()
                })
            });
        ticks.retain(|tick| {
            extended
                .spans()
                .iter()
                .any(|&(from, to)| tick.time_ms >= from as f64 && tick.time_ms <= to as f64)
        });
        deal.tick = infer_tick(&ticks);
        rows.push(Replay {
            previous_ms: previous_by_deal
                .get(&(
                    deal.core_uid,
                    deal.strategy_id,
                    deal.buy_ms,
                    deal.coin.clone(),
                ))
                .copied()
                .flatten(),
            deal,
            ticks,
            ui_ticks,
            extended_covered: covered.covers(&extended),
            current,
            legacy,
            values,
            dump_ms,
            covered,
            ui_spans,
        });
    }
    eprintln!(
        "CALIBRATION loaded={} missing={} gate_authorised=false legacy_omitted_price_ma=excluded",
        rows.len(),
        missing
    );
    for extended in [false, true] {
        if batch2 && extended {
            break;
        }
        // Only UI-held tape can support the gate; no result here changes the gate switch.
        let cohort = if extended {
            "extended_diagnostic"
        } else {
            "ui_gate"
        };
        let mut best: Option<(f64, f64, &'static str, DropsVariant)> = None;
        for profile in ["v752", "legacy", "v752_legacy_subset"] {
            if batch2 && profile != "v752" {
                break;
            }
            let mut pool = Vec::new();
            let (mut refused, mut degenerate, mut absent) = (0, 0, missing);
            for row in &rows {
                if profile != "v752" && row.legacy.is_none() {
                    continue;
                }
                let ticks = if extended {
                    row.extended_covered.then_some(&row.ticks)
                } else {
                    row.ui_ticks.as_ref()
                };
                let Some(ticks) = ticks else {
                    absent += 1;
                    continue;
                };
                let p = if profile == "legacy" {
                    row.legacy.as_ref().expect("paired legacy")
                } else {
                    &row.current
                };
                let mut warmup = p.clone();
                warmup.last_price_ma = warmup.last_price_ma.max(3);
                let check = DropsEntry(&warmup).trace(&row.deal, ticks);
                if check.refused {
                    refused += 1;
                    continue;
                }
                if check.degenerate {
                    degenerate += 1;
                    continue;
                }
                // Every hypothesis and sensitivity uses this fixed, prequalified population.
                pool.push((row, ticks, p));
            }
            if batch2 {
                timing_batch(&pool, refused, degenerate, absent);
                continue;
            }
            for (name, variant) in formulas() {
                let mut metrics = Metrics::default();
                for &(row, ticks, p) in &pool {
                    metrics.observe_tape(row, ticks, p, variant);
                }
                metrics.print(
                    &format!("{name}_{profile}_{cohort}"),
                    refused,
                    degenerate,
                    absent,
                );
                let score = (
                    share(metrics.xpd, metrics.n),
                    share(metrics.joint, metrics.n),
                );
                if profile == "v752"
                    && metrics.n > 0
                    && best.as_ref().is_none_or(|b| score > (b.0, b.1))
                {
                    best = Some((score.0, score.1, name, variant));
                }
            }
            if let Some((_, _, name, variant)) = best.filter(|_| profile == "v752") {
                for ma in [0, 1, 2, 3] {
                    let mut metrics = Metrics::default();
                    for &(row, ticks, p) in &pool {
                        let p = DropsParams {
                            last_price_ma: ma,
                            ..p.clone()
                        };
                        metrics.observe_tape(row, ticks, &p, variant);
                    }
                    metrics.print(
                        &format!("best_{name}_v752_lastma{ma}_{cohort}"),
                        refused,
                        degenerate,
                        absent,
                    );
                }
            }
        }
    }
}

/// Missing and even observations must not inflate medians or shares.
#[test]
fn calibration_summary_handles_empty_and_even_samples() {
    assert_eq!(median(&[]), "n/a");
    assert_eq!(median(&[9.0, 1.0, 5.0, 3.0]), "4.0000");
    assert_eq!(share(0, 0), 0.0);
    assert_eq!(share(1, 4), 25.0);
}

/// Read all provenance and gating fields explicitly, independent of production model keys.
const EXTRA_KEYS: &[&str] = &[
    "NextDetectPenalty",
    "IgnoreFilters",
    "IgnoreDelta",
    "Delta_3h_Min",
    "Delta_3h_Max",
    "Delta_24h_Min",
    "Delta_24h_Max",
    "Delta2_Type",
    "Delta2_Min",
    "Delta2_Max",
    "Delta3_Type",
    "Delta3_Min",
    "Delta3_Max",
];

/// Fixed paired population: unavailable probe inputs count as misses rather than disappearing.
type Pool<'a> = [(&'a Replay, &'a Vec<Tick>, &'a DropsParams)];

/// Linear interpolated quantiles retain resolution without printing individual observations.
fn percentile(values: &[f64], q: f64) -> String {
    if values.is_empty() {
        return "n/a".into();
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let i = q * (v.len() - 1) as f64;
    let lo = i.floor() as usize;
    let hi = i.ceil() as usize;
    format!("{:.4}", v[lo] + (v[hi] - v[lo]) * i.fract())
}

/// Missing observations remain explicit; timing shares use observed values and name the base.
fn distribution(name: &str, values: &[f64], timing: bool) {
    eprintln!(
        "DIAG {name} n={} p10={} p25={} p50={} p75={} p90={}",
        values.len(),
        percentile(values, 0.1),
        percentile(values, 0.25),
        percentile(values, 0.5),
        percentile(values, 0.75),
        percentile(values, 0.9)
    );
    if timing {
        eprintln!(
            "DIAG {name}_shares denominator=observed inside2s={:.2} inside5s={:.2} inside15s={:.2}",
            share(
                values.iter().filter(|v| v.abs() <= 2.0).count(),
                values.len()
            ),
            share(
                values.iter().filter(|v| v.abs() <= 5.0).count(),
                values.len()
            ),
            share(
                values.iter().filter(|v| v.abs() <= 15.0).count(),
                values.len()
            )
        );
    }
}

/// Use precisely the numeric parser that decides whether a dump value overrides a default.
fn raw_number(row: &Replay, key: &str) -> Option<f64> {
    let empty = HashMap::new();
    let v = StrategyValues {
        values: &row.values,
        defaults: &empty,
    }
    .num(key, f64::NAN);
    v.is_finite().then_some(v)
}

/// A low/filter window is computable only inside both retained and UI-requested coverage.
fn ui_covers(row: &Replay, from: i64, to: i64) -> bool {
    let span = Coverage::one((from, to));
    row.covered.covers(&span) && row.ui_spans.as_ref().is_some_and(|s| s.covers(&span))
}

/// Separate omitted/malformed fields and future dumps before interpreting fit quality.
fn provenance(pool: &Pool<'_>, cohort: &str) {
    for key in [
        "DropsMaxTime",
        "DropsPriceMA",
        "DropsLastPriceMA",
        "DropsPriceDelta",
    ] {
        let dump = pool
            .iter()
            .filter(|(r, _, _)| raw_number(r, key).is_some())
            .count();
        eprintln!(
            "DIAG params_{cohort}_{key} n={} dump={dump} default={}",
            pool.len(),
            pool.len() - dump
        );
    }
    eprintln!(
        "DIAG provenance_{cohort} n={} dump_after_deal={} dump_after_buy_set={} unknown={}",
        pool.len(),
        pool.iter()
            .filter(|(r, _, _)| r.dump_ms.is_some_and(|t| t > r.deal.buy_ms))
            .count(),
        pool.iter()
            .filter(|(r, _, _)| r
                .dump_ms
                .is_some_and(|t| t > r.deal.buy_set_ms.expect("buy_set")))
            .count(),
        pool.iter().filter(|(r, _, _)| r.dump_ms.is_none()).count()
    );
}

/// Compare core timing, actual threshold transitions, and comment scale on one fixed cohort.
fn diagnostics(pool: &Pool<'_>) {
    let (mut detect, mut crossing, mut closest, mut ratios, mut differences) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut buckets: std::collections::BTreeMap<String, (usize, Vec<f64>, Vec<f64>)> =
        std::collections::BTreeMap::new();
    let (mut censored, mut zero, mut edge, mut unknown_transition) = (0, 0, 0, 0);
    for &(row, ticks, p) in pool {
        let stamp = row.deal.buy_set_ms.expect("buy_set");
        let from = stamp - SEARCH_BEFORE_MS;
        let trace = DropsEntry(p).trace_variant(&row.deal, ticks, BEST);
        detect.extend(trace.detect_ms.map(|t| (stamp - t) as f64 / 1000.0));
        let mut prior = p
            .drop_at_variant(ticks, from - 1000, BEST)
            .map(|r| r.drop_pct >= p.price_delta_pct);
        for t in (from..=row.deal.buy_ms + 10000).step_by(1000) {
            let now = p
                .drop_at_variant(ticks, t, BEST)
                .map(|r| r.drop_pct >= p.price_delta_pct);
            if t == from && now == Some(true) {
                censored += 1;
            }
            if t == from && prior.is_none() {
                unknown_transition += 1;
            }
            if prior == Some(false) && now == Some(true) {
                crossing.push((stamp - t) as f64 / 1000.0);
                break;
            }
            prior = now;
        }
        let comment = row
            .deal
            .drops_detect
            .expect("Drops comment")
            .x_price_delta_pct;
        if let Some(r) = p.drop_at_variant(ticks, stamp, BEST) {
            let difference = comment - r.drop_pct;
            let ratio = (r.drop_pct.abs() > 1e-12).then_some(comment / r.drop_pct);
            differences.push(difference);
            ratios.extend(ratio);
            zero += usize::from(ratio.is_none());
            for name in [
                format!("max_time_{}s", p.max_time_s),
                format!("price_ma_{}s", p.price_ma_s),
            ] {
                let bucket = buckets.entry(name).or_default();
                bucket.0 += 1;
                bucket.1.extend(ratio);
                bucket.2.push(difference);
            }
        }
        // Earliest time wins exact ties; the solver does not use a fitted detector threshold.
        let solved = (stamp - 60000..=stamp + 5000)
            .step_by(1000)
            .filter_map(|t| {
                p.drop_at_variant(ticks, t, BEST)
                    .map(|r| (t, (r.drop_pct - comment).abs()))
            })
            .fold(None, |best: Option<(i64, f64)>, item| match best {
                Some(old) if old.1 <= item.1 => Some(old),
                _ => Some(item),
            });
        if let Some((t, _)) = solved {
            closest.push((t - stamp) as f64 / 1000.0);
            edge += usize::from(t == stamp - 60000 || t == stamp + 5000);
        }
    }
    eprintln!(
        "DIAG timing population={} first_pass_missing={} transition_missing={} left_censored={} transition_history_unknown={unknown_transition}",
        pool.len(),
        pool.len() - detect.len(),
        pool.len() - crossing.len(),
        censored
    );
    distribution("core_minus_model_detect_s", &detect, true);
    distribution("core_minus_first_transition_s", &crossing, true);
    distribution("comment_over_model_drop", &ratios, false);
    distribution("comment_minus_model_drop_pp", &differences, false);
    eprintln!(
        "DIAG xpd ratio_zero_denominator={zero} solver_grid_ms=1000 solver_tie=earliest solver_boundary={edge}"
    );
    distribution("closest_xpd_time_minus_buy_set_s", &closest, true);
    for (bucket, (n, ratio, diff)) in buckets {
        eprintln!("DIAG bucket_{bucket} deals={n}");
        distribution(&format!("ratio_{bucket}"), &ratio, false);
        distribution(&format!("difference_{bucket}_pp"), &diff, false);
    }
    provenance(pool, "ui_gate");
}

/// Count configured delta filters independently of whether their groups are enabled.
fn filter_counts(pool: &Pool<'_>) {
    for (tag, seconds) in [("3h", 10800), ("24h", 86400), ("1h", 3600), ("15m", 900)] {
        let (mut set, mut enabled, mut computable) = (0, 0, 0);
        for &(row, _, _) in pool {
            let empty = HashMap::new();
            let values = StrategyValues {
                values: &row.values,
                defaults: &empty,
            };
            let configured = |lo, hi| {
                raw_number(row, lo).unwrap_or(0.0) != 0.0
                    || raw_number(row, hi).unwrap_or(0.0) != 0.0
            };
            let set_here = match tag {
                "3h" => configured("Delta_3h_Min", "Delta_3h_Max"),
                "24h" => configured("Delta_24h_Min", "Delta_24h_Max"),
                _ => {
                    (values.text("Delta2_Type", "1h").trim() == tag
                        && configured("Delta2_Min", "Delta2_Max"))
                        || (values.text("Delta3_Type", "15m").trim() == tag
                            && configured("Delta3_Min", "Delta3_Max"))
                }
            };
            if !set_here {
                continue;
            }
            set += 1;
            if values.bool("IgnoreFilters", false) || values.bool("IgnoreDelta", false) {
                continue;
            }
            enabled += 1;
            let stamp = row.deal.buy_set_ms.expect("buy_set");
            computable += usize::from(ui_covers(row, stamp - seconds * 1000, stamp));
        }
        eprintln!(
            "DIAG filter_{tag} n={} configured={set} enabled={enabled} computable_at_buy_set={computable} not_computable={} model=count_only",
            pool.len(),
            enabled - computable
        );
    }
}

/// Evaluate one probe on the paired cohort, counting missing inputs as explicit failures.
fn score_probe(
    pool: &Pool<'_>,
    name: &str,
    probe: Probe,
    excluded: (usize, usize, usize),
) -> Metrics {
    let mut metrics = Metrics::default();
    let (mut input_missing, mut affected, mut penalty_default) = (0, 0, 0);
    for &(row, ticks, p) in pool {
        let mut deal = row.deal.clone();
        deal.tick = infer_tick(ticks);
        let ticker = probe.ticker.then(|| ticker_tape(ticks));
        let input = ticker.as_deref().unwrap_or(ticks);
        penalty_default += usize::from(
            probe.suppress
                && row.previous_ms.is_some()
                && raw_number(row, "NextDetectPenalty").is_none(),
        );
        let suppression = row.previous_ms.map(|t| {
            (
                t,
                t + (raw_number(row, "NextDetectPenalty")
                    .unwrap_or(30.0)
                    .max(0.0)
                    * 1000.0) as i64,
            )
        });
        if probe.suppress {
            let baseline = Probe::default().trace(p, &deal, ticks, input, None);
            affected += usize::from(
                baseline
                    .detect_ms
                    .zip(suppression)
                    .is_some_and(|(t, (previous, until))| t >= previous && t < until),
            );
        }
        let stamp = deal.buy_set_ms.expect("buy_set");
        let low_missing = probe.low_s > 0
            && p.price_is_low
            && !ui_covers(
                row,
                stamp - SEARCH_BEFORE_MS - i64::from(probe.low_s) * 1000,
                deal.buy_ms + 10000,
            );
        let mut trace = probe.trace(p, &deal, ticks, input, suppression);
        if trace.refused || low_missing {
            input_missing += 1;
            trace = DropsTrace {
                refused: true,
                ..Default::default()
            };
        }
        metrics.observe_trace(row, trace, probe.reading(p, input, stamp));
    }
    metrics.print(name, excluded.0, excluded.1, excluded.2);
    eprintln!(
        "DIAG probe_{name} paired_n={} input_unknown_counted_as_miss={input_missing} suppression_affected={affected} penalty_default30={penalty_default}",
        pool.len()
    );
    metrics
}

/// Select one knob per family by placement+execution agreement, then measure their combination.
fn timing_batch(pool: &Pool<'_>, refused: usize, degenerate: usize, absent: usize) {
    if std::env::var_os("MOON_DROPS_UNITS").is_some() {
        units::batch(pool, (refused, degenerate, absent));
        return;
    }
    diagnostics(pool);
    filter_counts(pool);
    let ex = (refused, degenerate, absent);
    score_probe(
        pool,
        "best_today_include_v752_ui_gate",
        Probe::default(),
        ex,
    );
    let trusted: Vec<_> = pool
        .iter()
        .copied()
        .filter(|(r, _, _)| {
            r.dump_ms.is_some_and(|t| t <= r.deal.buy_ms)
                && [
                    "DropsMaxTime",
                    "DropsPriceMA",
                    "DropsLastPriceMA",
                    "DropsPriceDelta",
                ]
                .iter()
                .all(|key| raw_number(r, key).is_some())
        })
        .collect();
    score_probe(
        &trusted,
        "params_from_dump_before_deal_v752_ui_gate",
        Probe::default(),
        (0, 0, pool.len() - trusted.len()),
    );
    let families = vec![
        [1, 2, 3, 5, 10]
            .into_iter()
            .map(|lag_s| {
                (
                    format!("lag_{lag_s}s"),
                    Probe {
                        lag_s,
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>(),
        [2, 3, 5]
            .into_iter()
            .map(|hold| {
                (
                    format!("hold_{hold}"),
                    Probe {
                        hold,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        vec![(
            "high_to_min".into(),
            Probe {
                high_to_min: true,
                ..Default::default()
            },
        )],
        vec![(
            "ticker_mid_latency".into(),
            Probe {
                ticker: true,
                ..Default::default()
            },
        )],
        [60, 3600]
            .into_iter()
            .map(|low_s| {
                (
                    format!("price_is_low_{low_s}s"),
                    Probe {
                        low_s,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        vec![(
            "previous_deal_penalty".into(),
            Probe {
                suppress: true,
                ..Default::default()
            },
        )],
    ];
    let base = score_probe(pool, "combination_selection_baseline", Probe::default(), ex);
    let baseline = (base.placement_joint, base.placement);
    let mut combined = Probe::default();
    for family in families {
        let mut best = (baseline, Probe::default());
        for (name, probe) in family {
            let metrics = score_probe(pool, &format!("{name}_v752_ui_gate"), probe, ex);
            let score = (metrics.placement_joint, metrics.placement);
            if score > best.0 {
                best = (score, probe);
            }
        }
        let p = best.1;
        combined.lag_s = combined.lag_s.max(p.lag_s);
        combined.hold = combined.hold.max(p.hold);
        combined.high_to_min |= p.high_to_min;
        combined.ticker |= p.ticker;
        combined.low_s = combined.low_s.max(p.low_s);
        combined.suppress |= p.suppress;
    }
    eprintln!(
        "DIAG combined_selection lag_s={} hold={} high_to_min={} ticker={} low_s={} suppression={} selection=placement_joint_then_placement ties=baseline in_sample=true gate_authorised=false",
        combined.lag_s,
        combined.hold,
        combined.high_to_min,
        combined.ticker,
        combined.low_s,
        combined.suppress
    );
    score_probe(pool, "combined_v752_ui_gate", combined, ex);
}

mod units;
