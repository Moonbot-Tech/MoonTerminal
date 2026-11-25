//! Unit checks for the position age on order lines (#1402).
//!
//! Explicit imports throughout: the chartdx parent re-exports `gpui::*`, whose own `test` shadows
//! the built-in attribute and makes `#[test]` expand recursively.

use moon_core::feed::OrderRow;

use super::{OrderAge, order_age_clock, order_age_text, position_fills, step_order_ages};
use crate::chartdx::{OrderLabel, PRIO_BUY};

/// An open order on the test market whose entry leg closed at `fill_ms`, holding a position.
fn holder(uid: u64, fill_ms: f64) -> OrderRow {
    OrderRow {
        market: "XALPHAUSDT".into(),
        market_display: "XALPHAUSDT".into(),
        coin: "XALPHA".into(),
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
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: fill_ms,
        price: 11.0,
        fill_pct: 100.0,
        strat: String::new(),
        strat_name: String::new(),
        strat_id: 0,
        status: String::new(),
        uid,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: true,
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

/// An entry-line caption carrying `age`, as the order sync builds it.
fn aged(age: Option<OrderAge>, text: &str, drawn: bool) -> OrderLabel {
    OrderLabel {
        uid: 1,
        price: 10.0,
        text: text.into(),
        above: true,
        color: 0,
        priority: PRIO_BUY,
        force: false,
        interacting: false,
        highlighted: false,
        pinned: false,
        age,
        age_drawn: drawn,
    }
}

/// The line states each holder's OWN fill — not the oldest one the caption counts from — and
/// leaves out a resting entry, a cancelled entry that only carries a close time, a holder with no
/// stated fill, an order the core has marked done and an order on another market.
#[test]
fn every_holder_gets_its_own_fill_and_nothing_else_does() {
    let mut resting = holder(3, 0.0);
    resting.filled = false;
    resting.fill_pct = 0.0;
    let mut cancelled = holder(4, 7_000.0);
    cancelled.filled = false;
    cancelled.fill_pct = 0.0;
    let mut done = holder(5, 8_000.0);
    done.job_is_done = true;
    let mut elsewhere = holder(6, 9_000.0);
    elsewhere.market = "XBETAUSDT".into();
    let rows = [
        holder(1, 1_000.0),
        holder(2, 5_000.5),
        resting,
        cancelled,
        done,
        elsewhere,
        holder(7, 0.0),
    ];
    assert_eq!(
        position_fills(&rows, "XALPHAUSDT"),
        vec![(1, 1_000), (2, 5_000)]
    );
}

/// The caption's spelling, then the order's tag; a fill dated a whole second or more ahead of the
/// clock prints no age and leaves the bare tag the line carried before.
#[test]
fn the_age_reads_like_the_caption_then_the_tag() {
    let _locale = crate::test_locale::force("ru");
    let age = |tag: &str| OrderAge {
        since_ms: 1_000_000,
        tag: tag.into(),
    };
    assert_eq!(order_age_text(&age("[4]"), 1_000_000), "0с [4]");
    assert_eq!(order_age_text(&age("[4]"), 1_042_000), "42с [4]");
    assert_eq!(
        order_age_text(&age(""), 1_000_000 + 12 * 60_000 + 3_000),
        "12м 03с"
    );
    assert_eq!(order_age_text(&age("[4]"), 999_001), "0с [4]");
    assert_eq!(order_age_text(&age("[4]"), 999_000), "[4]");
}

/// The clock is the caption's one-second wall grid, floored — including before the epoch, where a
/// truncating division would round toward zero and step a second late.
#[test]
fn the_clock_floors_to_the_second() {
    assert_eq!(order_age_clock(1_234_999), 1_234_000);
    assert_eq!(order_age_clock(1_234_000), 1_234_000);
    assert_eq!(order_age_clock(-1), -1_000);
}

/// A step re-spells every aged caption, drawn or not, but reports a change only for one the last
/// text pass drew: an off-screen caption must not wake the scene, and one that reads the same must
/// keep its string so its text run does not reshape.
#[test]
fn a_step_wakes_only_for_a_drawn_caption_that_changed() {
    let _locale = crate::test_locale::force("ru");
    let age = || {
        Some(OrderAge {
            since_ms: 0,
            tag: "[1]".into(),
        })
    };
    let mut labels = vec![aged(age(), "41с [1]", false), aged(None, "[2]", true)];
    assert!(
        !step_order_ages(&mut labels, 42_000),
        "off screen: no present"
    );
    assert_eq!(labels[0].text, "42с [1]", "re-spelled all the same");
    assert_eq!(labels[1].text, "[2]", "an unaged caption is untouched");

    labels[0].age_drawn = true;
    assert!(
        !step_order_ages(&mut labels, 42_000),
        "same figure: no present"
    );
    assert!(
        step_order_ages(&mut labels, 43_000),
        "drawn and moved: present"
    );
    assert_eq!(labels[0].text, "43с [1]");
}
