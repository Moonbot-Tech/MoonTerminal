//! What the service promises, checked without a window.
//!
//! The parts that need a running application — the tick chain, the two notification channels — are
//! driven live and read back through `crowd_tick`, `crowd_trades`, `crowd_detect` and `crowd_render`
//! in `logs/render_diag.log`. What is testable here is the decision that owns every connection this
//! module can open, and it is exercised on the SHIPPED function rather than on a copy of it.

use super::{union, wanted};
use moon_core::crowd::Wants;

/// One reader's claim.
fn claim(trades: bool, coins: bool, traders: bool) -> Wants {
    Wants {
        trades,
        coins,
        traders,
    }
}

#[test]
fn nobody_asking_reads_nothing() {
    assert!(
        !wanted(union(false, std::iter::empty())),
        "an idle terminal opened a connection"
    );
}

#[test]
fn the_rule_alone_opens_the_trade_stream_and_nothing_else() {
    let only_rule = union(true, std::iter::empty());
    assert!(only_rule.trades);
    assert!(
        !only_rule.boards(),
        "the rule pulled in boards it never reads"
    );
}

#[test]
fn two_screens_asking_for_different_halves_are_one_connection_each() {
    let both = union(
        false,
        [claim(true, false, false), claim(false, true, false)].into_iter(),
    );
    assert!(both.trades && both.coins);
    assert!(!both.traders, "a board nobody shows was opened");
}

#[test]
fn a_dropped_screen_stops_counting_towards_the_union() {
    // `demand` collects dead leases before folding, so a released claim reaches this as an absent
    // one; what it must not do is leave the connection open behind it.
    assert!(wanted(union(
        false,
        [claim(true, false, false)].into_iter()
    )));
    assert!(
        !wanted(union(false, std::iter::empty())),
        "a closed window left the socket open behind it"
    );
}

#[test]
fn a_screen_cannot_switch_the_rule_off() {
    // The rule's claim is independent of every screen's: switching the last table off must not stop
    // the stream the rule is counting.
    let after = union(true, [claim(false, false, false)].into_iter());
    assert!(
        after.trades,
        "closing the last table stopped the rule from counting"
    );
}
