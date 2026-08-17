//! Seed the bench with ONE figure of every drawing tool the build knows.
//!
//! The bench exists to judge what is drawn on top of price, and drawing tools are half of that.
//! Placing them by hand before every session is exactly the variable the bench removes, so the
//! set is generated instead: one figure per [`FigureKind`] arm, laid out across the window the
//! bench's own trades occupy.
//!
//! Coverage is enforced by construction, not by a list kept in step by hand: [`seed`] matches on
//! every arm through [`figure_for`], so a tool added to `FigureKind` fails to compile here until
//! it is given a place on the bench.

use crate::figures::tools::{
    Channel, FibRetracement, HLine, MbFib, Position, Ray, Rect, Segment, Triangle,
};
use crate::figures::{DrawStyle, FigNode, Figure, FigureKind, FigureStore, LineKind};
use crate::session::CoreId;

/// Every tool the build can draw, in the order they are laid out left to right.
///
/// A `FigureKind` value is built per slot by [`figure_for`]; this list only fixes the ORDER, and a
/// new tool is added in one place because the builder's match is exhaustive.
const SLOTS: [Slot; 9] = [
    Slot::HLine,
    Slot::Segment,
    Slot::Ray,
    Slot::Rect,
    Slot::Triangle,
    Slot::Channel,
    Slot::FibRetracement,
    Slot::MbFib,
    Slot::Position,
];

/// One laid-out drawing tool.
#[derive(Clone, Copy)]
enum Slot {
    HLine,
    Segment,
    Ray,
    Rect,
    Triangle,
    Channel,
    FibRetracement,
    MbFib,
    Position,
}

/// The window a seeded set is laid out over: the span the trades occupy and their price range.
#[derive(Clone, Copy, Debug)]
pub struct SeedWindow {
    /// Start of the trading day in Unix milliseconds.
    pub from_ms: f64,
    /// End of the trading day in Unix milliseconds.
    pub to_ms: f64,
    /// Lowest traded price.
    pub low: f64,
    /// Highest traded price.
    pub high: f64,
}

impl SeedWindow {
    /// Time at `fraction` across the window.
    fn t(&self, fraction: f64) -> f64 {
        self.from_ms + (self.to_ms - self.from_ms) * fraction
    }

    /// Price at `fraction` up the window, 0 being the low.
    fn p(&self, fraction: f64) -> f64 {
        self.low + (self.high - self.low) * fraction
    }
}

/// Write one figure of every tool into the bench's `figures.json`.
///
/// The figures are spread across the window rather than stacked: overlapping tools hide each
/// other's handles, and the point of the bench is to see each one.
///
/// Args:
///     core: Core the bench runs as; a figure belongs to the core it was drawn on.
///     market: Market the bench carries.
///     window: Span and price range to lay the set out over.
///
/// Returns:
///     How many figures were written.
pub fn seed(core: CoreId, market: &str, window: SeedWindow) -> usize {
    // The bench's `cfg/` is created empty on every run, so this starts from nothing; loading
    // rather than constructing keeps the one path that knows the file's shape.
    let mut store = FigureStore::load();
    let created_ms = window.to_ms;
    for (index, slot) in SLOTS.iter().enumerate() {
        // Each tool gets its own horizontal band so nothing lands on top of anything else.
        let left = 0.05 + 0.10 * index as f64;
        let kind = figure_for(*slot, &window, left);
        store.add(core, market, Figure::new(kind, style_for(index), created_ms));
    }
    store.save();
    SLOTS.len()
}

/// Build the figure for one slot, anchored at `left` across the window.
///
/// The match is exhaustive over [`Slot`], and every arm builds its `FigureKind` directly, so a new
/// drawing tool cannot be added to the build without also being given a place here.
fn figure_for(slot: Slot, window: &SeedWindow, left: f64) -> FigureKind {
    let width = 0.08;
    let (t0, t1) = (window.t(left), window.t(left + width));
    match slot {
        Slot::HLine => FigureKind::HLine(HLine {
            price: window.p(0.82),
        }),
        Slot::Segment => FigureKind::Segment(Segment {
            a: FigNode::new(t0, window.p(0.25)),
            b: FigNode::new(t1, window.p(0.62)),
        }),
        Slot::Ray => FigureKind::Ray(Ray {
            a: FigNode::new(t0, window.p(0.20)),
            b: FigNode::new(t1, window.p(0.38)),
        }),
        Slot::Rect => FigureKind::Rect(Rect {
            a: FigNode::new(t0, window.p(0.30)),
            b: FigNode::new(t1, window.p(0.55)),
        }),
        Slot::Triangle => FigureKind::Triangle(Triangle {
            a: FigNode::new(t0, window.p(0.28)),
            b: FigNode::new(t1, window.p(0.34)),
            c: FigNode::new(window.t(left + width * 0.5), window.p(0.60)),
        }),
        Slot::Channel => FigureKind::Channel(Channel {
            price1: window.p(0.66),
            price2: window.p(0.74),
        }),
        Slot::FibRetracement => FigureKind::FibRetracement(FibRetracement {
            a: FigNode::new(t0, window.p(0.20)),
            b: FigNode::new(t1, window.p(0.70)),
            hidden_levels: 0,
        }),
        Slot::MbFib => FigureKind::MbFib(MbFib::spanning(
            FigNode::new(t0, window.p(0.22)),
            FigNode::new(t1, window.p(0.68)),
        )),
        Slot::Position => FigureKind::Position(Position {
            t0_ms: t0,
            t1_ms: t1,
            entry: window.p(0.45),
            target: window.p(0.72),
            stop: window.p(0.32),
        }),
    }
}

/// Style for the figure in slot `index`.
///
/// Colours and line kinds are varied deliberately: a bench drawn in one colour and one thickness
/// cannot show whether the renderer honours either.
fn style_for(index: usize) -> DrawStyle {
    const COLORS: [[u8; 4]; 5] = [
        [0xE8, 0xA3, 0x3D, 0xFF],
        [0x5A, 0xA9, 0xFF, 0xFF],
        [0x2F, 0xD3, 0x7E, 0xFF],
        [0xFF, 0x5B, 0x5B, 0xFF],
        [0xC8, 0x8C, 0xF0, 0xFF],
    ];
    const KINDS: [LineKind; 4] = [
        LineKind::Solid,
        LineKind::Dash,
        LineKind::Dot,
        LineKind::DashDot,
    ];
    let color = COLORS[index % COLORS.len()];
    DrawStyle {
        color,
        // Every other figure is filled, so both filled and unfilled rendering is on screen at once.
        fill: if index.is_multiple_of(2) {
            [color[0], color[1], color[2], 0x30]
        } else {
            [0, 0, 0, 0]
        },
        thickness: 1.0 + (index % 3) as f32,
        kind: KINDS[index % KINDS.len()],
    }
}

#[cfg(test)]
mod tests;
