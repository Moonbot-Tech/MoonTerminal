use super::*;
use crate::venue::{Brand, MarketKind};

const DAY: i64 = 86_400_000;
const NOW: i64 = 500 * DAY;

/// Synthetic closed positions share no production core or market identity.
fn trade(id: i64, brand: Brand, kind: MarketKind, age_ms: i64) -> CatchupTrade {
    let close_ms = NOW - age_ms;
    CatchupTrade {
        id: TradeId {
            core: 7,
            rec_id: id,
        },
        exchange: "synthetic".into(),
        market: "SYNUSDT".into(),
        venue: Venue { brand, kind },
        open_ms: close_ms - 60_000,
        close_ms,
    }
}

/// Prioritizing recent tape would lose the REST-only window nearest its retention deadline.
#[test]
fn expiring_rest_is_first_and_archive_backed_last() {
    let trades = vec![
        trade(1, Brand::Bybit, MarketKind::Futures, 5 * DAY),
        trade(2, Brand::Gate, MarketKind::Spot, DAY),
        trade(3, Brand::Gate, MarketKind::Spot, 28 * DAY),
    ];
    let planned = plan(&trades, |_| Coverage::none(), &HashMap::new(), NOW, 0, DAY);
    assert_eq!(
        planned
            .jobs
            .iter()
            .map(|job| job.trade.id.rec_id)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    assert_eq!(planned.jobs[0].expires_at_ms, trades[2].open_ms + 29 * DAY);
}

/// Coverage must subtract held islands rather than refetching the trade's entire hull.
#[test]
fn held_complete_and_holed_windows_are_distinguished() {
    let row = trade(1, Brand::Gate, MarketKind::Spot, DAY);
    let whole = needed(&row, 1000, DAY);
    let planned = plan(
        std::slice::from_ref(&row),
        |_| whole.clone(),
        &HashMap::new(),
        NOW,
        1000,
        DAY,
    );
    assert_eq!(planned.complete, 1);
    assert!(planned.jobs.is_empty());
    let held = Coverage::from_spans([
        (row.open_ms - 1000, row.open_ms + 9999),
        (row.open_ms + 20000, row.close_ms + 1000),
    ]);
    let planned = plan(
        std::slice::from_ref(&row),
        |_| held.clone(),
        &HashMap::new(),
        NOW,
        1000,
        DAY,
    );
    assert_eq!(
        planned.jobs[0].missing,
        Coverage::one((row.open_ms + 10000, row.open_ms + 19999))
    );
}

/// A permanent refusal or an unexpired retry must never become another venue request.
#[test]
fn permanent_and_retry_latches_omit_requests_until_due() {
    let row = trade(1, Brand::Gate, MarketKind::Spot, DAY);
    for latch in [Latch::Permanent, Latch::RetryAfter(NOW + 1)] {
        let latched = HashMap::from([(row.id, latch)]);
        assert!(
            plan(
                std::slice::from_ref(&row),
                |_| Coverage::none(),
                &latched,
                NOW,
                0,
                DAY
            )
            .jobs
            .is_empty()
        );
    }
    let latched = HashMap::from([(row.id, Latch::RetryAfter(NOW))]);
    assert_eq!(
        plan(&[row], |_| Coverage::none(), &latched, NOW, 0, DAY)
            .jobs
            .len(),
        1
    );
}

/// Catch-up must wait until live recording has had its full margin and give-up interval.
#[test]
fn live_tail_waits_without_a_permanent_refusal() {
    let row = trade(1, Brand::Bybit, MarketKind::Futures, 120_999);
    let planned = plan(
        std::slice::from_ref(&row),
        |_| Coverage::none(),
        &HashMap::new(),
        NOW,
        1000,
        DAY,
    );
    assert_eq!(planned.too_fresh, 1);
    assert_eq!(planned.no_route + planned.out_of_reach, 0);
    assert!(planned.jobs.is_empty());
    assert_eq!(
        plan(
            &[row],
            |_| Coverage::none(),
            &HashMap::new(),
            NOW + 1,
            1000,
            DAY
        )
        .jobs
        .len(),
        1
    );
}

/// The 365-day close bound must exclude old rows even when their venue has an archive.
#[test]
fn depth_excludes_366_days_but_keeps_364_day_archives() {
    let rows = [
        trade(1, Brand::Bybit, MarketKind::Futures, 366 * DAY),
        trade(2, Brand::Bybit, MarketKind::Futures, 364 * DAY),
    ];
    let planned = plan(&rows, |_| Coverage::none(), &HashMap::new(), NOW, 0, DAY);
    assert_eq!(planned.out_of_reach, 1);
    assert_eq!(planned.jobs.len(), 1);
    assert_eq!(planned.jobs[0].trade.id, rows[1].id);
}

/// A REST-only trade crossing retention may still recover its reachable exit without old slices.
#[test]
fn rest_retention_crossing_keeps_only_reachable_missing_tape() {
    let mut row = trade(1, Brand::Gate, MarketKind::Spot, 28 * DAY);
    row.open_ms = NOW - 31 * DAY;
    let planned = plan(
        &[row],
        |_| Coverage::none(),
        &HashMap::new(),
        NOW,
        0,
        40 * DAY,
    );
    assert_eq!(
        planned.jobs[0].missing,
        Coverage::one((
            NOW - 29 * DAY + venue_caps::RETENTION_SAFETY_MS,
            NOW - 28 * DAY
        ))
    );
}

/// A source-less row must be permanently omitted after its first recorder planning pass.
#[test]
fn no_route_is_latched_by_recorder_and_absent_on_resubmit() {
    let row = trade(1, Brand::Htx, MarketKind::Spot, DAY);
    let mut state = state::RecorderCatchup::default();
    state.submit(vec![row.clone()]);
    let first = state.prepare(|_| Coverage::none(), NOW, 0, DAY);
    assert_eq!(first.no_route, 1);
    assert!(first.jobs.is_empty());
    state.submit(vec![row]);
    let second = state.prepare(
        |_| panic!("latched row must not ask disk"),
        NOW + DAY,
        0,
        DAY,
    );
    assert_eq!(second.no_route, 0);
    assert!(second.jobs.is_empty());
}

/// Held reachable tape must not make an unfillable old hole count as a completed trade.
#[test]
fn clipped_away_missing_window_is_out_of_reach_and_latched() {
    let mut row = trade(1, Brand::Gate, MarketKind::Spot, 28 * DAY);
    row.open_ms = NOW - 31 * DAY;
    let earliest = NOW - 29 * DAY + venue_caps::RETENTION_SAFETY_MS;
    let mut state = state::RecorderCatchup::default();
    state.submit(vec![row.clone()]);
    let result = state.prepare(|_| Coverage::one((earliest, NOW)), NOW, 0, 40 * DAY);
    assert_eq!(result.out_of_reach, 1);
    assert_eq!(result.complete, 0);
    assert!(result.jobs.is_empty());
    state.submit(vec![row]);
    assert!(
        state
            .prepare(
                |_| panic!("permanent window asked disk again"),
                NOW + DAY,
                0,
                40 * DAY
            )
            .jobs
            .is_empty()
    );
}

/// Complete and permanently refused IDs must stop repeated disk reads but age out after 365 days.
#[test]
fn complete_ids_skip_resubmission_and_old_settlements_are_pruned() {
    let row = trade(1, Brand::Gate, MarketKind::Spot, DAY);
    let mut state = state::RecorderCatchup::default();
    state.submit(vec![row.clone()]);
    assert_eq!(
        state
            .prepare(|trade| needed(trade, 0, DAY), NOW, 0, DAY)
            .complete,
        1
    );
    state.submit(vec![row.clone()]);
    assert!(
        state
            .prepare(|_| panic!("completed ID was re-planned"), NOW + DAY, 0, DAY)
            .jobs
            .is_empty()
    );
    state.prepare(|_| panic!("nothing pending"), NOW + 366 * DAY, 0, DAY);
    state.submit(vec![row]);
    assert_eq!(
        state
            .prepare(|_| panic!("old row asked disk"), NOW + 366 * DAY, 0, DAY)
            .out_of_reach,
        1
    );

    let refused = trade(2, Brand::Htx, MarketKind::Spot, DAY);
    let mut state = state::RecorderCatchup::default();
    state.submit(vec![refused.clone()]);
    assert_eq!(state.prepare(|_| Coverage::none(), NOW, 0, DAY).no_route, 1);
    state.prepare(|_| panic!("nothing pending"), NOW + 366 * DAY, 0, DAY);
    state.submit(vec![refused]);
    assert_eq!(
        state
            .prepare(|_| panic!("old row asked disk"), NOW + 366 * DAY, 0, DAY)
            .out_of_reach,
        1
    );
}

use crate::feed::{Side, Tick};
use crate::market::trade_replay::{archive, gate, rest};
use executor::{ARCHIVE_BYTES_PER_PASS, Failure};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// Derive a test job from the real planner so each transport receives its real missing focus.
fn job(id: i64, brand: Brand, kind: MarketKind) -> CatchupJob {
    plan(
        &[trade(id, brand, kind, DAY)],
        |_| Coverage::none(),
        &HashMap::new(),
        NOW,
        0,
        DAY,
    )
    .jobs
    .remove(0)
}

/// A synthetic print whose timestamp is independent of coverage calculations.
fn tick(at: i64) -> Tick {
    Tick {
        time_ms: at as f64,
        price: 10.0,
        qty: 2.0,
        side: Side::Buy,
    }
}

/// A complete REST page returns its inclusive endpoints, without network or credentials.
fn good_rest(
    _: TradeRoute,
    _: &str,
    from: i64,
    to: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    rest::publish_exchange_notice(rest::ExchangeNotice::default());
    Ok(rest::TradePage {
        ticks: vec![tick(from), tick(to)],
        next: None,
    })
}

/// Unexpected REST work on an archive-only fixture is a regression, not an empty answer.
fn no_rest(
    _: TradeRoute,
    _: &str,
    _: i64,
    _: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    panic!("unexpected REST request")
}

/// Unexpected archive work on a REST-only fixture must fail before any I/O could happen.
fn no_archive(
    _: ArchiveRoute,
    _: &str,
    _: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    panic!("unexpected archive request")
}

/// Fixture refusal publishes the same 418/Retry-After notice consumed by production paging.
fn throttled_rest(
    _: TradeRoute,
    _: &str,
    _: i64,
    _: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    rest::publish_exchange_notice(rest::ExchangeNotice {
        status: 418,
        retry_after_s: Some(60),
        used_weight_1m: None,
    });
    Err(rest::FetchError::Throttled {
        retry_after_s: Some(60),
        diagnostic: "synthetic HTTP 418".into(),
    })
}

/// Unknown symbols are a stable market refusal; no backoff should turn them into hourly retries.
fn unknown_rest(
    _: TradeRoute,
    _: &str,
    _: i64,
    _: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    rest::publish_exchange_notice(rest::ExchangeNotice::default());
    Err(rest::FetchError::UnknownSymbol)
}

/// A corrupt published day stays uncovered without refusing other daily files for the trade.
fn corrupt_archive(
    _: ArchiveRoute,
    _: &str,
    _: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    (
        Err(archive::ArchiveError::Corrupt(
            "synthetic malformed row".into(),
        )),
        0,
    )
}

/// A synthetic archive intentionally overshoots the asked span to exercise the filing boundary.
fn overshoot_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    let (from, to) = spans.hull().unwrap();
    (
        Ok(archive::ArchiveFill {
            covered: Coverage::one((from - 1000, to + 1000)),
            ticks: vec![tick(from - 1), tick(from), tick(to), tick(to + 1)],
            deferred: None,
        }),
        0,
    )
}

/// Simulate a completed archive stretch followed by a host throttle on the next day.
fn partial_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    let (from, _) = spans.hull().unwrap();
    (
        Ok(archive::ArchiveFill {
            covered: Coverage::one((from, from + 1000)),
            ticks: vec![tick(from)],
            deferred: Some(archive::ArchiveError::Throttled {
                retry_after_s: Some(60),
            }),
        }),
        0,
    )
}

/// Catch-up must keep completed coverage even when the same archive fill throttles its host.
#[test]
fn deferred_archive_throttle_keeps_completed_coverage() {
    let job = job(1, Brand::Bybit, MarketKind::Futures);
    let (from, _) = job.missing.hull().unwrap();
    let outcome = executor::run_job_at(
        &job,
        NOW,
        &gate::ReplayGate::new(),
        no_rest,
        partial_archive,
    );
    assert_eq!(outcome.file, Coverage::one((from, from + 1000)));
    assert_eq!(outcome.ticks.len(), 1);
    assert_eq!(outcome.failure, Some(Failure::Throttled));
}

/// A throttled host must leave later same-host jobs pending without new transport attempts.
#[test]
fn throttled_host_defers_later_same_host_jobs() {
    let jobs = vec![
        job(1, Brand::Gate, MarketKind::Spot),
        job(2, Brand::Gate, MarketKind::Spot),
    ];
    let result = executor::run_pass_with(
        jobs,
        || NOW,
        &gate::ReplayGate::new(),
        (throttled_rest, no_archive),
        0,
        executor::run_job_at,
        |_, outcome| assert!(outcome.file.is_empty()),
    );
    assert_eq!(result.finished[0].failure, Some(Failure::Throttled));
    assert!(result.finished[0].attempted);
    assert!(!result.finished[1].attempted);
    assert_eq!(result.finished[1].failure, Some(Failure::Retry));
}

/// Refuse Gate's host while allowing another host through the same real paginator.
fn one_host_throttled_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    if route == TradeRoute::GateSpotTrades {
        throttled_rest(route, market, from, to, cursor)
    } else {
        good_rest(route, market, from, to, cursor)
    }
}

/// A throttle must defer its host's next job without blocking another venue's recovery.
#[test]
fn throttled_host_does_not_stop_another_host() {
    let jobs = vec![
        job(1, Brand::Gate, MarketKind::Spot),
        job(2, Brand::Gate, MarketKind::Spot),
        job(3, Brand::Binance, MarketKind::Spot),
    ];
    let mut filed = Vec::new();
    let result = executor::run_pass_with(
        jobs,
        || NOW,
        &gate::ReplayGate::new(),
        (one_host_throttled_rest, no_archive),
        0,
        executor::run_job_at,
        |job, outcome| {
            if !outcome.file.is_empty() {
                filed.push(job.trade.id.rec_id);
            }
        },
    );
    assert_eq!(result.finished[0].failure, Some(Failure::Throttled));
    assert!(!result.finished[1].attempted);
    assert_eq!(result.finished[1].failure, Some(Failure::Retry));
    assert!(result.finished[2].attempted);
    assert_eq!(result.finished[2].failure, None);
    assert_eq!(filed, vec![3]);
}

/// Catch-up must publish a 418 into the same gate a different caller subsequently claims.
#[test]
fn catchup_418_blocks_other_shared_gate_callers() {
    let mut job = job(1, Brand::Gate, MarketKind::Spot);
    let now = crate::util::now_unix_ms_i64();
    job.missing = Coverage::one((now - 1000, now - 1));
    let outcome = run_job(&job, throttled_rest, no_archive);
    assert_eq!(outcome.failure, Some(Failure::Throttled));
    assert!(
        gate::shared()
            .claim(TradeRoute::GateSpotTrades.host(), std::time::Instant::now())
            .is_err()
    );
}

/// Unknown REST symbols are permanent; a damaged archive day must leave its trade retryable.
#[test]
fn unknown_rest_is_permanent_but_corrupt_archive_is_retryable() {
    let rest_job = job(1, Brand::Gate, MarketKind::Spot);
    assert_eq!(
        executor::run_job_at(
            &rest_job,
            NOW,
            &gate::ReplayGate::new(),
            unknown_rest,
            no_archive
        )
        .failure,
        Some(Failure::Permanent)
    );
    let archive_job = job(2, Brand::Bybit, MarketKind::Futures);
    assert_eq!(
        executor::run_job_at(
            &archive_job,
            NOW,
            &gate::ReplayGate::new(),
            no_rest,
            corrupt_archive
        )
        .failure,
        Some(Failure::Retry)
    );
}

/// Filing a returned hull would falsely mark unrequested ground held and retain overshooting ticks.
#[test]
fn filed_harvest_is_clipped_exactly_to_missing_spans() {
    let mut job = job(1, Brand::Bybit, MarketKind::Futures);
    let from = job.trade.open_ms;
    job.missing = Coverage::from_spans([(from, from + 10), (from + 100, from + 110)]);
    let outcome = executor::run_job_at(
        &job,
        NOW,
        &gate::ReplayGate::new(),
        no_rest,
        overshoot_archive,
    );
    assert_eq!(outcome.file, job.missing);
    assert_eq!(
        outcome
            .ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![from, from + 110]
    );
}

static HOLE_REQUESTS: Mutex<Vec<(i64, i64)>> = Mutex::new(Vec::new());
/// Record every real paginator slice to compare it with an independently specified held island.
fn hole_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    HOLE_REQUESTS.lock().unwrap().push((from, to));
    good_rest(route, market, from, to, cursor)
}

/// Intersection must happen before paging, so a held interior never becomes a venue request.
#[test]
fn holed_trade_sends_no_request_for_held_slices() {
    HOLE_REQUESTS.lock().unwrap().clear();
    let mut job = job(1, Brand::Gate, MarketKind::Spot);
    let from = job.trade.open_ms;
    job.missing = Coverage::from_spans([(from, from + 999), (from + 2000, from + 2999)]);
    let outcome = executor::run_job_at(&job, NOW, &gate::ReplayGate::new(), hole_rest, no_archive);
    assert_eq!(outcome.file, job.missing);
    assert_eq!(
        *HOLE_REQUESTS.lock().unwrap(),
        vec![(from, from + 999), (from + 2000, from + 2999)]
    );
}

static BUDGET_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Simulate a small compressed download that spends the last available kilobyte of a pass.
fn budget_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    budget: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    if budget < 1024 {
        return (Err(archive::ArchiveError::BudgetDeferred), 0);
    }
    BUDGET_CALLS.fetch_add(1, Ordering::Relaxed);
    (
        Ok(archive::ArchiveFill {
            covered: spans.clone(),
            ticks: vec![tick(spans.hull().unwrap().0)],
            deferred: None,
        }),
        1024,
    )
}

/// The compressed-byte cap defers later archive days but must keep a following REST job runnable.
#[test]
fn spent_archive_budget_defers_archives_and_continues_rest() {
    BUDGET_CALLS.store(0, Ordering::Relaxed);
    let first = job(1, Brand::Bybit, MarketKind::Futures);
    let mut second = job(2, Brand::Bybit, MarketKind::Futures);
    // Keep this job two days away: a midnight endpoint one day away shares a cached file,
    // whose paid-for coverage is correctly reusable even after the download budget is spent.
    second.missing = Coverage::one((
        second.trade.open_ms - 2 * DAY,
        second.trade.close_ms - 2 * DAY,
    ));
    let rest = job(3, Brand::Gate, MarketKind::Spot);
    let mut filed = Vec::new();
    let result = executor::run_pass_with(
        vec![first, second, rest],
        || NOW,
        &gate::ReplayGate::new(),
        (good_rest, budget_archive),
        ARCHIVE_BYTES_PER_PASS - 1024,
        executor::run_job_at,
        |job, outcome| {
            if !outcome.file.is_empty() {
                filed.push(job.trade.id.rec_id);
            }
        },
    );
    assert_eq!(BUDGET_CALLS.load(Ordering::Relaxed), 1);
    assert_eq!(result.archive_bytes, ARCHIVE_BYTES_PER_PASS);
    assert_eq!(result.finished[1].failure, Some(Failure::Retry));
    assert!(
        !result.finished[1].attempted,
        "budget-deferred job must not count as failed"
    );
    assert_eq!(result.finished[2].failure, None);
    assert_eq!(filed, vec![3, 1]);
}

static FAILED_ARCHIVE_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Report compressed bytes received before a synthetic transport disconnect.
fn failed_archive(
    _: ArchiveRoute,
    _: &str,
    _: &Coverage,
    budget: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    if budget < 128 {
        return (Err(archive::ArchiveError::BudgetDeferred), 0);
    }
    FAILED_ARCHIVE_CALLS.fetch_add(1, Ordering::Relaxed);
    (
        Err(archive::ArchiveError::Transient(
            "synthetic disconnect after body bytes".into(),
        )),
        128,
    )
}

/// Failed bodies spend this pass's local budget and defer another day without recording failure.
#[test]
fn failed_archive_bytes_spend_only_the_fetchers_budget() {
    FAILED_ARCHIVE_CALLS.store(0, Ordering::Relaxed);
    let first = job(1, Brand::Bybit, MarketKind::Futures);
    let mut second = job(2, Brand::Bybit, MarketKind::Futures);
    second.trade.market = "SECONDUSDT".into();
    let result = executor::run_pass_with(
        vec![first, second],
        || NOW,
        &gate::ReplayGate::new(),
        (no_rest, failed_archive),
        ARCHIVE_BYTES_PER_PASS - 128,
        executor::run_job_at,
        |_, outcome| assert!(outcome.file.is_empty()),
    );
    assert_eq!(FAILED_ARCHIVE_CALLS.load(Ordering::Relaxed), 1);
    assert_eq!(result.archive_bytes, ARCHIVE_BYTES_PER_PASS);
    assert!(result.finished[0].attempted);
    assert!(!result.finished[1].attempted);
    assert_eq!(result.finished[1].failure, Some(Failure::Retry));
}

/// A successful REST portion followed by an archive budget skip is still deferred, not failed.
#[test]
fn mixed_job_budget_deferral_does_not_count_successful_rest_as_failed() {
    let mut job = job(1, Brand::Binance, MarketKind::Futures);
    job.missing = Coverage::from_spans([
        (NOW - 3 * DAY, NOW - 3 * DAY + 1000),
        (NOW - DAY, NOW - DAY + 1000),
    ]);
    let result = executor::run_pass_with(
        vec![job],
        || NOW,
        &gate::ReplayGate::new(),
        (good_rest, no_archive),
        ARCHIVE_BYTES_PER_PASS,
        executor::run_job_at,
        |_, outcome| assert_eq!(outcome.file, Coverage::one((NOW - DAY, NOW - DAY + 1000))),
    );
    assert!(!result.finished[0].attempted);
    assert_eq!(result.finished[0].failure, Some(Failure::Retry));
}

static GROUP_CALLS: AtomicUsize = AtomicUsize::new(0);
/// One synthetic day returns every requested trade's endpoints from the union passed to it.
fn grouped_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    GROUP_CALLS.fetch_add(1, Ordering::Relaxed);
    assert_eq!(spans.spans().len(), 2);
    (
        Ok(archive::ArchiveFill {
            covered: spans.clone(),
            ticks: spans
                .spans()
                .iter()
                .flat_map(|&(from, to)| [tick(from), tick(to)])
                .collect(),
            deferred: None,
        }),
        1234,
    )
}

/// Same-file trades must share one fill while each receives only its own exhaustive spans.
#[test]
fn same_route_market_day_trades_share_one_archive_fill() {
    GROUP_CALLS.store(0, Ordering::Relaxed);
    let first = job(1, Brand::Bybit, MarketKind::Futures);
    let mut second = job(2, Brand::Bybit, MarketKind::Futures);
    let start = first.trade.close_ms.div_euclid(DAY) * DAY - DAY;
    let mut first = first;
    first.missing = Coverage::one((start + 1000, start + 1999));
    second.missing = Coverage::one((start + 3000, start + 3999));
    let mut filed = Vec::new();
    let result = executor::run_pass_with(
        vec![first, second],
        || NOW,
        &gate::ReplayGate::new(),
        (no_rest, grouped_archive),
        0,
        executor::run_job_at,
        |job, outcome| {
            assert_eq!(outcome.file, job.missing);
            assert_eq!(outcome.ticks.len(), 2);
            filed.push(job.trade.id.rec_id);
        },
    );
    assert_eq!(GROUP_CALLS.load(Ordering::Relaxed), 1);
    assert_eq!(
        result.archive_bytes, 1234,
        "shared day bytes are charged once, not per trade"
    );
    assert_eq!(filed, vec![1, 2]);
    assert!(result.finished.iter().all(|job| job.failure.is_none()));
}

/// Clearing settled IDs on eviction must allow an hourly resubmit to find missing tape again.
#[test]
fn eviction_rechecks_previously_complete_trade() {
    let row = trade(1, Brand::Gate, MarketKind::Spot, DAY);
    let mut state = state::RecorderCatchup::default();
    state.submit(vec![row.clone()]);
    assert_eq!(
        state
            .prepare(|trade| needed(trade, 0, DAY), NOW, 0, DAY)
            .complete,
        1
    );
    state.evicted();
    state.submit(vec![row]);
    let plan = state.prepare(|_| Coverage::none(), NOW, 0, DAY);
    assert_eq!(plan.jobs.len(), 1);
    assert_eq!(plan.complete, 0);
}

/// An authoritative empty page is safe away from the edge, but must not cover edge uncertainty.
fn empty_rest(
    _: TradeRoute,
    _: &str,
    _: i64,
    _: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    rest::publish_exchange_notice(rest::ExchangeNotice::default());
    Ok(rest::TradePage {
        ticks: Vec::new(),
        next: None,
    })
}

/// Filing edge-empty slices would permanently hide tape just beyond a rolling venue boundary.
#[test]
fn empty_rest_near_retention_edge_retries_without_coverage() {
    let mut row = job(1, Brand::Gate, MarketKind::Spot);
    row.missing = Coverage::one((NOW - 29 * DAY + 3_600_000, NOW - 29 * DAY + 3_601_000));
    let answer = executor::run_job_at(&row, NOW, &gate::ReplayGate::new(), empty_rest, no_archive);
    assert!(answer.file.is_empty());
    assert_eq!(answer.failure, Some(Failure::Retry));
    row.missing = Coverage::one((NOW - 28 * DAY + 1, NOW - 28 * DAY + 1000));
    let answer = executor::run_job_at(&row, NOW, &gate::ReplayGate::new(), empty_rest, no_archive);
    assert_eq!(answer.file, row.missing);
    assert_eq!(answer.failure, None);
}

/// Return an empty old edge slice and prints for the still-reachable exit neighborhood.
fn mixed_edge_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    if from < NOW - 28 * DAY {
        empty_rest(route, market, from, to, cursor)
    } else {
        good_rest(route, market, from, to, cursor)
    }
}

/// Prints in another slice must not make an empty retention-edge neighborhood authoritative.
#[test]
fn ready_rest_harvest_keeps_empty_edge_slice_uncovered() {
    let mut row = job(1, Brand::Gate, MarketKind::Spot);
    let edge = (NOW - 29 * DAY + 3_600_000, NOW - 29 * DAY + 3_601_000);
    let fresh = (NOW - DAY, NOW - DAY + 1000);
    row.missing = Coverage::from_spans([edge, fresh]);
    let answer = executor::run_job_at(
        &row,
        NOW,
        &gate::ReplayGate::new(),
        mixed_edge_rest,
        no_archive,
    );
    assert_eq!(answer.file, Coverage::one(fresh));
    assert_eq!(answer.failure, Some(Failure::Retry));
}

/// Both-source trades must use their reachable REST expiry instead of archive-last priority.
#[test]
fn both_source_jobs_sort_by_rest_expiry() {
    let rows = [
        trade(1, Brand::Binance, MarketKind::Futures, DAY),
        trade(2, Brand::Binance, MarketKind::Futures, DAY + 3_600_000),
    ];
    let plan = plan(&rows, |_| Coverage::none(), &HashMap::new(), NOW, 0, DAY);
    assert_eq!(
        plan.jobs
            .iter()
            .map(|job| job.trade.id.rec_id)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert!(plan.jobs.iter().all(|job| job.expires_at_ms < i64::MAX));
}

static SOURCE_ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Record real REST transport calls independently of the pass's job order.
fn ordered_rest(
    route: TradeRoute,
    market: &str,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    SOURCE_ORDER.lock().unwrap().push("rest");
    good_rest(route, market, from, to, cursor)
}
/// Record real archive transport calls independently of the pass's job order.
fn ordered_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    SOURCE_ORDER.lock().unwrap().push("archive");
    (
        Ok(archive::ArchiveFill {
            covered: spans.clone(),
            ticks: vec![tick(spans.hull().unwrap().0)],
            deferred: None,
        }),
        1,
    )
}

/// Interleaving per-job downloads would let slow archives postpone another trade's expiring REST.
#[test]
fn every_rest_portion_precedes_any_archive_download() {
    SOURCE_ORDER.lock().unwrap().clear();
    let mut mixed = job(1, Brand::Binance, MarketKind::Futures);
    mixed.missing = Coverage::from_spans([
        (NOW - 3 * DAY, NOW - 3 * DAY + 1000),
        (NOW - DAY, NOW - DAY + 1000),
    ]);
    executor::run_pass_with(
        vec![mixed, job(2, Brand::Gate, MarketKind::Spot)],
        || NOW,
        &gate::ReplayGate::new(),
        (ordered_rest, ordered_archive),
        0,
        executor::run_job_at,
        |_, _| {},
    );
    assert_eq!(
        *SOURCE_ORDER.lock().unwrap(),
        vec!["rest", "rest", "archive"]
    );
}

/// A day seam reports unknown only when its entire file is missing.
fn unknown_archive(
    _: ArchiveRoute,
    _: &str,
    _: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    (Err(archive::ArchiveError::UnknownSymbol), 0)
}
/// The first missing file must leave the later available day reachable.
fn one_missing_archive(
    route: ArchiveRoute,
    market: &str,
    spans: &Coverage,
    budget: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    if spans.hull().unwrap().0 < NOW - 4 * DAY {
        unknown_archive(route, market, spans, budget)
    } else {
        (
            Ok(archive::ArchiveFill {
                covered: spans.clone(),
                ticks: Vec::new(),
                deferred: None,
            }),
            0,
        )
    }
}

/// Per-day unknown results become permanent only if every requested daily file is unknown.
#[test]
fn archive_trade_latches_only_when_every_day_is_unknown() {
    let mut row = job(1, Brand::Bybit, MarketKind::Futures);
    row.missing = Coverage::one((NOW - 5 * DAY, NOW - 3 * DAY - 1));
    let trade = row.trade.clone();
    let cache = crate::market::trade_replay::trade_cache::TradeCache::open_evictable(
        std::path::PathBuf::from(":memory:"),
    )
    .unwrap();
    assert!(cache.sync(std::time::Duration::from_secs(2)));
    let partial = executor::run_pass_with(
        vec![row.clone()],
        || NOW,
        &gate::ReplayGate::new(),
        (no_rest, one_missing_archive),
        0,
        executor::run_job_at,
        |_, _| {},
    );
    assert_eq!(partial.finished[0].failure, Some(Failure::Retry));
    let mut partial_state = state::RecorderCatchup::default();
    partial_state.submit(vec![trade.clone()]);
    partial_state.finish(&cache, partial, NOW);
    partial_state.submit(vec![trade.clone()]);
    assert_eq!(
        partial_state
            .prepare(|_| Coverage::none(), NOW + DAY, 0, DAY)
            .jobs
            .len(),
        1
    );
    let all = executor::run_pass_with(
        vec![row],
        || NOW,
        &gate::ReplayGate::new(),
        (no_rest, unknown_archive),
        0,
        executor::run_job_at,
        |_, _| {},
    );
    assert_eq!(all.finished[0].failure, Some(Failure::Permanent));
    let mut all_state = state::RecorderCatchup::default();
    all_state.submit(vec![trade.clone()]);
    all_state.finish(&cache, all, NOW);
    all_state.submit(vec![trade]);
    assert!(
        all_state
            .prepare(
                |_| panic!("permanently refused trade read disk"),
                NOW + DAY,
                0,
                DAY
            )
            .jobs
            .is_empty()
    );
}

/// An unpublished day returns no completed work and no download failure.
fn unpublished_archive(
    _: ArchiveRoute,
    _: &str,
    _: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    (
        Ok(archive::ArchiveFill {
            covered: Coverage::none(),
            ticks: Vec::new(),
            deferred: None,
        }),
        0,
    )
}

/// A pass with no publishable archive date is waiting, not a failed attempt in station status.
#[test]
fn unpublished_archive_wait_does_not_count_as_failed() {
    let result = executor::run_pass_with(
        vec![job(1, Brand::Bybit, MarketKind::Futures)],
        || NOW,
        &gate::ReplayGate::new(),
        (no_rest, unpublished_archive),
        0,
        executor::run_job_at,
        |_, _| {},
    );
    assert!(!result.finished[0].attempted);
    assert_ne!(result.finished[0].failure, Some(Failure::Permanent));
}

/// A slow preceding job must not leave the next REST boundary frozen at the pass's start.
#[test]
fn pass_refreshes_clock_for_each_rest_job() {
    let clock = std::cell::Cell::new(NOW);
    let seen = std::cell::RefCell::new(Vec::new());
    executor::run_pass_with(
        vec![
            job(1, Brand::Gate, MarketKind::Spot),
            job(2, Brand::Gate, MarketKind::Futures),
        ],
        || {
            let now = clock.get();
            clock.set(now + 1000);
            now
        },
        &gate::ReplayGate::new(),
        (no_rest, no_archive),
        0,
        |job, now, _, _, _| {
            seen.borrow_mut().push(now);
            let mut answer = executor::JobOutcome::default();
            answer.file = job.missing.clone();
            answer
        },
        |_, _| {},
    );
    assert_eq!(*seen.borrow(), vec![NOW, NOW + 1000]);
}

static EXPIRED_ARCHIVE_SPANS: Mutex<Vec<Coverage>> = Mutex::new(Vec::new());
/// Expired REST ranges are a time refusal, not a host refusal.
fn expired_rest(
    _: TradeRoute,
    _: &str,
    _: i64,
    _: i64,
    _: Option<rest::TradeCursor>,
) -> Result<rest::TradePage, rest::FetchError> {
    rest::publish_exchange_notice(rest::ExchangeNotice::default());
    Err(rest::FetchError::OutOfRetention)
}
/// Record the archive fallback's received portions after the REST classifier expires a slice.
fn expired_fallback_archive(
    _: ArchiveRoute,
    _: &str,
    spans: &Coverage,
    _: u64,
) -> (Result<archive::ArchiveFill, archive::ArchiveError>, u64) {
    EXPIRED_ARCHIVE_SPANS.lock().unwrap().push(spans.clone());
    (
        Ok(archive::ArchiveFill {
            covered: spans.clone(),
            ticks: Vec::new(),
            deferred: None,
        }),
        0,
    )
}

/// A Binance -4166 slice must move into archive work while leaving the REST host open.
#[test]
fn expired_rest_portion_moves_to_archive() {
    EXPIRED_ARCHIVE_SPANS.lock().unwrap().clear();
    let mut row = job(1, Brand::Binance, MarketKind::Futures);
    row.missing = Coverage::from_spans([
        (NOW - 3 * DAY, NOW - 3 * DAY + 1000),
        (NOW - DAY, NOW - DAY + 1000),
    ]);
    let gate = gate::ReplayGate::new();
    let missing = row.missing.clone();
    executor::run_pass_with(
        vec![row],
        || NOW,
        &gate,
        (expired_rest, expired_fallback_archive),
        0,
        executor::run_job_at,
        |_, _| {},
    );
    assert_eq!(
        Coverage::from_spans(
            EXPIRED_ARCHIVE_SPANS
                .lock()
                .unwrap()
                .iter()
                .flat_map(|coverage| coverage.spans().iter().copied())
        ),
        missing
    );
    assert!(
        gate.claim(
            TradeRoute::BinanceUsdMAggTrades.host(),
            std::time::Instant::now()
        )
        .is_ok()
    );
}
