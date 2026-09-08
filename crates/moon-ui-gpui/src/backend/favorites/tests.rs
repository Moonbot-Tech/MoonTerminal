//! Coverage for the override rule the star draws through.
//!
//! `fav_market` and the tick need a live `Backend`, so what is tested here is the predicate they
//! both decide with.

use super::{FAV_LOCAL_TTL, fav_local_settled};
use std::time::Duration;

/// An override lives only until the core agrees, or until the slow channel's budget is spent.
///
/// Breakage this pins: settling on the TTL alone. A press the core takes immediately would then go
/// on asserting itself for another half minute, and a mark removed in MoonBot inside that window
/// would be masked by our own stale answer — the chart star and the dropdown's row, which reads the
/// core verbatim, would disagree about one coin.
#[test]
fn an_override_settles_when_the_core_agrees_or_the_window_ends() {
    let fresh = Duration::from_secs(1);

    assert!(
        !fav_local_settled(true, fresh, false),
        "queued and not yet echoed: the star keeps what the press asked for"
    );
    assert!(
        fav_local_settled(true, fresh, true),
        "the core agrees: nothing left to assert"
    );
    assert!(
        fav_local_settled(false, fresh, false),
        "an unmark the core already shows is settled too"
    );
    assert!(
        fav_local_settled(true, FAV_LOCAL_TTL, false),
        "the budget is spent: the core's own answer wins again"
    );
}

/// The star's override is reconciled on the SAME coordination tick as the panic control's, and
/// settling one advances the revision that repaints it.
///
/// Breakage this pins, in the shape the panic contract beside it pins: dropping the revision bump
/// leaves a star asserting a write the core never took, on a market quiet enough that nothing else
/// repaints the chart; dropping the tick call leaves the override map growing for the session.
#[test]
fn the_override_is_reconciled_on_the_slow_tick_and_repaints_when_it_settles() {
    let source = include_str!("../favorites.rs");
    let tick = source
        .split("fn tick_fav_local(")
        .nth(1)
        .expect("tick_fav_local must remain a distinct backend method");
    assert!(
        tick.contains("self.fav_rev = self.fav_rev.wrapping_add(1);"),
        "settling an override must advance the revision that repaints the star"
    );

    let boot = include_str!("../../startup/boot.rs");
    let coordination = boot
        .split("let coord_backend = backend.clone();")
        .nth(1)
        .and_then(|tail| tail.split("cx.spawn(async move |cx| {").nth(1))
        .expect("boot must retain its slow coordination task");
    assert!(
        coordination.contains("if b.tick_fav_local() {")
            && coordination.contains("b.mark_backend_dirty(cx);"),
        "reconciliation must run on the unconditional coordination loop and mark a repaint"
    );
}
