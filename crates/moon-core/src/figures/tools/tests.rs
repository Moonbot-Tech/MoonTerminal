//! Registry contract, generic drag/pick behaviour, and the test doubles every tool test uses.

use super::*;
use crate::figures::proj::Proj;
use crate::figures::sink::{GeomSink, LabelPlace, LabelText, Stroke};
use crate::figures::style::LineKind;
use crate::figures::Figure;

/// Linear projection standing in for a chart pane: 1 px per 1000 ms, 1 px per 0.5 price, with
/// price growing UPWARD (y decreasing), as on the real chart.
pub(super) struct TestProj;

impl TestProj {
    pub(super) const T0_MS: f64 = 1_000_000.0;
    pub(super) const PX_PER_MS: f64 = 0.001;
    pub(super) const P0: f64 = 100.0;
    pub(super) const PX_PER_PRICE: f64 = 2.0;
}

impl Proj for TestProj {
    fn x_of_time(&self, time_ms: f64) -> f32 {
        ((time_ms - Self::T0_MS) * Self::PX_PER_MS) as f32
    }

    fn y_of_price(&self, price: f64) -> f32 {
        ((Self::P0 - price) * Self::PX_PER_PRICE) as f32
    }

    fn time_at_x(&self, x: f32) -> f64 {
        Self::T0_MS + x as f64 / Self::PX_PER_MS
    }

    fn price_at_y(&self, y: f32) -> f64 {
        Self::P0 - y as f64 / Self::PX_PER_PRICE
    }
}

/// Records what a tool emitted, so a geometry test asserts on primitives instead of pixels.
#[derive(Default)]
pub(super) struct RecSink {
    pub(super) hlines: Vec<f64>,
    pub(super) segs: Vec<(FigNode, FigNode)>,
    /// `(t0_ms, t1_ms, p0, p1)` of each filled band.
    pub(super) bands: Vec<(f64, f64, f64, f64)>,
    /// Alpha each band was filled with, in the same order.
    pub(super) band_alphas: Vec<f32>,
    pub(super) handles: Vec<FigNode>,
    pub(super) labels: Vec<(FigNode, LabelPlace, LabelText)>,
}

impl GeomSink for RecSink {
    fn hline(&mut self, price: f64, _stroke: &Stroke) {
        self.hlines.push(price);
    }

    fn seg(&mut self, a: FigNode, b: FigNode, _stroke: &Stroke) {
        self.segs.push((a, b));
    }

    fn band(&mut self, t0_ms: f64, t1_ms: f64, p0: f64, p1: f64, color: [f32; 4]) {
        self.bands.push((t0_ms, t1_ms, p0, p1));
        self.band_alphas.push(color[3]);
    }

    fn handle(&mut self, at: FigNode, _color: [f32; 4]) {
        self.handles.push(at);
    }

    fn label(&mut self, at: FigNode, place: LabelPlace, text: LabelText, _color: [f32; 4]) {
        self.labels.push((at, place, text));
    }
}

/// Build context with the given interaction state and a stroke no assertion depends on.
pub(super) fn ctx(hot: bool, handles: bool) -> BuildCtx {
    BuildCtx {
        stroke: Stroke {
            color: [1.0, 1.0, 1.0, 1.0],
            thickness: 1.0,
            kind: LineKind::Solid,
        },
        fill: [1.0, 1.0, 1.0, 1.0],
        hot,
        handles,
    }
}

/// Collect a figure's primitives through the generic entry point.
pub(super) fn build(kind: &FigureKind, ctx: BuildCtx) -> RecSink {
    let mut sink = RecSink::default();
    build_figure(kind, &ctx, &mut sink);
    sink
}

/// `n` distinct nodes: a real figure, never a degenerate one. A tool may legitimately draw
/// nothing for a zero-size figure, which would make a contract test pass for the wrong reason.
fn nodes(n: usize) -> Vec<FigNode> {
    (0..n)
        .map(|i| FigNode::new(TestProj::T0_MS + i as f64 * 1_000.0, 100.0 + i as f64 * 5.0))
        .collect()
}

fn figure(kind: FigureKind) -> Figure {
    Figure::new(kind, crate::figures::DrawStyle::default(), 0)
}

#[test]
fn registry_rows_match_their_tool_and_are_uniquely_keyed() {
    let mut keys = Vec::new();
    for def in REGISTRY {
        assert_eq!(def.tool.def().key, def.key, "def() must find its own row");
        assert!(def.clicks >= 1, "{} places no node", def.key);
        assert!(!def.locale_key.is_empty(), "{} has no name key", def.key);
        keys.push(def.key);
    }
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(before, keys.len(), "duplicate tool key in the registry");
}

#[test]
fn make_refuses_a_half_placed_figure() {
    for def in REGISTRY {
        let short = nodes(def.clicks as usize - 1);
        assert!(
            (def.make)(&short).is_none(),
            "{} built a figure from {} of {} nodes",
            def.key,
            short.len(),
            def.clicks
        );
        let full = nodes(def.clicks as usize);
        let made = (def.make)(&full).expect("full node set must build");
        assert_eq!(
            made.tool(),
            def.tool,
            "{} built a figure of another tool",
            def.key
        );
    }
}

#[test]
fn every_tool_previews_before_its_last_click() {
    let cursor = FigNode::new(TestProj::T0_MS + 5_000.0, 110.0);
    for def in REGISTRY {
        let placed = nodes(def.clicks as usize - 1);
        assert!(
            (def.preview)(&placed, cursor).is_some(),
            "{} shows nothing under the cursor before its last click",
            def.key
        );
    }
}

#[test]
fn tool_cycle_visits_every_tool_once() {
    let first = REGISTRY[0].tool;
    let mut seen = vec![first];
    let mut t = first;
    for _ in 1..REGISTRY.len() {
        t = t.next();
        assert!(!seen.contains(&t), "cycle repeats {t:?} before wrapping");
        seen.push(t);
    }
    assert_eq!(t.next(), first, "cycle must wrap to the first tool");
}

#[test]
fn body_drag_moves_a_figure_by_the_delta() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let mut fig = figure(FigureKind::Segment(Segment { a, b }));
    assert!(drag_figure(&mut fig, Grab::Body, 2_000.0, -5.0));
    let FigureKind::Segment(seg) = &fig.kind else {
        panic!("kind changed under a drag")
    };
    assert_eq!(seg.a, FigNode::new(TestProj::T0_MS + 2_000.0, 95.0));
    assert_eq!(seg.b, FigNode::new(TestProj::T0_MS + 12_000.0, 115.0));
}

#[test]
fn a_zero_delta_drag_reports_no_change() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let mut fig = figure(FigureKind::Segment(Segment { a, b }));
    assert!(!drag_figure(&mut fig, Grab::Body, 0.0, 0.0));
}

#[test]
fn a_grab_of_a_handle_the_tool_does_not_have_does_nothing() {
    // A stale grab index must not panic or move something else: tools differ in handle count.
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let mut fig = figure(FigureKind::Segment(Segment { a, b }));
    assert!(!drag_figure(&mut fig, Grab::Handle(9), 1.0, 1.0));
    assert_eq!(fig.kind, FigureKind::Segment(Segment { a, b }));
}

#[test]
fn handle_drag_moves_only_the_grabbed_handle() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let mut fig = figure(FigureKind::Segment(Segment { a, b }));
    assert!(drag_figure(&mut fig, Grab::Handle(1), 0.0, 4.0));
    let FigureKind::Segment(seg) = &fig.kind else {
        panic!("kind changed under a drag")
    };
    assert_eq!(seg.a, a, "the other end must not move");
    assert_eq!(seg.b.price, 124.0);
}

#[test]
fn knot_tools_pick_the_nearest_handle_within_the_threshold() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let kind = FigureKind::Segment(Segment { a, b });
    // `b` sits at x=10, y=-40; aim 3 px away from it.
    let near_b = (13.0, -40.0);
    assert_eq!(pick_handle(&kind, near_b, &TestProj, 6.0), Some(1));
    assert_eq!(
        pick_handle(&kind, near_b, &TestProj, 2.0),
        None,
        "a tighter threshold must miss"
    );
}

#[test]
fn picking_a_figure_takes_the_nearest_body_and_misses_past_the_threshold() {
    let far = figure(FigureKind::HLine(HLine { price: 90.0 }));
    let mut near = figure(FigureKind::HLine(HLine { price: 100.0 }));
    near.id = 7;
    let figs = [far, near];
    // price 100 is at y=0, price 90 at y=20; aim 2 px under the nearer line.
    assert_eq!(pick_figure(&figs, (5.0, 2.0), &TestProj, 6.0), Some(7));
    assert_eq!(
        pick_figure(&figs, (5.0, 10.0), &TestProj, 6.0),
        None,
        "a point between the two lines is close to neither"
    );
    assert_eq!(pick_figure([].iter(), (5.0, 2.0), &TestProj, 6.0), None);
}

#[test]
fn price_line_tools_pick_by_vertical_distance_alone() {
    let kind = FigureKind::Channel(Channel {
        price1: 100.0,
        price2: 110.0,
    });
    // price 110 is at y=-20; x is irrelevant for a full-width line.
    assert_eq!(pick_handle(&kind, (900.0, -21.0), &TestProj, 6.0), Some(1));
    assert_eq!(pick_handle(&kind, (900.0, 1.0), &TestProj, 6.0), Some(0));
}

#[test]
fn only_a_selected_knot_tool_emits_handles() {
    let a = FigNode::new(TestProj::T0_MS, 100.0);
    let b = FigNode::new(TestProj::T0_MS + 10_000.0, 120.0);
    let seg = FigureKind::Segment(Segment { a, b });
    assert_eq!(build(&seg, ctx(false, true)).handles, vec![a, b]);
    assert!(build(&seg, ctx(true, false)).handles.is_empty());
    let ch = FigureKind::Channel(Channel {
        price1: 100.0,
        price2: 110.0,
    });
    assert!(
        build(&ch, ctx(true, true)).handles.is_empty(),
        "a price-line tool draws no knots"
    );
}

#[test]
fn only_a_ratio_scale_labels_an_idle_chart() {
    // A readout that appears on hover costs nothing at rest; a Fibonacci scale is the exception,
    // because a level whose price shows only under the cursor cannot be read at a glance.
    for def in REGISTRY {
        let kind = (def.make)(&nodes(def.clicks as usize)).expect("full node set must build");
        let idle = build(&kind, ctx(false, false)).labels.len();
        if def.tool == FigureTool::FibRetracement {
            assert!(idle > 0, "a scale must name its levels at rest");
        } else {
            assert_eq!(idle, 0, "{} labels an idle chart", def.key);
        }
    }
}
