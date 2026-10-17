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
/// Brief locks plan the gaps and insert the result; disk I/O runs with no tile lock held.
/// A read waits on the cache thread for up to its timeout, and a progress publisher or another
/// held-data query must not pay for a disk it never asked about. A filing invalidates only its
/// market's inspected tiles; failed reads leave those tiles retryable.
///
/// Returns whether every read it needed succeeded; without a disk there is nothing to read.
pub(super) fn hydrate(
    tiles: &Mutex<TickTileStore>,
    persisted: Option<&super::trade_cache::TradeCache>,
    key: &TileKey,
    focus: &Coverage,
) -> bool {
    let Some(cache) = persisted else {
        return true;
    };
    hydrate_with(
        tiles,
        key,
        focus,
        || trade_cache::market_revision(key),
        |from, to| cache.read(&key.0, &key.1, from, to),
    )
}

/// Revision and read seams keep repeat, failure and concurrent-filing tests deterministic.
fn hydrate_with(
    tiles: &Mutex<TickTileStore>,
    key: &TileKey,
    focus: &Coverage,
    revision: impl Fn() -> u64,
    read: impl FnMut(i64, i64) -> Option<Vec<trade_cache::StoredSpan>>,
) -> bool {
    hydrate_into(tiles, key, focus, revision, read, None)
}

/// [`hydrate_with`], optionally mirroring the focus into `answer`: the tiles held when the plan
/// was taken plus every disk read, so another reader evicting shared tiles between the plan and
/// the answer cannot shrink what this query proves held.
fn hydrate_into(
    tiles: &Mutex<TickTileStore>,
    key: &TileKey,
    focus: &Coverage,
    revision: impl Fn() -> u64,
    mut read: impl FnMut(i64, i64) -> Option<Vec<trade_cache::StoredSpan>>,
    mut answer: Option<&mut TickTileStore>,
) -> bool {
    // Sample before reading: a filing during I/O must invalidate the next query too.
    let revision = revision();
    let (generation, needed) = {
        let mut store = lock_tiles(tiles);
        if let Some(answer) = answer.as_deref_mut() {
            *answer = store.snapshot(key, focus);
        }
        store.hydration_plan(key, focus, revision)
    };
    let mut settled = true;
    for &(from, to) in needed.spans() {
        match read(from, to) {
            Some(spans) => {
                if let Some(answer) = answer.as_deref_mut() {
                    for span in &spans {
                        answer.insert(
                            key.clone(),
                            span.from_ms,
                            span.to_ms,
                            span.ticks.clone(),
                            span.source,
                        );
                    }
                }
                lock_tiles(tiles).hydrate_from(key, generation, revision, (from, to), spans);
            }
            None => settled = false,
        }
    }
    settled
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
    match super::trade_cache::handle() {
        Some(cache) => answer_with(
            tiles,
            &key,
            &query.spans,
            || trade_cache::market_revision(&key),
            |from, to| cache.read(&key.0, &key.1, from, to),
        ),
        None => {
            let mirror = lock_tiles(tiles).snapshot(&key, &query.spans);
            compose_answer(&mirror, &key, &query.spans, true)
        }
    }
}

/// Seams of [`held_answer`]. The answer is composed from this query's own mirror of the store,
/// never re-read from the shared one: parallel held readers insert and evict between the plan
/// and the answer, and the answer must equal what a serial read of the same tiles and disk gives.
fn answer_with(
    tiles: &Mutex<TickTileStore>,
    key: &TileKey,
    focus: &Coverage,
    revision: impl Fn() -> u64,
    read: impl FnMut(i64, i64) -> Option<Vec<trade_cache::StoredSpan>>,
) -> TickAnswer {
    let mut mirror = TickTileStore::default();
    let settled = hydrate_into(tiles, key, focus, revision, read, Some(&mut mirror));
    compose_answer(&mirror, key, focus, settled)
}

/// The held coverage of `focus` and every print inside it, ascending.
fn compose_answer(
    mirror: &TickTileStore,
    key: &TileKey,
    focus: &Coverage,
    settled: bool,
) -> TickAnswer {
    let covered = held_coverage(mirror, key, focus, Coverage::none());
    let mut ticks: Vec<Tick> = read_coverage(mirror, key, &covered)
        .into_iter()
        .flat_map(|(_, run)| run)
        .collect();
    ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    TickAnswer {
        ticks,
        covered,
        settled,
    }
}

#[cfg(test)]
mod tests;
