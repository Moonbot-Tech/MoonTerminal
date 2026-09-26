//! `MOON_TICKS_SEARCH=<kind>` of the real-data bench: the axis' search over the fit deals of each
//! of the kind's five strategies with the most of them — one strategy at a time, as the axis
//! searches a selection — the Delta Modifiers section alone — every other field locked at each strategy's own
//! value — so what the section can add over the strategies as they stand is read off a real
//! replica, with no window. `MOON_TICKS_SEARCH_RESTARTS` sets the restarts (10 by default).
//!
//! The grids are the axis' own automatic ranges (`params::range`) over the live strategies of
//! this machine and the searched strategy's values, cut into `MOON_TICKS_STEPS` steps (the
//! axis default when unset); `MOON_TICKS_GRIDS=legacy` searches the ladders the axis used before
//! 2026-09-25 instead, so the two can be held against each other on the same deals.
//! `MOON_TICKS_SEARCH_ALL=1` searches the whole exit rather than the Delta Modifiers section;
//! `MOON_TICKS_SEARCH_ENTRY` searches the Entry and Exit groups whole, alone or together
//! ([`run_groups`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::ParamKind;
use crate::db::tuner::ticks::params::ParamSection;
use crate::db::tuner::ticks::params::range::{
    Grids, Population, TickRange, field_span, resolve, steps_of,
};
pub(super) use crate::db::tuner::ticks::search::PreparedDeal;
use crate::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, SearchParams, check_corridors, clip_to_horizon, common_horizon_ms,
    point_cost, search_size, suggest, train_len, variant_picture, variant_tally,
};
use crate::db::tuner::ticks::{Deal, ModelSettings, TICK_PARAMS};
use crate::feed::types::Tick;

/// One fit deal as the search takes it: its tape, its entry line, and the values its strategy
/// held at the buy as the base — the app lays the strategy's CURRENT values instead.
pub(super) fn prepared(
    deal: &Deal,
    ticks: &[Tick],
    entry_line: Option<&[(i64, f64)]>,
    values: &HashMap<String, String>,
) -> PreparedDeal {
    let last_ms = ticks.last().map_or(deal.close_ms, |t| t.time_ms as i64);
    PreparedDeal {
        deal: deal.clone(),
        ticks: Arc::from(ticks),
        entry_line: entry_line.map(Arc::from),
        trail_ms: (last_ms - deal.close_ms).max(0),
        own: Arc::new(values.clone()),
    }
}

/// Run the search on each of the five strategies with the most deals.
pub(super) fn run(deals: Vec<PreparedDeal>, kind: &str, defaults: &HashMap<String, f64>) {
    let mut by_strategy: HashMap<(i64, u64), Vec<PreparedDeal>> = HashMap::new();
    for deal in deals {
        by_strategy
            .entry((deal.deal.strategy_id, deal.deal.core_uid))
            .or_default()
            .push(deal);
    }
    let mut groups: Vec<Vec<PreparedDeal>> = by_strategy.into_values().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    // `MOON_TICKS_SEARCH_TOP=<n>`: fewer strategies, for a search long enough to time on one.
    let top = std::env::var("MOON_TICKS_SEARCH_TOP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    for group in groups.into_iter().take(top) {
        run_one(group, kind, defaults);
    }
}

/// Run the search and print what it found, how long it took and how it went.
fn run_one(mut deals: Vec<PreparedDeal>, kind: &str, defaults: &HashMap<String, f64>) {
    if deals.is_empty() {
        eprintln!("search {kind}: no fit deal with a tape");
        return;
    }
    deals.sort_by_key(|d| d.deal.close_ms);
    if let Some(horizon) = common_horizon_ms(&deals) {
        clip_to_horizon(&mut deals, horizon);
    }
    let restarts = std::env::var("MOON_TICKS_SEARCH_RESTARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let legacy = std::env::var("MOON_TICKS_GRIDS").is_ok_and(|v| v == "legacy");
    let grids = if legacy {
        crate::db::tuner::ticks::search::test_grids::legacy().clone()
    } else {
        auto_grids(kind, &deals[0].own, defaults)
    };
    if let Ok(mode) = std::env::var("MOON_TICKS_SEARCH_ENTRY") {
        return run_groups(&deals, kind, defaults, &grids, restarts, &mode);
    }
    let whole_exit = std::env::var_os("MOON_TICKS_SEARCH_ALL").is_some();
    let locked: HashSet<String> = TICK_PARAMS
        .iter()
        .filter(|f| !whole_exit && f.section != ParamSection::DeltaModifiers)
        .map(|f| f.key.to_string())
        .collect();
    let held = HashMap::new();
    let params = SearchParams {
        held: &held,
        defaults,
        kind,
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: &grids,
        restarts,
        min_n: None,
        seed: Some(1),
        train_frac: 0.7,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings::default(),
        keep_corridor: false,
    };
    let started = Instant::now();
    let answer = suggest(&deals, &params, &SearchHandle::new());
    let elapsed = started.elapsed().as_millis();
    eprintln!(
        "search {kind} strategy {} on core {}: {} deal(s), {}, {restarts} restart(s), {} grids, {elapsed} ms",
        deals[0].deal.strategy_id,
        deals[0].deal.core_name,
        deals.len(),
        if whole_exit {
            "whole exit"
        } else {
            "Delta Modifiers only"
        },
        if legacy { "legacy" } else { "auto" }
    );
    let own = &deals[0].own;
    let section: Vec<(&str, &String)> = TICK_PARAMS
        .iter()
        .filter(|f| f.section == ParamSection::DeltaModifiers)
        .filter_map(|f| own.get(f.key).map(|v| (f.key, v)))
        .collect();
    eprintln!("  own Delta Modifiers: {section:?}");
    // The whole sample, the strategies as they stand against the answer.
    let whole = |values: &[(String, String)]| {
        let (tally, _) = variant_tally(&deals, defaults, kind, values, params.model);
        (tally.n, (tally.profit * 1000.0).round() / 1000.0)
    };
    eprintln!(
        "  as they stand, whole sample (n, profit): {:?}",
        whole(&[])
    );
    match answer {
        Ok(found) => eprintln!(
            "  found {:?}\n  whole sample {:?} · train n {} profit {:.3} · holdout {:?} · {:?}",
            found.values,
            whole(&found.values),
            found.train.n,
            found.train.profit,
            found
                .holdout
                .map(|t| (t.n, (t.profit * 1000.0).round() / 1000.0)),
            found.stats
        ),
        Err(miss) => eprintln!("  no answer: {miss:?}"),
    }
}

/// `MOON_TICKS_SEARCH_ENTRY=<mode>`: the whole Entry and Exit groups, every field free, as "Search
/// all" runs them — `entry`, `exit` or `both` from the strategy as it stands; `all` runs (a) the
/// entry, (b) the exit held over (a)'s answer and (c) both at once.
/// `MOON_TICKS_KEEP_CORRIDOR=0` drops the corridor guard the axis keeps on by default. The
/// search's own `[x] ticks search:` line — the base, the refusals — is printed beside each.
fn run_groups(
    deals: &[PreparedDeal],
    kind: &str,
    defaults: &HashMap<String, f64>,
    grids: &Grids,
    restarts: usize,
    mode: &str,
) {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Off)
        .filter_module(
            crate::diagnostics::TICKS_AXIS_TARGET,
            log::LevelFilter::Info,
        )
        .is_test(true)
        .try_init();
    eprintln!(
        "search {kind} strategy {} on core {}: {} deal(s), {restarts} restart(s), mode {mode}",
        deals[0].deal.strategy_id,
        deals[0].deal.core_name,
        deals.len()
    );
    let model = ModelSettings::default();
    let whole = |values: &[(String, String)]| {
        let (tally, _) = variant_tally(deals, defaults, kind, values, model);
        (tally.n, (tally.profit * 1000.0).round() / 1000.0)
    };
    // One training slice and one holdout for every point of the run: a search held over another's
    // answer drops the deals that answer leaves open (`closing::closable_at_base`) and cuts its
    // own slice, so the figures the searches report are not on the same deals.
    let closes: Vec<i64> = deals.iter().map(|d| d.deal.close_ms).collect();
    let (train, holdout) = deals.split_at(train_len(&closes, 0.7));
    // `variant_tally` drops a deal the point leaves open and holds no corridor rule, where the
    // search refuses such a point outright: the figure carries both counts, so a point the search
    // would refuse reads as one (`open`, `nearer`, `inverted` not all zero).
    let on = |slice: &[PreparedDeal], values: &[(String, String)]| {
        let (tally, _) = variant_tally(slice, defaults, kind, values, model);
        let open = slice
            .iter()
            .filter(|d| {
                variant_picture(d, defaults, kind, values, model)
                    .outcome
                    .left_open()
            })
            .count();
        let corridor = check_corridors(
            slice.iter().map(|d| (&d.deal, d.own.as_ref())),
            defaults,
            values,
            model,
        );
        format!(
            "(n {}, profit {:.3}, open {open}, nearer {}, inverted {})",
            tally.n, tally.profit, corridor.nearer, corridor.inverted
        )
    };
    eprintln!(
        "  as they stand, whole sample (n, profit): {:?}",
        whole(&[])
    );
    let step = |label: &str, held: &HashMap<String, String>, entry: bool, exit: bool| {
        let (answer, ms) = search_groups(deals, kind, defaults, grids, restarts, held, entry, exit);
        let found = match answer {
            Ok(found) => found.values,
            Err(miss) => {
                eprintln!("  {label}: no answer {miss:?}, {ms} ms");
                Vec::new()
            }
        };
        let mut merged = held.clone();
        merged.extend(found.iter().cloned());
        let mut merged: Vec<(String, String)> = merged.into_iter().collect();
        merged.sort();
        eprintln!(
            "  {label}: entry moved {:?} · fixed train {} holdout {} · {ms} ms",
            entry_fields(&found),
            on(train, &merged),
            on(holdout, &merged)
        );
        (found, merged, ms)
    };
    let none = HashMap::new();
    match mode {
        "entry" => drop(step("(a) entry", &none, true, false)),
        "exit" => drop(step("(exit) exit alone", &none, false, true)),
        "both" => drop(step("(c) both", &none, true, true)),
        "all" => {
            let (a_found, a, _) = step("(a) entry", &none, true, false);
            let (_, seq, _) = step("(b) exit over (a)", &a.into_iter().collect(), false, true);
            let (c, _, _) = step("(c) both", &none, true, true);
            // Is (c) a coordinate-wise optimum the sequence beats? The sequence's point, (c)'s,
            // and (c)'s exit with (a)'s entry — the entry move away from (c) the descent would
            // have to take — on the one training slice.
            let is_entry =
                |key: &str| !entry_fields(&[(key.to_string(), String::new())]).is_empty();
            let mut mixed: Vec<(String, String)> =
                c.iter().filter(|(k, _)| !is_entry(k)).cloned().collect();
            mixed.extend(a_found.iter().filter(|(k, _)| is_entry(k)).cloned());
            eprintln!(
                "  train: sequence (a)+(b) {} · (c) {} · (c)'s exit with (a)'s entry {}",
                on(train, &seq),
                on(train, &c),
                on(train, &mixed)
            );
        }
        other => {
            eprintln!("  unknown MOON_TICKS_SEARCH_ENTRY {other:?}: entry|exit|both|all")
        }
    }
}

/// One search of the chosen groups over `held`, every field of them free, and how long it took.
#[allow(clippy::too_many_arguments)]
fn search_groups(
    deals: &[PreparedDeal],
    kind: &str,
    defaults: &HashMap<String, f64>,
    grids: &Grids,
    restarts: usize,
    held: &HashMap<String, String>,
    vary_entry: bool,
    vary_exit: bool,
) -> (
    Result<
        crate::db::tuner::ticks::search::SearchResult,
        crate::db::tuner::ticks::search::SearchMiss,
    >,
    u128,
) {
    let locked = HashSet::new();
    let params = SearchParams {
        held,
        defaults,
        kind,
        vary_entry,
        vary_exit,
        locked: &locked,
        grids,
        restarts,
        min_n: None,
        seed: Some(1),
        train_frac: 0.7,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings::default(),
        keep_corridor: std::env::var("MOON_TICKS_KEEP_CORRIDOR").map_or(true, |v| v != "0"),
    };
    // What the axis would say before the run: the count and, at the measured cost of a point, the
    // time — held against what the run then took. `MOON_TICKS_SEARCH_DRY=1` stops there.
    let size = search_size(&params);
    let cost = point_cost(
        deals,
        defaults,
        kind,
        params.model,
        params.train_frac,
        params.restarts,
    );
    eprintln!(
        "    estimate: {:.0} point(s), {:.0} entry point(s), {:?} a point, ≈ {:.1} s",
        size.points,
        size.entry_points,
        cost,
        size.time(cost).as_secs_f64()
    );
    if std::env::var_os("MOON_TICKS_SEARCH_DRY").is_some() {
        return (Err(crate::db::tuner::ticks::search::SearchMiss::Nothing), 0);
    }
    let started = Instant::now();
    let answer = suggest(deals, &params, &SearchHandle::new());
    let ms = started.elapsed().as_millis();
    if let Ok(found) = &answer {
        eprintln!(
            "    actual: {} point(s) scored, {} entry point(s), {:.1} s, {:?} a scored point",
            found.stats.evaluations,
            found.stats.entry_points,
            ms as f64 / 1000.0,
            started.elapsed() / found.stats.evaluations.max(1) as u32
        );
    }
    match &answer {
        Ok(found) => eprintln!(
            "    found {:?}\n    train n {} profit {:.3} · holdout {:?} · {:?}",
            found.values,
            found.train.n,
            found.train.profit,
            found
                .holdout
                .as_ref()
                .map(|t| (t.n, (t.profit * 1000.0).round() / 1000.0)),
            found.stats
        ),
        Err(miss) => eprintln!("    no answer: {miss:?}"),
    }
    (answer, ms)
}

/// The Entry group's fields among a search's answer.
fn entry_fields(values: &[(String, String)]) -> Vec<&(String, String)> {
    values
        .iter()
        .filter(|(key, _)| {
            TICK_PARAMS.iter().any(|f| {
                f.key == key && f.group == crate::db::tuner::ticks::params::ParamGroup::Entry
            })
        })
        .collect()
}

/// The axis' automatic grids for one strategy: the live strategies of `kind` on this machine, the
/// strategy's own values as the selection, `MOON_TICKS_STEPS` steps per field.
fn auto_grids(kind: &str, own: &HashMap<String, String>, defaults: &HashMap<String, f64>) -> Grids {
    let deps = crate::feed::strategy_deps::FieldDeps::bundled();
    let numbers: Vec<&'static str> = TICK_PARAMS
        .iter()
        .filter(|f| f.kind == ParamKind::Num)
        .map(|f| f.key)
        .collect();
    let mut keys: Vec<String> = numbers.iter().map(|k| k.to_string()).collect();
    for key in &numbers {
        keys.extend(deps.conditions_of(key).map(str::to_string));
    }
    let live = crate::db::tuner::live_strategies(&keys);
    let population = Population::of(&live, defaults, &deps);
    let steps = steps_of(
        std::env::var("MOON_TICKS_STEPS")
            .ok()
            .and_then(|v| v.parse().ok()),
    );
    let kinds = [kind.to_string()];
    let mut grids = Grids::default();
    for key in numbers {
        let default = defaults.get(&key.to_ascii_lowercase()).copied();
        let selected: Vec<f64> = own
            .get(key)
            .and_then(|v| v.trim().replace(',', ".").parse::<f64>().ok())
            .or(default)
            .into_iter()
            .collect();
        let span = field_span(&population.values(&kinds, key), default, &selected);
        let points = resolve(span.as_ref(), &TickRange::default(), false, steps).points;
        if !points.is_empty() {
            grids.insert(key, points);
        }
    }
    grids
}
