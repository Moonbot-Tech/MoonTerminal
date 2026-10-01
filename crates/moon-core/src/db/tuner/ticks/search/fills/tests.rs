use super::super::tests::prepared;
use super::*;
use crate::db::tuner::ticks::MshotParams;

/// One strategy's parameters with a MoonShot entry `price_pct` away.
fn at(price_pct: f64) -> Vec<(EntryParams, ExitParams)> {
    let entry = MshotParams {
        price_pct,
        ..MshotParams::default()
    };
    vec![(EntryParams::MoonShot(entry), ExitParams::default())]
}

#[test]
fn an_entry_scored_again_reads_the_fills_it_replayed() {
    let deals: Vec<PreparedDeal> = (1..=4).map(|uid| prepared(uid, 101.0)).collect();
    let of_deal = vec![0; deals.len()];
    let cache = FillCache::default();
    let first = cache.fills(&deals, &of_deal, &at(1.0));
    let replayed: Vec<Option<Fill>> = deals
        .iter()
        .map(|d| entry_fill(&d.deal, &d.ticks, &at(1.0)[0].0, None))
        .collect();
    assert_eq!(&first[..], &replayed[..]);
    assert_eq!(cache.reused(), 0);
    // The same entry under another exit is the same fills, read back.
    let mut other_exit = at(1.0);
    other_exit[0].1.sell_price_pct = 3.0;
    let again = cache.fills(&deals, &of_deal, &other_exit);
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(cache.reused(), 1);
    // Another entry is replayed.
    let moved = cache.fills(&deals, &of_deal, &at(2.0));
    assert!(!Arc::ptr_eq(&first, &moved));
    assert_eq!(cache.reused(), 1);
}

#[test]
fn the_cache_keeps_the_latest_entries_and_replays_an_older_one() {
    let deals = vec![prepared(1, 101.0)];
    let of_deal = vec![0];
    let cache = FillCache::default();
    for k in 0..=KEPT {
        cache.fills(&deals, &of_deal, &at(1.0 + k as f64 / 100.0));
    }
    // The latest is read back; the first, pushed out by KEPT newer ones, is replayed.
    cache.fills(&deals, &of_deal, &at(1.0 + KEPT as f64 / 100.0));
    assert_eq!(cache.reused(), 1);
    cache.fills(&deals, &of_deal, &at(1.0));
    assert_eq!(cache.reused(), 1);
}
