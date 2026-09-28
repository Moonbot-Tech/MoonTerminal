//! Unit regressions for Mini App mass-action targeting and per-core list order.

use std::cmp::Ordering;
use std::collections::HashMap;

use moon_core::venue::CoreVenue;

use std::time::{Duration, Instant};

use moon_core::telegram::web::dto::StrategyPendingDto;

use moon_core::feed::OrderRow;

use super::{
    by_section, distance_text, natural_cmp, order_to_entry_pct, scope_targets, strategy_pending,
    trade_strategy,
};

/// `mini_app.rs:scope_targets` keeps only visible cores, in visible order, once each.
///
/// Mutation: return the requested ids unfiltered or skip the dedupe. A mass
/// trading switch then commands a core the owner does not see, or the same core
/// twice. Oracle: requested [3, 99, 1, 3] against visible [1, 2, 3] is [1, 3].
#[test]
fn scope_targets_drops_unknown_and_duplicate_cores() {
    assert_eq!(scope_targets(&[3, 99, 1, 3], &[1, 2, 3]), vec![1, 3]);
    assert_eq!(scope_targets(&[99], &[1, 2, 3]), Vec::<u64>::new());
}

/// `mini_app.rs:natural_cmp` reads digit runs as numbers and ignores letter case.
///
/// Mutation: compare the raw strings. "Account № 10" then sorts before "№ 9".
#[test]
fn natural_cmp_orders_numbers_by_value() {
    assert_eq!(natural_cmp("Account № 9", "Account № 10"), Ordering::Less);
    assert_eq!(natural_cmp("core 21", "core 3"), Ordering::Greater);
    assert_eq!(natural_cmp("alpha", "Beta"), Ordering::Less);
    assert_eq!(natural_cmp("core 007", "core 7"), Ordering::Less);
    assert_eq!(natural_cmp("core", "core 1"), Ordering::Less);
    assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
}

/// `mini_app.rs:by_section` groups by the terminal's exchange sections and natural-sorts names.
///
/// Mutation: skip the in-section sort, or keep the input order across sections. Oracle: the
/// unidentified core leads (the terminal's unknown-first rule), each venue's cores stay together,
/// and "№ 9" precedes "№ 10" inside its section.
#[test]
fn by_section_groups_by_exchange_then_natural_name() {
    let venues = HashMap::from([
        (1, CoreVenue::identify(6, "", None)),
        (2, CoreVenue::identify(2, "", None)),
        (3, CoreVenue::identify(6, "", None)),
        (4, CoreVenue::identify(2, "", None)),
    ]);
    let rows = vec![
        (1u64, "№ 10".to_string()),
        (2, "b".to_string()),
        (3, "№ 9".to_string()),
        (5, "lost".to_string()),
        (4, "A".to_string()),
    ];
    let ordered = by_section(rows, &venues, |(id, _)| *id, |(_, name)| name);
    let ids: Vec<u64> = ordered.iter().map(|(_, (id, _))| *id).collect();
    assert_eq!(ids[0], 5, "the unidentified core leads, as in the terminal");
    // Section order oracle: the terminal's own partition of the two venues.
    let terminal = crate::core_order::exchange_sections([(2, venues.get(&2)), (6, venues.get(&1))]);
    let first_is_code_2 = terminal[0].1 == [2];
    let expected: [u64; 4] = if first_is_code_2 {
        [4, 2, 3, 1]
    } else {
        [3, 1, 4, 2]
    };
    assert_eq!(
        &ids[1..],
        &expected,
        "terminal section order, natural names inside"
    );
    assert_eq!(ordered[1].0, ordered[2].0);
    assert_ne!(ordered[2].0, ordered[3].0);
}

/// `mini_app.rs:strategy_pending` keeps a toggle Pending inside the 45 s window, then TimedOut
/// only while the row disagrees AND no fresh strategy list arrived.
///
/// Mutation: `if checked == wanted || rev_now != rev_before {` -> `if checked == wanted {`.
/// A core that rebuilt its strategy list without flipping the row then keeps the timed-out chip
/// forever. Oracle: the documented window (45 s) and the settle rule, not the function's output.
#[test]
fn strategy_pending_times_out_until_row_agrees_or_list_moves() {
    let sent = Instant::now();
    let entry = (true, sent, 5, 7);
    let at = |secs| sent + Duration::from_secs(secs);
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(44)),
        Some(StrategyPendingDto::Pending)
    );
    assert_eq!(
        strategy_pending(entry, 5, 7, false, at(46)),
        Some(StrategyPendingDto::TimedOut)
    );
    assert_eq!(strategy_pending(entry, 5, 7, true, at(46)), None);
    assert_eq!(
        strategy_pending(entry, 5, 8, false, at(46)),
        None,
        "a fresh strategy list after the window is the truth"
    );
}

/// Build a resting long entry at `entry` with the mark at `mark`, no fill yet.
fn resting_order(entry: f64, mark: f32) -> OrderRow {
    OrderRow {
        market: "LINKUSDT".into(),
        market_display: "LINKUSDT".into(),
        coin: "LINK".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 10.0,
        remaining_size: 0.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: entry,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: mark,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 1,
        status: String::new(),
        uid: 1,
        emulator: false,
        job_is_done: false,
        pending: true,
        filled: false,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

/// `mini_app.rs:order_to_entry_pct` states the distance to a resting entry, by side, and only
/// while the order holds no position.
///
/// Mutation: drop the position gate, or ignore the side. A filled order then shows a distance
/// beside its PnL, or a short's entry above the market reads as already passed.
/// Oracle: long entry 100 at mark 110 is +10 %, short entry 100 at mark 80 is +20 %.
#[test]
fn order_to_entry_pct_measures_resting_entries_only() {
    let long = resting_order(100.0, 110.0);
    assert_eq!(order_to_entry_pct(&long), Some(10.0));

    let mut short = resting_order(100.0, 80.0);
    short.is_short = true;
    assert_eq!(order_to_entry_pct(&short), Some(20.0));

    let mut filled = resting_order(100.0, 110.0);
    filled.filled = true;
    filled.fill_pct = 100.0;
    assert_eq!(order_to_entry_pct(&filled), None);

    assert_eq!(order_to_entry_pct(&resting_order(100.0, 0.0)), None);
}

#[test]
fn the_telegram_log_prefix_still_matches_this_module() {
    // moon-core raises this prefix to `info` by default but cannot verify it from its own side;
    // a module move or a `[[bin]]` rename would mute the owner-command lines again.
    let prefix = moon_core::diagnostics::TELEGRAM_TARGET;
    assert!(
        module_path!().starts_with(prefix),
        "the default filter raises {prefix:?}, but this module logs as {:?}",
        module_path!()
    );
}

#[test]
fn distance_to_entry_text_carries_no_sign() {
    assert_eq!(distance_text(1.5).as_deref(), Some("1.50%"));
    assert_eq!(distance_text(-1.5).as_deref(), Some("1.50%"));
}

/// `mini_app.rs:trade_strategy` names strategy trades, marks only `0` manual, and leaves a missing
/// id unknown.
///
/// Mutation: drop negative ids (`u64::try_from`), treat a missing id as manual, or skip the stored
/// name. A strategy whose Delphi-signed id is negative then shows as manual in the trade sheet, or
/// one the core no longer lists shows as a bare id. Oracle: id `-7` names the strategy listed
/// under `(-7i64) as u64`; an unlisted id with a stored name shows that name.
#[test]
fn trade_strategy_names_signed_ids_and_separates_manual_from_unknown() {
    let names = |sid: u64| match sid {
        5 => Some("Demo Alpha".to_string()),
        sid if sid == (-7i64) as u64 => Some("Demo Beta".to_string()),
        _ => None,
    };
    assert_eq!(
        trade_strategy(Some(5), "Demo Old", names),
        (Some("Demo Alpha".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(-7), "", names),
        (Some("Demo Beta".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(9), " Demo Gone ", names),
        (Some("Demo Gone".to_string()), false)
    );
    assert_eq!(
        trade_strategy(Some(9), "", names),
        (Some("#9".to_string()), false)
    );
    assert_eq!(trade_strategy(Some(0), "Demo Alpha", names), (None, true));
    assert_eq!(trade_strategy(None, "", names), (None, false));
}
