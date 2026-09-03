//! Time and dump every UI-visible SQLite read the terminal makes against a throwaway synthetic
//! replica, calling exactly what the UI calls with the UI's own default arguments.
//!
//! Measurement instrument for attributing Analytics/Report/tuner wait time to a named query
//! instead of guessing at it. Pairs with `db::trace`: this binary is the one place in the tree
//! that installs the read profiler, so every `open_reader`/`open_strategies`/`open_ro`/
//! `KlineCache::open` connection born after that call reports its statement timings here.
//!
//! The caller must supply a disposable data root; `set_data_dir_override` makes this process
//! resolve its data paths beneath it. Build one with `tools/gen_replica.py <data_dir> <cores>
//! <rows> [span_days]`, which also writes the `data/fixture.json` manifest this harness reads.
//!
//! Usage:
//!     cargo run --release -p moon-core --example db_read_timing -- <data_dir> [repeats] [--plan] [--dump]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use moon_core::db::analytics::{GroupStat, Query, calendar_data, strategy_base_data, summary_data};
use moon_core::db::trace::{ProfiledStatement, install_read_profiler};
use moon_core::db::tuner::threshold_search::{SearchHandle, SearchParams};
use moon_core::db::tuner::{FIELDS, TimeAxes, Variant};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{ProfitMetric, ReportFilter, RowScope};
use moon_core::db::{ProfitScope, ReadFail};

// ---------------------------------------------------------------------------------------------
// The profiler sink. `install_read_profiler` takes a plain `fn`, so all state it can touch is
// process-global. A `Mutex<Vec<_>>` rather than a thread-local: `KlineCache::open` moves its
// connection into a worker thread, and a thread-local sink would silently never see its
// statements.
// ---------------------------------------------------------------------------------------------

static CAPTURED: Mutex<Vec<ProfiledStatement>> = Mutex::new(Vec::new());

/// Append one connection-local PROFILE event to the process-wide harness sink.
fn record_stmt(stmt: ProfiledStatement) {
    CAPTURED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(stmt);
}

/// Take and clear everything captured since the last drain.
fn drain_captured() -> Vec<ProfiledStatement> {
    std::mem::take(
        &mut *CAPTURED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

// ---------------------------------------------------------------------------------------------
// Fixture manifest — H1's contract, frozen in the spec. H2 never guesses these.
// ---------------------------------------------------------------------------------------------

/// Shape and provenance recorded by `tools/gen_replica.py` for one disposable fixture.
#[derive(serde::Deserialize, Debug)]
struct FixtureManifest {
    rows: i64,
    cores: i64,
    strategy_groups: i64,
    coins: i64,
    span_days: i64,
    reports_bytes: i64,
    analyzed: bool,
    seed: i64,
}

/// Read the fixture manifest below `root`, printing an actionable failure before exiting on error.
fn read_manifest(root: &std::path::Path) -> FixtureManifest {
    let path = root.join("data").join("fixture.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        eprintln!(
            "[FAIL] fixture manifest unreadable at {}: {error}",
            path.display()
        );
        std::process::exit(1);
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        eprintln!(
            "[FAIL] fixture manifest malformed at {}: {error}",
            path.display()
        );
        std::process::exit(1);
    })
}

// ---------------------------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------------------------

/// One timed closure invocation and whether its captured SQL can be replayed for planning.
struct Sample {
    wall_ms: f64,
    sql_ms: f64,
    replayable: bool,
}

/// Run `body` `repeats` times, reporting wall time, summed SQL time and their difference for
/// both the first sample and the warm median.
///
/// "First" is not "cold": no cache is reset between surfaces, so a later surface can inherit
/// pages an earlier one already warmed — see the process-wide note printed once in `main`.
///
/// Args:
///     label: ASCII surface name printed in the report.
///     repeats: At least two timed runs.
///     body: Work under measurement.
///     outcome: Formats the FIRST call's result for the printed "outcome" line — run after
///         that call's timer already stopped, so it costs nothing against the first sample.
///
/// Returns:
///     Every sample, and every statement captured across all `repeats` calls (for `--plan`).
fn measure<T>(
    label: &str,
    repeats: usize,
    mut body: impl FnMut() -> T,
    outcome: impl FnOnce(&T) -> String,
) -> (Vec<Sample>, Vec<ProfiledStatement>) {
    let mut samples = Vec::with_capacity(repeats);
    let mut all_stmts = Vec::new();
    let mut first_outcome = None;
    let mut outcome = Some(outcome);
    for i in 0..repeats {
        drain_captured();
        let started = Instant::now();
        let value = body();
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        let captured = drain_captured();
        let sql_ms: f64 = captured
            .iter()
            .map(|s| s.duration.as_secs_f64() * 1000.0)
            .sum();
        let replayable = captured.iter().all(|s| s.expanded);
        if i == 0 {
            if let Some(outcome) = outcome.take() {
                first_outcome = Some(outcome(&value));
            }
        }
        drop(value);
        all_stmts.extend(captured);
        samples.push(Sample {
            wall_ms,
            sql_ms,
            replayable,
        });
    }
    let first = &samples[0];
    let mut warm: Vec<&Sample> = samples[1..].iter().collect();
    warm.sort_by(|a, b| a.wall_ms.total_cmp(&b.wall_ms));
    let median = warm.get(warm.len() / 2).copied().unwrap_or(first);
    let note = |s: &Sample| {
        if s.replayable {
            ""
        } else {
            "  [NOT REPLAYABLE]"
        }
    };
    println!(
        "{label:<46} first {:>9.1} ms  sql {:>9.1} ms  diff {:>9.1} ms{}",
        first.wall_ms,
        first.sql_ms,
        first.wall_ms - first.sql_ms,
        note(first)
    );
    println!(
        "{:<46} warm {:>9.1} ms  sql {:>9.1} ms  diff {:>9.1} ms{}",
        "",
        median.wall_ms,
        median.sql_ms,
        median.wall_ms - median.sql_ms,
        note(median)
    );
    if let Some(outcome) = first_outcome {
        println!("{:<46} outcome: {outcome}", "");
    }
    (samples, all_stmts)
}

/// Build the default query one open Analytics or tuner window issues for a period.
fn query(from: i64, to: i64, metric: ProfitMetric, cores: Vec<u64>) -> Query {
    Query {
        axis: moon_core::db::ReportAxis::from_measured(Default::default(), chrono_tz::UTC),
        previous_period_basis: Default::default(),
        from,
        to,
        cores,
        side: Default::default(),
        emulator: None,
        strategies: Vec::new(),
        strategy_name_mask: String::new(),
        metric,
        valuation: ValuationMode::Historical,
        prefer_usdt: false,
    }
}

/// Format a read result for the harness's ASCII outcome column.
fn ok_or_err<T>(result: &Result<T, ReadFail>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("{error:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Discovery — the harness never hardcodes a core, strategy or coin identity; it reads them from
// whatever fixture it was pointed at.
// ---------------------------------------------------------------------------------------------

/// Fixture identities discovered from the replica instead of assumed by the harness.
struct Pool {
    cores: Vec<(u64, String)>,
    strategies: Vec<moon_core::db::ReportStrategyKey>,
    /// The first `strategies` entry that names a REAL strategy — `strategy_id != 0` — for the
    /// strat-db-backed calls (`versions_with_stats`, `strategy_purge_rows`), which know nothing
    /// about `orders_rep`'s liquidation sentinel and legitimately find no rows for it.
    first_real_strategy: Option<moon_core::db::ReportStrategyKey>,
    coins: Vec<String>,
    kline_key: Option<(String, String, u32)>,
}

/// Discover fixture identities while keeping discovery traffic out of the first timed sample.
fn discover_pool() -> Pool {
    let cores = moon_core::db::open_reader()
        .and_then(|conn| moon_core::db::distinct_cores(&conn))
        .unwrap_or_default();
    // NOT `db::distinct_strategies`: on this machine's fixture, `strat.strategies` carries a
    // NULL `name` for at least one row, and `distinct_strategies` decodes that column
    // unconditionally (`row.get::<_, String>(2)`), so the call fails with "Invalid column type
    // Null at index: 2, name: name" — see the report. Pool discovery must not depend on that
    // path; a direct scan of `orders_rep` gives the same `(core_uid, strategy_id)` identities.
    // Deliberately UNFILTERED: this list's length feeds the fixture-drift guard below, which
    // compares it against the manifest's own `groups_seen` count — including `strategyid=0`,
    // `orders_rep`'s forced sentinel for a liquidation row with no owning strategy. Filtering it
    // out here would desync the two counts and misfire the guard. See `first_real_strategy`
    // below for the probe that skips the sentinel instead.
    let strategies: Vec<moon_core::db::ReportStrategyKey> = moon_core::db::open_reader()
        .map(|conn| {
            conn.prepare("SELECT DISTINCT core_uid, strategyid FROM orders_rep")
                .and_then(|mut stmt| {
                    stmt.query_map([], |row| {
                        Ok(moon_core::db::ReportStrategyKey {
                            core_uid: row.get::<_, i64>(0)? as u64,
                            strategy_id: row.get::<_, i64>(1)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let coins = moon_core::db::open_reader()
        .map(|conn| {
            conn.prepare("SELECT DISTINCT coin FROM orders_rep LIMIT 5")
                .and_then(|mut stmt| {
                    stmt.query_map([], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let kline_key = {
        let path = moon_core::config::paths::klines_db_path();
        // `chunks`/`chunks_v2` both name their third column `kind`, never `kind_min` — that
        // name belongs only to `MergeItem`/`read_range`'s Rust-side parameter. `chunks` is also
        // the wrong TABLE to probe: it is the legacy v1 store `gen_klines` seeds for only
        // `LEGACY_MARKETS` markets on one kind and one day, while `chunks_v2` is the write
        // target `merge_batch_blocking` actually fills for every market and kind the fixture
        // claims to hold (`market/kline_cache.rs`'s own module doc). Reading `chunks_v2` here
        // picks a key that is guaranteed to exist, straight from the cache's own contents.
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT exchange, market, kind FROM chunks_v2 LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u32>(2)?,
                        ))
                    },
                )
                .ok()
            })
    };
    let first_real_strategy = strategies.iter().find(|k| k.strategy_id != 0).copied();
    // The discovery reads above are real database traffic; they must not pollute the first
    // measured surface's SQL sum.
    drain_captured();
    Pool {
        cores,
        strategies,
        first_real_strategy,
        coins,
        kline_key,
    }
}

// ---------------------------------------------------------------------------------------------
// Dump — arrival-order rows over the compared fields, at full `{:?}` precision, so two dumps
// from two binaries over one fixture diff to empty when a change is identity-preserving.
// ---------------------------------------------------------------------------------------------

/// Print one ordered group collection in the dump format used for cross-binary comparisons.
fn dump_groups(tag: &str, kind: &str, groups: &[GroupStat]) {
    for (index, group) in groups.iter().enumerate() {
        println!("{tag}|{kind}|{index}|{group:?}");
    }
}

/// Dump every comparable read result for one period without running the timing loop.
fn dump_all(period_name: &str, from: i64, to: i64, pool: &Pool) {
    let q = |metric| query(from, to, metric, Vec::new());

    let summary = summary_data(&q(ProfitMetric::Quote), false);
    println!("{period_name}|summary|data|{:?}", summary.data);

    let strategies = strategy_base_data(&q(ProfitMetric::Quote), false);
    match &strategies.data {
        Ok(ProfitScope::Comparable { unit, data }) => {
            println!(
                "{period_name}|strategy_base|unit|{unit:?}|trades={}",
                data.trades
            );
            dump_groups(period_name, "strategy", &data.strategies);
            dump_groups(period_name, "coin", &data.coins);
        }
        other => println!("{period_name}|strategy_base|{other:?}"),
    }

    let calendar_day = calendar_data(&q(ProfitMetric::Quote), None, false, false);
    println!("{period_name}|calendar_daily|{:?}", calendar_day.period);
    let calendar_hour = calendar_data(&q(ProfitMetric::Quote), None, true, false);
    println!("{period_name}|calendar_hourly|{:?}", calendar_hour.period);

    let monitor = moon_core::db::analytics::profit_monitor(&q(ProfitMetric::Quote));
    println!("{period_name}|profit_monitor|{monitor:?}");

    if let Some(conn) = moon_core::db::open_reader().ok() {
        if let Ok(snap) = moon_core::db::read_snapshot(&conn) {
            let filter = ReportFilter::default();
            if let Ok(cores) = moon_core::db::distinct_cores(&snap) {
                println!("{period_name}|distinct_cores|{cores:?}");
            }
            if let Ok(strategies) = moon_core::db::distinct_strategies(&snap, &filter) {
                println!("{period_name}|distinct_strategies|{strategies:?}");
            }
            if let Ok(table) = moon_core::db::query_reports(&snap, &filter, "closedate", true, 200)
            {
                println!("{period_name}|report_cols|{:?}", table.cols);
                for (index, row) in table.rows.iter().enumerate() {
                    println!("{period_name}|report_row|{index}|{row:?}");
                }
            }
            if let Ok(totals) = moon_core::db::query_totals(&snap, &filter) {
                println!("{period_name}|report_totals|{totals:?}");
            }
        }
    }

    if let (Some((core_uid, _)), false) = (pool.cores.first(), pool.coins.is_empty()) {
        if let Ok(conn) = moon_core::db::open_reader() {
            if let Ok(history) =
                moon_core::db::query_chart_trade_history(&conn, *core_uid, &pool.coins, None, 50)
            {
                for (index, record) in history.records.iter().enumerate() {
                    println!("{period_name}|chart_trade|{index}|{record:?}");
                }
            }
        }
    }

    if let Some(key) = pool.first_real_strategy {
        let versions =
            moon_core::strat_db::stats::versions_with_stats(key.core_uid, key.strategy_id);
        for (index, version) in versions.iter().enumerate() {
            println!("{period_name}|version_stats|{index}|{version:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .expect("usage: db_read_timing <data_dir> [repeats] [--plan] [--dump]"),
    );
    let repeats = args
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(2);
    let raw_args: Vec<String> = std::env::args().collect();
    let plan_mode = raw_args.iter().any(|value| value == "--plan");
    let dump_mode = raw_args.iter().any(|value| value == "--dump");

    assert!(
        moon_core::config::paths::set_data_dir_override(root.clone()),
        "the data root must be installed before any path resolves"
    );
    let _permit = moon_core::db::report_recovery::prepare();
    let manifest = read_manifest(&root);
    println!("data root: {}", root.display());
    println!(
        "reports:   {}",
        moon_core::config::paths::reports_db_path().display()
    );
    println!(
        "fixture:   rows={} cores={} strategy_groups={} coins={} span_days={} reports_bytes={} \
         analyzed={} seed={}",
        manifest.rows,
        manifest.cores,
        manifest.strategy_groups,
        manifest.coins,
        manifest.span_days,
        manifest.reports_bytes,
        manifest.analyzed,
        manifest.seed
    );
    println!(
        "note:      no cache is reset between surfaces — every surface after the first inherits \
         pages warmed by whatever ran before it, so \"first\" below means \"first repetition of \
         this closure\", not \"never-warmed\""
    );

    install_read_profiler(record_stmt);
    let pool = discover_pool();

    // --- Fixture drift guard --------------------------------------------------------------
    let mut drift: Vec<String> = Vec::new();
    if pool.cores.len() as i64 != manifest.cores {
        drift.push(format!(
            "core count: fixture claims {}, replica shows {}",
            manifest.cores,
            pool.cores.len()
        ));
    }
    if (pool.strategies.len() as i64) < manifest.strategy_groups {
        drift.push(format!(
            "strategy groups: fixture claims {}, replica shows only {}",
            manifest.strategy_groups,
            pool.strategies.len()
        ));
    }
    let reports_bytes = std::fs::metadata(moon_core::config::paths::reports_db_path())
        .map(|meta| meta.len() as i64)
        .unwrap_or(0);
    let low = manifest.reports_bytes / 2;
    let high = manifest.reports_bytes.saturating_mul(2).max(1);
    if reports_bytes < low || reports_bytes > high {
        drift.push(format!(
            "reports.sqlite size: fixture claims {} bytes, on disk {} bytes (outside [{low}, {high}])",
            manifest.reports_bytes, reports_bytes
        ));
    }
    if !drift.is_empty() {
        eprintln!("[FAIL] fixture drift detected:");
        for line in &drift {
            eprintln!("  - {line}");
        }
        std::process::exit(1);
    }

    if dump_mode {
        let end = 1_780_000_000i64;
        let periods: [(&str, i64, i64); 4] = [
            ("today", end - 86_400, end),
            ("month", end - 30 * 86_400, end),
            ("year", end - 365 * 86_400, end),
            ("all", -1, end),
        ];
        for (name, from, to) in periods {
            dump_all(name, from, to, &pool);
        }
        return;
    }

    let end = 1_780_000_000i64;
    let periods: [(&str, i64, i64); 4] = [
        ("today", end - 86_400, end),
        ("month", end - 30 * 86_400, end),
        ("year", end - 365 * 86_400, end),
        ("all", -1, end),
    ];
    let metrics = [
        ("quote", ProfitMetric::Quote),
        ("percent", ProfitMetric::Percent),
    ];

    let mut plan_pool: Vec<ProfiledStatement> = Vec::new();
    let mut missing_guarantee: Vec<String> = Vec::new();
    // Real production failures that happen to surface here — never fixture drift, so kept in a
    // separate bucket the closing report never conflates with the guarantees above.
    let mut production_failures: Vec<String> = Vec::new();

    let scoped_cores: Vec<u64> = pool.cores.iter().take(3).map(|(uid, _)| *uid).collect();

    for (name, from, to) in periods {
        println!("\n=== period: {name} ===");
        for (metric_name, metric) in metrics {
            let label_suffix = format!("[{name}/{metric_name}]");
            let q = query(from, to, metric, Vec::new());
            let q_scoped = query(from, to, metric, scoped_cores.clone());

            let (_, stmts) = measure(
                &format!("summary_data {label_suffix}"),
                repeats,
                || summary_data(&q, false),
                |r| {
                    match &r.data {
                        Ok(ProfitScope::Comparable { data, .. })
                            if data.strategies.is_empty() && data.coins.is_empty() =>
                        {
                            missing_guarantee
                                .push(format!("summary_data: no groups {label_suffix}"));
                        }
                        Err(_) => {
                            missing_guarantee.push(format!("summary_data: errored {label_suffix}"));
                        }
                        _ => {}
                    }
                    ok_or_err(&r.data)
                },
            );
            plan_pool.extend(stmts);

            if metric_name == "quote" && !scoped_cores.is_empty() {
                let (_, stmts) = measure(
                    &format!(
                        "summary_data [{name}/scoped-{}-of-{}]",
                        scoped_cores.len(),
                        pool.cores.len()
                    ),
                    repeats,
                    || summary_data(&q_scoped, false),
                    |r| ok_or_err(&r.data),
                );
                plan_pool.extend(stmts);
            }

            let (_, stmts) = measure(
                &format!("strategy_base_data {label_suffix}"),
                repeats,
                || strategy_base_data(&q, false),
                |r| {
                    match &r.data {
                        Ok(ProfitScope::Comparable { data, .. })
                            if data.strategies.is_empty() && data.coins.is_empty() =>
                        {
                            missing_guarantee
                                .push(format!("strategy_base_data: no groups {label_suffix}"));
                        }
                        Err(_) => missing_guarantee
                            .push(format!("strategy_base_data: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.data)
                },
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("calendar_data daily {label_suffix}"),
                repeats,
                || calendar_data(&q, None, false, false),
                |r| {
                    match &r.period {
                        Ok(ProfitScope::Comparable { data, .. }) if data.current.is_empty() => {
                            missing_guarantee
                                .push(format!("calendar_data daily: no cells {label_suffix}"));
                        }
                        Err(_) => missing_guarantee
                            .push(format!("calendar_data daily: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.period)
                },
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("calendar_data hourly {label_suffix}"),
                repeats,
                || calendar_data(&q, None, true, false),
                |r| {
                    match &r.period {
                        Ok(ProfitScope::Comparable { data, .. }) if data.current.is_empty() => {
                            missing_guarantee
                                .push(format!("calendar_data hourly: no cells {label_suffix}"));
                        }
                        Err(_) => missing_guarantee
                            .push(format!("calendar_data hourly: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.period)
                },
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("profit_monitor {label_suffix}"),
                repeats,
                || moon_core::db::analytics::profit_monitor(&q),
                |r| {
                    if r.is_err() {
                        missing_guarantee.push(format!("profit_monitor: errored {label_suffix}"));
                    }
                    ok_or_err(r)
                },
            );
            plan_pool.extend(stmts);

            let variants = vec![Variant::default()];
            let field = FIELDS.first().map(|f| f.col).unwrap_or("profitbtc");

            let (_, stmts) = measure(
                &format!("filter_tuner_data {label_suffix}"),
                repeats,
                || moon_core::db::tuner::filter_tuner_data(&q, &variants, field, 20),
                |r| {
                    match &r.stats {
                        Ok(v) if v.is_empty() => missing_guarantee
                            .push(format!("filter_tuner_data: no KPI rows {label_suffix}")),
                        Err(_) => missing_guarantee
                            .push(format!("filter_tuner_data: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.stats)
                },
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("time_tuner_data {label_suffix}"),
                repeats,
                || moon_core::db::tuner::time_tuner_data(&q, &variants),
                |r| {
                    match &r.stats {
                        Ok(v) if v.is_empty() => missing_guarantee
                            .push(format!("time_tuner_data: no KPI rows {label_suffix}")),
                        Err(_) => missing_guarantee
                            .push(format!("time_tuner_data: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.stats)
                },
            );
            plan_pool.extend(stmts);

            let picked: Vec<String> = pool.coins.first().cloned().into_iter().collect();
            let (_, stmts) = measure(
                &format!("coin_tuner_data {label_suffix}"),
                repeats,
                || moon_core::db::tuner::coin_tuner_data(&q, &variants, &q, &picked),
                |r| {
                    match &r.kpi {
                        Ok(v) if v.is_empty() => missing_guarantee
                            .push(format!("coin_tuner_data: no KPI rows {label_suffix}")),
                        Err(_) => missing_guarantee
                            .push(format!("coin_tuner_data: errored {label_suffix}")),
                        _ => {}
                    }
                    ok_or_err(&r.kpi)
                },
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("variant_stats {label_suffix}"),
                repeats,
                || moon_core::db::tuner::variant_stats(&q, &variants),
                ok_or_err,
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("histogram {label_suffix}"),
                repeats,
                || moon_core::db::tuner::histogram(&q, field, 20),
                ok_or_err,
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("suggest_field {label_suffix}"),
                repeats,
                || moon_core::db::tuner::suggest_field(&q, field, 30, 20, true),
                ok_or_err,
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("suggest_time {label_suffix}"),
                repeats,
                || {
                    moon_core::db::tuner::suggest_time(
                        &q,
                        30,
                        20,
                        true,
                        TimeAxes {
                            week: true,
                            day: true,
                            hour: true,
                            ..Default::default()
                        },
                    )
                },
                ok_or_err,
            );
            plan_pool.extend(stmts);

            let locked = vec![None; FIELDS.len()];
            let (_, stmts) = measure(
                &format!("threshold_search::suggest {label_suffix}"),
                repeats,
                || {
                    let handle = SearchHandle::new();
                    moon_core::db::tuner::threshold_search::suggest(
                        &q,
                        SearchParams {
                            restarts: 8,
                            min_n: None,
                            locked: &locked,
                            edges: 20,
                            round: true,
                            seed: Some(20260903),
                            train_frac: 1.0,
                            compose: false,
                        },
                        &handle,
                    )
                },
                ok_or_err,
            );
            plan_pool.extend(stmts);

            if !pool.coins.is_empty() {
                let (_, stmts) = measure(
                    &format!("strategies_for_coins {label_suffix}"),
                    repeats,
                    || moon_core::db::analytics::strategies_for_coins(&q, &pool.coins),
                    ok_or_err,
                );
                plan_pool.extend(stmts);
            }

            // --- The Report: mirrors run_report_query's own sequence -----------------------
            for (sort_name, sort_key, desc) in [
                ("closedate DESC", "closedate", true),
                ("profitbtc DESC", "profitbtc", true),
            ] {
                let filter = ReportFilter {
                    date_from: if from < 0 { None } else { Some(from) },
                    date_to: Some(to),
                    rows: RowScope::ClosedAndOpen,
                    ..ReportFilter::default()
                };
                let (_, stmts) = measure(
                    &format!("report [{name}/{sort_name}]"),
                    repeats,
                    || -> Result<(usize, usize), ReadFail> {
                        let conn = moon_core::db::open_reader()?;
                        let snap = moon_core::db::read_snapshot(&conn)?;
                        let cores = moon_core::db::distinct_cores(&snap)?;
                        // Mirrors `run_report_query`'s own OPTIONAL treatment (`.transpose()`
                        // there, not `?`): a strategy-scope refresh failing must not sink the
                        // rows/totals read it rides beside. See the report for why this call
                        // fails against this fixture's `strategies.sqlite` (a NULL `name`).
                        let strategies = moon_core::db::distinct_strategies(&snap, &filter).ok();
                        let table =
                            moon_core::db::query_reports(&snap, &filter, sort_key, desc, 500)?;
                        let totals = moon_core::db::query_totals(&snap, &filter)?;
                        let _ = (cores, strategies, totals);
                        Ok((table.rows.len(), 0))
                    },
                    |r| {
                        match r {
                            Ok((rows, _)) if *rows == 0 => missing_guarantee
                                .push(format!("report [{name}/{sort_name}]: no rows")),
                            Err(_) => missing_guarantee
                                .push(format!("report [{name}/{sort_name}]: errored")),
                            _ => {}
                        }
                        ok_or_err(r)
                    },
                );
                plan_pool.extend(stmts);
            }

            let (_, stmts) = measure(
                &format!("distinct_cores {label_suffix}"),
                repeats,
                || {
                    moon_core::db::open_reader()
                        .and_then(|conn| moon_core::db::distinct_cores(&conn))
                },
                ok_or_err,
            );
            plan_pool.extend(stmts);

            let (_, stmts) = measure(
                &format!("distinct_strategies {label_suffix}"),
                repeats,
                || {
                    moon_core::db::open_reader().and_then(|conn| {
                        moon_core::db::distinct_strategies(&conn, &ReportFilter::default())
                    })
                },
                ok_or_err,
            );
            plan_pool.extend(stmts);

            if let Some((core_uid, _)) = pool.cores.first() {
                let (_, stmts) = measure(
                    &format!("query_chart_trade_history {label_suffix}"),
                    repeats,
                    || {
                        moon_core::db::open_reader().and_then(|conn| {
                            moon_core::db::query_chart_trade_history(
                                &conn,
                                *core_uid,
                                &pool.coins,
                                None,
                                200,
                            )
                        })
                    },
                    ok_or_err,
                );
                plan_pool.extend(stmts);

                let sample_record = moon_core::db::open_reader().ok().and_then(|conn| {
                    moon_core::db::query_chart_trade_history(&conn, *core_uid, &pool.coins, None, 1)
                        .ok()
                        .and_then(|history| history.records.into_iter().next())
                });
                if let Some(record) = sample_record {
                    let (_, stmts) = measure(
                        &format!("query_trade_meta {label_suffix}"),
                        repeats,
                        || {
                            moon_core::db::open_reader()
                                .and_then(|conn| moon_core::db::query_trade_meta(&conn, &record))
                        },
                        ok_or_err,
                    );
                    plan_pool.extend(stmts);
                }
            }

            if let Some(key) = pool.first_real_strategy {
                let (_, stmts) = measure(
                    &format!("strategy_purge_rows {label_suffix}"),
                    repeats,
                    || {
                        moon_core::db::open_reader()
                            .and_then(|conn| moon_core::db::strategy_purge_rows(&conn, key))
                    },
                    ok_or_err,
                );
                plan_pool.extend(stmts);

                let (_, stmts) = measure(
                    &format!("versions_with_stats {label_suffix}"),
                    repeats,
                    || {
                        moon_core::strat_db::stats::versions_with_stats(
                            key.core_uid,
                            key.strategy_id,
                        )
                    },
                    |v| {
                        if v.is_empty() {
                            missing_guarantee
                                .push(format!("versions_with_stats: no versions {label_suffix}"));
                        }
                        format!("{} versions", v.len())
                    },
                );
                plan_pool.extend(stmts);
            }

            let targets: Vec<(i64, Option<u64>)> = pool
                .strategies
                .iter()
                .map(|k| (k.strategy_id, Some(k.core_uid)))
                .collect();
            if !targets.is_empty() {
                let (_, stmts) = measure(
                    &format!("coin_lists::coin_lists {label_suffix}"),
                    repeats,
                    || moon_core::db::coin_lists::coin_lists(&targets),
                    |r| {
                        match r {
                            Ok(rows) if rows.black.is_empty() && rows.white.is_empty() => {
                                missing_guarantee
                                    .push(format!("coin_lists: no rows {label_suffix}"));
                            }
                            // `scope_sql`'s one-OR-term-per-strategy join hits SQLite's
                            // "Expression tree is too large" parser limit once enough strategies
                            // are selected — a real production bug (`db/coin_lists/mod.rs`
                            // `scope_sql`, recorded for the goal owner), not this fixture
                            // drifting from what the harness expects. Kept out of
                            // `missing_guarantee` so the two classes are never conflated.
                            Err(error) => {
                                let detail = format!("{error:?}");
                                if detail.contains("too large") {
                                    production_failures
                                        .push(format!("coin_lists {label_suffix}: {detail}"));
                                } else {
                                    missing_guarantee
                                        .push(format!("coin_lists: errored {label_suffix}"));
                                }
                            }
                            _ => {}
                        }
                        ok_or_err(r)
                    },
                );
                plan_pool.extend(stmts);
            }

            let (_, stmts) = measure(
                &format!("open_reader alone {label_suffix}"),
                repeats,
                || moon_core::db::open_reader().map(|_| ()),
                ok_or_err,
            );
            plan_pool.extend(stmts);

            if let Some((exchange, market, kind_min)) = &pool.kline_key {
                let path = moon_core::config::paths::klines_db_path();
                let cache = moon_core::market::kline_cache::KlineCache::open(path);
                if let Some(cache) = cache {
                    let (_, stmts) = measure(
                        &format!("KlineCache::read_range {label_suffix}"),
                        repeats,
                        || cache.read_range(exchange, market, *kind_min, 0, i64::MAX),
                        |r| match r {
                            Some(rows) if rows.is_empty() => {
                                missing_guarantee.push(format!(
                                    "KlineCache::read_range: no rows {label_suffix}"
                                ));
                                "0 rows".to_string()
                            }
                            Some(rows) => format!("{} rows", rows.len()),
                            None => {
                                missing_guarantee.push(format!(
                                    "KlineCache::read_range: timed out {label_suffix}"
                                ));
                                "timed out".to_string()
                            }
                        },
                    );
                    plan_pool.extend(stmts);
                } else {
                    missing_guarantee
                        .push(format!("KlineCache::open failed [{name}/{metric_name}]"));
                }
            } else if pool.kline_key.is_none() {
                missing_guarantee.push(format!(
                    "KlineCache: no (exchange, market, kind_min) row in klines.sqlite [{name}]"
                ));
            }
        }
    }

    // Neither surface below can be called from this example crate: `report_quote_ordinals` is
    // a private `fn` inside a private `mod worker` (`db/valuation/worker.rs:1953`,
    // `db/valuation/mod.rs:12`), and `basis::probe` is a private `fn` inside the private
    // `mod basis` of `db::analytics` (`db/analytics/basis.rs:94`, `db/analytics/mod.rs:25`) —
    // both files are out of this branch's edit scope, and a copied SQL string is explicitly
    // forbidden by the spec. Never made `pub` to reach them; kept first-class instead so the
    // omission cannot be mistaken for "measured and cheap" — printed here AND carried into the
    // `--plan` table itself (`run_plan_mode`), not only as this footnote.
    for (label, reason) in NOT_MEASURED {
        println!("\n[NOT MEASURED] {label} - {reason}");
    }

    // Print everything the run promised — the timing table above, `--plan`'s EQP dump below —
    // BEFORE reporting the guard's own failures and exiting. A guard that hides the artifact it
    // was meant to protect is worse than no guard; only the ORDER changed here, the exit code on
    // a real drift is unchanged.
    if plan_mode {
        run_plan_mode(plan_pool, &NOT_MEASURED);
    }

    if !production_failures.is_empty() {
        eprintln!("[FAIL] production bug(s) hit while measuring (not fixture drift):");
        for line in &production_failures {
            eprintln!("  - {line}");
        }
    }
    if !missing_guarantee.is_empty() {
        eprintln!("[FAIL] guaranteed-non-empty surfaces did not deliver:");
        for line in &missing_guarantee {
            eprintln!("  - {line}");
        }
    }
    if !production_failures.is_empty() || !missing_guarantee.is_empty() {
        std::process::exit(1);
    }
}

/// Surfaces the deliverable promises a number for but this example genuinely cannot reach —
/// see the two reasons printed beside this constant's use in `main`.
const NOT_MEASURED: [(&str, &str); 2] = [
    (
        "valuation reconciliation row",
        "report_quote_ordinals is private, unreachable from an example",
    ),
    (
        "basis::probe full scan",
        "private fn in a private module, unreachable from an example, and db/analytics/mod.rs \
         is out of edit scope",
    ),
];

/// Register STUB scalar functions matching the crate's own calendar/time-tuner SQL surface, so
/// `EXPLAIN QUERY PLAN` can resolve their names on this example's own reader connection.
/// `db::analytics::time_zone::install` is `pub(crate)` and installs the REAL bodies against a
/// captured `ReportAxis`; an example cannot call it, which is why these are stubs rather than
/// the genuine functions. `EXPLAIN QUERY PLAN` only resolves a function's name and declared
/// properties and never evaluates its body, so a stub returning a constant is correct for this
/// purpose — but the PLAN it enables is real, which is why every plan taken on this connection
/// is printed with an explicit marker (see `run_plan_mode`) rather than silently passed off as
/// having run against production functions.
///
/// Registered with the SAME `SQLITE_UTF8 | SQLITE_DETERMINISTIC` flags the real functions use,
/// so the planner sees the same function properties the production connection would.
fn install_plan_stub_functions(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("mt_to_utc", 2, flags, |_ctx| Ok(0i64))?;
    conn.create_scalar_function("mt_local_bucket", 3, flags, |_ctx| Ok(0i64))?;
    conn.create_scalar_function("mt_minute_of_day", 2, flags, |_ctx| Ok(0i64))?;
    conn.create_scalar_function("mt_minute_of_week", 2, flags, |_ctx| Ok(0i64))?;
    conn.create_scalar_function("mt_core_minute_of_day", 1, flags, |_ctx| Ok(0i64))?;
    conn.create_scalar_function("mt_core_minute_of_week", 1, flags, |_ctx| Ok(0i64))?;
    // The real function returns its argument casefolded; a stub only needs to resolve, but
    // returning the argument unchanged keeps its declared return type (TEXT) honest too.
    conn.create_scalar_function("mt_unicode_casefold", 1, flags, |ctx| ctx.get::<String>(0))?;
    Ok(())
}

/// `--plan`: dedup captured statements by exact text, print total time, call count and
/// `EXPLAIN QUERY PLAN` for every replayable one; mark placeholder-only captures instead of
/// ranking them; and list every `not_measured` surface honestly instead of silently ranking
/// past it.
fn run_plan_mode(pool: Vec<ProfiledStatement>, not_measured: &[(&str, &str)]) {
    struct Agg {
        total_ms: f64,
        count: usize,
        replayable: bool,
    }
    let mut by_text: HashMap<String, Agg> = HashMap::new();
    for stmt in pool {
        let entry = by_text.entry(stmt.sql.clone()).or_insert(Agg {
            total_ms: 0.0,
            count: 0,
            replayable: true,
        });
        entry.total_ms += stmt.duration.as_secs_f64() * 1000.0;
        entry.count += 1;
        entry.replayable = entry.replayable && stmt.expanded;
    }
    let (mut ranked, mut not_replayable): (Vec<(String, Agg)>, Vec<(String, Agg)>) =
        by_text.into_iter().partition(|(_, agg)| agg.replayable);
    ranked.sort_by(|a, b| b.1.total_ms.total_cmp(&a.1.total_ms));
    not_replayable.sort_by(|a, b| b.1.total_ms.total_cmp(&a.1.total_ms));

    println!(
        "\n=== --plan: {} ranked statements ({} not replayable, {} not measured, see below) ===",
        ranked.len(),
        not_replayable.len(),
        not_measured.len()
    );
    let conn = match moon_core::db::open_reader() {
        Ok(conn) => {
            if let Err(error) = install_plan_stub_functions(&conn) {
                println!("(cannot register plan-stub functions: {error:?})");
            }
            Some(conn)
        }
        Err(error) => {
            println!("(cannot open a reader for EXPLAIN QUERY PLAN: {error:?})");
            None
        }
    };
    // Statements whose EXPLAIN fails with "no such table" ran on the writer's own connection or
    // the kline cache's own connection, neither of which this reader has attached — a different,
    // unfixable gap from the mt_* function names the stubs above resolve. Collected here instead
    // of printed inline, so the count of planless statements is explained under its own heading
    // rather than merely reported one line at a time.
    let mut no_such_table: Vec<(String, f64, usize)> = Vec::new();
    for (sql, agg) in ranked {
        println!(
            "\n-- total {:>9.1} ms  calls {:>4}",
            agg.total_ms, agg.count
        );
        println!("   {sql}");
        let Some(conn) = &conn else { continue };
        match conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    Ok(format!(
                        "id={} parent={} detail={}",
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(3)?
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
            }) {
            Ok(lines) => {
                println!("   [stub-resolved: mt_* bodies are stubs, the PLAN is real]");
                for line in lines {
                    println!("     {line}");
                }
            }
            Err(error) => {
                let text = error.to_string();
                if text.contains("no such table") {
                    no_such_table.push((sql, agg.total_ms, agg.count));
                } else {
                    println!("     plan failed: {error}");
                }
            }
        }
    }

    println!(
        "\n=== --plan: {} statement(s) with no plan (writer-only or kline-cache-only table, \
         not attached to this reader) ===",
        no_such_table.len()
    );
    for (sql, total_ms, count) in &no_such_table {
        println!("\n-- total {total_ms:>9.1} ms  calls {count:>4}  NO PLAN (table not attached)");
        println!("   {sql}");
    }

    // Placeholder-only captures never had their real literal-bound shape recorded, so they
    // cannot be replayed with EXPLAIN QUERY PLAN and must never drive the optimisation order
    // above — printed here, visible but unranked, instead of being dropped silently.
    println!(
        "\n=== --plan: {} not-replayable captures (never ranked) ===",
        not_replayable.len()
    );
    for (sql, agg) in not_replayable {
        println!(
            "\n-- total {:>9.1} ms  calls {:>4}  NOT REPLAYABLE (placeholder capture)",
            agg.total_ms, agg.count
        );
        println!("   {sql}");
    }

    // Promised numbers this example can never produce, listed in the same ranked table rather
    // than only as a footnote elsewhere — a reader scanning just this section still sees them
    // flagged, never silently absent.
    println!(
        "\n=== --plan: {} not-measured surfaces (never ranked, never callable from here) ===",
        not_measured.len()
    );
    for (label, reason) in not_measured {
        println!("\n-- NOT MEASURED  {label}");
        println!("   {reason}");
    }
}
