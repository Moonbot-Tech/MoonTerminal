//! Telegram events: which entries are announced, and the per-core ring the bot drains.

use super::tg_open_is_fresh;
use crate::feed::CoreTgEvent;
use crate::session::store::CoreData;

/// An open trade resent after a reconnect must not be announced as a buy happening now, and a
/// stamp the axis could not lift is no entry at all; a buy within two minutes is.
#[test]
fn only_a_fresh_entry_is_announced() {
    let now = 1_800_000_000_000;
    assert!(tg_open_is_fresh(now - 1_000, now));
    assert!(tg_open_is_fresh(now - 120_000, now));
    assert!(!tg_open_is_fresh(now - 120_001, now));
    assert!(!tg_open_is_fresh(0, now));
    assert!(
        tg_open_is_fresh(now + 5_000, now),
        "a core clock slightly ahead is still now"
    );
    assert!(
        !tg_open_is_fresh(now + 600_000, now),
        "a clock far ahead must not make resent trades look new"
    );
}

/// A reconnect, or a new report schema, announces the open rows anew: one trade is filed once.
#[test]
fn an_entry_is_filed_once_per_trade() {
    let mut core = CoreData::new();
    assert!(core.tg_opened_once(7));
    assert!(!core.tg_opened_once(7));
    assert!(core.tg_opened_once(8));
}

/// The bot reads the ring by `seq`: numbers must keep rising past the trim, or a cursor would
/// skip or repeat rows once the oldest go.
#[test]
fn the_ring_keeps_rising_numbers_past_its_cap() {
    let mut core = CoreData::new();
    for i in 0..300 {
        core.push_tg_event(
            i,
            CoreTgEvent::Detect {
                market: format!("C{i}"),
                msg: String::new(),
                strat_name: "s".into(),
                is_short: false,
            },
        );
    }
    assert_eq!(core.tg_events_seq, 300);
    assert_eq!(core.tg_events.len(), 256);
    assert_eq!(core.tg_events.front().map(|row| row.seq), Some(45));
    assert_eq!(core.tg_events.back().map(|row| row.seq), Some(300));
}
