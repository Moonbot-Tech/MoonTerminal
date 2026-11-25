use std::time::{Duration, Instant};

use moon_chart::entry_range::{FADE_STEPS, SHOW};
use moon_core::feed::EntryPriceCorrection;
use moon_core::session::CoreStore;

use super::{append_entry_range, entry_range_sig, pane_entry_range};

/// Two cores, both serving `XALPHAUSDT`, with a correction filed on core 1 only.
fn store_with_notice(at: Instant) -> CoreStore {
    let mut store = CoreStore::default();
    store.ensure(1);
    store.ensure(2);
    let correction = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, 0.4, 0.45).unwrap();
    store
        .core_mut(1)
        .unwrap()
        .file_entry_price_notices(vec![correction], at);
    store
}

/// The notice reaches the chart of ITS core and market only. A pane showing the same market on
/// another core reads that core's store and draws nothing; a pane of another market on the right
/// core draws nothing either.
#[test]
fn a_notice_lights_only_its_core_and_market() {
    let at = Instant::now();
    let store = store_with_notice(at);
    let notices = |core| &store.core(core).unwrap().entry_price_notices;
    let drawn = pane_entry_range(notices(1), "XALPHAUSDT", at).expect("its own chart");
    assert_eq!(drawn.plan.band, Some((0.4, 0.45)));
    assert!(pane_entry_range(notices(2), "XALPHAUSDT", at).is_none());
    assert!(pane_entry_range(notices(1), "XBETAUSDT", at).is_none());
}

/// The wake deadline is the step's end on the notice's own clock, and at the expiry the pane
/// draws nothing and signs `0` — the value a pane that never had a notice signs, so the expiry
/// rebuild is the last one.
#[test]
fn the_deadline_walks_the_fade_and_ends_at_expiry() {
    let at = Instant::now();
    let store = store_with_notice(at);
    let notices = &store.core(1).unwrap().entry_price_notices;
    let first = pane_entry_range(notices, "XALPHAUSDT", at).unwrap();
    assert_eq!(first.due, at + SHOW / FADE_STEPS);
    let same_step = pane_entry_range(notices, "XALPHAUSDT", first.due - Duration::from_nanos(1));
    assert_eq!(
        entry_range_sig(same_step.as_ref()),
        entry_range_sig(Some(&first))
    );
    let next = pane_entry_range(notices, "XALPHAUSDT", first.due).unwrap();
    assert_ne!(entry_range_sig(Some(&next)), entry_range_sig(Some(&first)));
    let last = pane_entry_range(notices, "XALPHAUSDT", at + SHOW - Duration::from_millis(1));
    assert_eq!(last.unwrap().due, at + SHOW);
    let gone = pane_entry_range(notices, "XALPHAUSDT", at + SHOW);
    assert!(gone.is_none());
    assert_eq!(entry_range_sig(gone.as_ref()), 0);
    assert_ne!(entry_range_sig(Some(&first)), 0);
}

/// A repeat of the same correction restarts the display under a new signature, even though every
/// price in it is the same: the core sent it again because the user tried again.
#[test]
fn a_repeat_restarts_the_notice() {
    let at = Instant::now();
    let mut store = store_with_notice(at);
    let before = pane_entry_range(
        &store.core(1).unwrap().entry_price_notices,
        "XALPHAUSDT",
        at,
    );
    let later = at + SHOW / 2;
    let correction = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, 0.4, 0.45).unwrap();
    store
        .core_mut(1)
        .unwrap()
        .file_entry_price_notices(vec![correction], later);
    let after = pane_entry_range(
        &store.core(1).unwrap().entry_price_notices,
        "XALPHAUSDT",
        later,
    )
    .unwrap();
    assert_eq!(after.plan.step, 0, "the fade starts over");
    assert_ne!(
        entry_range_sig(Some(&after)),
        entry_range_sig(before.as_ref())
    );
}

/// With no limit on either side the pane draws the requested line and its caption but no band; a
/// single limit draws a band open on the other side.
#[test]
fn geometry_follows_the_limits() {
    let at = Instant::now();
    let theme = moon_core::config::ChartTheme::default();
    let mut store = CoreStore::default();
    store.ensure(1);
    let core = store.core_mut(1).unwrap();
    core.file_entry_price_notices(
        vec![
            EntryPriceCorrection::from_wire("XNONEUSDT", 0.5, 0.0, 0.0).unwrap(),
            EntryPriceCorrection::from_wire("XLOWUSDT", 0.5, 0.55, 0.0).unwrap(),
        ],
        at,
    );
    let notices = &store.core(1).unwrap().entry_price_notices;
    let (mut zones, mut hlines, mut labels) = (Vec::new(), Vec::new(), Vec::new());
    let visible = (0.4, 0.6);
    let none = pane_entry_range(notices, "XNONEUSDT", at).unwrap();
    append_entry_range(
        &none,
        &theme,
        visible,
        1.0,
        &mut zones,
        &mut hlines,
        &mut labels,
    );
    assert!(zones.is_empty());
    assert_eq!(hlines.len(), 1);
    assert_eq!(hlines[0].price, 0.5);
    assert_eq!(labels.len(), 1);
    assert!(
        labels[0].visible(false),
        "the line-labels switch must not hide it"
    );
    let low = pane_entry_range(notices, "XLOWUSDT", at).unwrap();
    append_entry_range(
        &low,
        &theme,
        visible,
        1.0,
        &mut zones,
        &mut hlines,
        &mut labels,
    );
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].price0, 0.55);
    assert!(zones[0].price1 > 0.6, "the open side runs off the plot");
    assert!(zones[0].price1.is_finite());
}
