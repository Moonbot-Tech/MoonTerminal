//! Time the Analytics report reads against a throwaway synthetic replica.
//!
//! Measurement instrument for the "Analytics and the tuner take seconds to open" work: it
//! reproduces the exact read sequence one window open performs, so the wait can be attributed
//! to a named query instead of guessed at. The caller must supply a disposable data root;
//! `set_data_dir_override` makes this process resolve its data paths beneath that root.
//!
//! Usage:
//!     cargo run --release -p moon-core --example analytics_timing -- <data_dir> [repeats]
//!
//! Build the data root first with `tools/gen_replica.py`, which writes a synthetic
//! `data/reports.sqlite` and `data/strategies.sqlite` at a chosen row and core count.

use std::path::PathBuf;
use std::time::Instant;

use moon_core::db::analytics::{
    strategy_base_data, summary_data, undated_closes, GroupStat, Query,
};
use moon_core::db::ProfitScope;

/// Run one closure `repeats` times and print its best and median wall-clock cost.
///
/// Args:
///     label: ASCII name printed in the report column.
///     repeats: At least two timed runs; the first is reported separately as the cold one.
///     body: Work under measurement.
fn time<T>(label: &str, repeats: usize, mut body: impl FnMut() -> T) {
    let mut samples = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let started = Instant::now();
        let value = body();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        drop(value);
    }
    let cold = samples[0];
    let mut warm = samples[1..].to_vec();
    warm.sort_by(f64::total_cmp);
    let median = warm.get(warm.len() / 2).copied().unwrap_or(cold);
    println!("{label:<28} cold {cold:>8.1} ms   warm-median {median:>8.1} ms");
}

/// Build the query one open Analytics window issues for a whole-history period.
///
/// Args:
///     from: Inclusive period start in UTC seconds, or a negative all-history sentinel.
///     to: Exclusive period end in UTC seconds.
///
/// Returns:
///     The default-filter query every tab shares.
fn query(from: i64, to: i64) -> Query {
    Query {
        time_zone: chrono_tz::UTC,
        previous_period_basis: Default::default(),
        from,
        to,
        cores: Vec::new(),
        side: Default::default(),
        emulator: None,
        strategies: Vec::new(),
        metric: Default::default(),
        valuation: Default::default(),
        prefer_usdt: false,
    }
}

/// Time selected raw SQL shapes to attribute the cost of Analytics reads.
///
/// Args:
///     from: Inclusive period start, resolved from the all-history sentinel by the caller.
///     to: Exclusive period end.
///     name: ASCII period label.
///     repeats: Timed runs per shape.
fn probe_sql(from: i64, to: i64, name: &str, repeats: usize) {
    let conn = match moon_core::db::open_reader() {
        Ok(conn) => conn,
        Err(error) => {
            println!("  raw probes skipped: {error:?}");
            return;
        }
    };
    let from = if from < 0 { 1 } else { from };
    let period = "closedate >= ?1 AND closedate < ?2 AND closedate > 0 AND COALESCE(deleted,0) = 0";
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM orders_rep WHERE {period}"),
            rusqlite::params![from, to],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    println!("  rows in period: {count}");
    time(&format!("  raw COUNT(*) [{name}]"), repeats, || {
        let value: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM orders_rep WHERE {period}"),
                rusqlite::params![from, to],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        value
    });
    time(&format!("  raw group aggregate [{name}]"), repeats, || {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT CAST(strategyid AS TEXT) || '@' || CAST(core_uid AS TEXT) AS k,
                        SUM(profitbtc), COUNT(*)
                 FROM orders_rep WHERE {period} GROUP BY k ORDER BY 2 DESC, k"
            ))
            .expect("probe aggregate prepares");
        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| row.get::<_, i64>(2))
            .expect("probe aggregate runs")
            .count();
        rows
    });
    time(&format!("  raw full projection [{name}]"), repeats, || {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT closedate, buydate, profitbtc, spentbtc, core_uid, core_name, coin,
                        strategyid, isshort, emulator, basecurrency, boughtq, buyprice,
                        sellprice, sellreason
                 FROM orders_rep WHERE {period} ORDER BY closedate"
            ))
            .expect("probe projection prepares");
        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| row.get::<_, i64>(0))
            .expect("probe projection runs")
            .count();
        rows
    });
}

/// Print the compared fields of every strategy-base group row in source order.
///
/// The A-B side of this instrument: run it before and after a change to the enrichment path and
/// diff the two dumps. Order is printed as it arrives, so a reordering shows up as a diff too.
///
/// Args:
///     from: Inclusive period start, or the all-history sentinel.
///     to: Exclusive period end.
///     name: ASCII period label.
fn dump(from: i64, to: i64, name: &str) {
    let q = query(from, to);
    let read = strategy_base_data(&q, false);
    let line = |kind: &str, index: usize, group: &GroupStat| {
        println!(
            "{name}|{kind}|{index}|{}|{}|{}|{}|{}|{:?}|{}|{:.6}|{}|{:.6}|{:.6}|{:.6}|{}|{}|{}|{:.6}|{:?}",
            group.key,
            group.name,
            group.kind,
            group.core,
            group.cores_n,
            group.alive,
            group.n,
            group.profit,
            group.wins,
            group.pf,
            group.best,
            group.worst,
            group.lastedit,
            group.bl,
            group.wl,
            group.raw_profit,
            group.quote,
        );
    };
    match read.data {
        Ok(ProfitScope::Comparable { unit, data }) => {
            println!(
                "{name}|unit|{unit:?}|trades={}|{}|{}",
                data.trades, data.from, data.to
            );
            for (index, group) in data.strategies.iter().enumerate() {
                line("strategy", index, group);
            }
            for (index, group) in data.coins.iter().enumerate() {
                line("coin", index, group);
            }
        }
        Ok(ProfitScope::Empty(data)) => println!("{name}|empty|trades={}", data.trades),
        Ok(ProfitScope::Split(_)) => println!("{name}|split"),
        Err(error) => println!("{name}|error|{error:?}"),
    }
}

/// Run timing measurements or a deterministic group dump for the supplied disposable replica.
fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .expect("usage: analytics_timing <data_dir> [repeats]"),
    );
    let repeats = args
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(2);
    assert!(
        moon_core::config::paths::set_data_dir_override(root.clone()),
        "the data root must be installed before any path resolves"
    );
    // `open_reader` refuses without the process-lifetime lease the application takes at startup.
    let _permit = moon_core::db::report_recovery::prepare();
    println!("data root: {}", root.display());
    println!(
        "reports:   {}",
        moon_core::config::paths::reports_db_path().display()
    );

    let end = 1_780_000_000i64;
    let dump_only = std::env::args().any(|value| value == "--dump");
    let periods: [(&str, i64, i64); 3] = [
        ("month", end - 30 * 86_400, end),
        ("year", end - 365 * 86_400, end),
        ("all", -1, end),
    ];

    if dump_only {
        for (name, from, to) in periods {
            dump(from, to, name);
        }
        return;
    }

    for (name, from, to) in periods {
        println!("\n=== period: {name} ===");
        let q = query(from, to);
        let probe = summary_data(&q, false);
        println!(
            "  summary outcome: {}",
            match &probe.data {
                Ok(_) => "ok".to_string(),
                Err(error) => format!("{error:?}"),
            }
        );
        drop(probe);
        time(&format!("summary_data [{name}]"), repeats, || {
            summary_data(&q, false)
        });
        time(&format!("strategy_base_data [{name}]"), repeats, || {
            strategy_base_data(&q, false)
        });
        time(&format!("open then tuner [{name}]"), repeats, || {
            let first = summary_data(&q, false);
            let second = strategy_base_data(&q, false);
            (first, second)
        });
        time(&format!("undated_closes [{name}]"), repeats, || {
            undated_closes(&q)
        });
        probe_sql(from, to, name, repeats);
    }
}
