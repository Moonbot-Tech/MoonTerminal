//! Opt-in Analytics read timings on a disposable copy, with no persisted identities in output.
//!
//! Run this test alone with --ignored --exact --nocapture --test-threads=1 in release mode.
//! MOON_ANALYTICS_BENCH_DIR is the application data root (on Windows it contains data/).
//! MOON_ANALYTICS_BENCH_SQL_DUMP optionally writes the top 30 expanded statements outside the repo.
//! The profiler and path override are process-global; never combine this probe with other tests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::Instant;

use chrono::Datelike;
use rusqlite::StatementStatus;

use super::super::{self as analytics, Query};
use crate::config::paths;
use crate::db::trace::{self, ProfiledStatement};
use crate::db::valuation::ValuationMode;
use crate::db::{self, ProfitMetric, ReadResult, ReportAxis, SideFilter, tuner};

static CAPTURED: Mutex<Vec<ProfiledStatement>> = Mutex::new(Vec::new());
static CAPTURING: AtomicBool = AtomicBool::new(false);
const DAY: i64 = 86_400;

/// Capture only action traffic, excluding discovery and the plan replay itself.
fn record(statement: ProfiledStatement) {
    if CAPTURING.load(Ordering::Relaxed) {
        CAPTURED.lock().expect("profiler mutex").push(statement);
    }
}

/// Drain the process-wide sink after all action threads have joined.
fn drain() -> Vec<ProfiledStatement> {
    std::mem::take(&mut *CAPTURED.lock().expect("profiler mutex"))
}

/// Keep read failures explicit without printing errors that can carry private SQL literals.
#[track_caller]
fn checked<T>(label: &str, read: ReadResult<T>) -> T {
    match read {
        Ok(value) => value,
        Err(error) => panic!(
            "benchmark read failed: {label}; variant={:?}; kind={:?}; code={:?}; private details withheld",
            std::mem::discriminant(&error),
            error.kind(),
            error.code()
        ),
    }
}

/// Replay diagnostics for one distinct expanded statement, never mixed into measured SQL time.
#[derive(Default)]
struct Plan {
    class: String,
    fullscan: i64,
    vm: i64,
    rows: usize,
}

/// Aggregate one exact statement across actions while retaining its independent replay counters.
#[derive(Default)]
struct Statement {
    ms: f64,
    calls: usize,
    plan: Plan,
}

/// One invocation; SQL counts and duplicates refer to this action only.
#[derive(Clone)]
struct Sample {
    wall: f64,
    sql: f64,
    statements: usize,
    duplicates: usize,
    rows: usize,
    texts: Vec<String>,
    degraded: bool,
}

/// First invocation and warm samples retained until every statement has been replayed.
struct Action {
    surface: String,
    scope: String,
    first: f64,
    warm: Sample,
    attached: bool,
    failures: usize,
}

/// Optional action-specific wall time, used for the maximum simultaneous reader latency.
struct Outcome {
    rows: usize,
    wall: Option<f64>,
}

impl From<usize> for Outcome {
    /// Ordinary actions use the enclosing timer.
    fn from(rows: usize) -> Self {
        Self { rows, wall: None }
    }
}

impl From<(usize, f64)> for Outcome {
    /// Contention reports the slowest synchronized reader rather than thread startup time.
    fn from((rows, wall): (usize, f64)) -> Self {
        Self {
            rows,
            wall: Some(wall),
        }
    }
}

/// Measure five invocations; warm time averages the middle two of the last four samples.
/// Counts and plans describe the upper middle sample; SQL time averages the same middle pair.
fn measure<T: Into<Outcome>>(
    actions: &mut Vec<Action>,
    statements: &mut HashMap<String, Statement>,
    surface: &str,
    scope: &str,
    mut body: impl FnMut() -> T,
) {
    let attached = db::open_reader().ok().is_some_and(|conn| {
        conn.query_row("SELECT count(*) FROM strat.strategies", [], |row| {
            row.get::<_, i64>(0)
        })
        .is_ok_and(|count| count > 0)
    });
    let mut samples = Vec::new();
    let mut failures = 0;
    for _ in 0..5 {
        drain();
        CAPTURING.store(true, Ordering::Relaxed);
        let start = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body));
        let failed = result.is_err();
        failures += usize::from(failed);
        let outcome = result
            .map(Into::into)
            .unwrap_or_else(|_| Outcome::from(0usize));
        let wall = outcome
            .wall
            .unwrap_or_else(|| start.elapsed().as_secs_f64() * 1_000.0);
        CAPTURING.store(false, Ordering::Relaxed);
        let captured = drain();
        let mut counts = HashMap::<String, usize>::new();
        let mut sql = 0.0;
        for item in &captured {
            let ms = item.duration.as_secs_f64() * 1_000.0;
            sql += ms;
            let key = if item.expanded {
                item.sql.clone()
            } else {
                // Placeholder-only captures cannot safely be replayed.
                format!("NOT_EXPANDED {}", item.sql)
            };
            *counts.entry(key.clone()).or_default() += 1;
            let entry = statements.entry(key).or_default();
            entry.ms += ms;
            entry.calls += 1;
        }
        samples.push(Sample {
            wall,
            sql,
            statements: captured.len(),
            duplicates: counts
                .iter()
                .filter(|(sql, _)| !sql.starts_with("NOT_EXPANDED "))
                .map(|(_, n)| n.saturating_sub(1))
                .sum(),
            rows: outcome.rows,
            texts: counts
                .into_iter()
                .flat_map(|(sql, count)| std::iter::repeat_n(sql, count))
                .collect(),
            degraded: failed || outcome.rows == 0 || !attached,
        });
    }
    let degraded = samples.iter().any(|sample| sample.degraded);
    let first = samples.remove(0).wall;
    samples.sort_by(|a, b| a.wall.total_cmp(&b.wall));
    let middle = samples.len() / 2;
    let wall = (samples[middle - 1].wall + samples[middle].wall) / 2.0;
    let sql = (samples[middle - 1].sql + samples[middle].sql) / 2.0;
    let mut warm = samples.remove(middle);
    warm.wall = wall;
    warm.sql = sql;
    warm.degraded = degraded;
    actions.push(Action {
        surface: surface.into(),
        scope: scope.replace("m1", "CURRENT(empty-rates fallback)"),
        first,
        warm,
        attached,
        failures,
    });
}

/// Match a plan's table or alias back to the five persistent tables under investigation.
fn target_scan(detail: &str, sql: &str) -> bool {
    let Some(scanned) = detail
        .strip_prefix("SCAN ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return false;
    };
    let tokens: Vec<_> = sql
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .filter(|token| !token.is_empty())
        .collect();
    [
        "orders_rep",
        "closed_sell_reports",
        "trade_values",
        "strategies",
        "strategy_versions",
    ]
    .iter()
    .any(|table| {
        if scanned.rsplit('.').next() == Some(*table) {
            return true;
        }
        tokens.iter().enumerate().any(|(index, token)| {
            if token.rsplit('.').next() != Some(*table) {
                return false;
            }
            let mut alias = tokens.get(index + 1).copied();
            if alias.is_some_and(|word| word.eq_ignore_ascii_case("AS")) {
                alias = tokens.get(index + 2).copied();
            }
            alias == Some(scanned)
        })
    })
}

/// Resolve production functions on a fresh reader, then fully step only read-only SQL.
fn replay(sql: &str) -> Plan {
    let normalized = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = normalized.to_ascii_uppercase();
    let strategy = [
        "FROM STRATEGIES",
        "FROM STRATEGY_VERSIONS",
        "JOIN STRATEGIES",
        "JOIN STRATEGY_VERSIONS",
    ]
    .iter()
    .any(|table| upper.contains(table));
    let reports = [
        "ORDERS_REP",
        "CLOSED_SELL_REPORTS",
        "TRADE_VALUES",
        "STRAT.",
        "VALUATION.",
    ]
    .iter()
    .any(|table| upper.contains(table));
    if !upper.starts_with("SELECT ") || (!strategy && !reports) || sql.starts_with("NOT_EXPANDED ")
    {
        return Plan {
            class: "UNSUPPORTED".into(),
            ..Default::default()
        };
    }
    let attempt = || -> Option<Plan> {
        let reader;
        let strategy_reader;
        let conn = if strategy && !reports {
            strategy_reader = rusqlite::Connection::open_with_flags(
                paths::strategies_db_path(),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .ok()?;
            &strategy_reader
        } else {
            reader = db::open_reader().ok()?;
            &*reader
        };
        if !strategy || reports {
            let axis = ReportAxis::load(conn, chrono_tz::UTC).ok()?;
            analytics::time_zone::install(conn, &axis).ok()?;
        }
        let mut statement = conn.prepare(sql).ok()?;
        if !statement.readonly() {
            return Some(Plan {
                class: "NOT_READONLY".into(),
                ..Default::default()
            });
        }
        let mut explain = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).ok()?;
        let details = explain
            .query_map([], |row| row.get::<_, String>(3))
            .ok()?
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let scan = details.iter().any(|line| target_scan(line, sql));
        let search = details.iter().any(|line| line.contains("SEARCH"));
        let temp = details.iter().any(|line| line.contains("TEMP B-TREE"));
        let class = [
            scan.then_some("SCAN"),
            search.then_some("SEARCH"),
            temp.then_some("TEMP_B-TREE"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("+");
        let mut count = 0;
        {
            let mut rows = statement.query([]).ok()?;
            while rows.next().ok()?.is_some() {
                count += 1;
            }
        }
        Some(Plan {
            class: if class.is_empty() {
                "OTHER".into()
            } else {
                class
            },
            fullscan: i64::from(statement.get_status(StatementStatus::FullscanStep)),
            vm: i64::from(statement.get_status(StatementStatus::VmStep)),
            rows: count,
        })
    };
    attempt().unwrap_or_else(|| Plan {
        class: "UNSUPPORTED".into(),
        ..Default::default()
    })
}

/// Remove all literal values and comments before exposing a SQL prefix in public output.
fn public_sql(sql: &str) -> String {
    let mut chars = sql.chars().peekable();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\'' || ch == '"' || ch == '`' || ch == '[' {
            let end = if ch == '[' { ']' } else { ch };
            out.push('?');
            while let Some(next) = chars.next() {
                if next == end {
                    if chars.peek() == Some(&end) {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        } else if ch.is_ascii_digit() {
            out.push('?');
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                chars.next();
            }
        } else if ch == '-' && chars.peek() == Some(&'-') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else {
            out.push(if ch.is_ascii() && !ch.is_control() {
                ch
            } else {
                ' '
            });
        }
    }
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect()
}

/// Discovery identities stay in memory; labels use only their rank by report row count.
struct Pool {
    cores: Vec<u64>,
    strategies: Vec<(i64, Option<u64>)>,
    coins: Vec<String>,
}

/// Discover the largest cores, strategies and five coins without printing persisted names.
fn discover() -> Pool {
    let conn = checked("discover.open_reader", db::open_reader());
    let cores = conn.prepare("SELECT core_uid FROM orders_rep GROUP BY core_uid ORDER BY count(*) DESC, core_uid LIMIT 5")
        .expect("core discovery").query_map([], |row| Ok(row.get::<_, i64>(0)? as u64))
        .expect("core discovery").collect::<Result<Vec<_>, _>>().expect("core discovery");
    let strategies = conn.prepare("SELECT strategyid, core_uid FROM orders_rep WHERE strategyid != 0 GROUP BY strategyid, core_uid ORDER BY count(*) DESC, core_uid, strategyid")
        .expect("strategy discovery").query_map([], |row| Ok((row.get(0)?, Some(row.get::<_, i64>(1)? as u64))))
        .expect("strategy discovery").collect::<Result<Vec<_>, _>>().expect("strategy discovery");
    let coins = conn.prepare("SELECT coin FROM orders_rep WHERE coin IS NOT NULL GROUP BY coin ORDER BY count(*) DESC, coin LIMIT 5")
        .expect("coin discovery").query_map([], |row| row.get(0))
        .expect("coin discovery").collect::<Result<Vec<_>, _>>().expect("coin discovery");
    Pool {
        cores,
        strategies,
        coins,
    }
}

/// Count rendered summary points, including honest split scopes without scalar payloads.
fn summary_rows(q: &Query, cores: bool) -> usize {
    let result = analytics::summary_data(q, cores);
    checked("summary_data.undated", result.undated);
    if let Some(cores) = result.cores {
        checked("summary_data.cores", cores);
    }
    checked("summary_data.data", result.data)
        .data()
        .map_or(0, |s| s.days.len())
}

/// Count strategy rows and check every independent result from the compound surface.
fn strategy_rows(q: &Query) -> usize {
    let result = analytics::strategy_base_data(q, true);
    checked("strategy_base_data.undated", result.undated);
    if let Some(cores) = result.cores {
        checked("strategy_base_data.cores", cores);
    }
    checked("strategy_base_data.data", result.data)
        .data()
        .map_or(0, |s| s.strategies.len())
}

/// Build one period/metric scope with the actual persisted core-clock correction.
fn query(to: i64, days: i64, metric: usize, axis: &ReportAxis) -> Query {
    Query {
        from: if days == 0 { -1 } else { to - days * DAY },
        to,
        axis: axis.clone(),
        metric: if metric == 2 {
            ProfitMetric::Percent
        } else {
            ProfitMetric::Quote
        },
        prefer_usdt: metric != 2,
        valuation: if metric == 1 {
            ValuationMode::Current
        } else {
            ValuationMode::Historical
        },
        ..Default::default()
    }
}

/// Exercise every Batch 1 surface without running the app or accessing a default data root.
#[test]
#[ignore = "requires a disposable data root in MOON_ANALYTICS_BENCH_DIR; run alone"]
fn real_data_read_bench() {
    // Windows canonicalization adds a verbatim prefix that SQLite file URIs cannot attach.
    let root = std::path::absolute(PathBuf::from(
        std::env::var_os("MOON_ANALYTICS_BENCH_DIR").expect("MOON_ANALYTICS_BENCH_DIR is required"),
    ))
    .expect("copy root must be absolute");
    assert!(root.is_dir(), "copy root must exist");
    assert!(
        paths::set_data_dir_override(root.clone()),
        "run this test alone"
    );
    // Check the side-effect-free path first: reports_db_path may migrate a flat root.
    assert!(paths::db_dir_path().starts_with(&root));
    assert!(
        paths::db_dir_path().is_dir(),
        "copy requires the platform database subdirectory; no files moved"
    );
    assert!(paths::reports_db_path().starts_with(&root));
    assert!(
        paths::reports_db_path().is_file(),
        "copy reports database is missing"
    );
    let _permit = db::report_recovery::prepare().expect("copy recovery lease unavailable");
    if std::env::var("MOON_ANALYTICS_BENCH_ANALYZE").as_deref() == Ok("1") {
        for path in [
            paths::reports_db_path(),
            paths::strategies_db_path(),
            paths::valuation_db_path(),
        ] {
            assert!(path.starts_with(&root), "ANALYZE must stay on the copy");
            let conn = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .expect("copy ANALYZE connection");
            conn.execute_batch("ANALYZE").expect("ANALYZE on copy");
        }
    }
    let pool = discover();
    assert!(
        !pool.cores.is_empty() && !pool.strategies.is_empty(),
        "copy must contain cores and strategies"
    );
    let conn = checked("setup.open_reader", db::open_reader());
    let axis = checked("setup.report_axis", ReportAxis::load(&conn, chrono_tz::UTC));
    drop(conn);
    let now = chrono::DateTime::from_timestamp(crate::util::now_unix_ms_i64().div_euclid(1000), 0)
        .expect("current timestamp");
    let to = now.timestamp().div_euclid(DAY) * DAY + DAY;
    assert!(
        trace::install_read_profiler(record),
        "run this test in a fresh process"
    );
    let mut actions = Vec::new();
    let mut statements = HashMap::new();
    macro_rules! bench {
        ($name:expr, $scope:expr, $body:expr) => {
            measure(&mut actions, &mut statements, $name, $scope, $body)
        };
    }
    println!(
        "N=5; FIRST is process-order first, not OS cold; WARM is median of four; periods end at next UTC midnight."
    );
    println!(
        "ROWS counts returned payload rows; split scopes can return zero scalar rows. Counts/plans use upper middle sample. SQL_MS is PROFILE time (1ms resolution); contention sums overlap."
    );
    bench!("open_reader", "all", || {
        checked("open_reader", db::open_reader());
        0
    });
    let basis_reader = checked("basis_probe.open_reader", db::open_reader());
    bench!("basis_probe", "test-only/uncached", || checked(
        "basis_probe",
        analytics::basis::margin_cores(&basis_reader)
    )
    .len());
    let probe = actions.last().expect("basis measurement").warm.clone();
    drop(basis_reader);
    for days in [1, 30, 365, 0] {
        for metric in 0..3 {
            let q = query(to, days, metric, &axis);
            for read_cores in [false, true] {
                bench!(
                    "summary_data",
                    &format!("D{days}/m{metric}/cores={read_cores}"),
                    || summary_rows(&q, read_cores)
                );
            }
        }
    }
    for metric in 0..3 {
        for (label, cores) in [
            ("all", vec![]),
            ("core#1", pool.cores[..1].to_vec()),
            ("C5", pool.cores.clone()),
        ] {
            for (side_label, side) in [
                ("all", SideFilter::All),
                ("long", SideFilter::Long),
                ("short", SideFilter::Short),
            ] {
                let mut q = query(to, 365, metric, &axis);
                q.cores = cores.clone();
                q.side = side;
                for read_cores in [false, true] {
                    bench!(
                        "summary_data",
                        &format!("D365/m{metric}/{label}/{side_label}/cores={read_cores}"),
                        || summary_rows(&q, read_cores)
                    );
                }
            }
        }
        for days in [365, 0] {
            for one in [false, true] {
                let mut q = query(to, days, metric, &axis);
                if one {
                    q.cores = pool.cores[..1].to_vec();
                }
                bench!(
                    "strategy_base_data",
                    &format!("D{days}/m{metric}/core#1={one}"),
                    || strategy_rows(&q)
                );
            }
        }
        for days in [30, 365, 1] {
            let mut q = query(to, days, metric, &axis);
            if days == 30 {
                q.from = now
                    .date_naive()
                    .with_day(1)
                    .expect("month start")
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_utc()
                    .timestamp();
            }
            let mut previous = q.clone();
            previous.to = q.from;
            previous.from -= q.to - q.from;
            bench!(
                "calendar_data",
                &format!(
                    "{}/m{metric}/comparison/hourly={}",
                    if days == 30 {
                        "MONTH".to_string()
                    } else {
                        format!("D{days}")
                    },
                    days == 1
                ),
                || {
                    let data = analytics::calendar_data(&q, Some(&previous), days == 1, true);
                    checked("calendar_data.undated", data.undated);
                    if let Some(cores) = data.cores {
                        checked("calendar_data.cores", cores);
                    }
                    checked("calendar_data.period", data.period)
                        .data()
                        .map_or(0, |period| period.current.len())
                }
            );
            let raw = actions.last().expect("Calendar measurement");
            let probe_queries = probe
                .texts
                .iter()
                .filter(|sql| sql.contains("spentbtc * lev / (boughtq * buyprice)"))
                .count();
            let calendar_queries = raw
                .warm
                .texts
                .iter()
                .filter(|sql| sql.contains("spentbtc * lev / (boughtq * buyprice)"))
                .count();
            let calls = if probe_queries == 0 {
                0.0
            } else {
                calendar_queries as f64 / probe_queries as f64
            };
            let mut warm = raw.warm.clone();
            warm.wall = (warm.wall - calls * probe.wall).max(0.0);
            warm.sql = (warm.sql - calls * probe.sql).max(0.0);
            actions.push(Action {
                surface: "calendar_production_cache_ESTIMATE".into(),
                scope: raw.scope.clone(),
                first: (raw.first - calls * probe.wall).max(0.0),
                warm,
                attached: raw.attached,
                failures: raw.failures,
            });
        }
    }
    let base = query(to, 365, 0, &axis);
    bench!("undated_closes", "D365", || {
        checked("undated_closes", analytics::undated_closes(&base));
        1
    });
    let variants = [tuner::Variant::default()];
    let field = tuner::FIELDS[0].col;
    for metric in [0, 2] {
        for one in [false, true] {
            let mut q = query(to, 365, metric, &axis);
            if one {
                q.strategies = pool.strategies[..1].to_vec();
            }
            let scope = format!("D365/m{metric}/strat#1={one}");
            bench!("filter_tuner_data", &scope, || {
                let data = tuner::filter_tuner_data(&q, &variants, field, 40);
                checked("filter_tuner_data.stats", data.stats);
                checked("filter_tuner_data.histogram", data.histogram).len()
            });
            bench!("variant_stats", &scope, || checked(
                "variant_stats",
                tuner::variant_stats(&q, &variants)
            )
            .len());
            bench!("histogram", &scope, || checked(
                "histogram",
                tuner::histogram(&q, field, 40)
            )
            .len());
            bench!("suggest_field", &scope, || usize::from(
                checked(
                    "suggest_field",
                    tuner::suggest_field(&q, field, 10, 40, true)
                )
                .is_some()
            ));
            if metric == 0 {
                bench!("time_tuner_data", &scope, || {
                    let data = tuner::time_tuner_data(&q, &variants);
                    checked("time_tuner_data.stats", data.stats);
                    checked("time_tuner_data.slider", data.slider);
                    checked("time_tuner_data.profiles", data.profiles).len()
                });
                bench!("suggest_time", &scope, || {
                    checked(
                        "suggest_time",
                        tuner::suggest_time(
                            &q,
                            10,
                            40,
                            true,
                            tuner::TimeAxes {
                                week: true,
                                day: true,
                                hour: true,
                                ..Default::default()
                            },
                        ),
                    );
                    1
                });
                bench!("coin_tuner_data", &scope, || {
                    let data = tuner::coin_tuner_data(&q, &variants, &base, &pool.coins);
                    checked("coin_tuner_data.kpi", data.kpi);
                    checked("coin_tuner_data.picked_strategies", data.picked_strategies);
                    checked("coin_tuner_data.stats", data.stats).len()
                });
                bench!("strategies_for_coins", &scope, || checked(
                    "strategies_for_coins",
                    analytics::strategies_for_coins(&q, &pool.coins)
                )
                .len());
            }
        }
    }
    for (label, strategies) in [
        ("strat#1", pool.strategies[..1].to_vec()),
        (
            "S50",
            pool.strategies[..pool.strategies.len().min(50)].to_vec(),
        ),
        ("all", vec![]),
    ] {
        let mut q = base.clone();
        q.strategies = strategies;
        bench!("read_deals", label, || checked(
            "read_deals",
            tuner::ticks::read_deals(&q)
        )
        .deals
        .len());
    }
    let pairs: Vec<_> = pool
        .strategies
        .iter()
        .map(|(id, core)| (*id, core.expect("discovered core")))
        .collect();
    bench!("strategy_kinds", "all", || tuner::strategy_kinds(&pairs)
        .len());
    let keys = [field.to_string()];
    bench!("live_strategies", "first-cache-miss/then-warm", || {
        tuner::live_strategies(&keys).len()
    });
    let param_keys = tuner::ticks::params::param_keys();
    for (label, selected) in [
        ("strat#1", pool.strategies[..1].to_vec()),
        (
            "S50",
            pool.strategies[..pool.strategies.len().min(50)].to_vec(),
        ),
        ("all", pool.strategies.clone()),
    ] {
        let mut q = base.clone();
        q.strategies = selected.clone();
        let deals = checked("entry_exit.read_deals", tuner::ticks::read_deals(&q)).deals;
        let mut own = HashMap::new();
        bench!("entry_exit_own_values_reads", label, || {
            own.clear();
            for deal in &deals {
                own.entry((deal.strategy_id, Some(deal.core_uid)))
                    .or_insert_with(|| {
                        tuner::strategy_current_values(
                            deal.strategy_id,
                            Some(deal.core_uid),
                            &param_keys,
                        )
                    });
            }
            own.values().map(HashMap::len).sum::<usize>()
        });
        bench!("entry_exit_now_values_missing_reads", label, || {
            selected
                .iter()
                .filter(|key| !own.contains_key(key))
                .map(|&(id, core)| tuner::strategy_current_values(id, core, &param_keys).len())
                .sum::<usize>()
        });
        bench!(
            "strategy_values_at",
            &format!("{label}/first-100-deals"),
            || {
                deals
                    .iter()
                    .take(100)
                    .filter_map(|deal| {
                        tuner::strategy_values_at(
                            deal.strategy_id,
                            Some(deal.core_uid),
                            deal.buy_ms,
                            &param_keys,
                        )
                    })
                    .map(|values| values.len())
                    .sum::<usize>()
            }
        );
        bench!("coin_lists", label, || {
            let lists = checked("coin_lists", db::coin_lists::coin_lists(&selected));
            lists.black.len() + lists.white.len()
        });
    }
    for days in [30, 365] {
        for metric in [0, 2] {
            let q = query(to, days, metric, &axis);
            let scope = format!("D{days}/m{metric}");
            bench!("profit_monitor_snapshot", &scope, || {
                let data = checked(
                    "profit_monitor_snapshot",
                    analytics::profit_monitor_snapshot(&q),
                );
                data.scope.data().map_or(0, |s| s.cores.len())
                    + data
                        .currencies
                        .iter()
                        .map(|c| c.data.cores.len())
                        .sum::<usize>()
            });
            bench!("query_totals_sliced", &scope, || {
                let conn = checked("query_totals_sliced.open_reader", db::open_reader());
                let filter = db::ReportFilter {
                    date_from: Some(q.from),
                    date_to: Some(q.to - 1),
                    rows: db::RowScope::Closed,
                    axis: q.axis.clone(),
                    valuation: q.valuation,
                    ..Default::default()
                };
                let slices: Vec<_> = (0..30)
                    .map(|i| db::TotalsSlice {
                        core_uids: None,
                        date_from: Some(to - (30 - i) * DAY),
                        date_to: Some(to - (29 - i) * DAY - 1),
                    })
                    .collect();
                checked(
                    "query_totals_sliced",
                    db::query_totals_sliced(&conn, &filter, &slices),
                )
                .len()
            });
        }
    }
    let (id, core) = pool.strategies[0];
    bench!("strategy_filters", "strat#1", || {
        tuner::strategy_filters(id, core, &HashMap::new());
        1
    });
    bench!("strategy_cores", "strat#1", || tuner::strategy_cores(id)
        .len());
    bench!("steady_state_repeat", "D365/summary+strategies/x10", || {
        (0..10)
            .map(|_| summary_rows(&base, true) + strategy_rows(&base))
            .sum::<usize>()
    });
    bench!("contention", "D365/8-threads/max-wall", || {
        let barrier = Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let start = Instant::now();
                        let rows = summary_rows(&base, true);
                        (start.elapsed(), rows)
                    })
                })
                .collect();
            let results: Vec<_> = handles
                .into_iter()
                .map(|h| h.join().expect("summary thread"))
                .collect();
            let max = results
                .iter()
                .map(|(wall, _)| wall.as_secs_f64() * 1000.0)
                .fold(0.0, f64::max);
            (results.iter().map(|(_, rows)| rows).sum::<usize>(), max)
        })
    });
    for (sql, statement) in &mut statements {
        statement.plan = replay(sql);
    }
    actions.sort_by(|a, b| b.warm.wall.total_cmp(&a.warm.wall));
    println!(
        "RANK SURFACE/ACTION SCOPE FIRST_MS WARM_MS SQL_MS NONSQL_MS STMTS DUPS WORST_PLAN FULLSCAN_STEPS ROWS STRATEGIES_ATTACHED RESULT FAILED_SAMPLES"
    );
    for (rank, action) in actions.iter().enumerate() {
        let plans: Vec<_> = action
            .warm
            .texts
            .iter()
            .map(|sql| &statements[sql].plan)
            .collect();
        let worst = plans.iter().max_by_key(|p| {
            (
                p.class.contains("SCAN"),
                p.class.contains("TEMP"),
                p.fullscan,
            )
        });
        println!(
            "{} {} {} {:.3} {:.3} {:.3} {:.3} {} {} {} {} {} {} {} {}",
            rank + 1,
            action.surface,
            action.scope,
            action.first,
            action.warm.wall,
            action.warm.sql,
            action.warm.wall - action.warm.sql,
            action.warm.statements,
            action.warm.duplicates,
            worst.map_or("NONE", |p| p.class.as_str()),
            plans.iter().map(|p| p.fullscan).sum::<i64>(),
            action.warm.rows,
            action.attached,
            if action.failures > 0 {
                "FAILED"
            } else if action.warm.degraded {
                "DEGRADED"
            } else {
                "ROWS>0"
            },
            action.failures
        );
    }
    let mut ranked: Vec<_> = statements.into_iter().collect();
    ranked.sort_by(|a, b| b.1.ms.total_cmp(&a.1.ms));
    if let Some(path) = std::env::var_os("MOON_ANALYTICS_BENCH_SQL_DUMP") {
        let path = std::path::absolute(PathBuf::from(path)).expect("SQL dump absolute path");
        let parent = path
            .parent()
            .expect("SQL dump parent")
            .canonicalize()
            .expect("SQL dump parent exists");
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        assert!(
            !parent.starts_with(repo),
            "SQL dump must be outside repository"
        );
        assert!(!path.exists(), "SQL dump must be a new local file");
        let dump: Vec<_> = ranked.iter().take(30).map(|(sql, _)| sql).collect();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("create private SQL dump");
        serde_json::to_writer_pretty(file, &dump).expect("write private SQL dump");
    }
    println!("TOP_STATEMENTS TOTAL_MS CALLS PLAN FULLSCAN_STEPS VM_STEPS REPLAY_ROWS SQL_PREFIX");
    for (sql, statement) in ranked {
        println!(
            "{:.3} {} {} {} {} {} {}",
            statement.ms,
            statement.calls,
            statement.plan.class,
            statement.plan.fullscan,
            statement.plan.vm,
            statement.plan.rows,
            public_sql(&sql)
        );
    }
    println!(
        "NOT MEASURED: run_ready_pass/count_strategies replay; resolve_addresses (UI crate); GPUI paint/layout; cx.notify rebuild"
    );
    println!(
        "m0=Quote+USDT/Historical CURRENT(empty-rates fallback)=offline Current m2=Percent D0=ALL; query_totals_sliced returns all native/USDT/percent totals in both metric-labelled runs."
    );
    println!(
        "Calendar production-cache ESTIMATE subtracts separately measured uncached basis cost per observed probe; counts/plans remain RAW. It is not a production-binary measurement."
    );
    println!(
        "DEGRADED means an empty payload, missing/empty strategies attachment, or a caught read failure; an empty optional result may be legitimate. Entry/Exit rows count returned parameter values; UI folding is NOT MEASURED."
    );
    assert!(
        actions.iter().all(|action| action.failures == 0),
        "read failures occurred; see DEGRADED rows above"
    );
}
