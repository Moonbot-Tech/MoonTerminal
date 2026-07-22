//! Order book model for the glass layer: cumulative depth rectangles spanning
//! from one price level to the next, plus separate level lines.
//!
//! Each bar family is normalized against its own maximum within the panel's
//! visible price window (`build_instances`), not against the entire book.
//! Otherwise, when zoomed into a narrow price range, levels near the spread are
//! only a tiny fraction of the full cumulative depth and the whole book
//! collapses into hair-thin bars. Within the visible window, the largest
//! cumulative depth bar spans the full width and the largest individual level
//! line reaches 85%, so a transient wall stands out clearly at its price.

use crate::feed::OrderBook;

/// An order book rectangle instance whose layout matches the native book shaders.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LevelInstance {
    /// Level price: one edge of a fill bar or the center of a level line.
    pub price: f32,
    /// Signed delta from `price` to the fill bar's other price edge.
    pub span: f32,
    /// Bar length from 0 to 1 as a fraction of the zone width.
    pub len_norm: f32,
    /// 0 = bid fill, 1 = ask fill, 2 = bid level line, 3 = ask level line.
    pub kind: f32,
}

/// CPU representation of a visible order book level for text labels. GPU instances carry only
/// geometry and normalized length, while labels also need the side and actual quantity.
#[derive(Clone, Copy, Debug)]
pub struct BookDepthPoint {
    pub price: f32,
    /// Individual level quantity in the base asset.
    pub qty: f32,
    /// Cumulative quantity from the spread through this level, in the base asset.
    pub cum_qty: f32,
    /// Individual level notional (`qty * price`), equivalent to `Quantity * Rate` in Moonbot.
    pub notional: f32,
    /// Cumulative notional from the spread through this level.
    pub cum_notional: f32,
    pub is_ask: bool,
}

/// Raw order book level independent of the window: geometry and quantities. `len_norm` is
/// calculated later in `build_instances` for the visible window of a specific panel.
#[derive(Clone, Copy)]
struct RawLevel {
    price: f32,
    /// Signed delta to the bar's other price edge. The best bid or ask extends
    /// deeper into the book rather than into the spread.
    span: f32,
    /// Individual level quantity used for the separate level line.
    qty: f32,
    /// Individual level notional (`qty * price`).
    notional: f32,
    /// Cumulative quantity from the spread through this level, used for the depth bar.
    cum: f32,
    /// Cumulative notional from the spread through this level.
    cum_notional: f32,
    is_ask: bool,
}

#[derive(Default)]
pub struct OrderBookModel {
    /// Bids in descending price order, followed by asks in ascending price order.
    raw: Vec<RawLevel>,
}

impl OrderBookModel {
    pub fn update(&mut self, book: &OrderBook) {
        self.raw.clear();

        // The exchange supplies an already sorted book (bids descending, asks ascending), and
        // that order is preserved through wire -> parse -> feed because moonproto does not
        // reorder it. `push_side` derives each span from a neighboring level and REQUIRES this
        // order. Debug assertions guard the assumption, while release builds avoid redundant
        // sorting and cloning on the market-data refresh path.
        debug_assert!(
            book.bids.windows(2).all(|w| w[0].price >= w[1].price),
            "bids must arrive descending"
        );
        debug_assert!(
            book.asks.windows(2).all(|w| w[0].price <= w[1].price),
            "asks must arrive ascending"
        );

        push_side(&mut self.raw, &book.bids, false);
        push_side(&mut self.raw, &book.asks, true);
    }

    /// Builds GPU instances, normalizing bar lengths against the maximum among levels whose
    /// bars overlap the visible window `[lo, hi]` in price units. Levels outside the window are
    /// not emitted: the scissor still guards bar edges, but the CPU and GPU do not process
    /// order book data that is known to be invisible.
    pub fn build_instances(&self, lo: f32, hi: f32, out: &mut Vec<LevelInstance>) {
        out.clear();

        // Both bid and ask bars share the denominator for the visible window, keeping walls on
        // either side visually comparable. Invisible levels are omitted from the GPU buffer:
        // the order book is drawn as ordinary opaque rectangles over the visible price range.
        let mut max_qty = 1e-6_f32;
        let mut max_cum = 1e-6_f32;
        let mut visible: Vec<&RawLevel> = Vec::new();
        for r in &self.raw {
            if !level_overlaps(r, lo, hi) {
                continue;
            }
            max_qty = max_qty.max(r.qty);
            max_cum = max_cum.max(r.cum);
            visible.push(r);
        }

        let inv_max_cum = 1.0 / max_cum.max(1e-6);
        let inv_max_qty = 1.0 / max_qty.max(1e-6);
        out.reserve(visible.len().saturating_mul(2));

        for r in &visible {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.cum * inv_max_cum).clamp(0.0, 1.0),
                kind: if r.is_ask { 1.0 } else { 0.0 },
            });
        }

        for r in visible {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.qty * inv_max_qty).clamp(0.0, 1.0) * 0.85,
                kind: if r.is_ask { 3.0 } else { 2.0 },
            });
        }
    }

    /// Collects depth points whose bars overlap `[lo, hi]` for CPU-side order book label
    /// calculations. This is not a separate history store, but a snapshot of the same book model
    /// used to build GPU instances.
    pub fn collect_visible_depth(&self, lo: f32, hi: f32, out: &mut Vec<BookDepthPoint>) {
        out.clear();
        for r in &self.raw {
            if level_overlaps(r, lo, hi) {
                out.push(BookDepthPoint {
                    price: r.price,
                    qty: r.qty,
                    cum_qty: r.cum,
                    notional: r.notional,
                    cum_notional: r.cum_notional,
                    is_ask: r.is_ask,
                });
            }
        }
    }

    /// Returns the number of order book levels for the diagnostic counter.
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Returns the book's best `(bid, ask)`. If only one side is populated, its best price is
    /// returned in both positions for a zero spread. An empty book or invalid best price returns
    /// `None`. `raw` stores descending bids followed by ascending asks, so the first `!is_ask`
    /// entry is the best bid and the first `is_ask` entry is the best ask. These prices drive
    /// chart auto-focus before any trades arrive, centering and sizing the range from the book
    /// rather than from the default value of zero.
    pub fn best_bid_ask(&self) -> Option<(f32, f32)> {
        let best_bid = self.raw.iter().find(|r| !r.is_ask).map(|r| r.price);
        let best_ask = self.raw.iter().find(|r| r.is_ask).map(|r| r.price);
        let (bid, ask) = match (best_bid, best_ask) {
            (Some(b), Some(a)) => (b, a),
            (Some(p), None) | (None, Some(p)) => (p, p),
            (None, None) => return None,
        };
        (bid.is_finite() && ask.is_finite() && bid > 0.0 && ask > 0.0).then_some((bid, ask))
    }
}

fn level_overlaps(r: &RawLevel, lo: f32, hi: f32) -> bool {
    let other = r.price + r.span;
    r.price.max(other) >= lo && r.price.min(other) <= hi
}

fn push_side(out: &mut Vec<RawLevel>, levels: &[crate::feed::Level], is_ask: bool) {
    let n = levels.len();
    let mut cum = 0.0_f32;
    let mut cum_notional = 0.0_f32;
    for i in 0..n {
        let l = levels[i];
        cum += l.qty;
        let notional = l.qty * l.price;
        cum_notional += notional;
        // The signed edge delta makes the best level extend deeper into the book, while the
        // remaining levels join back to their neighbor toward the spread. This keeps bids and
        // asks from overlapping in the spread while preserving continuous depth.
        let span = if n > 1 {
            let neighbor = if i > 0 {
                levels[i - 1].price
            } else {
                levels[1].price
            };
            neighbor - levels[i].price
        } else {
            let width = (l.price.abs() * 0.0005).max(1e-6);
            if is_ask {
                width
            } else {
                -width
            }
        }
        .clamp(-f32::MAX, f32::MAX);
        // A degenerate span (duplicate price or neighboring prices collapsed to the same f32)
        // gets a nominal width scaled to the price. Do not use an absolute epsilon as the zero
        // threshold or fallback width: on micro-priced markets such as BOME with a 1e-7 tick,
        // an absolute 1e-6 swallowed the real spacing across the book. Bars then stopped meeting
        // their neighbors, leaving black gaps wherever holes in the price ladder exceeded 1e-6.
        let span = if span == 0.0 {
            let w = (l.price.abs() * 1e-7).max(f32::MIN_POSITIVE);
            if is_ask {
                w
            } else {
                -w
            }
        } else {
            span
        };

        out.push(RawLevel {
            price: l.price,
            span,
            qty: l.qty,
            notional,
            cum,
            cum_notional,
            is_ask,
        });
    }
}
