//! Cross-core variant accuracy and chronological search A/B, using one harness in both trees.
//!
//! Run each ignored test separately (one process and replica lease per test). First run NEW's
//! `real_data_variant_accuracy` and `real_data_search_ab`, then OLD's copies with the emitted
//! picks, then compare the JSON lines outside the repository. Only `compat.rs` differs;
//! OLD also needs `search::auto_grids` made `pub(super)` for this harness.
//! `MOON_TICKS_DATA_DIR` is a disposable snapshot root containing data and logs: opening caches
//! may prune retention. `MOON_TICKS_OUT` optionally appends JSON lines; `MOON_TICKS_SHA` labels
//! the measured revision. `MOON_TICKS_PAIR_MS` is the nonnegative pair window (default 3000).
//! `MOON_TICKS_AB_PICKS=core:id,...` reuses NEW's selected strategy/core groups in OLD.
//! `MOON_TICKS_PAIRS` freezes directed pairs from a file (`a_core:a_uid:b_core:b_uid` per line).
//! Run NEW first, export its pairs with compare.py --pairs-out, then run OLD with that file
//! and NEW's `MOON_TICKS_AB_PICKS`. After initial A/B runs, compare.py --uids-out exports
//! every shared identity with NEW's cohort labels and both fit flags; rerun BOTH arms with that
//! `MOON_TICKS_AB_UIDS` file to freeze the final A/B population and cohorts.
//! The UID file is JSON lines with `core_uid`, `report_uid` and `cohort` (train or holdout).
//! `MOON_TICKS_AB_SEED` fixes search randomness (default 1), and
//! `MOON_TICKS_SEARCH_RESTARTS` sets restarts (default 10). `MOON_TICKS_DEFAULTS` supplies
//! comma-separated numeric schema defaults (`key=value`), with lowercase keys.
//! The shared automatic-grid helper also reads `MOON_TICKS_STEPS`; leave it unset for UI
//! grid defaults, or hold it identical in both runs. Latency uses the bench default, without
//! latency environment overrides: all `MOON_TICKS_LATENCY_*` bench envs are ignored here.
//! Outputs contain identifiers, never display names or coins.
//! Variant timing uses the whole population: it is calibration, not training. A/B picks
//! and cohorts are frozen with zero lag before timing is fitted without any holdout rows.

use std::path::PathBuf;

mod ab;
mod compat;
mod load;
mod out;
mod pairs;
mod variant;

/// Pair admission window shared by matching and output provenance; malformed input is an error.
fn pair_ms() -> u64 {
    std::env::var("MOON_TICKS_PAIR_MS")
        .map_or(3000, |v| v.parse().expect("nonnegative pair window"))
}

/// Replay another core's settings on each admitted tape, retaining unfit rows in the population.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_variant_accuracy() {
    let root = std::env::var_os("MOON_TICKS_DATA_DIR").expect("MOON_TICKS_DATA_DIR");
    assert!(crate::config::paths::set_data_dir_override(PathBuf::from(
        root
    )));
    let _permit = crate::db::report_recovery::prepare().expect("reports replica lease unavailable");
    let defaults = load::defaults_from_env();
    let loaded = load::load_all(&defaults, true);
    let mut sink = out::Sink::new();
    sink.meta(pairs::RULE);
    for deal in &loaded {
        sink.line(variant::deal_line(deal));
    }
    let sides: Vec<_> = loaded.iter().map(pairs::Side::of).collect();
    let matched = if let Some(path) = std::env::var_os("MOON_TICKS_PAIRS") {
        use std::io::BufRead;
        let identities: std::collections::BTreeMap<_, _> = loaded
            .iter()
            .enumerate()
            .map(|(i, r)| ((r.deal.core_uid, r.deal.report_uid), i))
            .collect();
        std::io::BufReader::new(std::fs::File::open(path).expect("open frozen pairs"))
            .lines().filter_map(|line| {
                let line = line.expect("read frozen pair");
                let line = line.trim();
                if line.is_empty() { return None; }
                let fields: Vec<_> = line.split(':').collect();
                assert_eq!(fields.len(), 4, "pair a_core:a_uid:b_core:b_uid");
                let a = (fields[0].parse::<u64>().expect("pair A core"), fields[1].parse::<i64>().expect("pair A uid"));
                let b = (fields[2].parse::<u64>().expect("pair B core"), fields[3].parse::<i64>().expect("pair B uid"));
                if let (Some(&ai), Some(&bi)) = (identities.get(&a), identities.get(&b)) {
                    Some((ai, bi))
                } else {
                    sink.line(serde_json::json!({"type":"pair_missing", "a_core":a.0,"a_uid":a.1,"b_core":b.0,"b_uid":b.1}));
                    None
                }
            }).collect()
    } else {
        pairs::match_pairs(&sides, pair_ms())
    };
    let mut counts = std::collections::BTreeMap::<String, Vec<serde_json::Value>>::new();
    let mut unordered = std::collections::BTreeSet::new();
    for (a, b) in matched {
        unordered.insert((a.min(b), a.max(b)));
        let line = variant::score_pair(
            &loaded[a],
            &loaded[b],
            &defaults,
            pairs::differs(&sides[a], &sides[b], &defaults),
            pairs::entry_differs(&sides[a], &sides[b], &defaults),
        );
        counts
            .entry(loaded[a].deal.kind.clone())
            .or_default()
            .push(line.clone());
        sink.line(line);
    }
    eprintln!("{}", pairs::RULE);
    eprintln!("unordered pairs {}", unordered.len());
    for (kind, rows) in counts {
        let count = |key: &str| rows.iter().filter(|r| r[key] == true).count();
        let mut errors: Vec<_> = rows
            .iter()
            .filter_map(|r| r["profit_err_pp"].as_f64())
            .map(f64::abs)
            .collect();
        errors.sort_by(f64::total_cmp);
        let median = if errors.is_empty() {
            None
        } else {
            Some((errors[(errors.len() - 1) / 2] + errors[errors.len() / 2]) / 2.0)
        };
        eprintln!(
            "{kind}: pairs {} filled {} time_ok {} price_ok {} median_abs_profit_err {median:?}",
            rows.len(),
            count("filled"),
            count("exit_time_ok"),
            count("exit_price_ok")
        );
    }
    sink.finish();
}

/// Measure UI-style searches on each model's fit sample, with independently ordered metrics.
#[test]
#[ignore = "needs a live data root in MOON_TICKS_DATA_DIR"]
fn real_data_search_ab() {
    let root = std::env::var_os("MOON_TICKS_DATA_DIR").expect("MOON_TICKS_DATA_DIR");
    assert!(crate::config::paths::set_data_dir_override(PathBuf::from(
        root
    )));
    let _permit = crate::db::report_recovery::prepare().expect("reports replica lease unavailable");
    let defaults = load::defaults_from_env();
    let mut loaded = load::load_all(&defaults, false);
    let mut sink = out::Sink::new();
    sink.meta(pairs::RULE);
    let picks = ab::pick(&loaded);
    let allowed = ab::allowed_uids();
    let holdout = ab::holdout_keys(&loaded, &picks, allowed.as_ref());
    load::fit_ab_lags(&mut loaded, &holdout, &defaults);
    if let Some(labels) = &allowed {
        for (&(core, uid), cohort) in labels {
            if !loaded
                .iter()
                .any(|r| r.deal.core_uid == core && r.deal.report_uid == uid)
            {
                sink.line(serde_json::json!({"type":"ab_missing","core_uid":core,"report_uid":uid,"cohort":cohort}));
            }
        }
    }
    for &(core, id) in &picks {
        ab::run_pick(
            &loaded,
            core,
            id,
            &defaults,
            &picks,
            allowed.as_ref(),
            &mut sink,
        );
    }
    sink.finish();
}
