use super::*;
use crate::market::tape_recorder::TradeId;
use crate::venue::{Brand, MarketKind, Venue};

const DAY: i64 = 86_400_000;

/// An offline protecting core cannot authorize clipping an unlisted trade's old entry.
#[test]
fn incomplete_listing_defers_the_age_clip() {
    let result = kept(
        Some(100),
        &[],
        &HashMap::new(),
        true,
        &HashSet::from([inventory().keys[0].clone()]),
    );
    assert!(result.contains((0, 1000)));
    assert!(!kept(Some(100), &[], &HashMap::new(), true, &HashSet::new()).contains_ms(50));
}

/// Catch-up's one-year limit must not cut the setup of a trade crossing a five-year edge.
#[test]
fn straddler_older_than_a_year_keeps_its_entry_tape() {
    let now = 2500 * DAY;
    let edge = now - 1825 * DAY;
    let result = kept(
        Some(edge),
        &[trade(edge - DAY, edge + DAY, false)],
        &HashMap::new(),
        true,
        &HashSet::new(),
    );
    assert!(result.contains_ms(edge - DAY - 10));
    assert!(!result.contains_ms(edge - DAY - 11));
}

/// Synthetic identities share one market so overlapping emulator windows exercise protection.
fn trade(open: i64, close: i64, emulator: bool) -> catchup::CatchupTrade {
    catchup::CatchupTrade {
        id: TradeId {
            core: 9,
            rec_id: open,
        },
        exchange: "synthetic".into(),
        market: "SYNUSDT".into(),
        venue: Venue {
            brand: Brand::Bybit,
            kind: MarketKind::Futures,
        },
        open_ms: open,
        close_ms: close,
        emulator,
    }
}

/// Inventory coverage has no trade IDs, so an independently specified key pins ownership.
fn inventory() -> Inventory {
    Inventory {
        keys: vec![("synthetic".into(), "SYNUSDT".into())],
        range_ms: Some((0, 1000)),
    }
}

/// Return the single fixture's retained coverage without depending on map iteration order.
fn kept(
    edge: Option<i64>,
    trades: &[catchup::CatchupTrade],
    live: &HashMap<Key, Coverage>,
    emulator: bool,
    incomplete: &HashSet<Key>,
) -> Coverage {
    keep_map(
        &inventory(),
        trades,
        live,
        incomplete,
        KeepPolicy {
            edge,
            listed_from: i64::MIN,
            record_emulator: emulator,
            margin_ms: 10,
            long_ms: 1000,
        },
    )
    .remove(&inventory().keys[0])
    .unwrap()
}

/// Clipping at the entry instead of retaining an in-range exit would lose a real trade's setup.
#[test]
fn keep_map_keeps_only_in_range_plain_trades() {
    let result = kept(
        Some(100),
        &[trade(50, 110, false), trade(0, 20, false)],
        &HashMap::new(),
        true,
        &HashSet::new(),
    );
    assert!(result.contains((40, 120)));
    assert!(!result.contains_ms(30));
    assert!(result.contains_ms(999));
}

/// Removing emulator tape without subtracting real protection would cut an open position's entry.
#[test]
fn open_real_trade_protects_shared_emulator_tape() {
    let live = HashMap::from([(inventory().keys[0].clone(), Coverage::one((40, 70)))]);
    let result = kept(
        Some(100),
        &[trade(30, 130, true)],
        &live,
        false,
        &HashSet::new(),
    );
    assert!(result.contains((40, 70)));
    assert!(!result.contains_ms(110));
}

/// A forever horizon must preserve unattributable old spans instead of inheriting archive depth.
#[test]
fn forever_never_deletes_anything() {
    assert_eq!(
        kept(None, &[], &HashMap::new(), true, &HashSet::new()),
        Coverage::one((i64::MIN, i64::MAX))
    );
}

/// Retention must not reuse catch-up's one-year archive cap.
#[test]
fn five_years_keeps_a_two_year_old_window() {
    let now = 2000 * DAY;
    let result = kept(
        Some(now - 1825 * DAY),
        &[trade(now - 731 * DAY, now - 730 * DAY, false)],
        &HashMap::new(),
        true,
        &HashSet::new(),
    );
    assert!(result.contains_ms(now - 730 * DAY));
}

/// An offline or unresolved report can hide a real trade sharing emulator tape.
#[test]
fn incomplete_listing_defers_emulator_purge() {
    let incomplete = HashSet::from([inventory().keys[0].clone()]);
    let result = kept(
        Some(100),
        &[trade(110, 120, true)],
        &HashMap::new(),
        false,
        &incomplete,
    );
    assert!(result.contains((100, i64::MAX)));
}

/// A capped listing omits older real closes that can share an emulator entry or cross its cutoff with their tail.
#[test]
fn unlisted_old_real_windows_survive_emulator_subtraction() {
    let now = 2000 * DAY;
    let listed_from = now - 365 * DAY;
    let old_entry = now - 450 * DAY;
    let recent_exit = now - 350 * DAY;
    let trades = [
        trade(old_entry, recent_exit, true),
        trade(listed_from + 20_000, listed_from + 40_000, true),
    ];
    let mut map = keep_map(
        &inventory(),
        &trades,
        &HashMap::new(),
        &HashSet::new(),
        KeepPolicy {
            edge: Some(now - 1825 * DAY),
            listed_from,
            record_emulator: false,
            margin_ms: 60_000,
            long_ms: 1000,
        },
    );
    let keep = map.remove(&inventory().keys[0]).unwrap();
    assert!(
        keep.contains((old_entry - 60_000, old_entry + 60_000)),
        "the real close 450 days ago was not listed"
    );
    assert!(
        keep.contains((listed_from, listed_from + 30_000)),
        "an unlisted close 30 seconds before the cutoff still needs its 60-second tail"
    );
    assert!(!keep.contains_ms(recent_exit));
    assert!(!keep.contains_ms(listed_from + 70_000));
}

/// Apply the production map through the real cache worker and verify its durable span bounds.
#[test]
fn trim_applies_keep_map() {
    use crate::market::trade_replay::tick_tiles::TileSource;
    use crate::market::trade_replay::trade_cache::{Compact, TradeCache};
    let cache = TradeCache::open_evictable(":memory:".into()).unwrap();
    cache.insert("synthetic", "SYNUSDT", 0, 200, vec![], TileSource::Core);
    let keep = keep_map(
        &inventory(),
        &[],
        &HashMap::new(),
        &HashSet::new(),
        KeepPolicy {
            edge: Some(100),
            listed_from: i64::MIN,
            record_emulator: true,
            margin_ms: 10,
            long_ms: 1000,
        },
    );
    let outcome = cache
        .trim_start(Arc::new(keep), Compact::Incremental, None)
        .recv()
        .unwrap()
        .unwrap();
    assert_eq!(outcome.report.spans_cut, 1);
    assert_eq!(
        cache.held_spans("synthetic", "SYNUSDT", 0, 200).unwrap(),
        vec![(100, 200)]
    );
}
