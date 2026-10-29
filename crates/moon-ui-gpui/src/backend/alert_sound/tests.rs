//! The edge rule behind the price-approach alert: announce a leg that has just come inside its
//! band, and only then.

use std::collections::HashSet;

use super::{AlertLeg, leg_to_announce};

fn set(legs: &[(u64, AlertLeg)]) -> HashSet<(u64, AlertLeg)> {
    legs.iter().copied().collect()
}

/// Both alerts armed, which is the case every test but the last one is about.
fn both(_: AlertLeg) -> bool {
    true
}

/// A leg already inside its band on the previous pass stays silent.
///
/// Plausible breakage: announcing the LEVEL rather than the edge re-fires for as long as the price
/// stays inside the band — several times a second on a moving market, which is a siren rather than
/// an alert, and no test of the distance arithmetic would notice.
#[test]
fn a_leg_that_was_already_near_stays_silent() {
    let inside = set(&[(7, AlertLeg::Exit)]);
    assert_eq!(leg_to_announce(&inside, &inside, both), None);
    // It leaves the band and comes back: that IS a new crossing and announces again.
    assert_eq!(
        leg_to_announce(&inside, &set(&[]), both),
        Some(AlertLeg::Exit)
    );
}

/// Leaving the band announces nothing; only an inward crossing does.
#[test]
fn leaving_the_band_is_not_an_announcement() {
    assert_eq!(
        leg_to_announce(&set(&[]), &set(&[(7, AlertLeg::Exit)]), both),
        None
    );
}

/// The exit wins when both legs cross in the same batch, and exactly one leg is announced.
///
/// Plausible breakage: playing both makes the player cut the first sound off mid-way, so the user
/// hears one clipped noise and cannot tell which alert fired.
#[test]
fn the_exit_is_announced_first_when_both_cross() {
    let near = set(&[(7, AlertLeg::Entry), (9, AlertLeg::Exit)]);
    assert_eq!(
        leg_to_announce(&near, &set(&[]), both),
        Some(AlertLeg::Exit)
    );
}

/// An UNARMED exit does not take the pass from an armed entry that crossed in the same batch.
///
/// Plausible breakage: filtering the winner instead of the candidates makes the exit — which wins
/// the tie — swallow the pass and answer `None`, so the entry alert stays silent for as long as any
/// exit keeps crossing. Both legs are tracked whichever alert is armed, which is what puts an
/// unarmed leg in front of this filter at all.
#[test]
fn an_unarmed_leg_does_not_swallow_the_pass() {
    let near = set(&[(7, AlertLeg::Entry), (9, AlertLeg::Exit)]);
    let entry_only = |leg: AlertLeg| leg == AlertLeg::Entry;
    assert_eq!(
        leg_to_announce(&near, &set(&[]), entry_only),
        Some(AlertLeg::Entry)
    );
    // And with nothing armed at all, nothing is announced.
    assert_eq!(leg_to_announce(&near, &set(&[]), |_| false), None);
}

/// The retained set is keyed by ORDER AND LEG, so one leg being inside its band cannot suppress
/// the other's announcement.
///
/// `filled` makes the two exclusive for a live order today — an order is either waiting at its
/// entry or holding a position — so this states the keying rather than a state the scan reaches.
/// Plausible breakage: keying by order id alone silently drops one of the two the day an order
/// carries both.
#[test]
fn the_two_legs_of_one_order_do_not_shadow_each_other() {
    let was = set(&[(7, AlertLeg::Exit)]);
    let near = set(&[(7, AlertLeg::Exit), (7, AlertLeg::Entry)]);
    assert_eq!(leg_to_announce(&near, &was, both), Some(AlertLeg::Entry));
}

/// A different order crossing while another is already inside its band is still announced.
#[test]
fn another_order_crossing_is_its_own_announcement() {
    let was = set(&[(1, AlertLeg::Exit)]);
    let near = set(&[(1, AlertLeg::Exit), (2, AlertLeg::Exit)]);
    assert_eq!(leg_to_announce(&near, &was, both), Some(AlertLeg::Exit));
}

/// Time the near-band rebuild at ten cores of 200 open orders, the pass the UI thread runs when
/// either order revision of a core moves.
///
/// The price lock is not in this probe: each order already has its market's price, which is what
/// a per-pass cache leaves the arithmetic. The revision gate that skips an unchanged core is in
/// `play_price_alert_sounds` and is not re-entered here.
#[test]
#[ignore = "manual heavy-user timing probe"]
fn bench_goal_e_price_alert_scan() {
    use std::hint::black_box;
    use std::time::Instant;

    use moon_core::feed::OrderRow;
    use moon_core::feed::order_math::{pct_to_entry, pct_to_exit};

    fn row(uid: u64, market: &str, filled: bool) -> OrderRow {
        OrderRow {
            market_display: market.to_owned(),
            coin: "XALPHA".into(),
            market: market.to_owned(),
            quote: "USDT".into(),
            is_short: false,
            size: 1.0,
            remaining_size: 1.0,
            sl_on: false,
            ts_on: false,
            vstop_on: false,
            sl_fixed: false,
            ts_fixed: false,
            vstop_fixed: false,
            vstop_level: 0.0,
            vstop_vol: 0.0,
            buy_price: 10.0,
            sell_price: 12.0,
            create_time_ms: 1_000.0,
            sell_create_time_ms: 0.0,
            entry_fill_time_ms: 0.0,
            price: 11.0,
            fill_pct: if filled { 100.0 } else { 0.0 },
            strat: "UNIT_01".into(),
            strat_name: String::new(),
            strat_id: 0,
            status: String::new(),
            uid,
            emulator: false,
            job_is_done: false,
            pending: false,
            filled,
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

    let cores: Vec<Vec<OrderRow>> = (0..10)
        .map(|core| {
            (0..200)
                .map(|i| {
                    row(
                        core * 1_000 + i,
                        if i % 2 == 0 {
                            "XALPHAUSDT"
                        } else {
                            "EX-XXUSDT"
                        },
                        i % 3 == 0,
                    )
                })
                .collect()
        })
        .collect();
    let start = Instant::now();
    for _ in 0..30 {
        let mut total = 0usize;
        for orders in &cores {
            let mut price_of: std::collections::HashMap<&str, f64> =
                std::collections::HashMap::new();
            let mut near: HashSet<(u64, AlertLeg)> = HashSet::new();
            for order in orders {
                let last = *price_of.entry(order.market.as_str()).or_insert(11.0);
                if order.filled && pct_to_exit(order, last).is_some_and(|left| left <= 1.0) {
                    near.insert((order.uid, AlertLeg::Exit));
                }
                if order.fill_pct < 100.0
                    && pct_to_entry(order, last).is_some_and(|left| left <= 1.0)
                {
                    near.insert((order.uid, AlertLeg::Entry));
                }
            }
            total += near.len();
        }
        black_box(total);
    }
    println!(
        "goal_e_alert_scan_10x200: {} ns/iter",
        start.elapsed().as_nanos() / 30
    );
}
