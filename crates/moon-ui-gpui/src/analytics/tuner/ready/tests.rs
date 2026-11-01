//! Synthetic regressions for the strategy list's readiness pass: batching, ordering, cancel,
//! the rerun gap. No user data, no live view.
// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::super::ticks::readiness::Readiness;
use super::super::ticks::signature::SamplePolicy;
use super::super::ticks::state::DealRow;
use super::super::ticks::state::{RowAddress, TapeStatus, TicksData};
use super::super::ticks::{Fetched, PackedTape};
use super::{
    ChunkLimits, LAUNCH_DEBOUNCE, RERUN_GAP, ReadyClass, ReadyFilter, ReadyKey, ReadyPass,
    ReadyProgress, ReadyRun, count_strategies, group_by_strategy, verdict_deadline,
};
use moon_core::db::tuner::ticks::{Deal, Deltas, ModelSettings};

mod real_data;

/// Removing the warm-up or completion guards would show unstable or finished-pass estimates.
#[test]
fn eta_waits_for_work_and_warmup_and_stops_at_completion() {
    assert_eq!(super::eta(0, 10, Duration::from_secs(10)), None);
    assert_eq!(super::eta(1, 10, Duration::from_millis(2999)), None);
    assert_eq!(super::eta(10, 10, Duration::from_secs(10)), None);
    assert_eq!(super::eta(11, 10, Duration::from_secs(10)), None);
    assert_eq!(
        super::eta(2, 8, Duration::from_secs(3)),
        Some(Duration::from_secs(9))
    );
    assert_eq!(
        super::eta(2, 14, Duration::from_secs(3)),
        Some(Duration::from_secs(18))
    );
}

/// Kept strategies must not inflate measured speed during a partial readiness pass.
#[test]
fn partial_eta_excludes_kept_strategies() {
    let progress = ReadyProgress::default();
    progress.done.store(2, Ordering::Relaxed);
    progress.total.store(8, Ordering::Relaxed);
    progress.kept.store(100, Ordering::Relaxed);
    assert_eq!(progress.counts(), (102, 108));
    assert_eq!(
        progress.remaining(Duration::from_secs(6)),
        Some(Duration::from_secs(18))
    );
}

fn deal(uid: i64, strategy_id: i64, core_uid: u64) -> Deal {
    Deal {
        report_uid: uid,
        core_uid,
        core_name: String::new(),
        strategy_id,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
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
    }
}

fn key(scope_gen: u64, report_gen: u64) -> ReadyKey {
    // Class and status lines translate; the first lookup must not run on the 2 MiB test stack.
    crate::test_locale::warm();
    ReadyKey {
        scope_gen,
        report_gen,
        tape_rev: 0,
        model: ModelSettings::default(),
        sample: SamplePolicy {
            min_tail_s: 30,
            margin_ms: 0,
            long_position_ms: 0,
            entry_tape: crate::analytics::tuner::ticks::entry_tape::ALL,
        },
        refusal_revision: 0,
        addresses_rev: 0,
        calibration_rev: 0,
    }
}

fn ready(fit: usize, covered: usize) -> Readiness {
    Readiness {
        fit,
        covered,
        fetchable: 0,
        ..Readiness::default()
    }
}

/// Even a no-tape refusal names the search floor and omits a zero-yield fetch suggestion.
#[test]
fn readiness_tooltips_show_the_floor_breakdown_and_only_positive_fetch_yield() {
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let counts = Readiness {
            deals: 31,
            no_tape: 7,
            short_tail: 2,
            holed: 1,
            missed: 3,
            unjudged: 4,
            fit: 14,
            covered: 24,
            fetchable: 0,
        };
        let text = super::cell_tip(&counts);
        assert!(text.contains(&Readiness::min().to_string()));
        assert!(text.contains("31") && text.contains("14"));
        let key = super::super::plural_key("analytics.ticks.why.fetchable", 0);
        let fetch_line = rust_i18n::t!(key.as_str(), n = 0).to_string();
        assert!(!text.contains(&fetch_line));
        let counts = Readiness {
            fetchable: 9,
            ..counts
        };
        for n in [1, 2, 9, 21] {
            let key = super::super::plural_key("analytics.ticks.why.fetchable", n);
            assert!(
                super::cell_tip(&Readiness {
                    fetchable: n,
                    ..counts
                })
                .contains(&rust_i18n::t!(key.as_str(), n = n).to_string())
            );
        }
        let text = super::cell_tip(&Readiness::default());
        assert!(text.contains(&Readiness::min().to_string()));
        assert!(!text.contains("analytics."));
    }
}

/// Many strategies counted between two ticks fold in ONE batch: one generation bump, one
/// repaint — never one per strategy.
#[test]
fn partial_counts_fold_once_per_tick() {
    let mut pass = ReadyPass::default();
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    let before = pass.generation;
    for i in 0..50 {
        progress.push(format!("{i}@1"), ready(i, i));
    }
    assert!(pass.absorb(seq));
    assert_eq!(pass.generation, before + 1, "one bump for the whole batch");
    assert_eq!(pass.counts.len(), 50);
    assert!(
        !pass.absorb(seq),
        "an empty tick folds nothing and repaints nothing"
    );
    assert_eq!(pass.generation, before + 1);
    progress.push("x@1".into(), ready(1, 1));
    assert!(
        !pass.absorb(seq + 1),
        "a superseded pass's counts are not folded"
    );
}

/// A report move keeps the old counts on screen until the new pass replaces them; a scope move
/// drops them, they describe nothing any more.
#[test]
fn a_report_move_keeps_the_counts_a_scope_move_drops_them() {
    let mut pass = ReadyPass::default();
    let (seq, _) = pass.begin(key(1, 1), Instant::now());
    let counts = HashMap::from([("1@1".to_string(), ready(30, 30))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert!(matches!(pass.run, ReadyRun::Done));
    assert_eq!(
        pass.class("7@7"),
        ReadyClass::NoTape,
        "a strategy without trades is answered once the pass is done, not left on the dash"
    );
    pass.begin(key(1, 2), Instant::now());
    assert_eq!(pass.counts.len(), 1);
    pass.begin(key(2, 2), Instant::now());
    assert!(pass.counts.is_empty());
}

/// A report or tape change waits out the rerun gap since the last start; a first pass and a
/// scope change only wait the debounce.
#[test]
fn reruns_for_a_report_change_wait_the_gap() {
    let mut pass = ReadyPass::default();
    let now = Instant::now();
    assert_eq!(pass.delay(&key(1, 1), now), LAUNCH_DEBOUNCE);
    pass.begin(key(1, 1), now);
    assert_eq!(pass.delay(&key(1, 2), now), RERUN_GAP);
    assert_eq!(pass.delay(&key(2, 2), now), LAUNCH_DEBOUNCE);
    assert_eq!(
        pass.delay(&key(1, 2), now + RERUN_GAP + Duration::from_secs(1)),
        LAUNCH_DEBOUNCE
    );
}

/// The filter only narrows on "Entry/Exit"; a strategy without a count is pending until the
/// pass fails, then unread.
#[test]
fn the_filter_is_off_elsewhere_and_missing_rows_are_pending() {
    let mut pass = ReadyPass {
        chosen: Some(ReadyFilter::Can),
        ..Default::default()
    };
    assert_eq!(pass.filter(true), ReadyFilter::Can);
    assert_eq!(pass.filter(false), ReadyFilter::All);
    assert_eq!(pass.class("9@9"), ReadyClass::Pending);
    let (seq, _) = pass.begin(key(1, 1), Instant::now());
    pass.land(seq, Err(moon_core::db::ReadFail::NotReady));
    assert_eq!(pass.class("9@9"), ReadyClass::Unread);
    assert!(ReadyFilter::Can.keeps(ReadyClass::Can { fit: 30 }));
    assert!(!ReadyFilter::Can.keeps(ReadyClass::Few { fit: 3 }));
    assert!(!ReadyFilter::Can.keeps(ReadyClass::Pending));
}

/// The rows on screen are counted first, in the list's order; a strategy without an address
/// counts without being replayed; a cancel stops the pass.
#[test]
fn strategies_are_counted_in_list_order_and_a_cancel_stops() {
    let deals = vec![deal(1, 1, 1), deal(2, 2, 1), deal(3, 3, 1), deal(4, 2, 1)];
    let groups = group_by_strategy(deals, &HashMap::new(), &["3@1".into(), "1@1".into()]);
    let order: Vec<&str> = groups.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(order, ["3@1", "1@1", "2@1"]);
    assert_eq!(groups[2].1.len(), 2);
    let progress = ReadyProgress::default();
    let replays = AtomicUsize::new(0);
    let out = count_strategies(
        groups.clone(),
        |_| {
            replays.fetch_add(1, Ordering::Relaxed);
            empty_fetch()
        },
        |_| Vec::<DealRow>::new(),
        &progress,
        limits(64),
        0,
        0,
    )
    .expect("not cancelled");
    assert_eq!(
        replays.load(Ordering::Relaxed),
        0,
        "no address, nothing to fetch"
    );
    assert_eq!(out.len(), 3);
    assert_eq!(out["2@1"].covered, 0);
    assert_eq!(progress.counts(), (3, 3));
    assert_eq!(progress.drain().len(), 3);
    let cancelled = ReadyProgress::default();
    cancelled.cancel();
    assert!(
        count_strategies(
            groups,
            |_| empty_fetch(),
            |_| Vec::new(),
            &cancelled,
            limits(64),
            0,
            0
        )
        .is_none()
    );
}

fn market(coin: &str) -> (String, String) {
    ("EXA".to_string(), coin.to_string())
}

/// A finished pass: "1@1" has a tapeless row on XALPHA, "2@1" is fully covered.
fn finished() -> ReadyPass {
    let mut pass = ReadyPass::default();
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    progress.wait_on("1@1", market("XALPHA"));
    let counts = HashMap::from([
        ("1@1".to_string(), ready(3, 3)),
        ("2@1".to_string(), ready(40, 40)),
    ]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    pass
}

/// Leaving the tab and coming back, or picking a row, moves the tape revision (the card's own
/// replay files its markets) and nothing else: the finished counts stand, no pass starts and
/// the finished caption stays.
#[test]
fn a_tape_move_that_touches_no_tapeless_row_keeps_the_finished_pass() {
    let mut pass = finished();
    let seq = pass.seq;
    let moved = ReadyKey {
        tape_rev: 7,
        ..key(1, 1)
    };
    let refiled = HashSet::from([market("XBETA")]);
    let stale = pass
        .stale_strategies(&moved, &refiled, 0)
        .expect("same scope");
    assert!(stale.is_empty());
    pass.adopt(moved);
    assert_eq!(pass.launched, Some(moved));
    assert_eq!(pass.seq, seq, "no new pass");
    assert!(matches!(pass.run, ReadyRun::Done));
    assert_eq!(pass.counts.len(), 2);
    assert_eq!(
        pass.status(2),
        Some((
            rust_i18n::t!("analytics.ready.done", n = 1, m = 2).to_string(),
            false
        ))
    );
    assert_eq!(
        pass.stale_strategies(&moved, &HashSet::new(), 0),
        Some(HashSet::new()),
        "the same inputs again ask for nothing"
    );
}

/// A covered strategy is not recounted for its market's filing: only a tapeless row can gain.
#[test]
fn a_filing_recounts_only_the_strategies_waiting_on_it() {
    let mut pass = finished();
    let moved = ReadyKey {
        tape_rev: 7,
        ..key(1, 1)
    };
    let refiled = HashSet::from([market("XALPHA")]);
    let stale = pass
        .stale_strategies(&moved, &refiled, 0)
        .expect("same scope");
    assert_eq!(stale, HashSet::from(["1@1".to_string()]));
    let (seq, progress) = pass.begin_partial(moved, Instant::now(), &stale);
    assert_eq!(pass.counts.len(), 2, "the kept counts stay on screen");
    assert_eq!(
        progress.counts(),
        (1, 2),
        "the kept strategy counts as done, the stale one is still owed"
    );
    assert!(
        pass.fraction().is_some_and(|f| f < 1.0),
        "the bar is not full yet"
    );
    let counts = HashMap::from([("1@1".to_string(), ready(30, 30))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert_eq!(pass.counts.len(), 2, "merged, not replaced");
    assert_eq!(pass.counts["1@1"].fit, 30);
    assert_eq!(pass.counts["2@1"].fit, 40);
    assert_eq!(
        pass.stale_strategies(&moved, &refiled, 0),
        Some(HashSet::new()),
        "the recount left nothing waiting on XALPHA"
    );
}

/// A station refusal change is per market: it never restarts the pass, and only the strategy
/// with a tapeless row on the market whose refusals changed is recounted — the other keeps its
/// count and class on screen through the recount.
#[test]
fn a_refusal_change_recounts_only_the_strategies_on_its_market() {
    let mut pass = finished();
    let moved = ReadyKey {
        refusal_revision: 3,
        ..key(1, 1)
    };
    assert!(
        !key(1, 1).restarts(&moved),
        "a refusal change is no restart"
    );
    let mut stale = pass
        .stale_strategies(&moved, &HashSet::new(), 0)
        .expect("a refusal change is a partial recount");
    assert!(stale.is_empty());
    assert!(
        pass.trading_on(&HashSet::from([market("XBETA")]))
            .is_empty(),
        "another market"
    );
    stale.extend(pass.trading_on(&HashSet::from([market("XALPHA")])));
    assert_eq!(stale, HashSet::from(["1@1".to_string()]));
    let (seq, _) = pass.begin_partial(moved, Instant::now(), &stale);
    assert_eq!(pass.counts.len(), 2, "no clear");
    assert_eq!(pass.class("2@1"), ReadyClass::of(&ready(40, 40)));
    assert_eq!(
        pass.class("1@1"),
        ReadyClass::of(&ready(3, 3)),
        "old class until recounted"
    );
    let counts = HashMap::from([("1@1".to_string(), ready(30, 30))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert_eq!(pass.counts["1@1"].fit, 30);
    assert_eq!(pass.counts["2@1"].fit, 40, "kept");
}

/// A covered row's short tail is admitted or not by the station's refusals, so a refusal change
/// on its market recounts it, while a filing there leaves it standing.
#[test]
fn a_refusal_change_recounts_a_covered_strategy_on_its_market() {
    let mut pass = ReadyPass::default();
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    progress.trades_on("2@1", market("XGAMMA"));
    let counts = HashMap::from([("2@1".to_string(), ready(40, 40))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    let gamma = HashSet::from([market("XGAMMA")]);
    assert_eq!(
        pass.stale_strategies(&key(1, 1), &gamma, 0),
        Some(HashSet::new()),
        "a filing cannot change a covered row"
    );
    assert_eq!(pass.trading_on(&gamma), HashSet::from(["2@1".to_string()]));
}

/// A scope, model or sample move asks for a whole pass; so does anything before one finished.
#[test]
fn a_real_input_change_recounts_everything() {
    let pass = finished();
    let none = HashSet::new();
    let report = pass.stale_strategies(&key(1, 2), &none, 0);
    assert_eq!(
        report,
        Some(HashSet::new()),
        "a report move reads before it decides"
    );
    assert!(!pass.stands(&key(1, 2), report.as_ref()), "report moved");
    let window = ReadyKey {
        sample: SamplePolicy {
            margin_ms: 180_000,
            ..key(1, 1).sample
        },
        ..key(1, 1)
    };
    assert_eq!(
        pass.stale_strategies(&window, &none, 0),
        None,
        "window moved"
    );
    assert_eq!(
        pass.stale_strategies(&key(2, 1), &none, 0),
        None,
        "scope moved"
    );
    let model = ReadyKey {
        sample: SamplePolicy {
            min_tail_s: 60,
            ..key(1, 1).sample
        },
        ..key(1, 1)
    };
    assert_eq!(pass.stale_strategies(&model, &none, 0), None, "model moved");
    let mut running = ReadyPass::default();
    assert_eq!(
        running.stale_strategies(&key(1, 1), &none, 0),
        None,
        "never ran"
    );
    running.begin(key(1, 1), Instant::now());
    assert_eq!(
        running.stale_strategies(&key(1, 1), &none, 0),
        None,
        "still running"
    );
}

fn limits(max_count: usize) -> ChunkLimits {
    ChunkLimits {
        first_count: max_count,
        max_count,
        max_bytes: usize::MAX,
    }
}

fn empty_fetch() -> Fetched {
    Fetched {
        rows: Vec::new(),
        tapes: HashMap::new(),
        traces: HashMap::new(),
        settled: Default::default(),
    }
}

fn address() -> Arc<RowAddress> {
    Arc::new(RowAddress {
        core_uid: 1,
        venue: Some(moon_core::venue::Venue {
            brand: moon_core::venue::Brand::Bybit,
            kind: moon_core::venue::MarketKind::Futures,
        }),
        exchange_key: "2:00000000".into(),
        market: "XALPHAUSDT".into(),
        btc_market: None,
    })
}

/// Losing an unlisted address must not turn resolved rows into the pipeline's unaddressed set.
#[test]
fn unlisted_readiness_fetches_and_replays_resolved_rows() {
    let mut address = address();
    Arc::make_mut(&mut address).venue = None;
    Arc::make_mut(&mut address).exchange_key = moon_core::fixture::unlisted::EXCHANGE_KEY.into();
    let groups = vec![("200@1".into(), vec![(deal(200, 200, 1), Some(address))])];
    let progress = ReadyProgress::default();
    let counts = count_strategies(
        groups,
        fake_fetch,
        fake_replay,
        &progress,
        ChunkLimits {
            first_count: 1,
            max_count: 1,
            max_bytes: usize::MAX,
        },
        0,
        0,
    )
    .expect("not cancelled");
    assert_eq!(counts["200@1"].covered, 1);
}

/// A covered row whose tail is short of the floor is not fully held: a filing on its market can
/// turn it fit, so its strategy waits on that market. One holding the floor does not. Filed as
/// covered, the short one's strategy stayed one fit deal behind the card after the station
/// extended its tail.
#[test]
fn a_covered_row_with_a_short_tail_waits_on_its_market() {
    let groups = vec![
        ("1@1".into(), vec![(deal(2, 1, 1), Some(address()))]),
        ("2@1".into(), vec![(deal(4, 2, 1), Some(address()))]),
    ];
    let progress = ReadyProgress::default();
    count_strategies(
        groups,
        fake_fetch,
        |fetched| {
            fake_replay(fetched)
                .into_iter()
                .map(|mut row| {
                    let trail = if row.deal.report_uid == 2 {
                        10_000
                    } else {
                        60_000
                    };
                    row.held = Some((0, trail));
                    row
                })
                .collect()
        },
        &progress,
        limits(64),
        0,
        30,
    )
    .expect("not cancelled");
    let mut waiting = progress.drain_waiting();
    waiting.sort();
    let market = ("2:00000000".to_string(), "XALPHAUSDT".to_string());
    assert_eq!(
        waiting,
        vec![
            ("1@1".to_string(), market.clone(), true),
            ("2@1".to_string(), market, false),
        ]
    );
}

/// A fake stage C read: the targets as rows, nothing asked of a worker.
fn fake_fetch(targets: Vec<(Deal, Arc<RowAddress>)>) -> Fetched {
    let settled = targets.iter().map(|(deal, _)| deal.report_uid).collect();
    Fetched {
        settled,
        rows: targets
            .into_iter()
            .map(|(deal, address)| DealRow {
                deal,
                tape: TapeStatus::Missing,
                verdict: None,
                address: Some(address),
                ticks: None,
                entry_line: None,
                held: None,
            })
            .collect(),
        tapes: HashMap::new(),
        traces: HashMap::new(),
    }
}

/// A fake replay that decides every row off its uid alone: every second row covered.
fn fake_replay(fetched: Fetched) -> Vec<DealRow> {
    fetched
        .rows
        .into_iter()
        .map(|mut row| {
            if row.deal.report_uid % 2 == 0 {
                row.tape = TapeStatus::Covered;
            }
            row
        })
        .collect()
}

/// Strategies of 1 to 6 deals, every third deal without an address.
fn many_groups() -> Vec<super::StrategyRows> {
    let mut uid = 0;
    (0..23)
        .map(|s| {
            let rows = (0..=(s % 6))
                .map(|_| {
                    uid += 1;
                    let address = (uid % 3 != 0).then(address);
                    (deal(uid, s, 1), address)
                })
                .collect();
            (format!("{s}@1"), rows)
        })
        .collect()
}

/// What each strategy counts to, one at a time, without the pipeline.
fn sequential(groups: &[super::StrategyRows]) -> HashMap<String, Readiness> {
    groups
        .iter()
        .map(|(key, rows)| {
            let targets: Vec<(Deal, Arc<RowAddress>)> = rows
                .iter()
                .filter_map(|(d, a)| Some((d.clone(), a.clone()?)))
                .collect();
            let mut replayed = if targets.is_empty() {
                Vec::new()
            } else {
                fake_replay(fake_fetch(targets))
            };
            replayed.extend(
                rows.iter()
                    .filter(|(_, a)| a.is_none())
                    .map(|(d, _)| DealRow {
                        deal: d.clone(),
                        tape: TapeStatus::NoAddress,
                        verdict: None,
                        address: None,
                        ticks: None,
                        entry_line: None,
                        held: None,
                    }),
            );
            let mut data = TicksData {
                rows: replayed.into(),
                ..Default::default()
            };
            data.retain_within_cap();
            (key.clone(), Readiness::of_rows(&data.rows, 0, 0))
        })
        .collect()
}

/// Chunks of one, of three and of everything count exactly what one strategy at a time
/// counts, and every strategy reaches the progress.
#[test]
fn the_pipeline_counts_what_a_sequential_pass_counts_at_any_chunk_size() {
    let groups = many_groups();
    let reference = sequential(&groups);
    for cap in [1, 3, 64] {
        let progress = ReadyProgress::default();
        let out = count_strategies(
            groups.clone(),
            fake_fetch,
            fake_replay,
            &progress,
            limits(cap),
            0,
            0,
        )
        .expect("not cancelled");
        assert_eq!(out, reference, "chunk cap {cap}");
        assert_eq!(progress.counts(), (groups.len(), groups.len()));
        assert_eq!(progress.drain().len(), groups.len());
    }
}

fn fit_verdict() -> moon_core::db::tuner::ticks::Verdict {
    use moon_core::db::tuner::ticks::verify;
    moon_core::db::tuner::ticks::Verdict {
        entry: Some(true),
        entry_dev_pct: None,
        exit: Some(true),
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
        entry_finding: verify::EntryFinding::Hit,
        exit_finding: verify::ExitFinding::Hit,
        fill_clock_ms: None,
        rules: verify::RuleFlags::default(),
        stop: verify::StopFacts::default(),
    }
}

/// Two strategies whose tapes each fit the card's memory cap but not together: each is
/// counted under its own cap, so both keep their fit row.
#[test]
fn each_strategy_is_held_to_its_own_memory_cap() {
    let cap = super::super::ticks::state::MAX_RETAINED_BYTES;
    // Packed prints are 8 bytes; 5/8 of the cap each.
    let tape = PackedTape::Packed {
        base_ms: 0,
        prints: vec![(0u32, 1.0f32); cap / 8 * 5 / 8].into(),
    };
    assert!(tape.bytes() < cap);
    assert!(2 * tape.bytes() > cap);
    let groups = vec![
        ("1@1".to_string(), vec![(deal(1, 1, 1), Some(address()))]),
        ("2@1".to_string(), vec![(deal(2, 2, 1), Some(address()))]),
    ];
    let replay = |fetched: Fetched| -> Vec<DealRow> {
        fetched
            .rows
            .into_iter()
            .map(|mut row| {
                row.tape = TapeStatus::Covered;
                row.verdict = Some(fit_verdict());
                row.ticks = Some(tape.clone());
                row.held = Some((0, 3_600_000));
                row
            })
            .collect()
    };
    let out = count_strategies(
        groups,
        fake_fetch,
        replay,
        &ReadyProgress::default(),
        limits(64),
        0,
        0,
    )
    .expect("not cancelled");
    assert_eq!(out["1@1"].fit, 1);
    assert_eq!(out["2@1"].fit, 1);
}

/// A cancel while the first chunk replays stops the pass: no counts, and the chunk after the
/// one already fetched ahead is never fetched.
#[test]
fn a_cancel_stops_before_the_next_chunk_is_fetched() {
    let groups: Vec<super::StrategyRows> = (0..5)
        .map(|s| (format!("{s}@1"), vec![(deal(s, s, 1), Some(address()))]))
        .collect();
    let progress = ReadyProgress::default();
    let fetched = std::sync::Mutex::new(Vec::new());
    let out = count_strategies(
        groups,
        |targets| {
            fetched.lock().unwrap().push(targets[0].0.strategy_id);
            fake_fetch(targets)
        },
        |rows| {
            progress.cancel();
            fake_replay(rows)
        },
        &progress,
        limits(1),
        0,
        0,
    );
    assert!(out.is_none());
    let fetched = fetched.into_inner().unwrap();
    assert!(
        !fetched.contains(&2),
        "the third chunk was fetched: {fetched:?}"
    );
}

/// The caller's read token reaches the fetch thread and every replay task, so the app's
/// abandoned pass stops wherever it is.
#[test]
fn the_read_token_reaches_the_fetch_thread_and_every_replay_task() {
    let groups = many_groups();
    // Every strategy of `many_groups` has an addressed deal.
    let addressed = groups.len();
    let seen_fetch = AtomicUsize::new(0);
    let seen_replay = AtomicUsize::new(0);
    let token = moon_core::db::ReadCancellation::new();
    let probe = token.clone();
    let ours =
        || moon_core::db::current_read_cancellation().is_some_and(|t| t.same_request(&probe));
    moon_core::db::with_read_cancellation(token, || {
        count_strategies(
            groups,
            |targets| {
                if ours() {
                    seen_fetch.fetch_add(1, Ordering::Relaxed);
                }
                fake_fetch(targets)
            },
            |rows| {
                if ours() {
                    seen_replay.fetch_add(1, Ordering::Relaxed);
                }
                fake_replay(rows)
            },
            &ReadyProgress::default(),
            limits(3),
            0,
            0,
        )
    })
    .expect("not cancelled");
    assert_eq!(seen_fetch.load(Ordering::Relaxed), addressed);
    assert_eq!(seen_replay.load(Ordering::Relaxed), addressed);
}

/// A pass whose tapes reach the budget stops reading until the replay frees room: in memory,
/// never more than the budget and the one strategy fetched when it was reached.
#[test]
fn the_fetch_waits_for_room_at_the_byte_budget() {
    let groups: Vec<super::StrategyRows> = (0..12)
        .map(|s| (format!("{s}@1"), vec![(deal(s, s, 1), Some(address()))]))
        .collect();
    let per_tick = std::mem::size_of::<moon_core::feed::types::Tick>();
    let fetch = |targets: Vec<(Deal, Arc<RowAddress>)>| {
        let mut fetched = fake_fetch(targets);
        let tick = moon_core::feed::types::Tick {
            time_ms: 0.0,
            price: 1.0,
            qty: 1.0,
            side: moon_core::feed::types::Side::Buy,
        };
        let deals: Vec<Deal> = fetched.rows.iter().map(|r| r.deal.clone()).collect();
        for deal in deals {
            let window =
                moon_core::db::tuner::ticks::model_window(&deal, 0, i64::MAX).expect("a window");
            fetched.tapes.insert(
                deal.report_uid,
                (
                    vec![tick; 10],
                    moon_core::market::trade_replay::Coverage::none(),
                    window,
                ),
            );
        }
        fetched
    };
    // A slow replay, so the fetch thread runs into the budget.
    let replay = |rows: Fetched| {
        std::thread::sleep(Duration::from_millis(5));
        fake_replay(rows)
    };
    let progress = ReadyProgress::default();
    let out = count_strategies(
        groups,
        fetch,
        replay,
        &progress,
        ChunkLimits {
            first_count: 64,
            max_count: 64,
            max_bytes: 30 * per_tick,
        },
        0,
        0,
    )
    .expect("not cancelled");
    assert_eq!(out.len(), 12);
    assert!(
        progress.peak_bytes() <= 40 * per_tick,
        "peak {} prints over the budget and one strategy",
        progress.peak_bytes() / per_tick
    );
    assert!(
        progress.chunks.lock().unwrap().len() >= 4,
        "the budget cut the chunks"
    );
}

/// The first chunk is small so the first counts land early; each next one doubles up to the
/// cap.
#[test]
fn chunks_start_small_and_double_up_to_the_cap() {
    let groups: Vec<super::StrategyRows> = (0..40)
        .map(|s| (format!("{s}@1"), vec![(deal(s, s, 1), Some(address()))]))
        .collect();
    let progress = ReadyProgress::default();
    count_strategies(
        groups,
        fake_fetch,
        fake_replay,
        &progress,
        ChunkLimits {
            first_count: 4,
            max_count: 12,
            max_bytes: usize::MAX,
        },
        0,
        0,
    )
    .expect("not cancelled");
    assert_eq!(*progress.chunks.lock().unwrap(), vec![4, 8, 12, 12, 4]);
}

/// A replay that panics while the fetch thread waits at the byte budget: the panic reaches the
/// caller in bounded time instead of leaving the fetch thread waiting for bytes nobody frees.
#[test]
fn a_panicking_replay_does_not_leave_the_fetch_waiting() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let groups: Vec<super::StrategyRows> = (0..4)
            .map(|s| (format!("{s}@1"), vec![(deal(s, s, 1), Some(address()))]))
            .collect();
        let per_tick = std::mem::size_of::<moon_core::feed::types::Tick>();
        let fetch = |targets: Vec<(Deal, Arc<RowAddress>)>| {
            let mut fetched = fake_fetch(targets);
            let tick = moon_core::feed::types::Tick {
                time_ms: 0.0,
                price: 1.0,
                qty: 1.0,
                side: moon_core::feed::types::Side::Buy,
            };
            let deals: Vec<Deal> = fetched.rows.iter().map(|r| r.deal.clone()).collect();
            for deal in deals {
                let window = moon_core::db::tuner::ticks::model_window(&deal, 0, i64::MAX)
                    .expect("a window");
                fetched.tapes.insert(
                    deal.report_uid,
                    (
                        vec![tick; 10],
                        moon_core::market::trade_replay::Coverage::none(),
                        window,
                    ),
                );
            }
            fetched
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            count_strategies(
                groups,
                fetch,
                |_: Fetched| -> Vec<DealRow> { panic!("synthetic replay failure") },
                &ReadyProgress::default(),
                ChunkLimits {
                    first_count: 64,
                    max_count: 64,
                    // One strategy fills it: the fetch thread waits for room right away.
                    max_bytes: 10 * per_tick,
                },
                0,
                0,
            )
        }));
        let _ = done.send(outcome.is_err());
    });
    let panicked = finished
        .recv_timeout(Duration::from_secs(10))
        .expect("the pass hung after its replay panicked");
    assert!(panicked, "the replay's panic reaches the caller");
}

/// A report move that adds one deal to strategy A recounts A alone; B keeps its count, and a
/// strategy gone from the scope loses its count.
#[test]
fn a_report_move_recounts_only_the_strategies_whose_deals_changed() {
    let mut pass = ReadyPass::default();
    let none = HashSet::new();
    let first = group_by_strategy(vec![deal(1, 1, 1), deal(2, 2, 1)], &HashMap::new(), &[]);
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    assert_eq!(
        pass.recount_set(&first, None, &progress),
        None,
        "a whole pass"
    );
    let counts = HashMap::from([
        ("1@1".to_string(), ready(3, 3)),
        ("2@1".to_string(), ready(40, 40)),
    ]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));

    let moved = key(1, 2);
    let stale = pass.stale_strategies(&moved, &none, 0).expect("same scope");
    assert!(stale.is_empty());
    assert!(!pass.stands(&moved, Some(&stale)), "the deals must be read");
    let (seq, progress) = pass.begin_partial(moved, Instant::now(), &stale);
    let second = group_by_strategy(
        vec![deal(1, 1, 1), deal(3, 1, 1), deal(2, 2, 1)],
        &HashMap::new(),
        &[],
    );
    let recount = pass.recount_set(&second, Some(&stale), &progress);
    assert_eq!(recount, Some(HashSet::from(["1@1".to_string()])));
    assert_eq!(progress.counts(), (1, 2), "B kept, A owed");
    let counts = HashMap::from([("1@1".to_string(), ready(5, 5))]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert_eq!(pass.counts["1@1"].fit, 5);
    assert_eq!(pass.counts["2@1"].fit, 40, "B's readiness kept");

    let moved = key(1, 3);
    let stale = pass.stale_strategies(&moved, &none, 0).expect("same scope");
    let (_, progress) = pass.begin_partial(moved, Instant::now(), &stale);
    let third = group_by_strategy(vec![deal(1, 1, 1), deal(3, 1, 1)], &HashMap::new(), &[]);
    assert_eq!(
        pass.recount_set(&third, Some(&stale), &progress),
        Some(HashSet::new()),
        "nothing changed for A"
    );
    assert!(!pass.counts.contains_key("2@1"), "B left the scope");
}

/// An unchanged report whose verdict deadline passes recounts the affected strategy, once.
#[test]
fn a_passed_verdict_deadline_recounts_its_strategy() {
    let mut pass = ReadyPass::default();
    let none = HashSet::new();
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    progress.settle("1@1".to_string(), Some(1_000), true);
    progress.settle("2@1".to_string(), None, true);
    let counts = HashMap::from([
        ("1@1".to_string(), ready(3, 3)),
        ("2@1".to_string(), ready(40, 40)),
    ]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    let early = pass.stale_strategies(&key(1, 1), &none, 999);
    assert!(pass.stands(&key(1, 1), early.as_ref()));
    assert!(!pass.take_due(999));
    assert!(pass.take_due(1_000), "the clock flipped A");
    assert!(!pass.take_due(1_000), "armed once");
    let stale = pass.stale_strategies(&key(1, 1), &none, 1_000);
    assert_eq!(stale, Some(HashSet::from(["1@1".to_string()])));
    assert!(!pass.stands(&key(1, 1), stale.as_ref()));
}

/// A refused row waiting on an archive retry is due at its retry; a settled row never.
#[test]
fn verdict_deadline_is_the_next_archive_retry() {
    use moon_core::market::trade_replay::TickStatus;
    let row = |tape| DealRow {
        deal: deal(1, 1, 1),
        tape,
        verdict: None,
        address: None,
        ticks: None,
        entry_line: None,
        held: None,
    };
    let pending = row(TapeStatus::Refused(TickStatus::ArchivePending {
        retry_at_ms: 500,
    }));
    assert_eq!(verdict_deadline(&[pending], 0, 30), Some(500));
    let passed = row(TapeStatus::Refused(TickStatus::ArchivePending {
        retry_at_ms: 500,
    }));
    assert_eq!(verdict_deadline(&[passed], 600, 30), None);
    assert_eq!(verdict_deadline(&[row(TapeStatus::NoAddress)], 0, 30), None);
}

/// A missing row's verdict moves with the clock too (its window falls out of retention, or a
/// short tail stops being fillable): the deadline names that edge, the card's own rule.
#[test]
fn verdict_deadline_names_a_missing_rows_retention_edge() {
    let row = DealRow {
        deal: deal(1, 1, 1),
        tape: TapeStatus::Missing,
        verdict: None,
        address: Some(address()),
        ticks: None,
        entry_line: None,
        held: None,
    };
    let now_ms = 2_000;
    let at = verdict_deadline(std::slice::from_ref(&row), now_ms, 30).expect("an edge");
    assert!(at > now_ms);
    assert_eq!(
        super::super::ticks::signature::settled_until(std::slice::from_ref(&row), 30, now_ms),
        at,
        "the card and the list read one rule"
    );
    assert_eq!(
        verdict_deadline(std::slice::from_ref(&row), at, 30).map(|n| n > at),
        Some(true)
    );
}

/// The model margin and the long-position split change every window asked for: a whole pass.
#[test]
fn a_margin_or_long_position_change_restarts() {
    // Two finished passes' worth of keys outgrow the default test stack in a debug build.
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(margin_or_long_position_change_restarts)
        .expect("spawn")
        .join()
        .expect("no panic");
}

fn margin_or_long_position_change_restarts() {
    let pass = finished();
    let none = HashSet::new();
    for sample in [
        SamplePolicy {
            margin_ms: 5_000,
            ..key(1, 1).sample
        },
        SamplePolicy {
            long_position_ms: 60_000,
            ..key(1, 1).sample
        },
    ] {
        let moved = ReadyKey {
            sample,
            ..key(1, 1)
        };
        assert!(key(1, 1).restarts(&moved));
        assert_eq!(pass.stale_strategies(&moved, &none, 0), None);
    }
}

/// A strategy whose held reads did not all settle keeps no signature: the next pass recounts
/// it although its deals did not move, and its deadline asks again soon.
#[test]
fn an_unsettled_read_keeps_its_strategy_stale() {
    let mut pass = ReadyPass::default();
    let none = HashSet::new();
    let groups = group_by_strategy(vec![deal(1, 1, 1), deal(2, 2, 1)], &HashMap::new(), &[]);
    let (seq, progress) = pass.begin(key(1, 1), Instant::now());
    assert_eq!(pass.recount_set(&groups, None, &progress), None);
    progress.settle("1@1".to_string(), Some(5_000), false);
    progress.settle("2@1".to_string(), None, true);
    let counts = HashMap::from([
        ("1@1".to_string(), ready(0, 0)),
        ("2@1".to_string(), ready(40, 40)),
    ]);
    assert_eq!(pass.land(seq, Ok(Some(counts))), (true, false));
    assert_eq!(pass.next_deadline(), Some(5_000));
    let moved = key(1, 2);
    let stale = pass.stale_strategies(&moved, &none, 0).expect("same scope");
    let (_, progress) = pass.begin_partial(moved, Instant::now(), &stale);
    assert_eq!(
        pass.recount_set(&groups, Some(&stale), &progress),
        Some(HashSet::from(["1@1".to_string()])),
        "the unsettled strategy is owed a recount, the settled one stands"
    );
}

/// A count of unsettled rows arms the retry deadline from the pipeline itself.
#[test]
fn the_pipeline_reports_unsettled_reads() {
    let groups = vec![("1@1".to_string(), vec![(deal(1, 1, 1), Some(address()))])];
    let progress = ReadyProgress::default();
    count_strategies(
        groups,
        |targets| Fetched {
            settled: Default::default(),
            ..fake_fetch(targets)
        },
        fake_replay,
        &progress,
        limits(4),
        0,
        30,
    )
    .expect("not cancelled");
    let deadlines = progress.drain_deadlines();
    assert_eq!(deadlines.len(), 1);
    assert!(!deadlines[0].2, "not settled");
    assert!(deadlines[0].1.is_some_and(|at| at <= 5_000), "retried soon");
}

/// A calibration move changes the signature of the groups on that core only, and makes the
/// finished counts read the deals again.
#[test]
fn a_calibration_move_recounts_the_cores_groups() {
    let on = |core: u64| group_by_strategy(vec![deal(1, 1, core)], &HashMap::new(), &[]);
    let (calibrated, other) = (on(9_301), on(9_302));
    let before = (
        super::group_signature(&calibrated[0].1),
        super::group_signature(&other[0].1),
    );
    super::super::ticks::lags::set_for_test(9_301, 120.0);
    assert_ne!(super::group_signature(&calibrated[0].1), before.0);
    assert_eq!(super::group_signature(&other[0].1), before.1);
    let pass = finished();
    let moved = ReadyKey {
        calibration_rev: 1,
        ..key(1, 1)
    };
    let stale = pass.stale_strategies(&moved, &HashSet::new(), 0);
    assert!(!moved.restarts(&key(1, 1)));
    assert!(
        !pass.stands(&moved, stale.as_ref()),
        "the deals must be read"
    );
    let catalog = ReadyKey {
        addresses_rev: 7,
        ..key(1, 1)
    };
    assert!(!pass.stands(&catalog, stale.as_ref()), "an address landed");
}
