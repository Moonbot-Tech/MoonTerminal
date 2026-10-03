//! Pins for trade announcement. Each test names the edit that would break it.

use moon_core::telegram::notify::{CoreScope, NotifyLedger, TradeRule};

use super::*;

fn trade(core: u64, rec_id: i64, close_utc: i64) -> ClosedTrade {
    ClosedTrade {
        core,
        rec_id,
        close_utc,
        coin: "BTC".to_string(),
        core_name: "alpha".to_string(),
        strategy: "grid".to_string(),
        volume_usd: Some(100.0),
        profit_usd: Some(1.0),
        profit_pct: Some(0.5),
        open_utc: close_utc.saturating_sub(60),
    }
}

fn enabled(at: i64) -> NotifyLedger {
    NotifyLedger {
        trades_enabled_utc: Some(at),
        ..NotifyLedger::default()
    }
}

fn on_rule() -> TradeRule {
    TradeRule {
        on: true,
        ..TradeRule::default()
    }
}

fn ids(trades: &[ClosedTrade]) -> Vec<i64> {
    trades.iter().map(|trade| trade.rec_id).collect()
}

/// Ignoring `trades_enabled_utc`, or using a window shorter than 72 hours, would
/// either flood a chat with old closes or hide a close that is still in range.
#[test]
fn read_from_utc_is_the_later_of_enable_and_window() {
    assert_eq!(read_from_utc(&NotifyLedger::default(), 1_000), None);
    let early = enabled(100);
    let now = 100 + SEEN_WINDOW_SECS + 50;
    assert_eq!(read_from_utc(&early, now), Some(150));
    let late = enabled(500_000);
    assert_eq!(read_from_utc(&late, 500_010), Some(500_000));
}

/// Treating closes from before the switch as new would dump history into the chat
/// the moment trade notices turn on. The close at the enable second is in range.
#[test]
fn closes_before_enable_stay_silent() {
    let enabled_at = 1_700_000_000;
    let mut rows: Vec<_> = (1..=10).map(|i| trade(1, i, enabled_at - i)).collect();
    rows.push(trade(1, 11, enabled_at));
    let mut ledger = enabled(enabled_at);
    let got = decide(&on_rule(), &mut ledger, &[1], &rows, enabled_at);
    assert_eq!(ids(&got), vec![11]);
    let marked = ledger.seen.get(&1).expect("boundary row marked");
    assert_eq!(marked.keys().copied().collect::<Vec<_>>(), vec![11]);
}

/// Forgetting to record `seen` would send the same card again after a restart.
#[test]
fn restart_does_not_repeat_a_card() {
    let now = 1_700_000_000;
    let rows = [trade(1, 4, now - 10)];
    let mut ledger = enabled(now - 1_000);
    let first = decide(&on_rule(), &mut ledger, &[1], &rows, now);
    assert_eq!(ids(&first), vec![4]);
    let second = decide(&on_rule(), &mut ledger, &[1], &rows, now);
    assert!(second.is_empty());
}

/// A watermark on the highest `rec_id` would drop a row that closes late but
/// carries an older id, and the chat would miss that trade.
#[test]
fn late_row_with_a_lower_rec_id_is_announced_once() {
    let now = 1_800_000_000;
    let mut ledger = enabled(now - 1_000);
    let first = vec![trade(1, 10, now - 50), trade(1, 11, now - 40)];
    assert_eq!(
        ids(&decide(&on_rule(), &mut ledger, &[1], &first, now)),
        vec![10, 11]
    );
    let late = [trade(1, 3, now - 30)];
    assert_eq!(
        ids(&decide(&on_rule(), &mut ledger, &[1], &late, now)),
        vec![3]
    );
    assert!(decide(&on_rule(), &mut ledger, &[1], &late, now).is_empty());
}

/// Collapsing equal close times to one row would hide one of two trades that
/// closed in the same second.
#[test]
fn equal_close_times_keep_every_rec_id() {
    let now = 1_800_000_000;
    let close = now - 5;
    let rows = vec![trade(1, 8, close), trade(1, 2, close), trade(1, 5, close)];
    let mut ledger = enabled(now - 1_000);
    let got = decide(&on_rule(), &mut ledger, &[1], &rows, now);
    assert_eq!(ids(&got), vec![2, 5, 8]);
}

/// Moving a boundary from `>=` to `>` would swallow a trade that lands exactly
/// on the volume, profit, or loss figure the user set.
#[test]
fn filter_boundaries_send_the_exact_figure() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.min_volume_usd = Some(50.0);
    let mut row = trade(1, 1, now - 10);
    row.volume_usd = Some(50.0);
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.volume_usd = Some(49.0);
    row.rec_id = 2;
    assert_eq!(judged(&rule, &row, now), (false, true));

    rule = on_rule();
    rule.profit_at_least_usd = Some(5.0);
    row.profit_usd = Some(5.0);
    row.rec_id = 3;
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.profit_usd = Some(4.0);
    row.rec_id = 4;
    assert_eq!(judged(&rule, &row, now), (false, true));

    rule = on_rule();
    rule.loss_at_least_usd = Some(5.0);
    row.profit_usd = Some(-5.0);
    row.rec_id = 5;
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.profit_usd = Some(-4.0);
    row.rec_id = 6;
    assert_eq!(judged(&rule, &row, now), (false, true));

    rule.profit_at_least_usd = Some(5.0);
    row.profit_usd = Some(5.0);
    row.rec_id = 7;
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.profit_usd = Some(-5.0);
    row.rec_id = 8;
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.profit_usd = Some(1.0);
    row.rec_id = 9;
    assert_eq!(judged(&rule, &row, now), (false, true));
}

/// Sending a row with no volume when a minimum is set would announce a trade
/// the user asked to keep quiet. The row stays marked so it is not retried.
#[test]
fn missing_volume_is_not_sent_when_a_minimum_is_set() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.min_volume_usd = Some(50.0);
    let mut row = trade(1, 1, now - 10);
    row.volume_usd = None;
    assert_eq!(judged(&rule, &row, now), (false, true));
}

/// Announcing an unvalued profit while a threshold is set would card a row the
/// user cannot judge. With both thresholds off, that same row is a normal card.
#[test]
fn unvalued_profit_sends_only_when_both_thresholds_are_off() {
    let now = 1_700_000_000;
    let mut row = trade(1, 1, now - 10);
    row.profit_usd = None;
    let mut rule = on_rule();
    rule.profit_at_least_usd = Some(5.0);
    assert_eq!(judged(&rule, &row, now), (false, true));
    rule = on_rule();
    rule.loss_at_least_usd = Some(5.0);
    row.rec_id = 2;
    assert_eq!(judged(&rule, &row, now), (false, true));
    row.rec_id = 3;
    assert_eq!(judged(&on_rule(), &row, now), (true, true));
}

/// An explicit core list is intersected with the chat's grant. A core outside
/// that intersection must not produce a card.
#[test]
fn explicit_cores_outside_the_grant_stay_quiet() {
    let now = 1_800_000_000;
    let mut rule = on_rule();
    rule.cores = CoreScope::Only(vec![9]);
    let rows = vec![trade(1, 1, now - 5), trade(9, 2, now - 4)];
    let mut ledger = enabled(now - 100);
    let got = decide(&rule, &mut ledger, &[1, 2], &rows, now);
    assert!(got.is_empty());
    assert!(ledger.seen.is_empty());
}

/// A core that left the grant must not be announced, and must not be marked,
/// so a later grant can still consider the close.
#[test]
fn core_removed_from_visible_is_not_announced() {
    let now = 1_800_000_000;
    let row = trade(4, 1, now - 5);
    let mut ledger = enabled(now - 100);
    let got = decide(&on_rule(), &mut ledger, &[1, 2], &[row], now);
    assert!(got.is_empty());
    assert!(ledger.seen.is_empty());
}

/// Pruning with the wrong edge would either forget a close that is still inside
/// 72 hours or keep one the window has already released.
#[test]
fn pruning_drops_closes_older_than_the_window() {
    let now = 2_000_000_000;
    let edge = now - SEEN_WINDOW_SECS;
    let mut ledger = enabled(edge - 10_000);
    ledger.seen.entry(1).or_default().insert(10, edge - 1);
    ledger.seen.entry(1).or_default().insert(11, edge);
    let fresh = trade(1, 12, now);
    let got = decide(&on_rule(), &mut ledger, &[1], &[fresh], now);
    assert_eq!(ids(&got), vec![12]);
    let marked = ledger.seen.get(&1).expect("rows inside the window");
    assert_eq!(marked.keys().copied().collect::<Vec<_>>(), vec![11, 12]);
}

/// A hidden cap at the old UI page size would leave every close after the first
/// page unannounced.
#[test]
fn a_batch_larger_than_five_hundred_is_all_announced() {
    let now = 1_900_000_000;
    let mut rows: Vec<_> = (0..600).map(|i| trade(1, i, now - 600 + i)).collect();
    rows.reverse();
    let mut ledger = enabled(now - 10_000);
    let got = decide(&on_rule(), &mut ledger, &[1], &rows, now);
    assert_eq!(ids(&got), (0..600).collect::<Vec<_>>());
}

/// Leaving `seen` in place while the rule is off would keep judging old closes
/// after the user turned cards off.
#[test]
fn a_disabled_rule_clears_seen_and_sends_nothing() {
    let mut ledger = enabled(1_000);
    ledger.seen.entry(1).or_default().insert(5, 1_500);
    let row = trade(1, 6, 1_600);
    let got = decide(&TradeRule::default(), &mut ledger, &[1], &[row], 2_000);
    assert!(got.is_empty());
    assert!(ledger.seen.is_empty());
}

/// `(announced, marked)`.
fn judged(rule: &TradeRule, row: &ClosedTrade, now: i64) -> (bool, bool) {
    let mut ledger = enabled(now - 1_000);
    let got = decide(
        rule,
        &mut ledger,
        &[row.core],
        std::slice::from_ref(row),
        now,
    );
    let marked = ledger
        .seen
        .get(&row.core)
        .is_some_and(|rows| rows.contains_key(&row.rec_id));
    (got.len() == 1, marked)
}
