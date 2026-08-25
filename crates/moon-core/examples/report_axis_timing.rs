//! Measure what the per-core time axis costs the Report's period filter, and prove it changes no rows.
//!
//! The Report's window predicate used to be one comparison against a bare `closedate`. With per-core
//! offsets it becomes one branch per distinct offset, each naming its cores. Two things have to be
//! established before that ships, and neither is provable by reading the SQL:
//!
//! 1. **Cost.** `EXPLAIN QUERY PLAN` naming `idx_rep_core_close` does NOT prove the query is still
//!    fast — it proves only that the planner found an index it may or may not lean on. The branch
//!    count, the `IN` lists and the shifted bounds all change what the planner actually does. So
//!    this measures wall-clock time on a replica of realistic size.
//! 2. **Parity.** The grouped shape must return EXACTLY the rows the ungrouped one did whenever the
//!    offsets do not actually differ. A predicate that is fast and wrong is worse than a slow one.
//!
//! The zero-measurement case is the honest baseline and needs no separate build to compare against:
//! with nothing measured, `append_row_scope` emits a single branch with no core guard and unshifted
//! bounds, which is character-for-character the predicate that existed before offsets. So the
//! "identity" row below IS the pre-change shape, measured by the same binary in the same run.
//!
//! Usage:
//!     cargo run --release -p moon-core --example report_axis_timing -- <data_dir> [repeats]
//!
//! Build the data root first with `tools/gen_replica.py`. It must be DISPOSABLE — the generator
//! recreates its database files, and this example writes offset segments into the replica.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use moon_core::db::{OffsetSegment, ReportAxis, ReportFilter, RowScope};

/// Run one closure `repeats` times and report its cold and warm-median wall-clock cost.
///
/// Args:
///     label: ASCII name printed in the report column.
///     repeats: At least two timed runs; the first is reported separately as the cold one.
///     body: Work under measurement.
///
/// Returns:
///     The warm median in milliseconds, for the ratio the caller prints.
fn time<T>(label: &str, repeats: usize, mut body: impl FnMut() -> T) -> f64 {
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
    println!("{label:<34} cold {cold:>8.1} ms   warm-median {median:>8.1} ms");
    median
}

/// Build an axis assigning `offset_secs` to each named core from the beginning of time.
///
/// One segment per core, starting at 0 so it applies to the whole replica: this measures the
/// PREDICATE's shape, and a segment boundary mid-history would add a second variable to a
/// measurement meant to isolate one.
///
/// Args:
///     assignments: Core uid and the offset in force for it.
///
/// Returns:
///     The axis those assignments describe, displayed in UTC.
fn axis_of(assignments: &[(u64, i32)]) -> ReportAxis {
    let mut measured: HashMap<u64, Vec<OffsetSegment>> = HashMap::new();
    for (core_uid, offset_secs) in assignments {
        measured.insert(
            *core_uid,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: *offset_secs,
            }],
        );
    }
    ReportAxis::from_measured(measured, chrono_tz::UTC)
}

/// The filter one open Report window issues for a bounded period.
///
/// Args:
///     axis: Time axis under measurement.
///     from: Inclusive period start in UTC seconds.
///     to: Inclusive period end in UTC seconds.
///
/// Returns:
///     A default filter differing from its siblings only in the axis.
fn filter_of(axis: ReportAxis, from: i64, to: i64) -> ReportFilter {
    ReportFilter {
        core_uids: Vec::new(),
        date_from: Some(from),
        date_to: Some(to),
        coin: String::new(),
        exact_coins: None,
        side: Default::default(),
        emulator: None,
        deleted_only: false,
        rows: RowScope::Closed,
        axis,
        strategies: None,
        strategy_name_mask: String::new(),
        valuation: Default::default(),
    }
}

/// Row identity used for the parity comparison.
///
/// Compares the KEYS rather than whole rows: the money columns travel through a valuation that has
/// its own cache and its own freshness, so a difference there would be noise from a different
/// subsystem, while a difference in WHICH rows came back is exactly what this is watching for.
///
/// Args:
///     table: One query result.
///
/// Returns:
///     Sorted `(core_uid, row_id)` pairs.
fn row_keys(table: &moon_core::db::ReportTable) -> Vec<(u64, i64)> {
    let mut keys: Vec<(u64, i64)> = table
        .core_uids
        .iter()
        .copied()
        .zip(table.rec_ids.iter().copied())
        .collect();
    keys.sort_unstable();
    keys
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: report_axis_timing <data_dir> [repeats]");
        std::process::exit(2);
    };
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

    let conn = match moon_core::db::open_reader() {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("open_reader failed: {error:?}");
            std::process::exit(1);
        }
    };
    let cores: Vec<u64> = match moon_core::db::distinct_cores(&conn) {
        Ok(cores) => cores.into_iter().map(|(uid, _)| uid).collect(),
        Err(error) => {
            eprintln!("distinct_cores failed: {error:?}");
            std::process::exit(1);
        }
    };
    println!("cores in replica: {}", cores.len());
    if cores.len() < 2 {
        eprintln!("this measurement needs at least two cores to form more than one group");
        std::process::exit(1);
    }

    let end = 1_780_000_000i64;
    let from = end - 365 * 86_400;

    // Three shapes over the same period. `identity` is the pre-change predicate; `uniform` proves a
    // fleet that happens to share one offset still collapses to one branch; `mixed` is the cost
    // this whole change actually adds.
    let identity = axis_of(&[]);
    let uniform: Vec<(u64, i32)> = cores.iter().map(|uid| (*uid, 3 * 3_600)).collect();
    let mixed: Vec<(u64, i32)> = cores
        .iter()
        .enumerate()
        .map(|(index, uid)| {
            let offset = match index % 4 {
                0 => 0,
                1 => -4 * 3_600,
                2 => 3 * 3_600,
                _ => 5 * 3_600 + 1_800,
            };
            (*uid, offset)
        })
        .collect();

    let limit = 5_000;
    let sort = "closedate";
    let run = |axis: ReportAxis| {
        moon_core::db::query_reports(&conn, &filter_of(axis, from, end), sort, true, limit)
    };

    println!("\n-- timing, one year, limit {limit} --");
    let base = time("identity (pre-change shape)", repeats, || {
        run(identity.clone())
    });
    let one_group = time("uniform offset, one group", repeats, || {
        run(axis_of(&uniform))
    });
    let many = time("mixed offsets, four groups", repeats, || {
        run(axis_of(&mixed))
    });
    println!(
        "\nratio vs pre-change shape:   uniform {:.2}x   mixed {:.2}x",
        one_group / base.max(0.001),
        many / base.max(0.001)
    );

    // An UNBOUNDED read cannot name its cores, so it pays for a trailing `core_uid NOT IN (...)`
    // catch-all branch on top of one branch per measured offset. A SCOPED read names them and pays
    // for neither. Measuring both attributes the cost to the right half: if scoping is close to the
    // baseline, the catch-all is what to attack; if it is not, the OR itself is.
    println!("\n-- attribution: same offsets, but the read NAMES its cores --");
    let scoped = |assignments: &[(u64, i32)]| {
        let mut f = filter_of(axis_of(assignments), from, end);
        f.core_uids = cores.clone();
        moon_core::db::query_reports(&conn, &f, sort, true, limit)
    };
    let scoped_uniform = time("uniform, cores named", repeats, || scoped(&uniform));
    let scoped_mixed = time("mixed, cores named", repeats, || scoped(&mixed));
    println!(
        "\nratio vs pre-change shape:   uniform {:.2}x   mixed {:.2}x",
        scoped_uniform / base.max(0.001),
        scoped_mixed / base.max(0.001)
    );

    println!("\n-- row parity --");
    let Ok(base_rows) = run(identity.clone()) else {
        eprintln!("baseline query failed");
        std::process::exit(1);
    };
    // Every core on the SAME offset shifts every bound by the same amount, so the window covers a
    // different span of core-local seconds and the row set legitimately MAY differ. What must not
    // differ is the zero case: measuring every core at zero has to reproduce the unmeasured result
    // exactly, because the two describe the same world.
    let all_zero: Vec<(u64, i32)> = cores.iter().map(|uid| (*uid, 0)).collect();
    let Ok(zero_rows) = run(axis_of(&all_zero)) else {
        eprintln!("all-zero query failed");
        std::process::exit(1);
    };
    let base_keys = row_keys(&base_rows);
    let zero_keys = row_keys(&zero_rows);
    println!("unmeasured rows:      {}", base_keys.len());
    println!("all-cores-zero rows:  {}", zero_keys.len());
    if base_keys == zero_keys {
        println!("PARITY OK: measuring every core at zero returns the identical row set");
    } else {
        println!("PARITY FAILED: the grouped predicate changed which rows come back");
        std::process::exit(1);
    }
}
