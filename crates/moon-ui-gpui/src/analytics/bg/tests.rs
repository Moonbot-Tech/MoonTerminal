use super::*;

/// Replacing one half of a compound Filters read retires both of its destinations.
///
/// Removing the same-token sweep from `bg.rs:LatestReads::replace` leaves the KPI lane pointing
/// at cancelled work, so a histogram field change can strand the KPI in its loading state.
#[test]
fn replacing_compound_lane_reports_every_retired_destination() {
    let mut reads = LatestReads::default();
    let compound = reads.replace(&[ReadLane::FilterKpi, ReadLane::FilterHistogram]);
    let histogram = reads.replace(&[ReadLane::FilterHistogram]);
    let retired_kpi = reads.cancel(&[ReadLane::FilterKpi]);

    assert!(compound.is_cancelled());
    assert!(retired_kpi.is_empty());
    assert!(!histogram.is_cancelled());
    let retired_histogram = reads.cancel(&[ReadLane::FilterHistogram]);
    assert_eq!(retired_histogram, vec![ReadLane::FilterHistogram]);
    assert!(histogram.is_cancelled());
}

/// Scope invalidation cancels every destination owned by a shared request token.
///
/// Cancelling only the explicitly named lane lets the same obsolete SQLite connection continue
/// through the other destination until its full scan completes.
#[test]
fn invalidating_one_compound_lane_cancels_the_shared_request() {
    let mut reads = LatestReads::default();
    let compound = reads.replace(&[ReadLane::FilterKpi, ReadLane::FilterHistogram]);
    let mut retired = reads.cancel(&[ReadLane::FilterKpi]);
    retired.sort();

    assert!(compound.is_cancelled());
    assert_eq!(
        retired,
        vec![ReadLane::FilterKpi, ReadLane::FilterHistogram]
    );
}

/// A stale completion must not clear the token of the request that replaced it.
///
/// Making `LatestReads::finish` remove lanes by enum alone lets an old worker completion disable
/// cancellation for the newer query currently running in that lane.
#[test]
fn stale_completion_preserves_newer_lane_owner() {
    let mut reads = LatestReads::default();
    let old = reads.replace(&[ReadLane::Summary]);
    let new = reads.replace(&[ReadLane::Summary]);
    reads.finish(&old);
    let retired = reads.cancel(&[ReadLane::Summary]);

    assert!(old.is_cancelled());
    assert!(new.is_cancelled());
    assert_eq!(retired, vec![ReadLane::Summary]);
}

/// Destroying Analytics must cancel detached workers that no longer have a UI destination.
///
/// Removing `LatestReads::drop` lets a closed window's full-period SQLite scans continue until
/// completion because each detached worker still owns a clone of its cancellation token.
#[test]
fn dropping_the_registry_cancels_detached_requests() {
    let token = {
        let mut reads = LatestReads::default();
        let token = reads.replace(&[ReadLane::Summary]);
        token
    };

    assert!(token.is_cancelled());
}

/// Replaceable UI reads must carry their registry token into the SQLite worker scope.
///
/// Removing `with_read_cancellation(worker_cancellation, db)` leaves every generation and token
/// ownership test green while production SQL ignores the cancellation flag and runs to completion.
#[test]
fn latest_read_worker_installs_its_sqlite_cancellation_scope() {
    let source = include_str!("../bg.rs");
    let worker = source
        .split_once("pub(super) fn spawn_latest_db")
        .expect("latest-read helper")
        .1
        .split_once("#[cfg(test)]")
        .expect("latest-read helper boundary")
        .0;

    assert!(worker.contains("with_read_cancellation(worker_cancellation, db)"));
}

/// Scope invalidation and debounced coin edits must cancel work before replacement scheduling.
///
/// Removing these calls leaves generation guards intact but lets obsolete SQLite scans consume the
/// database until completion, which is the production regression the cancellation layer fixes.
#[test]
fn invalidation_paths_reach_the_latest_read_registry() {
    let analytics = include_str!("../mod.rs");
    let reload = analytics
        .split_once("fn reload(&mut self")
        .expect("Analytics reload")
        .1
        .split_once("fn reload_summary")
        .expect("reload boundary")
        .0;
    assert!(reload.contains("self.cancel_latest_reads();"));

    let tuner = include_str!("../tuner/mod.rs");
    for function in ["fn set_sel_strategy", "fn selection_scope_changed"] {
        let body = tuner
            .split_once(function)
            .unwrap_or_else(|| panic!("{function} body"))
            .1
            .split_once("\n    }")
            .expect("function boundary")
            .0;
        assert!(body.contains("self.cancel_axis_reads();"), "{function}");
    }

    let coins = include_str!("../tuner/coins/load.rs");
    let full = coins
        .split_once("fn reload_coins_inner")
        .expect("full Coins reload")
        .1
        .split_once("fn coin_universe")
        .expect("full Coins boundary")
        .0;
    let retired_narrow_reads = full
        .split_once("self.latest_reads.cancel(")
        .expect("full Coins narrow-read cancellation")
        .1
        .split_once("self.coins.seq")
        .expect("full Coins cancellation boundary")
        .0;
    assert!(retired_narrow_reads.contains("ReadLane::CoinKpi"));
    assert!(retired_narrow_reads.contains("ReadLane::CoinPicked"));
    let integrated_lanes = full
        .split_once("self.spawn_latest_db(")
        .expect("integrated Coins read")
        .1
        .split_once("show_overlay,")
        .expect("integrated Coins lane boundary")
        .0;
    assert!(integrated_lanes.contains("ReadLane::Coins"));
    assert!(!integrated_lanes.contains("ReadLane::CoinKpi"));
    assert!(!integrated_lanes.contains("ReadLane::CoinPicked"));

    let picked = coins
        .split_once("fn reload_picked_strats")
        .expect("picked-strategy reload")
        .1
        .split_once("fn toggle_coin_list")
        .expect("picked-strategy boundary")
        .0;
    let picked_cancel = picked.find(".cancel(").expect("picked cancellation");
    let empty_return = picked
        .find("picked.is_empty()")
        .expect("empty picked branch");
    assert!(
        picked_cancel < empty_return,
        "clearing the pick must cancel its active query"
    );

    let arm = coins
        .split_once("fn arm_coin_kpi")
        .expect("coin KPI invalidation")
        .1
        .split_once("fn run_coin_kpi")
        .expect("coin KPI boundary")
        .0;
    let cancel = arm.find(".cancel(").expect("immediate cancellation");
    let debounce = arm.find("KPI_DEBOUNCE").expect("debounce timer");
    assert!(cancel < debounce, "cancellation must precede the debounce");
}
