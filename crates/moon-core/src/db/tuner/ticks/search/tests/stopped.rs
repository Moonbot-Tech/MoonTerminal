//! Regressions for keeping a partial search without changing hard-abort trimming.

use super::*;

/// Supply a large restart budget so a stop cannot race a normally completed search.
fn params<'a>(
    held: &'a HashMap<String, String>,
    defaults: &'a HashMap<String, f64>,
    locked: &'a HashSet<String>,
) -> SearchParams<'a> {
    SearchParams {
        held,
        defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked,
        grids: test_grids::legacy(),
        restarts: 100_000,
        min_n: Some(1),
        seed: Some(7),
        train_frac: 0.5,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: false,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
    }
}

/// Discarding the tracker on Stop would lose a scored answer and its holdout assessment.
#[test]
fn finish_mid_search_keeps_answer_and_holdout() {
    let deals: Vec<_> = (1..=20)
        .map(|uid| {
            let mut deal = prepared(uid * 60, 101.0);
            // Both on-grid bases occur in train and holdout, so any untrimmed shared point moves one.
            Arc::make_mut(&mut deal.own).insert("SellLevelDelayNext".into(), (uid % 2).to_string());
            deal
        })
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = TICK_PARAMS
        .iter()
        .filter(|f| !["SellPrice", "StopLoss", "SellLevelDelayNext"].contains(&f.key))
        .map(|f| f.key.to_string())
        .collect();
    let grids = Grids::of([
        ("SellPrice", Arc::from([0.2, 0.7, 1.0])),
        ("StopLoss", Arc::from([-50.0, -1.0, -0.5])),
        ("SellLevelDelayNext", Arc::from([0.0, 1.0])),
    ]);
    let mut params = params(&held, &defaults, &locked);
    params.grids = &grids;
    let handle = SearchHandle::new();
    let found = std::thread::scope(|scope| {
        let search = scope.spawn(|| suggest(&deals, &params, &handle));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while handle.completed() == 0 && !search.is_finished() {
            if std::time::Instant::now() >= deadline {
                handle.cancel();
                panic!("synthetic search did not complete its first restart");
            }
            std::thread::yield_now();
        }
        handle.finish();
        search
            .join()
            .expect("search thread")
            .expect("partial answer")
    });
    assert!(handle.is_cancelled() && handle.keeps());
    assert!(!found.values.is_empty());
    assert!(found.train.n > 0);
    assert!(found.holdout.is_some());
    assert!(found.stats.stopped);
    assert_eq!(found.stats.requested, params.restarts);
    assert!(found.stats.restarts < found.stats.requested);
    assert!(found.searched.iter().any(|key| key == "SellLevelDelayNext"));
    assert!(
        !found
            .values
            .iter()
            .any(|(key, _)| key == "SellLevelDelayNext")
    );
}

/// A keep-stop before any scored point must not manufacture an answer from the base score.
#[test]
fn finish_before_search_answers_nothing() {
    let deals: Vec<_> = (1..=20).map(|uid| prepared(uid * 60, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = HashSet::new();
    let params = params(&held, &defaults, &locked);
    let handle = SearchHandle::new();
    handle.finish();
    assert_eq!(
        suggest(&deals, &params, &handle).unwrap_err(),
        SearchMiss::Nothing
    );
    assert!(handle.abandoned());
}

/// Treating finish like cancel would retain inert fields; treating cancel like finish would trim them.
#[test]
fn finish_trims_passengers_while_cancel_preserves_them() {
    let point: Point = [
        ("SellPrice", "1".into()),
        ("SellLevelDelayNext", "1".into()),
    ]
    .into_iter()
    .collect();
    let evaluate = |p: &Point| {
        let mut tally = Tally::default();
        tally.push(if p.contains_key("SellPrice") {
            5.0
        } else {
            1.0
        });
        Some(tally)
    };
    for keep in [false, true] {
        let handle = SearchHandle::new();
        let score = evaluate(&point);
        if keep {
            handle.finish();
        } else {
            handle.cancel();
        }
        assert_eq!(handle.keeps(), keep);
        let (trimmed, _) =
            drop_passengers(point.clone(), score, &Point::new(), &evaluate, 1, &|| {
                handle.aborts_trim()
            });
        assert!(trimmed.contains_key("SellPrice"));
        assert_eq!(trimmed.contains_key("SellLevelDelayNext"), !keep);
    }
}

/// An answer scored inside a restart that never completes is still the user's best so far: the
/// empty-runs early return must not discard it.
#[test]
fn finish_before_any_restart_completes_keeps_the_answer() {
    let deals: Vec<_> = (1..=20).map(|uid| prepared(uid * 60, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = TICK_PARAMS
        .iter()
        .filter(|f| f.key != "SellPrice" && f.key != "StopLoss")
        .map(|f| f.key.to_string())
        .collect();
    let mut params = params(&held, &defaults, &locked);
    params.restarts = 1;
    let handle = SearchHandle::new();
    handle.finish_on_answer();
    let found = suggest(&deals, &params, &handle).expect("answer met before the stop");
    assert!(handle.is_cancelled() && handle.keeps());
    assert!(handle.abandoned());
    assert_eq!(found.stats.restarts, 0);
    assert!(found.stats.stopped);
    assert_eq!(found.stats.requested, 1);
    assert_eq!(found.stats.best_restart, 0);
    assert_eq!(found.stats.passes, 0);
    assert!(!found.stats.converged);
    assert!(found.holdout.is_some());
}

/// A tracker point is not permission to return an answer after a hard cancel with no finished run.
#[test]
fn cancel_before_any_restart_completes_answers_nothing() {
    let deals: Vec<_> = (1..=20).map(|uid| prepared(uid * 60, 101.0)).collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let locked = TICK_PARAMS
        .iter()
        .filter(|f| f.key != "SellPrice" && f.key != "StopLoss")
        .map(|f| f.key.to_string())
        .collect();
    let mut params = params(&held, &defaults, &locked);
    params.restarts = 1;
    let handle = SearchHandle::new();
    handle.cancel_on_answer();
    assert_eq!(
        suggest(&deals, &params, &handle).unwrap_err(),
        SearchMiss::Nothing
    );
    assert!(handle.aborts_trim());
    assert!(handle.abandoned());
    assert_eq!(handle.completed(), 0);
}
