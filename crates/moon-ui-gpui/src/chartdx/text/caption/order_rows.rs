//! Backend-independent rectangle occupancy for order captions at the book edge.
//!
//! Ordinary numbers reserve their own rows before expendable size/depth captions. Only
//! horizontally overlapping numbers, pinned captions, or interacting captions displace them.

use std::collections::HashSet;

/// One caption's logical-pixel rectangle and visibility policy.
#[derive(Clone, Debug)]
pub(in crate::chartdx) struct OrderCaption {
    /// Index in the chart-label list, or in the book-label list for depth captions.
    pub source: usize,
    /// Depth captions draw inside the book; chart captions draw to its left.
    pub book: bool,
    /// Identity keeps a single order's size/depth pair intact across the edge.
    pub uid: u64,
    /// Unnumbered size and depth are expendable; numbered captions must remain.
    pub secondary: bool,
    /// Hovered and dragged orders retain every caption.
    pub interacting: bool,
    /// Pinned labels already have an edge stack and bypass row suppression/displacement.
    pub pinned: bool,
    /// Distance of the line from the current price, for secondary-caption arbitration.
    pub distance: f32,
    /// Text top before and after layout. `None` means the secondary caption is hidden.
    pub top: Option<f32>,
    /// Padded horizontal extent in logical pixels, measured with the drawing font.
    pub x: [f32; 2],
    /// Text line height, including its backing plate's vertical insets.
    pub height: f32,
}

/// Whether padded horizontal extents overlap, preserving a same-order pair across the edge.
fn competes(a: &OrderCaption, b: &OrderCaption) -> bool {
    !(a.uid == b.uid && a.book != b.book) && a.x[0] < b.x[1] && b.x[0] < a.x[1]
}

/// One merged vertical run of accepted captions that compete with the caption being placed.
#[derive(Clone, Copy)]
struct Band {
    lo: f32,
    hi: f32,
    /// Accepted index that owns the top edge. Equal distances keep the earlier caption.
    lo_at: usize,
    /// Accepted index that owns the bottom edge.
    hi_at: usize,
}

/// Keep `by_y` ordered by resolved top, then by accepted index.
fn insert_by_top(by_y: &mut Vec<usize>, index: usize, captions: &[OrderCaption]) {
    let Some(top) = captions[index].top else {
        return;
    };
    let at = by_y.partition_point(|&other| {
        let y = captions[other].top.unwrap_or(f32::NEG_INFINITY);
        y < top || (y == top && other < index)
    });
    by_y.insert(at, index);
}

/// Whether `top` intersects an already accepted caption that competes horizontally.
///
/// `by_y` is ordered by top. Only the band that can reach this row is scanned: a caption whose
/// top is more than `max_h` above, or at or below this row's bottom, cannot overlap it.
fn overlaps_accepted(
    index: usize,
    top: f32,
    height: f32,
    max_h: f32,
    by_y: &[usize],
    captions: &[OrderCaption],
) -> bool {
    let caption = &captions[index];
    let lower = top - max_h;
    let upper = top + height;
    let start =
        by_y.partition_point(|&other| captions[other].top.unwrap_or(f32::INFINITY) <= lower);
    by_y[start..]
        .iter()
        .copied()
        .take_while(|&other| captions[other].top.unwrap_or(f32::INFINITY) < upper)
        .any(|other| {
            let placed = &captions[other];
            let Some(y) = placed.top else {
                return false;
            };
            competes(caption, placed) && top < y + placed.height && y < top + height
        })
}

/// One clique of accepted captions: each competes with every other member.
///
/// Joining requires overlapping the common x intersection, so a right-aligned pile stays one
/// run. Its merged y bands are updated on insert. A later displaced caption reads that run
/// instead of scanning every accepted row again.
struct RunSet {
    bands: Vec<Band>,
    /// Highest left edge and lowest right edge. Every member covers this segment.
    inter_lo: f32,
    inter_hi: f32,
    /// Outer x bounds. Missing both means the caption overlaps no member.
    bound_lo: f32,
    bound_hi: f32,
    /// `(uid, book)` so the cross-edge pair exception is one lookup.
    members: HashSet<(u64, bool)>,
}

impl RunSet {
    fn from_caption(index: usize, caption: &OrderCaption) -> Self {
        let (lo, hi) = interval(caption);
        let mut members = HashSet::with_capacity(8);
        members.insert((caption.uid, caption.book));
        Self {
            bands: vec![Band {
                lo,
                hi,
                lo_at: index,
                hi_at: index,
            }],
            inter_lo: caption.x[0],
            inter_hi: caption.x[1],
            bound_lo: caption.x[0],
            bound_hi: caption.x[1],
            members,
        }
    }

    /// Whether `caption` competes with every member already in this run.
    fn accepts_all(&self, caption: &OrderCaption) -> bool {
        caption.x[0] < self.inter_hi
            && self.inter_lo < caption.x[1]
            && !self.members.contains(&(caption.uid, !caption.book))
    }

    /// Whether `caption` might compete with some member. A miss overlaps none.
    fn may_overlap(&self, caption: &OrderCaption) -> bool {
        caption.x[0] < self.bound_hi && self.bound_lo < caption.x[1]
    }

    fn insert(&mut self, index: usize, caption: &OrderCaption) {
        let (lo, hi) = interval(caption);
        insert_interval(&mut self.bands, lo, hi, index, index);
        self.inter_lo = self.inter_lo.max(caption.x[0]);
        self.inter_hi = self.inter_hi.min(caption.x[1]);
        self.bound_lo = self.bound_lo.min(caption.x[0]);
        self.bound_hi = self.bound_hi.max(caption.x[1]);
        self.members.insert((caption.uid, caption.book));
    }
}

fn interval(caption: &OrderCaption) -> (f32, f32) {
    let top = caption.top.expect("occupied caption has a row");
    (top, top + caption.height)
}

/// Record an occupied caption. A clique grows in place; a partial overlap is listed aside.
fn occupy(
    runs: &mut Vec<RunSet>,
    partial: &mut Vec<usize>,
    index: usize,
    captions: &[OrderCaption],
) {
    let caption = &captions[index];
    if caption.top.is_none() {
        return;
    }
    if let Some(run) = runs.iter_mut().find(|run| run.accepts_all(caption)) {
        run.insert(index, caption);
        return;
    }
    if runs.iter().any(|run| run.may_overlap(caption)) {
        partial.push(index);
    } else {
        runs.push(RunSet::from_caption(index, caption));
    }
}

/// Fold one occupied interval into disjoint y runs. Edge ownership matches the y-ordered scan:
/// the smaller accepted index keeps a shared edge.
fn insert_interval(bands: &mut Vec<Band>, lo: f32, hi: f32, lo_at: usize, hi_at: usize) {
    let mut i = bands.partition_point(|band| band.hi < lo);
    if i == bands.len() || bands[i].lo > hi {
        bands.insert(
            i,
            Band {
                lo,
                hi,
                lo_at,
                hi_at,
            },
        );
        return;
    }
    let mut merged_lo = lo;
    let mut merged_hi = hi;
    let mut merged_lo_at = lo_at;
    let mut merged_hi_at = hi_at;
    let start = i;
    while i < bands.len() && bands[i].lo <= merged_hi {
        let band = bands[i];
        if band.lo < merged_lo || (band.lo == merged_lo && band.lo_at < merged_lo_at) {
            merged_lo = band.lo;
            merged_lo_at = band.lo_at;
        }
        if band.hi > merged_hi || (band.hi == merged_hi && band.hi_at < merged_hi_at) {
            merged_hi = band.hi;
            merged_hi_at = band.hi_at;
        }
        i += 1;
    }
    bands.drain(start..i);
    bands.insert(
        start,
        Band {
            lo: merged_lo,
            hi: merged_hi,
            lo_at: merged_lo_at,
            hi_at: merged_hi_at,
        },
    );
}

/// Merged vertical runs of the accepted captions that compete with `index`, in top order.
fn competing_bands(index: usize, by_y: &[usize], captions: &[OrderCaption]) -> Vec<Band> {
    let caption = &captions[index];
    let mut bands: Vec<Band> = Vec::new();
    for &other in by_y {
        let placed = &captions[other];
        let Some(y) = placed.top else {
            continue;
        };
        if !competes(caption, placed) {
            continue;
        }
        let end = y + placed.height;
        if let Some(band) = bands.last_mut()
            && y <= band.hi
        {
            if y == band.lo && other < band.lo_at {
                band.lo_at = other;
            }
            if end > band.hi {
                band.hi = end;
                band.hi_at = other;
            } else if end == band.hi && other < band.hi_at {
                band.hi_at = other;
            }
        } else {
            bands.push(Band {
                lo: y,
                hi: end,
                lo_at: other,
                hi_at: other,
            });
        }
    }
    bands
}

/// Whether a candidate row intersects a merged run. Touching at an edge does not, matching the
/// strict inequalities of the old per-caption scan.
///
/// Bands are disjoint and ordered by top, so the first run that reaches past `candidate` is the
/// only one that can cover it.
fn hits_band(candidate: f32, height: f32, bands: &[Band]) -> bool {
    let at = bands.partition_point(|band| band.hi <= candidate);
    at < bands.len() && bands[at].lo < candidate + height
}

/// Bands the caption actually competes with.
///
/// A clique it fully competes with is already merged. Anything that overlaps a clique only in
/// part falls back to the y-ordered scan, which is the same merge the placements were checked
/// against.
fn bands_for(
    index: usize,
    by_y: &[usize],
    captions: &[OrderCaption],
    runs: &[RunSet],
    partial: &[usize],
) -> Vec<Band> {
    let caption = &captions[index];
    let awkward = runs
        .iter()
        .any(|run| run.may_overlap(caption) && !run.accepts_all(caption));
    if awkward {
        return competing_bands(index, by_y, captions);
    }
    let mut bands = Vec::new();
    for run in runs.iter().filter(|run| run.accepts_all(caption)) {
        for band in &run.bands {
            insert_interval(&mut bands, band.lo, band.hi, band.lo_at, band.hi_at);
        }
    }
    for &other in partial {
        let placed = &captions[other];
        let Some(y) = placed.top else {
            continue;
        };
        if competes(caption, placed) {
            insert_interval(&mut bands, y, y + placed.height, other, other);
        }
    }
    bands
}

/// The closest free row beside a competing caption, preferring one inside `visible`.
///
/// A free slot is flush with an edge of the merged runs. Any flush position buried inside a run
/// is occupied, so the edges are the whole candidate set the per-caption scan used to walk.
/// Equal distances keep the earlier accepted caption, and a caption's upper neighbour before its
/// lower one — the order that scan visited them.
fn nearest_free(
    index: usize,
    top: f32,
    height: f32,
    by_y: &[usize],
    captions: &[OrderCaption],
    runs: &[RunSet],
    partial: &[usize],
    visible: [f32; 2],
) -> Option<f32> {
    let bands = bands_for(index, by_y, captions, runs, partial);
    let mut outside: Option<(f32, usize, u8, f32)> = None;
    let mut inside: Option<(f32, usize, u8, f32)> = None;
    let candidates: Vec<(f32, usize, u8)> = bands
        .iter()
        .flat_map(|band| {
            [
                (band.lo - height, band.lo_at, 0u8),
                (band.hi, band.hi_at, 1u8),
            ]
        })
        .collect();
    for (candidate, at, side) in candidates {
        if hits_band(candidate, height, &bands) {
            continue;
        }
        let delta = (candidate - top).abs();
        let offer = |slot: &mut Option<(f32, usize, u8, f32)>| {
            let replace = match *slot {
                None => true,
                Some((best_delta, best_at, best_side, _)) => {
                    delta < best_delta
                        || (delta == best_delta
                            && (at < best_at || (at == best_at && side < best_side)))
                }
            };
            if replace {
                *slot = Some((delta, at, side, candidate));
            }
        };
        offer(&mut outside);
        if candidate >= visible[0] && candidate + height <= visible[1] {
            offer(&mut inside);
        }
    }
    inside.or(outside).map(|(_, _, _, candidate)| candidate)
}

/// Resolve captions in place without shaping text or allocating another frame buffer.
///
/// Pinned rows and interacting sizes reserve space before interacting numbers, so even two
/// protected orders cannot cover a number with a later size. Ordinary numbers reserve their own
/// rows before secondary captions, which keep only the nearest line in an overlapping rectangle.
/// Numbers move to the closest free band only around horizontally overlapping reserved captions.
/// The input uses padded logical-pixel rectangles; output retains source indices after sorting.
/// Displaced rows prefer the supplied visible band; when it is completely full they overflow
/// rather than dropping a numbered caption or clamping it onto an occupied row.
pub(in crate::chartdx::text) fn layout_order_captions(
    captions: &mut [OrderCaption],
    visible: [f32; 2],
) {
    captions.sort_unstable_by(|a, b| {
        let rank = |c: &OrderCaption| {
            if c.pinned {
                0
            } else if c.interacting && c.secondary {
                1
            } else if c.interacting {
                2
            } else if !c.secondary {
                3
            } else {
                4
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.distance.total_cmp(&b.distance))
            .then_with(|| a.uid.cmp(&b.uid))
            .then_with(|| a.book.cmp(&b.book))
            .then_with(|| a.source.cmp(&b.source))
    });
    // Tallest accepted band. The overlap query widens by this so a binary search of the tops
    // cannot skip a caption whose own height still reaches the row.
    let max_h = captions
        .iter()
        .fold(0.0_f32, |max, caption| max.max(caption.height));
    let mut by_y: Vec<usize> = Vec::with_capacity(captions.len());
    let mut runs: Vec<RunSet> = Vec::new();
    let mut partial: Vec<usize> = Vec::new();
    for i in 0..captions.len() {
        let Some(top) = captions[i].top else {
            continue;
        };
        let height = captions[i].height;
        if captions[i].pinned || !overlaps_accepted(i, top, height, max_h, &by_y, captions) {
            insert_by_top(&mut by_y, i, captions);
            occupy(&mut runs, &mut partial, i, captions);
            continue;
        }
        if captions[i].secondary {
            if !captions[i].interacting {
                captions[i].top = None;
            } else {
                insert_by_top(&mut by_y, i, captions);
                occupy(&mut runs, &mut partial, i, captions);
            }
            continue;
        }
        if let Some(placed) =
            nearest_free(i, top, height, &by_y, captions, &runs, &partial, visible)
        {
            captions[i].top = Some(placed);
        }
        insert_by_top(&mut by_y, i, captions);
        occupy(&mut runs, &mut partial, i, captions);
    }
}

#[cfg(test)]
mod tests;
