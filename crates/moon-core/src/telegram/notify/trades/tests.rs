//! Pins for trade announcement. Each test names the edit that would break it.

use crate::telegram::notify::{CoreScope, NotifyLedger, TradeRule};

use super::*;

/// Freeze the original offered-set walk so changing held-row cleanup cannot alter cards.
fn frozen_decide_gate(
    rule: &Gate<'_>,
    ledger: Book<'_>,
    visible: &[u64],
    trades: &[ClosedTrade],
    now_utc: i64,
) -> Vec<Announced> {
    if !rule.on {
        ledger.seen.clear();
        ledger.held.clear();
        return Vec::new();
    }
    let Some(read_from) = floor_from(ledger.enabled_utc, now_utc) else {
        prune_closes(ledger.seen, now_utc.saturating_sub(SEEN_WINDOW_SECS));
        return Vec::new();
    };
    let mut announced = Vec::new();
    // Held closes this read still offers; any other is let go below, so a close whose core left
    // the chat, or whose row is gone, cannot keep forcing reads until the window drops it.
    let mut offered: BTreeSet<(u64, i64)> = BTreeSet::new();
    for trade in trades {
        if trade.close_utc < read_from || !core_selected(rule, visible, trade.core) {
            continue;
        }
        offered.insert((trade.core, trade.rec_id));
        if ledger
            .seen
            .get(&trade.core)
            .is_some_and(|rows| rows.contains_key(&trade.rec_id))
        {
            continue;
        }
        let verdict = verdict(rule, trade);
        if verdict == Verdict::Unknown {
            let first = *ledger
                .held
                .entry(trade.core)
                .or_default()
                .entry(trade.rec_id)
                .or_insert(now_utc);
            if now_utc < first.saturating_add(HOLD_SECS) {
                continue;
            }
        }
        unhold(ledger.held, trade);
        ledger
            .seen
            .entry(trade.core)
            .or_default()
            .insert(trade.rec_id, trade.close_utc);
        if verdict != Verdict::Fail {
            announced.push(Announced {
                trade: trade.clone(),
                unchecked: verdict == Verdict::Unknown,
            });
        }
    }
    announced.sort_by(|left, right| {
        left.trade
            .close_utc
            .cmp(&right.trade.close_utc)
            .then(left.trade.rec_id.cmp(&right.trade.rec_id))
    });
    let window_edge = now_utc.saturating_sub(SEEN_WINDOW_SECS);
    prune_closes(ledger.seen, window_edge);
    ledger.held.retain(|core, rows| {
        rows.retain(|rec_id, _| offered.contains(&(*core, *rec_id)));
        !rows.is_empty()
    });
    announced
}

/// A full decision must preserve held clocks, filtered rows and stable tied-close ordering.
#[test]
fn decision_matches_frozen_gate_across_rechecks() {
    let mut rows: Vec<_> = (0..30)
        .map(|id| ClosedTrade {
            core: id as u64 % 3 + 1,
            rec_id: id,
            close_utc: 1_000 + id / 2,
            coin: "XALPHA".into(),
            core_name: "CORE_A".into(),
            strategy: "UNIT_01".into(),
            profit_usd: (id % 4 != 0).then_some(id as f64 - 15.0),
            profit_native: Some(1.0),
            volume_usd: Some(100.0),
            ..Default::default()
        })
        .collect();
    rows.reverse();
    rows.push(rows[3].clone());
    for mode in 0..8 {
        let rule = TradeRule {
            on: mode != 0,
            profit_at_least_usd: Some(3.0),
            loss_at_least_usd: Some(4.0),
            ..Default::default()
        };
        let mut old = NotifyLedger {
            trades_enabled_utc: (mode != 1).then_some(999),
            ..Default::default()
        };
        old.seen.entry(1).or_default().insert(0, 1_000);
        old.held.entry(1).or_default().insert(0, 1_000);
        old.held.entry(9).or_default().insert(999, 1_000);
        let mut new = old.clone();
        for now in [1_100, 1_200, 1_500, 300_000] {
            let gate = Gate {
                on: rule.on,
                cores: &rule.cores,
                min_volume_usd: None,
                profit_at_least_usd: rule.profit_at_least_usd,
                loss_at_least_usd: rule.loss_at_least_usd,
                by_quote: &rule.by_quote,
                needs_usd: mode == 2,
            };
            let expected = frozen_decide_gate(
                &gate,
                Book {
                    enabled_utc: old.trades_enabled_utc,
                    seen: &mut old.seen,
                    held: &mut old.held,
                },
                &[1, 2],
                &rows,
                now,
            );
            let actual = decide_gate(
                &gate,
                Book {
                    enabled_utc: new.trades_enabled_utc,
                    seen: &mut new.seen,
                    held: &mut new.held,
                },
                &[1, 2],
                &rows,
                now,
            );
            assert_eq!(actual, expected, "mode={mode} now={now}");
            assert_eq!(new, old, "mode={mode} now={now}");
        }
    }
}

/// Time every retained close at 5000 deals per core; rebuilding keys for seen cards is the regression.
#[test]
#[ignore = "scale probe"]
fn probe_notify_seen_scale() {
    let rows: Vec<_> = (0..50_000)
        .map(|id| ClosedTrade {
            core: id as u64 % 10 + 1,
            rec_id: id,
            close_utc: 1_000,
            coin: "XALPHA".into(),
            ..Default::default()
        })
        .collect();
    let rule = TradeRule {
        on: true,
        ..Default::default()
    };
    let gate = Gate {
        on: true,
        cores: &rule.cores,
        min_volume_usd: None,
        profit_at_least_usd: None,
        loss_at_least_usd: None,
        by_quote: &[],
        needs_usd: false,
    };
    let mut old = NotifyLedger {
        trades_enabled_utc: Some(999),
        ..Default::default()
    };
    for row in &rows {
        old.seen
            .entry(row.core)
            .or_default()
            .insert(row.rec_id, row.close_utc);
    }
    let mut new = old.clone();
    let visible: Vec<_> = (1..=10).collect();
    let start = std::time::Instant::now();
    for _ in 0..5 {
        assert!(
            frozen_decide_gate(
                &gate,
                Book {
                    enabled_utc: old.trades_enabled_utc,
                    seen: &mut old.seen,
                    held: &mut old.held
                },
                &visible,
                &rows,
                2_000
            )
            .is_empty()
        );
    }
    let before = start.elapsed().as_secs_f64() * 200.0;
    let start = std::time::Instant::now();
    for _ in 0..5 {
        assert!(decide(&rule, &mut new, &visible, &rows, 2_000).is_empty());
    }
    let after = start.elapsed().as_secs_f64() * 200.0;
    eprintln!("probe seen 50000: {before:.3} -> {after:.3} ms");
    assert_eq!(old, new);
}

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
        ..ClosedTrade::default()
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

fn ids(announced: &[Announced]) -> Vec<i64> {
    announced.iter().map(|card| card.trade.rec_id).collect()
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

/// A BTC-quoted close with its own amounts and no valuation yet.
fn btc_trade(rec_id: i64, close_utc: i64) -> ClosedTrade {
    let mut row = trade(1, rec_id, close_utc);
    row.quote = crate::db::QuoteCurrency::from_report_ordinal(0);
    row.profit_native = Some(0.0001);
    row.volume_native = Some(0.01);
    row
}

/// A volume or profit the row cannot evidence in any currency will never be valued, so a set
/// threshold fails it at once, as before; holding it would only send it unchecked later.
#[test]
fn an_unprovable_figure_fails_at_once() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.min_volume_usd = Some(50.0);
    let mut row = btc_trade(1, now - 10);
    row.volume_usd = None;
    row.volume_native = None;
    assert_eq!(judged(&rule, &row, now), (false, true));
    rule = on_rule();
    rule.profit_at_least_usd = Some(1.0);
    row.rec_id = 2;
    row.profit_usd = None;
    row.profit_native = None;
    assert_eq!(judged(&rule, &row, now), (false, true));
}

/// A held close the read no longer offers — its core left the chat — is let go, or its expired
/// hold would keep forcing reads for three days.
#[test]
fn a_held_close_the_read_no_longer_offers_is_released() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.profit_at_least_usd = Some(1.0);
    let mut row = btc_trade(1, now - 10);
    row.profit_usd = None;
    let mut ledger = enabled(now - 1_000);
    assert!(decide(&rule, &mut ledger, &[1], std::slice::from_ref(&row), now).is_empty());
    assert!(!ledger.held.is_empty());
    assert!(
        decide(
            &rule,
            &mut ledger,
            &[2],
            std::slice::from_ref(&row),
            now + 1
        )
        .is_empty()
    );
    assert!(ledger.held.is_empty());
    assert!(ledger.seen.is_empty(), "a released close is not marked");
}

/// Judging a row without a dollar volume against a minimum, or dropping it, would either send a
/// trade the user asked to keep quiet or lose one that only waits for its valuation. It is held
/// unmarked, and once the hold runs out it goes, marked unchecked.
#[test]
fn missing_volume_is_held_then_sent_unchecked() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.min_volume_usd = Some(50.0);
    let mut row = btc_trade(1, now - 10);
    row.volume_usd = None;
    let mut ledger = enabled(now - 1_000);
    let rows = std::slice::from_ref(&row);
    assert!(decide(&rule, &mut ledger, &[1], rows, now).is_empty());
    assert!(ledger.seen.is_empty());
    assert_eq!(hold_until(&ledger), Some(now + HOLD_SECS));
    assert!(decide(&rule, &mut ledger, &[1], rows, now + HOLD_SECS - 1).is_empty());
    let got = decide(&rule, &mut ledger, &[1], rows, now + HOLD_SECS);
    assert_eq!(ids(&got), vec![1]);
    assert!(got[0].unchecked);
    assert!(ledger.held.is_empty());
    assert!(
        ledger
            .seen
            .get(&1)
            .is_some_and(|rows| rows.contains_key(&1))
    );
}

/// A held row that gets its valuation must be judged by it at once, not sent unchecked at the
/// end of the hold.
#[test]
fn a_held_row_is_judged_once_its_valuation_lands() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.profit_at_least_usd = Some(5.0);
    let mut row = btc_trade(1, now - 10);
    row.profit_usd = None;
    let mut ledger = enabled(now - 1_000);
    assert!(decide(&rule, &mut ledger, &[1], std::slice::from_ref(&row), now).is_empty());
    row.profit_usd = Some(4.0);
    assert!(
        decide(
            &rule,
            &mut ledger,
            &[1],
            std::slice::from_ref(&row),
            now + 5
        )
        .is_empty()
    );
    assert!(ledger.held.is_empty(), "a judged row is no longer held");
    assert!(
        ledger
            .seen
            .get(&1)
            .is_some_and(|rows| rows.contains_key(&1))
    );
}

/// With both thresholds off, a row without a dollar profit is a normal card at once. With one
/// set, it waits.
#[test]
fn unvalued_profit_waits_only_when_a_threshold_is_set() {
    let now = 1_700_000_000;
    let mut row = btc_trade(1, now - 10);
    row.profit_usd = None;
    let mut rule = on_rule();
    rule.loss_at_least_usd = Some(5.0);
    assert_eq!(judged(&rule, &row, now), (false, false));
    row.rec_id = 3;
    assert_eq!(judged(&on_rule(), &row, now), (true, true));
}

/// A USD stablecoin's own amount stands for dollars, so a USDC trade is judged the moment it
/// lands; a BTC trade is not, since its amount is not dollars.
#[test]
fn a_stablecoin_amount_counts_one_to_one() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.profit_at_least_usd = Some(3.0);
    rule.min_volume_usd = Some(100.0);
    let mut row = trade(1, 1, now - 10);
    row.profit_usd = None;
    row.volume_usd = None;
    row.quote = crate::db::QuoteCurrency::from_report_ordinal(8);
    row.profit_native = Some(3.3);
    row.volume_native = Some(150.0);
    assert_eq!(judged(&rule, &row, now), (true, true));
    row.rec_id = 2;
    row.profit_native = Some(2.0);
    assert_eq!(judged(&rule, &row, now), (false, true));
    row.rec_id = 3;
    row.quote = crate::db::QuoteCurrency::from_report_ordinal(0);
    row.profit_native = Some(0.5);
    assert_eq!(judged(&rule, &row, now), (false, false));
}

/// One threshold already failing decides the row: waiting for the other's dollars would only
/// delay a card that will never be sent.
#[test]
fn a_known_failure_is_not_held() {
    let now = 1_700_000_000;
    let mut rule = on_rule();
    rule.min_volume_usd = Some(500.0);
    rule.profit_at_least_usd = Some(1.0);
    let mut row = trade(1, 1, now - 10);
    row.volume_usd = Some(100.0);
    row.profit_usd = None;
    assert_eq!(judged(&rule, &row, now), (false, true));
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

/// A RUB close with its own amounts and no valuation — none exists for RUB.
fn rub_trade(rec_id: i64, close_utc: i64, profit: f64, volume: f64) -> ClosedTrade {
    let mut row = trade(1, rec_id, close_utc);
    row.quote = crate::db::QuoteCurrency::from_ticker("RUB");
    row.profit_usd = None;
    row.volume_usd = None;
    row.profit_native = Some(profit);
    row.volume_native = Some(volume);
    row
}

/// A rule with a RUB row: the dollar thresholds stay set and must not touch a RUB trade.
fn rub_rule(row: crate::telegram::notify::QuoteThresholds) -> TradeRule {
    let mut rule = on_rule();
    rule.min_volume_usd = Some(1_000_000.0);
    rule.profit_at_least_usd = Some(1_000_000.0);
    rule.by_quote = vec![crate::telegram::notify::QuoteThresholds {
        quote: "RUB".to_string(),
        ..row
    }];
    rule
}

/// LinKvo 13.10: a RUB trade has no dollar value anywhere, so a dollar threshold held it five
/// minutes and sent it "unchecked". With a RUB row it is judged in rubles at once — its own
/// profit, its own volume, the row's floors — and never marked unchecked.
#[test]
fn a_currency_row_judges_the_trade_in_its_own_money_at_once() {
    use crate::telegram::notify::QuoteThresholds;
    let now = 1_700_000_000;
    let rule = rub_rule(QuoteThresholds {
        min_volume: Some(1_000.0),
        profit_at_least: Some(100.0),
        loss_at_least: Some(50.0),
        ..QuoteThresholds::default()
    });
    let rows = [
        rub_trade(1, now - 10, 150.0, 5_000.0),
        rub_trade(2, now - 9, 27.45, 5_000.0),
        rub_trade(3, now - 8, -60.0, 5_000.0),
        rub_trade(4, now - 7, 150.0, 500.0),
    ];
    let mut ledger = enabled(now - 1_000);
    let got = decide(&rule, &mut ledger, &[1], &rows, now);
    assert_eq!(ids(&got), vec![1, 3]);
    assert!(got.iter().all(|card| !card.unchecked));
    assert!(ledger.held.is_empty(), "nothing waits for a valuation");
    assert_eq!(ledger.seen[&1].len(), 4, "every judged trade is marked");
}

/// A row with no threshold lets every trade in its currency through at once — the way to hear
/// about a currency nothing values without filtering it.
#[test]
fn an_empty_currency_row_sends_every_trade_in_it() {
    let now = 1_700_000_000;
    let rule = rub_rule(crate::telegram::notify::QuoteThresholds::default());
    assert_eq!(
        judged(&rule, &rub_trade(1, now - 10, -0.5, 10.0), now),
        (true, true)
    );
}

/// A row's threshold on an amount the trade does not carry fails it at once: no valuation can
/// bring a ruble amount later, so holding it would only send it unchecked five minutes on.
#[test]
fn a_currency_row_fails_a_missing_amount_at_once() {
    use crate::telegram::notify::QuoteThresholds;
    let now = 1_700_000_000;
    let rule = rub_rule(QuoteThresholds {
        min_volume: Some(1_000.0),
        ..QuoteThresholds::default()
    });
    let mut row = rub_trade(1, now - 10, 150.0, 5_000.0);
    row.volume_native = None;
    assert_eq!(judged(&rule, &row, now), (false, true));
}

/// A row for one currency leaves a trade in another to the dollar thresholds, waiting included.
#[test]
fn a_currency_row_leaves_other_currencies_to_the_dollars() {
    let now = 1_700_000_000;
    let rule = rub_rule(crate::telegram::notify::QuoteThresholds::default());
    // Unvalued in both figures: a known dollar volume would fail the rule's floor at once.
    let mut row = btc_trade(1, now - 10);
    row.profit_usd = None;
    row.volume_usd = None;
    let mut ledger = enabled(now - 1_000);
    assert!(decide(&rule, &mut ledger, &[1], std::slice::from_ref(&row), now).is_empty());
    assert!(
        ledger
            .held
            .get(&1)
            .is_some_and(|rows| rows.contains_key(&1))
    );
}
