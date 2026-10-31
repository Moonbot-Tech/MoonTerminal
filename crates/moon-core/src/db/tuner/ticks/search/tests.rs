//! The search on a synthetic sample where the right answer is known.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::*;
use crate::db::tuner::ticks::Deltas;
use crate::feed::types::Side;

mod performance;
mod stopped;

/// A fact's covered look-back cannot license profit from a variant needing older history.
#[test]
fn longer_sell_level_variant_is_unjudged_without_profit() {
    let mut d = prepared(1, 104.0);
    d.deal.buy_ms = 0;
    d.deal.close_ms = 6_000;
    d.deal.sell_price = 105.0;
    d.deal.sell_reason = "Auto Price Down".into();
    d.ticks = Arc::from(vec![
        tick(-5_000, 105.0),
        tick(0, 100.0),
        tick(5_000, 100.0),
        tick(6_000, 105.0),
    ]);
    let own = ExitParams {
        sell_price_pct: 30.0,
        sell_level_delay_s: 5.0,
        sell_level_time_s: 10.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 0.0,
        sell_at_last_price: false,
        use_stop_loss: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let verdict = crate::db::tuner::ticks::verify::verify(
        &d.deal,
        &d.ticks,
        &EntryParams::Fact,
        &own,
        None,
        None,
    );
    assert!(
        crate::db::tuner::ticks::record::fit_for_search(&verdict),
        "{verdict:?}"
    );
    let variant = ExitParams {
        sell_level_time_s: 3600.0,
        ..own.clone()
    };
    d.own = Arc::new(HashMap::from([
        ("SellPrice".into(), "30".into()),
        ("SellLevelDelay".into(), "5".into()),
        ("SellLevelTime".into(), "10".into()),
        ("SellLevelCount".into(), "1".into()),
        ("SellLevelAdjust".into(), "0".into()),
        ("MShotSellAtLastPrice".into(), "NO".into()),
        ("UseStopLoss".into(), "NO".into()),
    ]));
    let deals = [d];
    let fact = scoring::score(&deals, &[0], &[(EntryParams::Fact, own)]);
    assert_eq!(fact.unjudged, 0);
    assert!(fact.spent > 0.0);
    let score = scoring::score(&deals, &[0], &[(EntryParams::Fact, variant)]);
    assert_eq!(score.unjudged, 1);
    assert_eq!(score.spent, 0.0);
    assert_eq!(score.open, 0);
    assert_eq!(score.open_profit, 0.0);
    assert_eq!(score.untraded, 0);
    let (column, money) = variant_tally_by_deal(
        &deals,
        &HashMap::new(),
        "PumpsDetection",
        &[("SellLevelTime".into(), "3600".into())],
        ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
    );
    assert_eq!(
        (column.unjudged, column.untraded, column.tally.n),
        (1, 0, 0)
    );
    assert_eq!(money, vec![(1, None)]);
    let picture = variant_picture(
        &deals[0],
        &HashMap::new(),
        "PumpsDetection",
        &[("SellLevelTime".into(), "3600".into())],
        ModelSettings::default(),
    );
    assert!(picture.outcome.fill.is_some());
    assert!(picture.outcome.exit.is_none());

    let mut searchable = deals[0].clone();
    Arc::make_mut(&mut searchable.own).insert("UseStopLoss".into(), "YES".into());
    let found = history_search(&[searchable], "SellLevelTime", &[10.0, 3600.0]);
    assert_eq!(found.train.n, 1);
    assert!(found.stats.unjudged_refused > 0);
    assert_eq!(found.stats.cornered, 0);
}

/// Search a single exit field so history regressions cannot hide behind unrelated moves.
fn history_search(deals: &[PreparedDeal], field: &'static str, values: &[f64]) -> SearchResult {
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove(field);
    let grids = Grids::of([(field, Arc::from(values))]);
    suggest(
        deals,
        &SearchParams {
            held: &held,
            defaults: &defaults,
            kind: "PumpsDetection",
            vary_entry: false,
            vary_exit: true,
            locked: &locked,
            grids: &grids,
            restarts: 1,
            min_n: Some(1),
            seed: Some(1),
            train_frac: 1.0,
            max_passes: DEFAULT_MAX_PASSES,
            keep_corridor: false,
            risk: Default::default(),
            screen_entry: false,
            model: ModelSettings {
                latency_ms: 0.0,
                ..Default::default()
            },
        },
        &SearchHandle::new(),
    )
    .expect("covered facts must retain a ranked answer")
}

/// Losing a single deal's exit must refuse the point instead of ranking its remaining wins.
#[test]
fn unjudged_loser_refuses_the_whole_search_point() {
    let mut loser = prepared(1, 100.0);
    loser.deal.buy_ms = 0;
    loser.deal.close_ms = 6_000;
    loser.deal.sell_price = 95.0;
    loser.deal.fact_pnl = -50.0;
    loser.ticks = Arc::from(vec![tick(0, 100.0), tick(5_000, 95.0), tick(6_000, 95.0)]);
    let winner = prepared(2, 104.0);
    let model = ModelSettings {
        latency_ms: 0.0,
        ..Default::default()
    };
    let unknown = ExitParams {
        sell_price_pct: 30.0,
        sell_level_delay_s: 5.0,
        sell_level_time_s: 3600.0,
        sell_level_count: 1,
        use_stop_loss: false,
        sell_at_last_price: false,
        model,
        ..Default::default()
    };
    let known = ExitParams {
        sell_price_pct: 1.0,
        use_stop_loss: false,
        sell_at_last_price: false,
        model,
        ..Default::default()
    };
    let deals = [loser, winner];
    let fills: Vec<_> = deals
        .iter()
        .map(|d| {
            Some(Fill {
                t_ms: d.deal.buy_ms,
                price: d.deal.buy_price,
            })
        })
        .collect();
    let mut replayed = closing::replay(
        &deals,
        &[0, 1],
        &[(EntryParams::Fact, unknown), (EntryParams::Fact, known)],
        &fills,
    );
    assert_eq!(
        (replayed.unjudged, replayed.closed.n, replayed.open),
        (1, 1, 0)
    );
    assert!(replayed.closed.profit > 0.0);
    assert!(replayed.closed_all().is_none());
    let tracker = candidate::Tracker::default();
    tracker.offer(&Point::new(), &replayed, 0, 1);
    assert!(tracker.answer().is_none());
    replayed.open = 1;
    tracker.offer(&Point::new(), &replayed, 0, 1);
    assert!(tracker.candidate().is_none());
}

/// Short formula history must not remove a take-closed fit fact from every search point.
#[test]
fn custom_ema_short_take_fact_keeps_a_ranked_search_answer() {
    let mut d = prepared(1, 104.0);
    d.deal.close_ms = 3_000;
    d.deal.sell_price = 104.0;
    d.ticks = Arc::from(vec![
        tick(1_000, 100.0),
        tick(2_000, 100.0),
        tick(3_000, 104.0),
    ]);
    let p = ExitParams {
        sell_price_pct: 4.0,
        sell_at_last_price: false,
        use_stop_loss: true,
        stop_loss_pct: -50.0,
        custom_ema: Some(Arc::new(
            super::super::custom_ema::parse("EMA(1h,1s) > 888").unwrap(),
        )),
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let verdict = crate::db::tuner::ticks::verify::verify(
        &d.deal,
        &d.ticks,
        &EntryParams::Fact,
        &p,
        None,
        None,
    );
    assert!(
        crate::db::tuner::ticks::record::fit_for_search(&verdict),
        "{verdict:?}"
    );
    d.own = Arc::new(HashMap::from([
        ("SellPrice".into(), "4".into()),
        ("StopLoss".into(), "-50".into()),
        ("UseStopLoss".into(), "YES".into()),
        ("MShotSellAtLastPrice".into(), "NO".into()),
        ("SellByCustomEMA".into(), "EMA(1h,1s) > 888".into()),
    ]));
    let found = history_search(&[d], "SellPrice", &[1.0, 4.0]);
    assert_eq!(found.train.n, 1);
    assert_eq!(found.stats.unjudged_refused, 0);
    assert!(found.train.profit > 0.0);
}

/// A non-invertible Spread fact shift cannot support a changed sell modifier.
#[test]
fn spread_variant_with_unknown_base_is_refused() {
    let mut d = prepared(1, 104.0);
    d.deal.kind = "Spread".into();
    d.deal.archived_take = Some(103.0);
    d.deal.deltas.d1h = 99.0;
    let own: HashMap<String, String> = [
        ("SellModifier".into(), "-1".into()),
        ("AddHourlyDelta".into(), "1".into()),
    ]
    .into();
    let defaults = HashMap::new();
    let model = ModelSettings::default();
    let exit = exit_params(
        &StrategyValues {
            values: &own,
            defaults: &defaults,
        },
        model,
    );
    d.deal.stop_anchor = Some(crate::db::tuner::ticks::StopAnchor::of(
        &d.deal, &exit, None,
    ));
    assert_eq!(d.deal.stop_anchor.unwrap().take_shift_pct, -99.0);
    assert_eq!(
        check_corridors([(&d.deal, &own)], &defaults, &[], model).take_unknown,
        0
    );
    let variant = [("SellModifier".into(), "0".into())];
    assert_eq!(
        check_corridors([(&d.deal, &own)], &defaults, &variant, model).take_unknown,
        1
    );
    let variant = ExitParams {
        sell_modifier: 0.0,
        ..exit
    };
    assert_eq!(
        unknown_takes(&[d], &[0], &[(EntryParams::Fact, variant)]),
        1
    );
}

/// A fact-calibrated hook cannot lend a percent fallback to search or hand-typed variants.
#[test]
fn unknown_hook_variant_is_refused_before_scoring() {
    let mut d = prepared(1, 104.0);
    d.deal.kind = "MoonHook".into();
    d.deal.hook_depth_pct = Some(4.0);
    d.deal.hook_stated_take_pct = Some(2.0);
    d.deal.sell_price = 102.0;
    d.own = Arc::new(
        [
            ("HookSellLevel".into(), "50".into()),
            ("HookSellFixed".into(), "NO".into()),
            ("StopLoss".into(), "-50".into()),
        ]
        .into(),
    );
    let defaults = HashMap::new();
    let model = ModelSettings {
        latency_ms: 0.0,
        ..Default::default()
    };
    let own = exit_params(
        &StrategyValues {
            values: &d.own,
            defaults: &defaults,
        },
        model,
    );
    let verdict = crate::db::tuner::ticks::verify::verify(
        &d.deal,
        &d.ticks,
        &EntryParams::Fact,
        &own,
        None,
        None,
    );
    assert!(
        crate::db::tuner::ticks::record::fit_for_search(&verdict),
        "{verdict:?}"
    );
    let variant = vec![("HookSellLevel".into(), "100".into())];
    let (manual, per_deal) = variant_tally_by_deal(
        std::slice::from_ref(&d),
        &defaults,
        "MoonHook",
        &variant,
        model,
    );
    assert_eq!(manual.unjudged, 1);
    assert_eq!(manual.tally.n, 0);
    assert_eq!(manual.untraded, 0);
    assert_eq!(manual.open, 0);
    assert_eq!(per_deal, vec![(d.deal.report_uid, None)]);
    let scored = variant_tally(
        std::slice::from_ref(&d),
        &defaults,
        "MoonHook",
        &variant,
        model,
    );
    assert_eq!(scored.unjudged, 1);
    assert_eq!(scored.tally.n, 0);
    assert_eq!(scored.untraded, 0);
    let picture = variant_picture(&d, &defaults, "MoonHook", &variant, model);
    assert!(picture.outcome.fill.is_some());
    assert_eq!(picture.outcome.exit, None);
    assert_eq!(picture.outcome.profit_pct, None);
    assert!(picture.sell_line.is_empty());
    let fact = variant_tally(std::slice::from_ref(&d), &defaults, "MoonHook", &[], model);
    assert_eq!(fact.unjudged, 0);
    assert_eq!(fact.tally.n, 1);
    let check = check_corridors([(&d.deal, d.own.as_ref())], &defaults, &variant, model);
    assert_eq!(check.take_unknown, 1);
    assert_eq!(
        check_corridors([(&d.deal, d.own.as_ref())], &defaults, &[], model).take_unknown,
        0
    );
    let mut grids = Grids::default();
    grids.insert("HookSellLevel", Arc::from([100.0]));
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("HookSellLevel");
    let held = HashMap::new();
    let found = suggest(
        &[d],
        &SearchParams {
            held: &held,
            defaults: &defaults,
            kind: "MoonHook",
            vary_entry: false,
            vary_exit: true,
            locked: &locked,
            grids: &grids,
            restarts: 1,
            min_n: Some(1),
            seed: Some(1),
            train_frac: 1.0,
            max_passes: DEFAULT_MAX_PASSES,
            keep_corridor: false,
            risk: Default::default(),
            screen_entry: false,
            model,
        },
        &SearchHandle::new(),
    );
    assert!(matches!(found, Err(SearchMiss::Corridor)), "{found:?}");
}

/// Search only the take on a chronological half sample, exposing embargo selection regressions.
fn embargo_search(deals: &[PreparedDeal]) -> SearchResult {
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    suggest(
        deals,
        &SearchParams {
            held: &held,
            defaults: &defaults,
            kind: "PumpsDetection",
            vary_entry: false,
            vary_exit: true,
            locked: &locked,
            grids: test_grids::legacy(),
            restarts: 1,
            min_n: Some(1),
            seed: Some(1),
            train_frac: 0.5,
            max_passes: DEFAULT_MAX_PASSES,
            keep_corridor: false,
            risk: Default::default(),
            screen_entry: false,
            model: ModelSettings {
                latency_ms: 0.0,
                ..Default::default()
            },
        },
        &SearchHandle::new(),
    )
    .expect("an embargo search answer")
}

/// Keeping a training replay's held tail would leak holdout prices into the fact baseline.
#[test]
fn search_purges_training_deal_whose_tape_reaches_holdout() {
    let mut deals: Vec<_> = [10, 99, 110, 120]
        .into_iter()
        .map(|uid| prepared(uid, 101.0))
        .collect();
    deals[1].deal.close_ms = 100_000;
    deals[1].trail_ms = 60_000;
    let found = embargo_search(&deals);
    assert_eq!(
        found.fact_train.n, 1,
        "two chronological training deals minus one overlapping tail"
    );
    assert_eq!(found.purged, 1);
    assert_eq!(found.fact_holdout.unwrap().n, 2);
}

/// A large overlapping winner must not choose a take that loses on the unseen-safe training deal.
#[test]
fn search_purge_changes_the_take_chosen_by_a_leaked_winner() {
    let mut deals: Vec<_> = [10, 99, 110, 120]
        .into_iter()
        .map(|uid| prepared(uid, 101.0))
        .collect();
    let t0 = deals[0].deal.buy_ms;
    deals[0].ticks = Arc::from(vec![
        tick(t0 - 500, 100.0),
        tick(t0, 100.0),
        tick(t0 + 300, 100.4),
        tick(t0 + 900, 99.0),
        tick(t0 + 5_000, 99.0),
    ]);
    deals[0].deal.close_ms = t0 + 5_000;
    deals[0].own = Arc::new(
        [
            ("SellPrice".into(), "0.2".into()),
            ("StopLoss".into(), "-0.5".into()),
        ]
        .into(),
    );
    deals[1].deal.close_ms = 100_000;
    deals[1].trail_ms = 60_000;
    deals[1].deal.spent = 100_000.0;
    let found = embargo_search(&deals);
    assert_eq!(found.values, vec![("SellPrice".into(), "0.4".into())]);
}

/// A strict comparison would discard a safe boundary, while ignoring run-up leaks MoonShot prices.
#[test]
fn purge_keeps_deals_ending_before_holdout_entry() {
    let mut deals: Vec<_> = [10, 99, 110]
        .into_iter()
        .map(|uid| prepared(uid, 101.0))
        .collect();
    let embargo = deals[2].deal.buy_ms - super::super::RUN_UP_MS;
    deals[0].deal.close_ms = embargo;
    deals[1].deal.close_ms = embargo + 1;
    deals[1].ticks = Arc::from([]);
    deals[2].deal.kind = "MoonShot".into();
    deals[2].ticks = Arc::from(vec![tick(embargo, 100.0)]);
    assert_eq!(purged_train(&deals, 2), vec![0]);
    assert_eq!(
        purged_train(&deals, 3),
        vec![0, 1, 2],
        "no holdout means no embargo"
    );
}

/// Ignoring the run-up before order creation would admit training tape used by holdout decisions.
#[test]
fn purge_starts_at_holdout_order_delta_evaluation() {
    let mut deals = vec![prepared(10, 101.0), prepared(110, 101.0)];
    deals[1].deal.buy_set_ms = Some(100_000);
    deals[1].ticks = Arc::from(vec![tick(110_000, 100.0)]);
    deals[0].deal.close_ms = 105_000 - super::super::RUN_UP_MS;
    deals[0].ticks = Arc::from([]);
    assert!(purged_train(&deals, 1).is_empty());
}

/// Truncating fractional prints would admit overlapping training and holdout tape.
#[test]
fn purge_rounds_fractional_tape_bounds_outward() {
    let mut deals = vec![prepared(10, 101.0), prepared(110, 101.0)];
    deals[0].deal.close_ms = 1_000;
    let mut last = tick(1_000, 100.0);
    last.time_ms = 1_000.7;
    deals[0].ticks = Arc::from(vec![last]);
    let mut first = tick(1_000, 100.0);
    first.time_ms = 1_000.3;
    deals[1].ticks = Arc::from(vec![first]);
    assert!(purged_train(&deals, 1).is_empty());
}

/// The earliest holdout tape or delta evaluation must embargo every training replay source.
#[test]
fn purge_uses_all_holdout_starts_and_training_window_ends() {
    let mut deals: Vec<_> = [10, 20, 110, 120]
        .into_iter()
        .map(|uid| prepared(uid, 101.0))
        .collect();
    deals[0].trail_ms = 19_000;
    deals[1].ticks = Arc::from(vec![tick(60_001, 100.0)]);
    deals[3].deal.buy_set_ms = Some(60_000);
    assert_eq!(purged_train(&deals, 2), vec![0]);
    deals[2].ticks = Arc::from(vec![tick(20_000, 100.0)]);
    assert!(purged_train(&deals, 2).is_empty());
    deals[2].ticks = Arc::from([]);
    deals[3].deal.buy_set_ms = Some(130_000);
    assert_eq!(
        purged_train(&deals, 2),
        vec![0, 1],
        "invalid order creation falls back to buy"
    );
}

pub(super) fn tick(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A PumpsDetection deal (entry from the fact, take by `SellPrice`) bought at 100 whose tape peaks at `peak` after the
/// fill, then falls back to the fact's exit.
pub(super) fn prepared(uid: i64, peak: f64) -> PreparedDeal {
    let deal = Deal {
        report_uid: uid,
        core_uid: 1,
        core_name: "CORE_A".into(),
        strategy_id: 1,
        kind: "PumpsDetection".into(),
        coin: "XALPHA".into(),
        buy_ms: 1_000 * uid,
        close_ms: 1_000 * uid + 900,
        buy_price: 100.0,
        sell_price: 100.2,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Sell Price".into(),
        fact_pnl: 2.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        quotes: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    };
    let t0 = deal.buy_ms;
    let ticks: Vec<Tick> = vec![
        tick(t0 - 500, 100.0),
        tick(t0, 100.0),
        tick(t0 + 300, peak),
        tick(t0 + 600, 100.2),
        tick(t0 + 900, 100.2),
    ];
    PreparedDeal {
        deal,
        ticks: Arc::from(ticks),
        entry_line: None,
        trail_ms: 0,
        own: Arc::new(base()),
    }
}

/// A deal of `prepared` whose own strategy takes at `take` per cent.
fn with_take(uid: i64, take: &str) -> PreparedDeal {
    let mut deal = prepared(uid, 101.0);
    deal.own = Arc::new(
        [("SellPrice", take), ("StopLoss", "-50")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    deal
}

/// The stamp of a tape's last print past the deal's close.
fn tape_end_ms(d: &PreparedDeal) -> i64 {
    d.ticks
        .last()
        .map(|t| (t.time_ms as i64) - d.deal.close_ms)
        .unwrap_or(0)
}

fn base() -> HashMap<String, String> {
    [("SellPrice", "0.2"), ("StopLoss", "-50")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_search_raises_the_take_to_what_every_tape_reaches() {
    // Every deal peaks at 101.0: a take of 1 % fills on all of them; 1.2 % on none.
    let deals: Vec<PreparedDeal> = (1..=8).map(|uid| prepared(uid, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS
        .iter()
        .filter(|f| f.group == ParamGroup::Exit)
        .map(|f| f.key.to_string())
        .collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 3,
        min_n: Some(4),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let handle = SearchHandle::new();
    let result = suggest(&deals, &params, &handle).expect("a result");
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "1".to_string())],
        "{result:?}"
    );
    assert_eq!(result.train.n, 8);
    assert!(
        (result.train.profit - 80.0).abs() < 1e-6,
        "{}",
        result.train.profit
    );
    assert!(result.holdout.is_none());
    assert_eq!(handle.completed(), 3);
    // The run's own account: every restart finished, each within the pass limit, and at least
    // one point was scored per restart.
    let stats = result.stats;
    assert_eq!(stats.restarts, 3);
    assert!(stats.best_restart < 3);
    assert!(stats.converged && stats.passes >= 1 && stats.passes <= DEFAULT_MAX_PASSES);
    assert!((1..=3).contains(&stats.distinct));
    assert!(stats.evaluations >= 3);
    // The same values through the variant column.
    let VariantScore { tally, spent, .. } = variant_tally(
        &deals,
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert!((tally.profit - 80.0).abs() < 1e-6);
    assert!((spent - 8_000.0).abs() < 1e-6);
    // And one deal of it as the trade pane draws it: the same parameters, the same replay — the
    // entry at the fact, the take at 1 % on the peak.
    let picture = variant_picture(
        &deals[0],
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert!(picture.corridor.is_empty(), "no entry model, no corridor");
    let outcome = picture.outcome;
    assert_eq!(outcome.fill.map(|f| f.t_ms), Some(deals[0].deal.buy_ms));
    let exit = outcome.exit.expect("an exit");
    assert_eq!(exit.kind, crate::db::tuner::ticks::ExitKind::Take);
    assert!((exit.price - 101.0).abs() < 1e-6, "{exit:?}");
    assert!((outcome.profit_pct.expect("a trade") - 1.0).abs() < 1e-6);
    // The sell's path from the fill: placed no earlier than the fill, standing at the take the
    // exit closed on when it closed.
    let fill_ms = outcome.fill.expect("a fill").t_ms;
    let first = picture.sell_line.first().expect("a sell line");
    assert!(first.t_ms >= fill_ms, "{:?}", picture.sell_line);
    let last = picture
        .sell_line
        .iter()
        .rev()
        .find(|point| point.t_ms <= exit.t_ms)
        .expect("a level at the close");
    assert!(
        (last.price - exit.price).abs() < 1e-6,
        "{:?}",
        picture.sell_line
    );
    // And the deal table's plan column: every deal's money, summing to the column's tally.
    let (
        VariantScore {
            tally: by_tally,
            spent: by_spent,
            ..
        },
        money,
    ) = variant_tally_by_deal(
        &deals,
        &defaults,
        "PumpsDetection",
        &result.values,
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert_eq!(by_tally.n, tally.n);
    assert!((by_tally.profit - tally.profit).abs() < 1e-9);
    assert!((by_spent - spent).abs() < 1e-9);
    assert_eq!(money.len(), 8);
    assert!(money.iter().all(|(_, m)| {
        m.is_some_and(|(money, pct)| (money - 10.0).abs() < 1e-6 && (pct - 1.0).abs() < 1e-6)
    }));
}

#[test]
fn the_holdout_is_scored_but_never_fitted_on() {
    // The first six deals peak at 101, the last two at 100.5: fitted on the first 75 %, the
    // search picks 1 %, which the holdout then fails to reach.
    let deals: Vec<PreparedDeal> = (1..=6)
        .map(|uid| prepared(uid * 60, 101.0))
        .chain((7..=8).map(|uid| prepared(uid * 60, 100.5)))
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 1,
        min_n: Some(3),
        seed: Some(1),
        train_frac: 0.75,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let handle = SearchHandle::new();
    let result = suggest(&deals, &params, &handle).expect("a result");
    assert_eq!(result.values[0].1, "1");
    assert_eq!(result.train.n, 6);
    let holdout = result.holdout.expect("a holdout");
    assert_eq!(holdout.n, 0, "neither held-back deal reaches 1 %");
    // search.rs `suggest`: the fact is tallied over the same two slices — six fitted deals and
    // two held back at 2.0 each — and an answer that leaves held-back deals open loses there.
    assert_eq!(result.fact_train.n, 6);
    assert!((result.fact_train.profit - 12.0).abs() < 1e-9);
    let fact_holdout = result.fact_holdout.expect("a fact holdout");
    assert_eq!(fact_holdout.n, 2);
    assert!((fact_holdout.profit - 4.0).abs() < 1e-9);
    assert_eq!(result.holdout_open, 2);
    assert!(result.holdout_loses);
}

/// search/scoring.rs `VariantScore::push`: a deal the variant bought and left open, or never traded, is
/// counted beside the tally instead of silently dropping out — dropping it would let a variant
/// that leaves its losers open read better than the fact over fewer deals.
#[test]
fn a_variant_accounts_for_every_deal_the_fact_is_tallied_over() {
    // Three tapes peak at 101 and take at 1 %; two peak at 100.5 and end flat, open under a take
    // of 1 % and a stop of -50 %.
    let deals: Vec<PreparedDeal> = (1..=3)
        .map(|uid| prepared(uid, 101.0))
        .chain((4..=5).map(|uid| prepared(uid, 100.5)))
        .collect();
    let score = variant_tally(
        &deals,
        &HashMap::new(),
        "PumpsDetection",
        &[("SellPrice".to_string(), "1".to_string())],
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    assert_eq!(score.tally.n, 3);
    assert_eq!(score.open, 2, "{score:?}");
    assert_eq!(score.untraded, 0, "{score:?}");
    let fact = fact_tally(&deals);
    assert_eq!(
        fact.n as usize,
        score.tally.n as usize + score.open + score.untraded
    );
}

/// search/tally.rs `default_min_n`: the floor is half the train deals, never under one — at a tenth
/// a point could keep a handful of trades and read as the best.
#[test]
fn the_default_trade_floor_is_half_the_train_deals() {
    assert_eq!(default_min_n(10), 5);
    assert_eq!(default_min_n(7), 3);
    assert_eq!(default_min_n(1), 1);
    assert_eq!(default_min_n(0), 1);
}

#[test]
fn a_cancelled_run_answers_nothing_and_nothing_varied_answers_nothing() {
    let deals: Vec<PreparedDeal> = (1..=3).map(|uid| prepared(uid, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let all: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: true,
        vary_exit: true,
        locked: &all,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 2,
        min_n: None,
        seed: Some(1),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let handle = SearchHandle::new();
    assert!(
        suggest(&deals, &params, &handle).is_err(),
        "everything locked"
    );
    let none: HashSet<String> = HashSet::new();
    let params = SearchParams {
        locked: &none,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        ..params
    };
    let handle = SearchHandle::new();
    handle.cancel();
    assert!(suggest(&deals, &params, &handle).is_err());
    assert!(handle.abandoned());
}

/// Reintroducing a sample minimum would discard the long deal's answered 90 s exit.
/// Search replays, column scores, per-deal results and the picture must agree on one open deal.
#[test]
fn each_deal_answers_exits_only_within_its_own_held_trail() {
    let mut deals: Vec<_> = [(1, 60_000), (2, 180_000)]
        .into_iter()
        .map(|(uid, trail)| {
            let mut deal = prepared(uid, 100.2);
            deal.trail_ms = trail;
            let close = deal.deal.close_ms;
            let mut ticks = deal.ticks.to_vec();
            ticks.extend([
                tick(close + 60_000, 100.2),
                tick(close + 90_000, 101.0),
                tick(close + 180_000, 101.0),
                tick(close + 180_001, 102.0),
            ]);
            deal.ticks = ticks.into();
            deal.clip_to_trail();
            deal
        })
        .collect();
    assert_eq!(tape_end_ms(&deals[0]), 60_000);
    assert_eq!(tape_end_ms(&deals[1]), 180_000);
    let model = ModelSettings {
        latency_ms: 0.0,
        ..ModelSettings::default()
    };
    let defaults = HashMap::new();
    let values = vec![("SellPrice".to_string(), "1".to_string())];
    let bases = Bases::of(&deals);
    let params = bases.params(
        &HashMap::new(),
        &defaults,
        &point_of(&values),
        "PumpsDetection",
        model,
    );
    let replay = closing::replay(
        &deals,
        &bases.of_deal,
        &params,
        &[
            Some(Fill {
                t_ms: deals[0].deal.buy_ms,
                price: 100.0,
            }),
            Some(Fill {
                t_ms: deals[1].deal.buy_ms,
                price: 100.0,
            }),
        ],
    );
    assert_eq!((replay.closed.n, replay.open), (1, 1));
    let score = variant_tally(&deals, &defaults, "PumpsDetection", &values, model);
    assert_eq!((score.tally.n, score.open), (1, 1));
    let (score, money) = variant_tally_by_deal(&deals, &defaults, "PumpsDetection", &values, model);
    assert_eq!((score.tally.n, score.open), (1, 1));
    assert!(money[0].1.is_none());
    assert!(money[1].1.is_some());
    let picture = variant_picture(&deals[1], &defaults, "PumpsDetection", &values, model);
    assert_eq!(
        picture.outcome.exit.expect("answered exit").t_ms,
        deals[1].deal.close_ms + 90_000
    );
    assert!(
        variant_picture(&deals[0], &defaults, "PumpsDetection", &values, model)
            .outcome
            .left_open()
    );

    // A longer held trail cannot answer an exit whose path enters an unproven hole.
    deals[1].deal.gap = Some(crate::db::tuner::ticks::gap::TapeGap {
        from_ms: deals[1].deal.close_ms + 60_000,
        to_ms: deals[1].deal.close_ms + 90_000,
        fact_line: None,
        fact_stop: None,
    });
    let score = variant_tally(&deals, &defaults, "PumpsDetection", &values, model);
    assert_eq!((score.tally.n, score.open), (0, 2));
}

/// Clipping must keep quiet coverage independent of other rows and clamp negative trails.
#[test]
fn held_trails_preserve_quiet_tapes_and_clamp_before_the_close() {
    let mut quiet = prepared(1, 101.0);
    quiet.trail_ms = 180_000;
    let ticks = Arc::clone(&quiet.ticks);
    quiet.clip_to_trail();
    assert!(Arc::ptr_eq(&ticks, &quiet.ticks));
    let mut odd = prepared(2, 101.0);
    odd.trail_ms = -1;
    let mut ticks = odd.ticks.to_vec();
    ticks.push(tick(odd.deal.close_ms + 1, 102.0));
    odd.ticks = ticks.into();
    odd.clip_to_trail();
    assert_eq!(tape_end_ms(&odd), 0);
}

/// A shift replays no path, so the fields that only move the path are not searched under it;
/// the corridor model searches them all.
#[test]
fn a_shift_does_not_search_the_path_only_fields() {
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let keys = |method| {
        let params = SearchParams {
            held: &held,
            defaults: &defaults,
            kind: "MoonShot",
            vary_entry: true,
            vary_exit: false,
            locked: &locked,
            grids: crate::db::tuner::ticks::search::test_grids::legacy(),
            restarts: 1,
            min_n: None,
            seed: Some(1),
            train_frac: 1.0,
            max_passes: DEFAULT_MAX_PASSES,
            keep_corridor: true,
            risk: Default::default(),
            screen_entry: false,
            model: ModelSettings {
                entry_method: method,
                ..ModelSettings::default()
            },
        };
        varied(&params).iter().map(|f| f.key).collect::<Vec<_>>()
    };
    let shift = keys(super::super::mshot::EntryMethod::Shift);
    let model = keys(super::super::mshot::EntryMethod::Model);
    for path_only in ["MShotRaiseWait", "MShotReplaceDelay", "MShotUsePrice"] {
        assert!(!shift.contains(&path_only), "{path_only} {shift:?}");
        assert!(model.contains(&path_only), "{path_only} {model:?}");
    }
    assert!(shift.contains(&"MShotPrice") && shift.contains(&"MShotAddDistance"));
    // Read at the strategy's value, never varied: the model ignores `FastShotAlgo`, and the
    // mark price and the price bug have no history to replay a corridor on — even with a grid.
    for fixed in ["FastShotAlgo", "MShotAddMarkDelta", "MShotAddPriceBug"] {
        assert!(!shift.contains(&fixed), "{fixed} {shift:?}");
        assert!(!model.contains(&fixed), "{fixed} {model:?}");
        assert!(
            crate::db::tuner::ticks::params::is_model_only(fixed),
            "{fixed}"
        );
    }
}

#[test]
fn a_field_the_strategies_disagree_on_runs_at_each_deals_own_value() {
    // Two strategies, one take each: 1 % and 0.2 %. A variant that leaves the take alone must
    // run every deal at its own strategy's take — 10 on the first deal, 2 on the second — and
    // not at a default for a field no one strategy holds for all.
    let deals = vec![with_take(1, "1"), with_take(2, "0.2")];
    let (VariantScore { tally, .. }, money) = variant_tally_by_deal(
        &deals,
        &HashMap::new(),
        "PumpsDetection",
        &[("StopLoss".to_string(), "0".to_string())],
        ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    );
    let money: Vec<Option<f64>> = money.iter().map(|(_, m)| m.map(|(v, _)| v)).collect();
    assert_eq!(tally.n, 2, "{money:?}");
    assert!(
        (money[0].unwrap_or(f64::NAN) - 10.0).abs() < 1e-6,
        "{money:?}"
    );
    assert!(
        (money[1].unwrap_or(f64::NAN) - 2.0).abs() < 1e-6,
        "{money:?}"
    );
}

#[test]
fn a_search_holds_each_deals_own_value_and_reports_a_value_one_strategy_lacks() {
    // Two strategies, takes of 1 % and 0.2 %, every tape peaking at 101. Left alone, each deal
    // keeps its own take: 10 and 2. Searched: 1 % wins on both, and it is a change — the second
    // strategy does not hold it — even though the first already does.
    let deals: Vec<PreparedDeal> = (1..=4)
        .map(|uid| with_take(uid, if uid % 2 == 1 { "1" } else { "0.2" }))
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    let model = ModelSettings {
        latency_ms: 0.0,
        ..ModelSettings::default()
    };
    let tally = variant_tally(&deals, &defaults, "PumpsDetection", &[], model).tally;
    assert!((tally.profit - 24.0).abs() < 1e-6, "{}", tally.profit);
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 3,
        min_n: Some(2),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model,
    };
    let result = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    assert_eq!(
        result.values,
        vec![("SellPrice".to_string(), "1".to_string())],
        "{result:?}"
    );
    assert!(
        (result.train.profit - 40.0).abs() < 1e-6,
        "{}",
        result.train.profit
    );
    assert_eq!(result.searched, vec!["SellPrice".to_string()]);
    // A held value of the searched field is set aside (LinKvo, 2026-09-25): the search starts
    // from the strategies, and the answer is reported against them — the same take again, not
    // "no change" because the held edits already had it.
    let held: HashMap<String, String> = [("SellPrice".to_string(), "0.4".to_string())].into();
    let params = SearchParams {
        held: &held,
        ..params
    };
    let again = suggest(&deals, &params, &SearchHandle::new()).expect("a result");
    assert_eq!(again.values, result.values, "{again:?}");
    assert!(
        (again.train.profit - 40.0).abs() < 1e-6,
        "{}",
        again.train.profit
    );
}

/// A floor no point can hold is not an answer: the search says it found nothing rather than
/// hand back the richest point that trades fewer deals than asked.
#[test]
fn a_trade_floor_no_point_keeps_finds_nothing() {
    let deals: Vec<PreparedDeal> = (1..=8).map(|uid| prepared(uid, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    locked.remove("SellPrice");
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 3,
        min_n: Some(9),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new());
    assert_eq!(result.map(|r| r.values), Err(SearchMiss::Floor));
    // Held by every point, the same search answers.
    let params = SearchParams {
        min_n: Some(8),
        ..params
    };
    assert!(suggest(&deals, &params, &SearchHandle::new()).is_ok());
}

/// A MoonShot corridor with its near bound, far bound and one-minute modifier.
fn corridor(far: f64, near: f64, add_1m: f64) -> MshotParams {
    MshotParams {
        price_pct: far,
        price_min_pct: near,
        modifiers: super::super::mshot::Modifiers {
            add_1m,
            ..Default::default()
        },
        ..MshotParams::default()
    }
}

/// The corridor is what the modifiers make of the base fields: a variant may move distance
/// between `MShotPrice` and `MShotAdd*`, but not end nearer the price than the trade's own — and
/// a larger modifier brings the order NEARER on a coin whose delta fell.
#[test]
fn a_corridor_is_kept_on_its_bounds_not_on_its_fields() {
    let own = corridor(2.5, 2.0, 0.02);
    let rose = Deltas {
        d1m: 1.0,
        ..Deltas::default()
    };
    let fell = Deltas {
        d1m: -1.0,
        ..Deltas::default()
    };
    // The found variant of the screenshot: the base halved, the modifier eightfold.
    assert!(!corridor(1.25, 0.6, 0.16).never_closer_than(&own, &[rose], true));
    // The same distance moved onto the modifier: kept while the coin rises…
    assert!(corridor(2.5, 2.0, 0.05).never_closer_than(&own, &[rose], true));
    // …and not once it falls, anywhere in the order's life.
    assert!(!corridor(2.5, 2.0, 0.05).never_closer_than(&own, &[rose, fell], true));
    // Wider on both bounds holds whatever the deltas; the trade's own holds against itself.
    assert!(corridor(3.0, 2.5, 0.02).never_closer_than(&own, &[rose, fell], true));
    assert!(own.never_closer_than(&own, &[rose, fell], true));

    // The guard reads each deal's own corridor, and a deal with the fact's entry keeps none.
    let mut moonshot = prepared(1, 101.0);
    moonshot.deal.own_entry = Some(EntryParams::MoonShot(own.clone()));
    moonshot.deal.deltas = fell;
    let guard = CorridorGuard::of(&[moonshot, prepared(2, 101.0)]);
    assert_eq!(guard.deals.len(), 1);
    let exit = ExitParams::default();
    let of = |variant: MshotParams| vec![(EntryParams::MoonShot(variant), exit.clone())];
    assert!(guard.holds(&[0, 0], &of(corridor(3.0, 2.5, 0.02))));
    assert!(!guard.holds(&[0, 0], &of(corridor(2.5, 2.0, 0.05))));
    assert!(guard.holds(&[0, 0], &[(EntryParams::Fact, exit.clone())]));
    // A shift places the order at the far bound and reads nothing of the near one.
    assert!(!corridor(2.5, 1.0, 0.02).never_closer_than(&own, &[rose], true));
    assert!(corridor(2.5, 1.0, 0.02).never_closer_than(&own, &[rose], false));
    let shifted = |mut variant: MshotParams| {
        variant.model.entry_method = EntryMethod::Shift;
        vec![(EntryParams::MoonShot(variant), exit.clone())]
    };
    assert!(guard.holds(&[0, 0], &shifted(corridor(3.0, 1.0, 0.02))));
    assert!(!guard.holds(&[0, 0], &shifted(corridor(2.0, 2.0, 0.02))));

    // The same rule asked of a typed variant before it is written: one of one MoonShot deal
    // comes nearer, the other deal has no corridor to hold.
    let mut moonshot = prepared(3, 101.0);
    moonshot.deal.kind = "MoonShot".into();
    moonshot.deal.own_entry = Some(EntryParams::MoonShot(own.clone()));
    let plain = prepared(4, 101.0);
    let base = base();
    let check = |values: &[(String, String)]| {
        let c = check_corridors(
            [(&moonshot.deal, &base), (&plain.deal, &base)],
            &HashMap::new(),
            values,
            ModelSettings::default(),
        );
        (c.nearer, c.inverted, c.checked)
    };
    let typed = |far: &str, near: &str| {
        vec![
            ("MShotPrice".to_string(), far.to_string()),
            ("MShotPriceMin".to_string(), near.to_string()),
        ]
    };
    assert_eq!(check(&typed("1.25", "0.6")), (1, 0, 1));
    assert_eq!(check(&typed("3", "2.5")), (0, 0, 1));
    // The screenshot's V1: the near field past the far one. Both effective bounds sit farther
    // than the trade's (the far one lifted to the near), so the corridor rule passes it — the
    // order rule does not.
    assert_eq!(check(&typed("2.6", "2.7")), (0, 1, 1));
}

/// The search never proposes a corridor whose near field is at or past its far one, whatever
/// the switch; an Exit-only search leaves the strategy's own fields alone.
#[test]
fn an_inverted_corridor_is_refused_only_where_the_entry_is_searched() {
    assert!(corridor(2.5, 2.0, 0.0).is_ordered());
    assert!(!corridor(1.7, 2.0, 0.0).is_ordered());
    assert!(!corridor(2.0, 2.0, 0.0).is_ordered());
    assert!(ordered(&EntryParams::Fact));
    assert!(!ordered(&EntryParams::MoonShot(corridor(1.7, 2.0, 0.0))));
    // A point inverts only a base that started in order: a strategy stored inverted does not
    // refuse every point of the search.
    let exit = ExitParams::default();
    let at = |far: f64, near: f64| {
        (
            EntryParams::MoonShot(corridor(far, near, 0.0)),
            exit.clone(),
        )
    };
    assert!(inverts(&[true, true], &[at(2.5, 2.0), at(1.7, 2.0)]));
    assert!(!inverts(&[true, false], &[at(2.5, 2.0), at(1.7, 2.0)]));
    assert!(!inverts(&[true], &[at(2.5, 2.0)]));
}

fn field(key: &str) -> &'static TickParam {
    TICK_PARAMS
        .iter()
        .find(|f| f.key == key)
        .expect("a grid field")
}

/// A distance that only moves between two fields together: every single move scores worse, the
/// pair — `MShotPrice` a step down, `MShotAdd1minDelta` a step up — scores better, and the
/// descent finds it where one field at a time never could.
#[test]
fn a_pair_move_reaches_what_no_single_move_does() {
    let price = field("MShotPrice");
    let add = field("MShotAdd1minDelta");
    let start: HashMap<&'static str, usize> = [(price.key, 10), (add.key, 5)].into();
    let target = (9, 6);
    let evaluate = |point: &Point| -> Option<Tally> {
        let at = (
            grid_index(
                crate::db::tuner::ticks::search::test_grids::legacy(),
                price,
                point,
                &start,
            )
            .expect("price"),
            grid_index(
                crate::db::tuner::ticks::search::test_grids::legacy(),
                add,
                point,
                &start,
            )
            .expect("add"),
        );
        let mut tally = Tally::default();
        tally.push(if at == target {
            10.0
        } else if at == (10, 5) {
            5.0
        } else {
            1.0
        });
        Some(tally)
    };
    let order = [price, add];
    let walked = descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &order,
        &coupled::Coupling::none(),
        &start,
        &evaluate,
        None,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert_eq!(
        (
            grid_index(
                crate::db::tuner::ticks::search::test_grids::legacy(),
                price,
                &walked.point,
                &start
            ),
            grid_index(
                crate::db::tuner::ticks::search::test_grids::legacy(),
                add,
                &walked.point,
                &start
            )
        ),
        (Some(9), Some(6))
    );
    assert!((walked.score.expect("scored").profit - 10.0).abs() < 1e-9);
    assert!(walked.converged);
    // Screened, the paying pair ranks first among the pair moves and is still taken.
    let quick = |p: &Point| screen::Screened::Scored(evaluate(p));
    let screened = descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &order,
        &coupled::Coupling::none(),
        &start,
        &evaluate,
        Some(&quick),
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert_eq!(screened.point, walked.point);
    // Without the pairs the walk stays where it began.
    let alone = descend(
        Point::new(),
        crate::db::tuner::ticks::search::test_grids::legacy(),
        &order,
        &[],
        &coupled::Coupling::none(),
        &start,
        &evaluate,
        None,
        1,
        DEFAULT_MAX_PASSES,
        &SearchHandle::new(),
    )
    .expect("not stopped");
    assert!(alone.point.is_empty(), "{:?}", alone.point);
}

/// A restart past the first starts near the base: one to three fields, a number field at most
/// three steps from where it stands.
#[test]
fn a_perturbed_start_stays_near_the_base() {
    let price = field("MShotPrice");
    let add = field("MShotAdd1minDelta");
    let start: HashMap<&'static str, usize> = [(price.key, 10), (add.key, 5)].into();
    let order = [price, add];
    for restart in 1..200 {
        let mut state = restart_seed(7, restart);
        let mut point = Point::new();
        perturb(
            &mut point,
            crate::db::tuner::ticks::search::test_grids::legacy(),
            &order,
            &start,
            &mut state,
        );
        assert!((1..=2).contains(&point.len()), "{point:?}");
        for f in order {
            if let Some(at) = point.get(f.key).and_then(|_| {
                grid_index(
                    crate::db::tuner::ticks::search::test_grids::legacy(),
                    f,
                    &point,
                    &start,
                )
            }) {
                assert!(at.abs_diff(start[f.key]) <= 3, "{} at {at}", f.key);
            }
        }
    }
    let mut items: Vec<usize> = (0..10).collect();
    shuffle(&mut items, &mut restart_seed(7, 1));
    let mut sorted = items.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        (0..10).collect::<Vec<_>>(),
        "a shuffle keeps every field"
    );
}

/// A search whose every point would bring a trade's corridor nearer the price than its own
/// comes back with that reason: the trade ran a corridor wider than any grid value, so nothing
/// the Entry group can be set to keeps it — and with the switch off the same search answers.
#[test]
fn a_search_that_no_point_can_keep_the_corridor_of_says_so() {
    let mut deals: Vec<PreparedDeal> = (1..=4).map(|uid| prepared(uid, 101.0)).collect();
    for d in &mut deals {
        d.deal.kind = "MoonShot".into();
        d.deal.own_entry = Some(EntryParams::MoonShot(corridor(100.0, 90.0, 0.0)));
    }
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "MoonShot",
        vary_entry: true,
        vary_exit: false,
        locked: &locked,
        grids: crate::db::tuner::ticks::search::test_grids::legacy(),
        restarts: 2,
        min_n: Some(1),
        seed: Some(7),
        train_frac: 1.0,
        max_passes: 2,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
    };
    let result = suggest(&deals, &params, &SearchHandle::new());
    assert_eq!(result.map(|r| r.values), Err(SearchMiss::Corridor));
    let params = SearchParams {
        keep_corridor: false,
        risk: Default::default(),
        screen_entry: false,
        ..params
    };
    let result = suggest(&deals, &params, &SearchHandle::new());
    assert!(!matches!(result, Err(SearchMiss::Corridor)), "{result:?}");
}

/// A value is compared as a value: a strategy's `1.0` and the search's `1` are one, so the field
/// does not land in В1 as a change. Another value is one, and so is any value of a field the
/// strategy leaves out — a switch's completed value is written with it.
#[test]
fn a_value_moves_as_a_value_not_as_text() {
    let own: HashMap<String, String> = [("PriceDownTimer".to_string(), "1.0".to_string())].into();
    let bases = Bases {
        owns: vec![&own],
        of_deal: vec![0],
    };
    let held = HashMap::new();
    assert!(!bases.moves(&held, "PriceDownTimer", "1"));
    assert!(bases.moves(&held, "PriceDownTimer", "2"));
    assert!(bases.moves(&held, "TakeProfit", "1"));
    // A blank base is no value: `0` is a change of it, not a spelling of it.
    let blank: HashMap<String, String> = [("UseTrailing".to_string(), String::new())].into();
    let bases = Bases {
        owns: vec![&blank],
        of_deal: vec![0],
    };
    assert!(bases.moves(&held, "UseTrailing", "0"));
}

/// A set under the floor is refused with its size; the floor itself is searched, and it leaves
/// a holdout at the 70 % share large enough to be a check.
#[test]
fn a_sample_under_the_floor_is_refused_with_its_size() {
    assert_eq!(
        sample_floor(MIN_SEARCH_DEALS - 1),
        Err(SearchMiss::TooFew {
            n: MIN_SEARCH_DEALS - 1
        })
    );
    assert_eq!(sample_floor(4), Err(SearchMiss::TooFew { n: 4 }));
    assert_eq!(sample_floor(MIN_SEARCH_DEALS), Ok(()));
    let closes: Vec<i64> = (1..=MIN_SEARCH_DEALS as i64).collect();
    let holdout = MIN_SEARCH_DEALS - train_len(&closes, 0.7);
    assert!(holdout as i64 >= MIN_HOLDOUT, "holdout {holdout}");
}

/// A field the score does not depend on leaves the answer; one it does stays, and so does one
/// whose removal the scoring refuses.
#[test]
fn a_field_that_moves_nothing_leaves_the_answer() {
    let tally = |profit: f64| {
        let mut t = Tally::default();
        for _ in 0..10 {
            t.push(profit / 10.0);
        }
        t
    };
    // `SellPrice` pays; `SellLevelDelayNext` moves nothing; without `PriceDownTimer` the point is
    // refused.
    let evaluate = |p: &Point| -> Option<Tally> {
        if !p.contains_key("PriceDownTimer") {
            return None;
        }
        Some(tally(if p.contains_key("SellPrice") {
            5.0
        } else {
            1.0
        }))
    };
    let point: Point = [
        ("SellPrice", "1.2".to_string()),
        ("SellLevelDelayNext", "1".to_string()),
        ("PriceDownTimer", "3".to_string()),
    ]
    .into_iter()
    .collect();
    let score = evaluate(&point);
    let (kept, kept_score) = drop_passengers(
        point.clone(),
        score.clone(),
        &Point::new(),
        &evaluate,
        5,
        &|| false,
    );
    let mut keys: Vec<&str> = kept.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(keys, ["PriceDownTimer", "SellPrice"]);
    assert_eq!(
        kept_score.map(|t| t.profit),
        score.as_ref().map(|t| t.profit)
    );
    // A refused point is answered as it stands.
    let refused: Point = [("SellLevelDelayNext", "1".to_string())]
        .into_iter()
        .collect();
    let (same, none) = drop_passengers(refused.clone(), None, &Point::new(), &evaluate, 5, &|| {
        false
    });
    assert_eq!(same, refused);
    assert!(none.is_none());
    // A field held on its grid is never put back, however little it moves.
    let pinned: Point = [("SellLevelDelayNext", "1".to_string())]
        .into_iter()
        .collect();
    let (held, _) = drop_passengers(point.clone(), score.clone(), &pinned, &evaluate, 5, &|| {
        false
    });
    assert!(held.contains_key("SellLevelDelayNext"), "{held:?}");
    // A stopped search answers its point as it stands.
    let (stopped, _) = drop_passengers(
        point.clone(),
        score.clone(),
        &Point::new(),
        &evaluate,
        5,
        &|| true,
    );
    assert!(stopped.contains_key("SellLevelDelayNext"), "{stopped:?}");
    // A field that trades another number of deals for the same result is not a passenger.
    let fewer = |p: &Point| -> Option<Tally> {
        let mut t = Tally::default();
        let n = if p.contains_key("SellLevelDelayNext") {
            10
        } else {
            5
        };
        // Exact halves and ones: both sum to 5.0 to the bit.
        let each = if n == 10 { 0.5 } else { 1.0 };
        for _ in 0..n {
            t.push(each);
        }
        Some(t)
    };
    let at_point = fewer(&point);
    let (kept, _) = drop_passengers(point, at_point, &Point::new(), &fewer, 1, &|| false);
    assert!(kept.contains_key("SellLevelDelayNext"), "{kept:?}");
}

/// Held aliases override own canonical formulas, while each layer prefers its canonical key.
#[test]
fn slot_point_prefers_held_then_canonical_formula() {
    for _ in 0..64 {
        let own = HashMap::from([
            ("SellByCustomEMA".into(), "EMA(2,1)>1".into()),
            ("sellbycustomema".into(), "EMA(3,1)>2".into()),
        ]);
        for held in [
            HashMap::new(),
            HashMap::from([("sellbycustomema".into(), "EMA(5,1)<3".into())]),
            HashMap::from([
                ("sellbycustomema".into(), "EMA(6,1)>888".into()),
                ("SellByCustomEMA".into(), "EMA(5,1)<3".into()),
            ]),
        ] {
            let expected = if held.is_empty() {
                "EMA(2,1)>-1"
            } else {
                "EMA(5,1)<-1"
            };
            let (_, exit) = params_of(
                &own,
                &held,
                &HashMap::new(),
                &Point::from([("SellByCustomEMA#1", "-1".into())]),
                "PumpsDetection",
                ModelSettings::default(),
            );
            assert_eq!(
                *exit.custom_ema.unwrap(),
                super::super::custom_ema::parse(expected).unwrap()
            );
            let base = if held.is_empty() { "1" } else { "3" };
            let bases = Bases {
                owns: vec![&own],
                of_deal: vec![0],
            };
            assert!(!bases.moves(&held, "SellByCustomEMA#1", base));
            assert!(bases.moves(&held, "SellByCustomEMA#1", "-1"));
            let defaults = HashMap::new();
            let locked = HashSet::new();
            let grids = Grids::of([("SellByCustomEMA#1", Arc::from([1.0, 3.0]))]);
            let params = SearchParams {
                held: &held,
                defaults: &defaults,
                kind: "PumpsDetection",
                vary_entry: false,
                vary_exit: true,
                locked: &locked,
                grids: &grids,
                restarts: 1,
                min_n: None,
                seed: Some(1),
                train_frac: 1.0,
                max_passes: 1,
                model: ModelSettings::default(),
                keep_corridor: false,
                risk: Default::default(),
                screen_entry: false,
            };
            assert_eq!(
                deps::dependents_of(&params, &[&own]).0["SellByCustomEMA#1"],
                usize::from(!held.is_empty())
            );
            let other = HashMap::from([("SellByCustomEMA".into(), "EMA(2,1)>1".into())]);
            let slots: Vec<_> = TICK_PARAMS
                .iter()
                .filter(|field| super::super::params::formula_slot(field.key).is_some())
                .collect();
            let offered = deps::compatible_slots(slots, &[&own, &other], &held);
            assert_eq!(
                offered.iter().map(|field| field.key).collect::<Vec<_>>(),
                ["SellByCustomEMA#1"]
            );
        }
    }
}

/// Start indices read formula digits, including a held formula's override.
#[test]
fn custom_ema_start_indices_use_projected_values() {
    let own = HashMap::from([("SellByCustomEMA".into(), "EMA(60,1)>0.25".into())]);
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let grids = Grids::of([("SellByCustomEMA#1", Arc::from([0.1, 0.25, 0.5]))]);
    let mut params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: &grids,
        restarts: 1,
        min_n: None,
        seed: Some(1),
        train_frac: 1.0,
        max_passes: 1,
        model: ModelSettings::default(),
        keep_corridor: false,
        risk: Default::default(),
        screen_entry: false,
    };
    assert_eq!(
        deps::dependents_of(&params, &[&own]).0["SellByCustomEMA#1"],
        1
    );
    let held_formula = HashMap::from([("SellByCustomEMA".into(), "EMA(60,1)>0.5".into())]);
    params.held = &held_formula;
    assert_eq!(
        deps::dependents_of(&params, &[&own]).0["SellByCustomEMA#1"],
        2
    );
}

/// Formula knobs are positional and require a shared structure and sentinel mask.
#[test]
fn custom_ema_slots_offered_only_for_shared_skeleton() {
    let own = |src: &str| HashMap::from([("SellByCustomEMA".into(), src.into())]);
    let a = own("EMA(60,1)>888 AND EMA(5,1)<-1");
    let b = own("ema(60s,1s)>999 AND ema(5s,1s)<-2");
    let swapped = own("EMA(60,1)>1 AND EMA(5,1)<-888");
    let different = own("EMA(61,1)>888 AND EMA(5,1)<-1");
    let empty = HashMap::new();
    let slots = TICK_PARAMS
        .iter()
        .filter(|f| super::super::params::formula_slot(f.key).is_some())
        .collect::<Vec<_>>();
    let keys = |owns: &[&HashMap<String, String>]| {
        deps::compatible_slots(slots.clone(), owns, &HashMap::new())
            .iter()
            .map(|f| f.key)
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&[&a, &b]), ["SellByCustomEMA#2"]);
    assert!(keys(&[&a, &swapped]).is_empty());
    assert!(keys(&[&a, &different]).is_empty());
    assert!(keys(&[&a, &empty]).is_empty());
    assert!(keys(&[]).is_empty());
}

/// A virtual edit must reach exit_params as a rewritten formula before the replay evaluates it.
#[test]
fn slot_point_rewrites_formula_before_walk() {
    let own = HashMap::from([("SellByCustomEMA".into(), "EMA(2,1)>1".into())]);
    let point = Point::from([("SellByCustomEMA#1", "-1".into())]);
    let (_, exit) = params_of(
        &own,
        &HashMap::new(),
        &HashMap::new(),
        &point,
        "PumpsDetection",
        ModelSettings::default(),
    );
    let tape = super::super::custom_ema::SecondSeries::from_ticks(&[tick(0, 100.0)], 0, 10);
    let formula = exit.custom_ema.as_ref().unwrap();
    assert_eq!(formula.eval(&tape, &[], 10), Ok(true));
    let (_, original) = params_of(
        &own,
        &HashMap::new(),
        &HashMap::new(),
        &Point::new(),
        "PumpsDetection",
        ModelSettings::default(),
    );
    assert_eq!(
        original.custom_ema.as_ref().unwrap().eval(&tape, &[], 10),
        Ok(false)
    );
    let bases = Bases {
        owns: vec![&own],
        of_deal: vec![0],
    };
    assert!(!bases.moves(&HashMap::new(), "SellByCustomEMA#1", "1.0"));
    assert!(bases.moves(&HashMap::new(), "SellByCustomEMA#1", "-1"));
}

/// Saving one draft to two targets must preserve each target's spelling and untouched digits.
#[test]
fn fold_formula_edits_rewrites_per_strategy_original() {
    let edits = vec![
        ("SellByCustomEMA#2".into(), "-0.25".into()),
        ("SellPrice".into(), "2".into()),
    ];
    for (source, expected) in [
        (
            "EMA(60,1)>+1% AND EMA(5,1)<-1",
            "EMA(60,1)>+1% AND EMA(5,1)<-0.25",
        ),
        (
            "ema(60s,1s) > 2 and ema(5s,1s) < -2%",
            "ema(60s,1s) > 2 and ema(5s,1s) < -0.25%",
        ),
    ] {
        let own = HashMap::from([("SellByCustomEMA".into(), source.into())]);
        assert_eq!(
            super::super::params::fold_formula_edits(&own, &edits),
            vec![
                ("SellPrice".into(), "2".into()),
                ("SellByCustomEMA".into(), expected.into())
            ]
        );
    }
}
