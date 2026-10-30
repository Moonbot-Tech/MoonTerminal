use super::*;

fn row(market: &str, bid: f64, ask: f64, mark: f64) -> Row {
    Row {
        market: market.to_owned(),
        bid,
        ask,
        mark,
        funding: None,
    }
}

#[test]
fn an_unchanged_row_adds_no_point() {
    let mut ring = ExchangeRing::default();
    assert_eq!(
        ring.apply(1_000, vec![row("BTCUSDT", 1.0, 2.0, 1.5)], false),
        1
    );
    assert_eq!(
        ring.apply(3_000, vec![row("BTCUSDT", 1.0, 2.0, 1.5)], false),
        0
    );
    assert_eq!(
        ring.apply(5_000, vec![row("BTCUSDT", 1.0, 2.0, 1.6)], false),
        1
    );
    assert_eq!(ring.size(), (1, 2));
}

#[test]
fn trim_keeps_the_state_at_the_cut() {
    let mut ring = ExchangeRing::default();
    ring.apply(1_000, vec![row("X", 1.0, 2.0, 1.0)], false);
    ring.apply(2_000, vec![row("X", 1.1, 2.0, 1.0)], false);
    ring.apply(3_000, vec![row("X", 1.2, 2.0, 1.0)], false);
    ring.trim(2_500);
    assert_eq!(ring.size(), (1, 2));
    let seg = ring.segment("X", 2_600, 4_000).unwrap();
    assert_eq!(seg.from_ms, 2_600);
    let bids: Vec<_> = seg.points.iter().map(|p| (p.t_ms, p.bid)).collect();
    assert_eq!(bids, vec![(2_600, 1.1), (3_000, 1.2)]);
}

#[test]
fn a_segment_takes_only_changes_inside_its_stretch() {
    let mut ring = ExchangeRing::default();
    for (t, bid) in [(1_000, 1.0), (2_000, 2.0), (3_000, 3.0), (4_000, 4.0)] {
        ring.apply(t, vec![row("X", bid, 9.0, 5.0)], false);
    }
    let seg = ring.segment("X", 2_000, 3_500).unwrap();
    let bids: Vec<_> = seg.points.iter().map(|p| (p.t_ms, p.bid)).collect();
    assert_eq!(bids, vec![(2_000, 2.0), (3_000, 3.0)]);
}

#[test]
fn a_market_first_seen_inside_the_stretch_starts_at_its_first_point() {
    let mut ring = ExchangeRing::default();
    ring.apply(5_000, vec![row("NEW", 1.0, 2.0, 1.5)], false);
    let seg = ring.segment("NEW", 1_000, 9_000).unwrap();
    assert_eq!(seg.from_ms, 5_000);
    assert!(ring.segment("NEW", 1_000, 4_000).is_none());
    assert!(ring.segment("NONE", 1_000, 9_000).is_none());
}

#[test]
fn a_late_or_dropped_list_opens_a_new_run() {
    let mut ring = ExchangeRing::default();
    ring.apply(1_000, Vec::new(), false);
    ring.apply(3_000, Vec::new(), false);
    let late = 3_000 + GAP_MS + 1_000;
    ring.apply(late, Vec::new(), false);
    ring.apply(late + 2_000, Vec::new(), true);
    // Late: the gap is the stretch past GAP_MS. Dropped: the run before ends at its last list.
    assert_eq!(
        ring.valid(i64::MAX).spans(),
        &[
            (1_000, 3_000 + GAP_MS),
            (late, late),
            (late + 2_000, late + 2_000 + GAP_MS),
        ]
    );
}

#[test]
fn validity_stops_at_the_horizon_and_past_the_last_list() {
    let mut ring = ExchangeRing::default();
    ring.apply(1_000, Vec::new(), false);
    assert_eq!(ring.valid(2_000).spans(), &[(1_000, 2_000)]);
    assert_eq!(ring.valid(i64::MAX).spans(), &[(1_000, 1_000 + GAP_MS)]);
}

#[test]
fn funding_is_kept_apart_and_only_on_change() {
    let mut ring = ExchangeRing::default();
    let with = |rate| Row {
        funding: Some((rate, 9_000_000)),
        ..row("X", 1.0, 2.0, 1.5)
    };
    ring.apply(1_000, vec![with(0.0001)], false);
    // Prices unchanged, funding unchanged: nothing.
    assert_eq!(ring.apply(3_000, vec![with(0.0001)], false), 0);
    assert_eq!(ring.size(), (1, 2));
    // Prices unchanged, funding moved: a funding point only.
    assert_eq!(ring.apply(5_000, vec![with(0.0002)], false), 0);
    assert_eq!(ring.size(), (1, 3));
    let seg = ring.segment("X", 4_000, 6_000).unwrap();
    assert_eq!(seg.points.len(), 1);
    let rates: Vec<_> = seg.funding.iter().map(|f| (f.t_ms, f.rate)).collect();
    assert_eq!(rates, vec![(4_000, 0.0001), (5_000, 0.0002)]);
}

#[test]
fn a_stamp_behind_the_last_point_is_lifted_to_it() {
    let mut ring = ExchangeRing::default();
    ring.apply(5_000, vec![row("X", 1.0, 2.0, 1.5)], false);
    ring.apply(4_000, vec![row("X", 1.1, 2.0, 1.5)], false);
    assert_eq!(ring.size(), (1, 2));
    // Both stand at 5 000; the later one is the state from there on.
    let seg = ring.segment("X", 5_000, 6_000).unwrap();
    let points: Vec<_> = seg.points.iter().map(|p| (p.t_ms, p.bid)).collect();
    assert_eq!(points, vec![(5_000, 1.1)]);
}
