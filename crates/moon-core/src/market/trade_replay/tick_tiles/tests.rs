use super::*;
use crate::feed::Side;

fn key() -> TileKey {
    ("3:00000000".into(), "BTCUSDT".into())
}

fn tick(time_ms: i64) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price: 10.0,
        qty: 1.0,
        side: Side::Buy,
    }
}

fn ticks(times: &[i64]) -> Vec<Tick> {
    times.iter().map(|&t| tick(t)).collect()
}

fn times(ticks: &[Tick]) -> Vec<i64> {
    ticks.iter().map(|t| t.time_ms as i64).collect()
}

/// An empty store has one gap — the whole span — and a filled one has none.
#[test]
fn gaps_report_only_the_uncovered_stretches() {
    let mut store = TickTileStore::default();
    assert_eq!(store.gaps(&key(), 100, 199), vec![(100, 199)]);
    store.insert(key(), 120, 149, ticks(&[130]), TileSource::Venue);
    assert_eq!(store.gaps(&key(), 100, 199), vec![(100, 119), (150, 199)]);
    store.insert(key(), 100, 119, Vec::new(), TileSource::Venue);
    store.insert(key(), 150, 199, ticks(&[160, 170]), TileSource::Venue);
    assert!(store.gaps(&key(), 100, 199).is_empty());
    assert!(store.covers(&key(), 100, 199));
    assert!(!store.covers(&key(), 99, 199));
    assert!(
        store.gaps(&key(), 200, 100).is_empty(),
        "inverted span asks nothing"
    );
}

/// A span overlapping held tiles is clipped to the gaps: the held prints are never doubled.
#[test]
fn insert_clips_to_the_gaps_and_never_double_counts() {
    let mut store = TickTileStore::default();
    store.insert(key(), 100, 199, ticks(&[110, 150, 190]), TileSource::Venue);
    // A wider re-fetch carrying the same prints again plus new edges.
    store.insert(
        key(),
        50,
        249,
        ticks(&[60, 110, 150, 190, 240]),
        TileSource::Venue,
    );
    assert_eq!(
        times(&store.read(&key(), 0, 300)),
        vec![60, 110, 150, 190, 240]
    );
    assert_eq!(store.held_ticks(), 5);
    assert!(store.covers(&key(), 50, 249));
}

/// Non-finite stamps and prints outside the span are dropped, and reads come back ascending
/// whatever order the venue answered in.
#[test]
fn insert_sorts_and_drops_what_is_not_inside() {
    let mut store = TickTileStore::default();
    let mut rows = ticks(&[190, 110, 150, 999]);
    rows.push(Tick {
        time_ms: f64::NAN,
        ..tick(0)
    });
    store.insert(key(), 100, 199, rows, TileSource::Venue);
    assert_eq!(times(&store.read(&key(), 100, 199)), vec![110, 150, 190]);
    assert_eq!(times(&store.read(&key(), 140, 199)), vec![150, 190]);
}

/// Abutting tiles form one run; a hole splits it; the run around a seed picks the wider side.
#[test]
fn coverage_run_follows_adjacency_not_the_hull() {
    let mut store = TickTileStore::default();
    store.insert(key(), 100, 199, Vec::new(), TileSource::Venue);
    store.insert(key(), 200, 299, Vec::new(), TileSource::Venue);
    store.insert(key(), 400, 449, Vec::new(), TileSource::Venue);
    assert_eq!(store.coverage_run(&key(), (150, 160)), Some((100, 299)));
    assert_eq!(store.coverage_run(&key(), (410, 420)), Some((400, 449)));
    assert_eq!(store.coverage_run(&key(), (300, 399)), None);
    // Both runs overlap this seed; the wider one is the answer.
    assert_eq!(store.coverage_run(&key(), (250, 420)), Some((100, 299)));
    assert_eq!(store.extend_over(&key(), (300, 399)), (100, 449));
    assert_eq!(store.extend_over(&key(), (600, 700)), (600, 700));
}

/// Keys never bleed into each other: the same market on another venue is another store.
#[test]
fn keys_are_isolated() {
    let mut store = TickTileStore::default();
    let other = ("4:00000000".into(), "BTCUSDT".into());
    store.insert(key(), 100, 199, ticks(&[150]), TileSource::Venue);
    assert_eq!(store.gaps(&other, 100, 199), vec![(100, 199)]);
    assert!(store.read(&other, 0, 300).is_empty());
}

/// Past the tick ceiling the oldest tile goes first, and the insert in progress is never the
/// one evicted, however large.
#[test]
fn eviction_is_oldest_first_and_spares_the_current_insert() {
    let mut store = TickTileStore::default();
    let half: Vec<i64> = (0..(TILE_STORE_MAX_TICKS as i64 / 2)).collect();
    store.insert(key(), 0, 999_999, ticks(&half), TileSource::Venue);
    store.insert(
        key(),
        1_000_000,
        1_999_999,
        ticks(&half.iter().map(|t| t + 1_000_000).collect::<Vec<_>>()),
        TileSource::Venue,
    );
    assert_eq!(store.held_ticks(), TILE_STORE_MAX_TICKS);
    // One more tick than the ceiling allows: the FIRST tile must go, not the third.
    let third: Vec<i64> = (2_000_000..2_000_002).collect();
    store.insert(
        key(),
        2_000_000,
        2_999_999,
        ticks(&third),
        TileSource::Venue,
    );
    assert!(!store.covers(&key(), 0, 999_999), "oldest tile evicted");
    assert!(store.covers(&key(), 1_000_000, 2_999_999));
    // A single insert wider than the whole ceiling is still held.
    let huge: Vec<i64> = (5_000_000..5_000_000 + TILE_STORE_MAX_TICKS as i64 + 10).collect();
    store.insert(key(), 5_000_000, 5_999_999, ticks(&huge), TileSource::Venue);
    assert!(store.covers(&key(), 5_000_000, 5_999_999));
    assert_eq!(store.held_ticks(), TILE_STORE_MAX_TICKS + 10);
    assert!(!store.covers(&key(), 1_000_000, 1_999_999));
}

/// Empty tiles cost no ticks but still count against the tile ceiling.
#[test]
fn empty_tiles_are_bounded_by_count() {
    let mut store = TickTileStore::default();
    for i in 0..(TILE_STORE_MAX_TILES as i64 + 5) {
        store.insert(key(), i * 100, i * 100 + 99, Vec::new(), TileSource::Venue);
    }
    assert!(
        !store.covers(&key(), 0, 99),
        "the first empty tile was evicted"
    );
    let last = TILE_STORE_MAX_TILES as i64 + 4;
    assert!(store.covers(&key(), last * 100, last * 100 + 99));
}

/// One insert over a hull with held stretches inside files every gap as its own tile, all under
/// ONE insert — so the eviction that insert runs spares every one of them, however many.
#[test]
fn one_insert_over_a_hull_files_every_gap_and_eviction_spares_them_all() {
    let mut store = TickTileStore::default();
    // Crowd the store right up to the tile ceiling with old tiles on another key.
    let other = ("4:00000000".into(), "ETHUSDT".into());
    for i in 0..TILE_STORE_MAX_TILES as i64 {
        store.insert(
            other.clone(),
            i * 100,
            i * 100 + 99,
            Vec::new(),
            TileSource::Venue,
        );
    }
    // Held stretches inside the hull, so the hull splits into three gaps.
    store.insert(key(), 120, 139, ticks(&[130]), TileSource::Venue);
    store.insert(key(), 160, 179, ticks(&[170]), TileSource::Venue);
    store.insert(key(), 100, 199, ticks(&[105, 150, 190]), TileSource::Venue);
    assert!(
        store.covers(&key(), 100, 199),
        "every gap of the hull filed"
    );
    assert_eq!(
        times(&store.read(&key(), 100, 199)),
        vec![105, 130, 150, 170, 190]
    );
    assert!(
        !store.covers(&other, 0, 99),
        "the ceiling was paid by the oldest tile of the other key, not by this insert"
    );
}

/// A tile of each source reads back with its own source, in tile order.
#[test]
fn read_by_source_keeps_each_tile_with_its_source() {
    let mut store = TickTileStore::default();
    store.insert(key(), 100, 199, ticks(&[150]), TileSource::Venue);
    store.insert(key(), 200, 299, ticks(&[250]), TileSource::Core);
    let runs = store.read_by_source(&key(), 0, 300);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].0, TileSource::Venue);
    assert_eq!(times(&runs[0].1), vec![150]);
    assert_eq!(runs[1].0, TileSource::Core);
    assert_eq!(times(&runs[1].1), vec![250]);
    assert_eq!(
        TileSource::from_code(TileSource::Core.code()),
        TileSource::Core
    );
    assert_eq!(TileSource::from_code(7), TileSource::Venue);
}

fn plan(slices: &[(i64, i64)], trade_len: usize, focus_len: usize) -> TickPlan {
    TickPlan {
        slices: slices.to_vec(),
        trade_len,
        focus_len,
    }
}

/// A fully covered plan leaves nothing to fetch; an uncovered one is fetched whole.
#[test]
fn residual_is_empty_when_covered_and_whole_when_not() {
    let mut store = TickTileStore::default();
    let p = plan(&[(100, 199), (200, 299), (0, 99)], 2, 3);
    assert_eq!(residual_plan(&p, &store, &key()), p);
    store.insert(key(), 0, 299, ticks(&[5, 150, 250]), TileSource::Venue);
    let residual = residual_plan(&p, &store, &key());
    assert!(residual.slices.is_empty());
    assert_eq!(residual.trade_len, 0);
    assert_eq!(residual.focus_len, 0);
}

/// A slice split by a cached stretch is walked from the edge touching the slices before it, so
/// the walk's completed prefix stays contiguous with what the store holds — forward here.
#[test]
fn residual_walks_a_split_slice_from_the_edge_that_touches_its_predecessors() {
    let mut store = TickTileStore::default();
    // Forward route: trade tiles ascending, then the trail, then the lead walking left.
    let p = plan(&[(100, 199), (200, 299), (300, 399), (0, 99)], 2, 4);
    store.insert(key(), 230, 259, Vec::new(), TileSource::Venue);
    store.insert(key(), 40, 59, Vec::new(), TileSource::Venue);
    let residual = residual_plan(&p, &store, &key());
    assert_eq!(
        residual.slices,
        vec![
            (100, 199),
            // Predecessor reaches 199 = from - 1: ascending.
            (200, 229),
            (260, 299),
            (300, 399),
            // Predecessor reaches 100 = to + 1: descending, away from the trade.
            (60, 99),
            (0, 39),
        ]
    );
    assert_eq!(residual.trade_len, 3);
    assert_eq!(residual.focus_len, 6);
}

/// The backward twin: trade tiles descending from the exit, split sub-spans descending too.
#[test]
fn residual_keeps_a_backward_plan_descending() {
    let mut store = TickTileStore::default();
    let p = plan(&[(200, 299), (100, 199), (0, 99), (300, 399)], 2, 4);
    store.insert(key(), 130, 159, Vec::new(), TileSource::Venue);
    let residual = residual_plan(&p, &store, &key());
    assert_eq!(
        residual.slices,
        vec![(200, 299), (160, 199), (100, 129), (0, 99), (300, 399)]
    );
    assert_eq!(residual.trade_len, 3);
    assert_eq!(residual.focus_len, 5);
}
