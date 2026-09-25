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
//! `MOON_TICKS_SEARCH_ALL=1` searches the whole exit rather than the Delta Modifiers section.

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
    DEFAULT_MAX_PASSES, SearchParams, clip_to_horizon, common_horizon_ms, suggest, variant_tally,
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
    for group in groups.into_iter().take(5) {
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
    let whole_exit = std::env::var_os("MOON_TICKS_SEARCH_ALL").is_some();
    let locked: HashSet<String> = TICK_PARAMS
        .iter()
        .filter(|f| !whole_exit && f.section != ParamSection::DeltaModifiers)
        .map(|f| f.key.to_string())
        .collect();
    let restarts = std::env::var("MOON_TICKS_SEARCH_RESTARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let held = HashMap::new();
    let legacy = std::env::var("MOON_TICKS_GRIDS").is_ok_and(|v| v == "legacy");
    let grids = if legacy {
        crate::db::tuner::ticks::search::test_grids::legacy().clone()
    } else {
        auto_grids(kind, &deals[0].own, defaults)
    };
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
