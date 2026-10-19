//! UI-default searches with an externally frozen holdout.
//!
//! Entry/Exit state defaults to 70% train (`ticks/state.rs::TICKS_DEFAULT_TRAIN`), unlike
//! By-filter's 100% (`filter/state.rs`). Both UI seed boxes are empty (random); this harness
//! pins seed 1 unless overridden. Corridor guard and entry screening are on, risk limits
//! unset, held/locked empty, exit varied and entry varied only for modelled kinds. Restarts
//! default to 10 and passes to DEFAULT_MAX_PASSES. The harness splits BEFORE fit filtering
//! by rounded 70% of the covered identity list; the search sees fit TRAIN deals only with
//! train_frac=1.0 (both revisions' train_split returns all rows). Its native holdout is thus
//! normally null. Harness holdout retains every covered later row, including unfit rows;
//! ab_deal records make those identities and fit flags explicit in both arms.
//! Fit train rows whose replay tail reaches the earliest holdout tape/decision stamp are
//! embargoed before the optimizer receives them; cohort labels remain unchanged.

use super::{compat, load::Loaded, out::Sink};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::search::{
    DEFAULT_MAX_PASSES, PreparedDeal, SearchParams, suggest_with_candidate, variant_picture,
};
use crate::db::tuner::ticks::{ModelSettings, entry_model_for};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;

/// Pick the largest fit group per requested kind, then one other factual-entry kind.
/// Ties use lower strategy id and then lower core uid; explicit picks preserve NEW's choices.
pub(super) fn pick(loaded: &[Loaded]) -> Vec<(u64, i64)> {
    if let Ok(spec) = std::env::var("MOON_TICKS_AB_PICKS") {
        return spec
            .split(',')
            .map(|p| {
                let (core, id) = p.trim().split_once(':').expect("AB pick core:id");
                (
                    core.parse().expect("AB pick core uid"),
                    id.parse().expect("AB pick strategy id"),
                )
            })
            .collect();
    }
    assert_eq!(
        compat::MODEL,
        "new",
        "OLD must reuse NEW's MOON_TICKS_AB_PICKS"
    );
    let mut groups = BTreeMap::<(String, i64, u64), usize>::new();
    for row in loaded.iter().filter(|r| r.fit) {
        *groups
            .entry((
                row.deal.kind.clone(),
                row.deal.strategy_id,
                row.deal.core_uid,
            ))
            .or_default() += 1;
    }
    let mut ranked: Vec<_> = groups.into_iter().collect();
    ranked.sort_by_key(|((_, id, core), n)| (std::cmp::Reverse(*n), *id, *core));
    let kinds = ["MoonShot", "MoonHook", "MoonStrike", "EMA"];
    let mut picks = Vec::new();
    for kind in kinds {
        if let Some(((_, id, core), _)) = ranked.iter().find(|((k, _, _), _)| k == kind) {
            picks.push((*core, *id));
        }
    }
    if let Some(((_, id, core), _)) = ranked
        .iter()
        .find(|((k, _, _), _)| !kinds.contains(&k.as_str()) && !entry_model_for(k))
    {
        picks.push((*core, *id));
    }
    picks
}

/// Read frozen identities and NEW's cohort labels; malformed files fail rather than widen scope.
pub(super) fn allowed_uids() -> Option<BTreeMap<(u64, i64), String>> {
    std::env::var_os("MOON_TICKS_AB_UIDS").map(|path| {
        std::io::BufReader::new(std::fs::File::open(path).expect("open AB identity list"))
            .lines()
            .map(|line| line.expect("read AB identity"))
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let row: Value = serde_json::from_str(line.trim()).expect("AB identity JSON");
                let cohort = row["cohort"].as_str().expect("AB identity cohort");
                assert!(matches!(cohort, "train" | "holdout"), "AB cohort label");
                (
                    (
                        row["core_uid"].as_u64().expect("AB identity core_uid"),
                        row["report_uid"].as_i64().expect("AB identity report_uid"),
                    ),
                    cohort.to_owned(),
                )
            })
            .collect()
    })
}

/// Prefer frozen labels even when missing identities would move the chronological count cut.
fn is_train(
    allowed: Option<&BTreeMap<(u64, i64), String>>,
    key: (u64, i64),
    index: usize,
    cut: usize,
) -> bool {
    allowed.map_or(index < cut, |labels| {
        labels.get(&key).expect("listed AB identity") == "train"
    })
}

/// Freeze a count-based boundary independent of model eligibility and native search embargoes.
fn cohort_cut(n: usize) -> usize {
    (n as f64 * 0.7).round() as usize
}

/// Identify every held-out row before fitting any A/B timing input.
pub(super) fn holdout_keys(
    loaded: &[Loaded],
    picks: &[(u64, i64)],
    allowed: Option<&BTreeMap<(u64, i64), String>>,
) -> HashSet<(u64, i64)> {
    let mut keys = HashSet::new();
    if let Some(labels) = allowed {
        keys.extend(
            labels
                .iter()
                .filter(|(_, label)| label.as_str() == "holdout")
                .map(|(&key, _)| key),
        );
    }
    for &(core, id) in picks {
        let mut group: Vec<_> = loaded
            .iter()
            .filter(|r| {
                r.deal.core_uid == core
                    && r.deal.strategy_id == id
                    && allowed.is_none_or(|labels| labels.contains_key(&(core, r.deal.report_uid)))
            })
            .collect();
        group.sort_by_key(|r| (r.deal.close_ms, r.deal.core_uid, r.deal.report_uid));
        for (i, row) in group.iter().enumerate() {
            let key = (core, row.deal.report_uid);
            if !is_train(allowed, key, i, cohort_cut(group.len())) {
                keys.insert(key);
            }
        }
    }
    keys
}

/// Equal boundary stamps share a replay price and must also be embargoed.
fn before_holdout(train_end: i64, holdout_start: Option<i64>) -> bool {
    holdout_start.is_none_or(|start| train_end < start)
}

/// Recompute metrics in exit order; identity tie-breaks make simultaneous exits reproducible.
pub(super) fn ordered_tally(mut rows: Vec<(i64, u64, i64, f64)>) -> Tally {
    rows.sort_by_key(|&(time, core, uid, _)| (time, core, uid));
    let mut tally = Tally::default();
    for (_, _, _, profit) in rows {
        tally.push(profit);
    }
    tally
}

/// Compact figures shared by native diagnostics and independent replay summaries.
fn tally_line(t: &Tally) -> Value {
    json!({"n":t.n,"profit":t.profit,"pf":t.profit_factor(),"max_dd":t.max_dd})
}

/// Replay a fixed slice and score closed outcomes, retaining nontrades in per-deal records.
fn replay(
    deals: &[PreparedDeal],
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
) -> (Tally, Vec<Value>) {
    let mut profits = Vec::new();
    let mut lines = Vec::new();
    for d in deals {
        let out = variant_picture(d, defaults, kind, values, ModelSettings::default()).outcome;
        let profit = out.profit_metric(&d.deal);
        if let Some((e, p)) = out.exit.zip(profit) {
            profits.push((e.t_ms, d.deal.core_uid, d.deal.report_uid, p));
        }
        lines.push(
            json!({"core_uid":d.deal.core_uid,"report_uid":d.deal.report_uid,
            "filled":out.fill.is_some(),"open":out.left_open(),"profit":profit,
            "exit_ms":out.exit.map(|e| e.t_ms)}),
        );
    }
    (ordered_tally(profits), lines)
}

/// Search one pick's fit training rows while keeping its later identities out of the optimizer.
pub(super) fn run_pick(
    loaded: &[Loaded],
    core: u64,
    id: i64,
    defaults: &HashMap<String, f64>,
    picks: &[(u64, i64)],
    allowed: Option<&BTreeMap<(u64, i64), String>>,
    sink: &mut Sink,
) {
    let mut group: Vec<_> = loaded
        .iter()
        .filter(|r| {
            r.deal.core_uid == core
                && r.deal.strategy_id == id
                && allowed.is_none_or(|set| set.contains_key(&(core, r.deal.report_uid)))
        })
        .collect();
    group.sort_by_key(|r| (r.deal.close_ms, r.deal.core_uid, r.deal.report_uid));
    let kind = group.first().map(|r| r.deal.kind.as_str()).unwrap_or("");
    let cut = cohort_cut(group.len());
    let cohorts: Vec<_> = group
        .iter()
        .enumerate()
        .map(|(i, r)| is_train(allowed, (core, r.deal.report_uid), i, cut))
        .collect();
    let n_train = cohorts.iter().filter(|&&train| train).count();
    sink.line(
        json!({"type":"ab_pick","core_uid":core,"strategy_id":id,"kind":kind,
        "picks":picks,"n":group.len(),"n_fit":group.iter().filter(|r| r.fit).count()}),
    );
    let mut deal_lines: Vec<_> = group
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({"type":"ab_deal","core_uid":core,"report_uid":r.deal.report_uid,
            "strategy_id":id,"kind":kind,"cohort":if cohorts[i] {"train"} else {"holdout"},"fit":r.fit})
        })
        .collect();
    let prepare = |r: &&Loaded| {
        super::super::search::prepared(&r.deal, &r.ticks, r.entry_line.as_deref(), &r.values)
    };
    let mut train: Vec<_> = group
        .iter()
        .enumerate()
        .filter(|(i, r)| cohorts[*i] && r.fit)
        .map(|(_, r)| prepare(r))
        .collect();
    let holdout: Vec<_> = group
        .iter()
        .enumerate()
        .filter(|(i, _)| !cohorts[*i])
        .map(|(_, r)| prepare(r))
        .collect();
    let holdout_start = holdout
        .iter()
        .map(|d: &PreparedDeal| {
            d.ticks
                .first()
                .map_or(d.deal.buy_ms, |t| (t.time_ms as i64).min(d.deal.buy_ms))
        })
        .min();
    let before = train.len();
    train.retain(|d| {
        before_holdout(
            d.deal.close_ms.saturating_add(d.trail_ms.max(0)),
            holdout_start,
        )
    });
    let embargoed = before - train.len();
    let mut result = json!({"type":"ab","core_uid":core,"strategy_id":id,"kind":kind,
        "picks":picks,"n":group.len(),"n_train":n_train,"n_holdout":holdout.len(),"n_search_train":train.len(),
        "embargoed":embargoed,"purged":null,"values":null,"search_train":null,"search_holdout":null,
        "train":null,"holdout":null,"base_holdout":null,"fact_holdout":null});
    if train.is_empty() {
        result["miss"] = json!("no fit training deals");
        for line in deal_lines {
            sink.line(line);
        }
        sink.line(result);
        return;
    }
    let grids = super::super::search::auto_grids(kind, &train[0].own, defaults);
    let restarts =
        std::env::var("MOON_TICKS_SEARCH_RESTARTS").map_or(10, |v| v.parse().expect("AB restarts"));
    let seed = std::env::var("MOON_TICKS_AB_SEED").map_or(1, |v| v.parse().expect("AB seed"));
    let held = HashMap::new();
    let locked = HashSet::new();
    let params = SearchParams {
        held: &held,
        defaults,
        kind,
        vary_entry: entry_model_for(kind),
        vary_exit: true,
        locked: &locked,
        grids: &grids,
        restarts,
        min_n: None,
        seed: Some(seed),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        model: ModelSettings::default(),
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: true,
    };
    result["seed"] = json!(seed);
    result["restarts"] = json!(restarts);
    result["search_train_frac"] = json!(1.0);
    let found = suggest_with_candidate(&train, &params, &SearchHandle::new());
    let base = replay(&holdout, defaults, kind, &[]).0;
    let fact = ordered_tally(
        holdout
            .iter()
            .map(|d| (d.deal.close_ms, core, d.deal.report_uid, d.deal.fact_pnl))
            .collect(),
    );
    result["base_holdout"] = tally_line(&base);
    result["fact_holdout"] = tally_line(&fact);
    match found.result {
        Ok(found) => {
            result["purged"] = json!(compat::purged_of(&found));
            result["values"] = json!(found.values);
            result["search_train"] = tally_line(&found.train);
            result["search_holdout"] = found.holdout.as_ref().map_or(Value::Null, tally_line);
            let (train_tally, train_lines) = replay(&train, defaults, kind, &found.values);
            let (holdout_tally, holdout_lines) = replay(&holdout, defaults, kind, &found.values);
            result["train"] = tally_line(&train_tally);
            result["holdout"] = tally_line(&holdout_tally);
            for line in train_lines.into_iter().chain(holdout_lines) {
                if let Some(target) = deal_lines
                    .iter_mut()
                    .find(|r| r["report_uid"] == line["report_uid"])
                {
                    target["replay"] = line;
                }
            }
            eprintln!(
                "{kind}: train {} holdout {} search answered",
                train.len(),
                holdout.len()
            );
        }
        Err(miss) => {
            result["miss"] = json!(format!("{miss:?}"));
            eprintln!(
                "{kind}: train {} holdout {} search refused",
                train.len(),
                holdout.len()
            );
        }
    }
    for line in deal_lines {
        sink.line(line);
    }
    sink.line(result);
}

#[cfg(test)]
mod tests;
