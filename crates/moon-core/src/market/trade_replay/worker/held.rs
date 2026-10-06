//! Held tick coverage, hydration and answers.

use super::*;

/// What the store proves exhaustive inside `focus`, seeded by a walk's own `harvest`: every
/// harvest stretch widened over the tiles abutting it, plus every run of tiles the store holds
/// inside the focus, all clipped to the focus.
///
/// The walk's stretches are not in the store yet — the answer is composed before they are
/// filed — so they seed the extension rather than being found by it. Two stretches a held tile
/// sits between coalesce through that tile; two with unwalked ground between them stay two.
///
/// Args:
///     store: The worker's tiles.
///     key: Exchange key and market.
///     focus: The window's own focus spans, the outer bound of what is served.
///     harvest: The walk's stretches, or none when nothing was walked.
///
/// Returns:
///     The served coverage; empty when neither the store nor the walk holds any of the focus.
pub(super) fn held_coverage(
    store: &TickTileStore,
    key: &TileKey,
    focus: &Coverage,
    harvest: Coverage,
) -> Coverage {
    let mut runs = Coverage::none();
    for &span in harvest.spans() {
        runs.add(store.extend_over(key, span));
    }
    for &span in focus.spans() {
        for run in store.coverage_runs(key, span) {
            runs.add(run);
        }
    }
    runs.clip(focus)
}

/// Every held run of prints inside `covered`, one entry per tile with its source, in ascending
/// order across the stretches.
pub(super) fn read_coverage(
    store: &TickTileStore,
    key: &TileKey,
    covered: &Coverage,
) -> Vec<(TileSource, Vec<Tick>)> {
    let mut runs = Vec::new();
    for &(from_ms, to_ms) in covered.spans() {
        runs.extend(store.read_by_source(key, from_ms, to_ms));
    }
    runs
}

/// One ascending run of prints for the chart, and the band's per-second slots summed over every
/// tile through ONE valuation — the market's own terms (`tick_value`): a core's ring reports the
/// same wire quantity as the venue's route (contracts on a contract market), so a tile's source
/// is provenance, never a different unit. Summing per tile and merging keeps a tile's slots
/// whole when the run is later thinned for drawing.
///
/// Args:
///     runs: Tile runs in any order, each with its source.
///     value: How this market's prints are valued.
///
/// Returns:
///     Every print ascending by time, and the merged slots.
pub(super) fn flatten_runs(
    runs: Vec<(TileSource, Vec<Tick>)>,
    value: super::venue_caps::TickValue,
) -> (Vec<Tick>, Vec<crate::market::source::SideSlot>) {
    let mut ticks = Vec::new();
    let mut slots = Vec::new();
    for (_, run) in runs {
        slots.extend(crate::market::source::side_slots_of_ticks(&run, value));
        ticks.extend(run);
    }
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    (ticks, crate::market::source::merge_side_slots(slots))
}

/// Hydrate the tile store from the disk around one focus.
///
/// The disk is the tile store's memory across a restart: whatever it holds around the focus is
/// inserted FIRST, so what the stage decides is decided against everything ever fetched or
/// captured for this market, not only what this session saw. A read that times out hydrates
/// nothing and costs at worst a fetch the disk could have spared.
///
/// The disk is read BEFORE the tile lock is taken: a read waits on the cache thread for up to
/// its timeout, and a progress publisher or a held-data query blocked on the lock for that long
/// would be paying for a disk it never asked about.
pub(super) fn hydrate(
    tiles: &Mutex<TickTileStore>,
    persisted: Option<&super::trade_cache::TradeCache>,
    key: &TileKey,
    focus: &Coverage,
) {
    let Some(cache) = persisted else {
        return;
    };
    let mut read = Vec::new();
    for &(from_ms, to_ms) in focus.spans() {
        read.extend(
            cache
                .read(&key.0, &key.1, from_ms, to_ms)
                .unwrap_or_default(),
        );
    }
    if read.is_empty() {
        return;
    }
    let mut store = lock_tiles(tiles);
    for span in read {
        store.insert(
            key.clone(),
            span.from_ms,
            span.to_ms,
            span.ticks,
            span.source,
        );
    }
}

/// Answer a [`TickQuery`]: hydrate the tiles from the disk around the asked stretches, then
/// read what they hold inside them. The same two steps the tick stage runs before deciding
/// what to fetch, with the fetch left out.
///
/// Args:
///     tiles: The worker's tile store.
///     query: The stretches asked for.
pub(super) fn held_answer(tiles: &Mutex<TickTileStore>, query: &TickQuery) -> TickAnswer {
    let key: TileKey = (query.exchange_key.clone(), query.market.clone());
    hydrate(
        tiles,
        super::trade_cache::handle().as_ref(),
        &key,
        &query.spans,
    );
    let (covered, runs) = {
        let store = lock_tiles(tiles);
        let covered = held_coverage(&store, &key, &query.spans, Coverage::none());
        let runs = read_coverage(&store, &key, &covered);
        (covered, runs)
    };
    let mut ticks: Vec<Tick> = runs.into_iter().flat_map(|(_, run)| run).collect();
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    TickAnswer { ticks, covered }
}
