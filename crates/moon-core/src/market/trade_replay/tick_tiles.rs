//! Coverage-keyed store of fetched trade prints, so a window is answered from what an earlier
//! window already paid for and only the uncovered remainder goes to the venue.
//!
//! # Why this exists beside the outcome ring
//!
//! The ring in [`super::worker`] is keyed by the EXACT window a trade asked for, and every trade
//! gets its own window, so two trades on one market never share an entry even when the first
//! window's tick span holds the second whole — the second window pages the same prints from the
//! venue again while the first sits beside it already showing them (#632). Bars never had this
//! problem: they are served from the shared kline cache by COVERAGE. This store gives the ticks
//! the same shape — tiles of `(from_ms, to_ms)` per `(venue, market)` — and a window is answered
//! from the tiles it already holds, fetching only the gaps.
//!
//! # Invariants
//!
//! - Tiles of one key never overlap. [`crate::feed::types::Tick`] carries no exchange trade id,
//!   so an overlap would be a double count nothing downstream can detect; [`TickTileStore::insert`]
//!   clips a new span to the gaps it actually fills.
//! - A tile's ticks are RAW — the venue's prints as they came, before `fit_ticks` thins them —
//!   because the volume band is summed from every print, and the thinning is a per-window
//!   decision (`worker::TICK_BUDGET` against the union served), not a property of the data.
//! - An EMPTY tile is a real answer: the venue was asked and held no print there. It costs no
//!   ticks, and it is what turns "no trades" into an answer a later window inherits.
//! - Memory is bounded by ticks held ([`TILE_STORE_MAX_TICKS`]) and by tile count
//!   ([`TILE_STORE_MAX_TILES`]); eviction is oldest-inserted first and never touches the tiles of
//!   the insert in progress, so one wide harvest is held rather than discarded on arrival.
//! - Keyed by venue and market, never by core: the prints are public, and a trade on another
//!   core over the same market is served from the same tiles.

use std::collections::HashMap;

use super::TickPlan;
use crate::feed::types::Tick;

/// Ceiling on raw ticks held across every tile.
///
/// Eight tick budgets — twice the outcome ring's own ceiling, because a tile holds the raw run
/// where the ring holds a thinned one. A `Tick` is 24 bytes, so this is about 8 MB: a busy
/// perpetual prints 50–100 a second, a thin alt a handful, so this holds a few hours of the
/// former or a day of the latter.
pub(crate) const TILE_STORE_MAX_TICKS: usize = 8 * super::worker::TICK_BUDGET;

/// Ceiling on tile count, so a long run of EMPTY tiles — which cost no ticks — still cannot grow
/// the store without bound.
pub(crate) const TILE_STORE_MAX_TILES: usize = 256;

/// What one key of the store identifies: the venue (spot and linear of one brand are DIFFERENT
/// venues on one host, and their markets are frequently named identically) and the
/// exchange-native market name.
pub(crate) type TileKey = (crate::venue::Venue, String);

/// One contiguous span the venue was asked for, with every print it answered.
#[derive(Clone, Debug)]
pub(crate) struct TickTile {
    /// First millisecond covered, inclusive.
    pub from_ms: i64,
    /// Last millisecond covered, inclusive.
    pub to_ms: i64,
    /// Ascending by time, every one inside `[from_ms, to_ms]`. Empty is an answer.
    pub ticks: Vec<Tick>,
    /// Insertion order across every key; eviction removes the smallest.
    seq: u64,
}

/// The tiles of every key, each key's tiles sorted by `from_ms` and pairwise disjoint.
#[derive(Debug, Default)]
pub(crate) struct TickTileStore {
    tiles: HashMap<TileKey, Vec<TickTile>>,
    next_seq: u64,
    held_ticks: usize,
    tile_count: usize,
}

impl TickTileStore {
    /// Sub-spans of `[from_ms, to_ms]` no tile of `key` covers, ascending and disjoint.
    ///
    /// Args:
    ///     key: Venue and market.
    ///     from_ms: Left edge, inclusive.
    ///     to_ms: Right edge, inclusive.
    ///
    /// Returns:
    ///     Empty when the span is covered whole; the whole span when nothing covers any of it;
    ///     nothing at all for an inverted span.
    pub fn gaps(&self, key: &TileKey, from_ms: i64, to_ms: i64) -> Vec<(i64, i64)> {
        if from_ms > to_ms {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cursor = from_ms;
        for tile in self.overlapping(key, from_ms, to_ms) {
            if tile.from_ms > cursor {
                out.push((cursor, tile.from_ms - 1));
            }
            cursor = cursor.max(tile.to_ms.saturating_add(1));
            if cursor > to_ms {
                break;
            }
        }
        if cursor <= to_ms {
            out.push((cursor, to_ms));
        }
        out
    }

    /// Whether `[from_ms, to_ms]` is covered whole, with no gap of any width.
    #[cfg(test)]
    pub fn covers(&self, key: &TileKey, from_ms: i64, to_ms: i64) -> bool {
        from_ms <= to_ms && self.gaps(key, from_ms, to_ms).is_empty()
    }

    /// Every print held inside `[from_ms, to_ms]`, ascending by time.
    ///
    /// Answers whatever is held, covered or not: the caller decides coverage through
    /// [`Self::coverage_run`] or [`Self::covers`] first, and this only collects.
    pub fn read(&self, key: &TileKey, from_ms: i64, to_ms: i64) -> Vec<Tick> {
        let mut out = Vec::new();
        for tile in self.overlapping(key, from_ms, to_ms) {
            out.extend(
                tile.ticks
                    .iter()
                    .filter(|t| {
                        let time_ms = t.time_ms as i64;
                        time_ms >= from_ms && time_ms <= to_ms
                    })
                    .copied(),
            );
        }
        out
    }

    /// Widen `span` over every tile that overlaps or abuts it, repeatedly, until no tile touches
    /// the result — the contiguous run `span` would belong to if it were a tile itself.
    ///
    /// `span` need not be covered: a walk's own just-fetched coverage, not yet inserted, is the
    /// caller's usual seed. The result always contains `span`.
    pub fn extend_over(&self, key: &TileKey, span: (i64, i64)) -> (i64, i64) {
        let Some(tiles) = self.tiles.get(key) else {
            return span;
        };
        let mut run = span;
        let mut grew = true;
        while grew {
            grew = false;
            for tile in tiles {
                let touches = tile.from_ms <= run.1.saturating_add(1)
                    && tile.to_ms >= run.0.saturating_sub(1);
                if touches && (tile.from_ms < run.0 || tile.to_ms > run.1) {
                    run = (run.0.min(tile.from_ms), run.1.max(tile.to_ms));
                    grew = true;
                }
            }
        }
        run
    }

    /// The contiguous covered run that overlaps `seed`, or `None` when no tile does.
    ///
    /// When two disjoint runs both overlap the seed, the wider one wins — one span is what a
    /// series can carry, and the wider one withholds more bars honestly.
    pub fn coverage_run(&self, key: &TileKey, seed: (i64, i64)) -> Option<(i64, i64)> {
        let mut best: Option<(i64, i64)> = None;
        for tile in self.overlapping(key, seed.0, seed.1) {
            let run = self.extend_over(key, (tile.from_ms, tile.to_ms));
            best = match best {
                Some(b) if b.1 - b.0 >= run.1 - run.0 => Some(b),
                _ => Some(run),
            };
        }
        best
    }

    /// Record one answered span: whatever of `[from_ms, to_ms]` is not yet covered becomes a
    /// tile holding the prints of `ticks` that fall inside it.
    ///
    /// The caller must have fetched the WHOLE span — every print inside it — or the clip below
    /// turns an unfetched stretch into a false "the market was quiet here". Ticks outside the span
    /// and non-finite stamps are dropped; the rest are sorted once here so every later read is
    /// already ascending.
    ///
    /// Args:
    ///     key: Venue and market.
    ///     from_ms: Left edge, inclusive.
    ///     to_ms: Right edge, inclusive.
    ///     ticks: Every print the venue answered for the span, any order.
    pub fn insert(&mut self, key: TileKey, from_ms: i64, to_ms: i64, mut ticks: Vec<Tick>) {
        let gaps = self.gaps(&key, from_ms, to_ms);
        if gaps.is_empty() {
            return;
        }
        ticks.retain(|t| t.time_ms.is_finite());
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        let seq = self.next_seq;
        self.next_seq += 1;
        let tiles = self.tiles.entry(key).or_default();
        for (gap_from, gap_to) in gaps {
            let inside: Vec<Tick> = ticks
                .iter()
                .filter(|t| {
                    let time_ms = t.time_ms as i64;
                    time_ms >= gap_from && time_ms <= gap_to
                })
                .copied()
                .collect();
            self.held_ticks += inside.len();
            self.tile_count += 1;
            tiles.push(TickTile {
                from_ms: gap_from,
                to_ms: gap_to,
                ticks: inside,
                seq,
            });
        }
        tiles.sort_by_key(|t| t.from_ms);
        self.evict(seq);
    }

    /// Raw ticks held across every tile.
    #[cfg(test)]
    pub fn held_ticks(&self) -> usize {
        self.held_ticks
    }

    /// Tiles of `key` that intersect `[from_ms, to_ms]`, in ascending order.
    fn overlapping(
        &self,
        key: &TileKey,
        from_ms: i64,
        to_ms: i64,
    ) -> impl Iterator<Item = &TickTile> {
        self.tiles
            .get(key)
            .into_iter()
            .flatten()
            .filter(move |t| t.to_ms >= from_ms && t.from_ms <= to_ms)
    }

    /// Drop oldest-inserted tiles until both ceilings hold, never one of insert `keep_seq`.
    fn evict(&mut self, keep_seq: u64) {
        while self.held_ticks > TILE_STORE_MAX_TICKS || self.tile_count > TILE_STORE_MAX_TILES {
            let oldest = self
                .tiles
                .iter()
                .flat_map(|(key, tiles)| tiles.iter().map(move |t| (t.seq, key.clone())))
                .filter(|(seq, _)| *seq != keep_seq)
                .min_by_key(|(seq, _)| *seq);
            let Some((seq, key)) = oldest else {
                return;
            };
            let Some(tiles) = self.tiles.get_mut(&key) else {
                return;
            };
            let Some(index) = tiles.iter().position(|t| t.seq == seq) else {
                return;
            };
            let removed = tiles.remove(index);
            self.held_ticks -= removed.ticks.len();
            self.tile_count -= 1;
            if tiles.is_empty() {
                self.tiles.remove(&key);
            }
        }
    }
}

/// The part of `plan` the store does not already answer: every slice minus the tiles covering
/// it, split into its uncovered sub-spans, in an order that keeps the walk's completed prefix
/// contiguous with what the store holds.
///
/// That order is load-bearing. `paginate_ticks` reports its coverage as the hull of the slices it
/// completed, which is honest only while every point inside that hull is either completed or
/// already held — the property the original plan has by construction (each prefix of it is
/// contiguous). A slice split by cached stretches keeps it only if its sub-spans are walked from
/// the edge that touches the slices before it: the `from` edge when those reach `from - 1`, the
/// `to` edge when they reach `to + 1`. The first slice of the plan touches nothing, and either
/// order is safe for it because the store holds everything between its own sub-spans.
///
/// Args:
///     plan: The window's own tiles, in fetch order.
///     store: What is already held.
///     key: Venue and market.
///
/// Returns:
///     The residual plan; empty `slices` means nothing needs fetching. `trade_len` and
///     `focus_len` count the residual sub-spans that came from the trade and focus prefixes.
pub(crate) fn residual_plan(plan: &TickPlan, store: &TickTileStore, key: &TileKey) -> TickPlan {
    let mut slices = Vec::new();
    let mut trade_len = 0;
    let mut focus_len = 0;
    for (index, &(from_ms, to_ms)) in plan.slices.iter().enumerate() {
        let mut gaps = store.gaps(key, from_ms, to_ms);
        let before = &plan.slices[..index];
        // A predecessor reaching `to_ms + 1` starts no later than it and ends past `to_ms`;
        // written without the bare `+ 1` so a stamp at the type's edge cannot wrap.
        let touches_right = before
            .iter()
            .any(|s| s.0 <= to_ms.saturating_add(1) && s.1 > to_ms);
        let touches_left = before
            .iter()
            .any(|s| s.0 < from_ms && s.1 >= from_ms.saturating_sub(1));
        if touches_right && !touches_left {
            gaps.reverse();
        }
        slices.extend(gaps);
        if index + 1 == plan.trade_len {
            trade_len = slices.len();
        }
        if index + 1 == plan.focus_len {
            focus_len = slices.len();
        }
    }
    TickPlan {
        slices,
        trade_len,
        focus_len,
    }
}

#[cfg(test)]
mod tests;
