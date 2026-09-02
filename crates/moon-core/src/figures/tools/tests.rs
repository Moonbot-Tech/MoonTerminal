//! Registry contract, generic drag/pick behaviour, and the test doubles every tool test uses.

use super::*;
use crate::figures::Figure;
use crate::figures::proj::Proj;
use crate::figures::sink::{GeomSink, LabelPlace, LabelText, Stroke};
use crate::figures::style::LineKind;

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
    /// Thickness of each full-width line, in the same order: a tool may weight one to show it can
    /// be grabbed, and geometry alone cannot see that.
    pub(super) hline_widths: Vec<f32>,
    pub(super) segs: Vec<(FigNode, FigNode)>,
    /// Colour of each segment, in the same order: a coloured ratio scale is wrong if the hues are
    /// wrong, and geometry alone cannot see that.
    pub(super) seg_colors: Vec<[f32; 4]>,
    /// Each ray, as `(origin, aimed-through)`. Kept apart from `segs` on purpose: a tool that
    /// emitted a segment where a ray was meant would draw a line that simply stops, and a recorder
    /// that folded the two together could not tell that from correct.
    pub(super) rays: Vec<(FigNode, FigNode)>,
    /// `(t0_ms, t1_ms, p0, p1)` of each filled band.
    pub(super) bands: Vec<(f64, f64, f64, f64)>,
    /// Colour each band was filled with, in the same order.
    pub(super) band_colors: Vec<[f32; 4]>,
    pub(super) handles: Vec<FigNode>,
    pub(super) labels: Vec<(FigNode, LabelPlace, LabelText)>,
    /// Colour of each label, in the same order.
    pub(super) label_colors: Vec<[f32; 4]>,
}

impl GeomSink for RecSink {
    fn hline(&mut self, price: f64, stroke: &Stroke) {
        self.hlines.push(price);
        self.hline_widths.push(stroke.thickness);
    }

    fn seg(&mut self, a: FigNode, b: FigNode, stroke: &Stroke) {
        self.segs.push((a, b));
        self.seg_colors.push(stroke.color);
    }

    fn ray(&mut self, a: FigNode, b: FigNode, _stroke: &Stroke) {
        self.rays.push((a, b));
    }

    fn band(&mut self, t0_ms: f64, t1_ms: f64, p0: f64, p1: f64, color: [f32; 4]) {
        self.bands.push((t0_ms, t1_ms, p0, p1));
        self.band_colors.push(color);
    }

    fn handle(&mut self, at: FigNode, _color: [f32; 4]) {
        self.handles.push(at);
    }

    fn label(&mut self, at: FigNode, place: LabelPlace, text: LabelText, color: [f32; 4]) {
        self.labels.push((at, place, text));
        self.label_colors.push(color);
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
        fill: [1.0, 1.0, 1.0, 0.2],
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
    Figure::new(kind, crate::figures::DrawStyle::default(), 0.0)
}

#[test]
fn registry_rows_match_their_tool_and_are_uniquely_keyed() {
    let mut keys = Vec::new();
    for def in REGISTRY {
        assert_eq!(def.tool.def().key, def.key, "def() must find its own row");
        assert!(def.clicks >= 1, "{} places no node", def.key);
        assert!(!def.locale_key.is_empty(), "{} has no name key", def.key);
        // A typed scale colours what it FILLS. Claiming a palette without filling anything would
        // strip the settings panel's fill swatches for a tool that has no fill to colour — a control
        // removed for a tool that never had it.
        assert!(
            def.scale_swatch.is_none() || def.fills,
            "{} claims a level palette but fills nothing",
            def.key
        );
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

/// A gesture-completed tool completes itself EXACTLY: press, release, and the nodes it derives add
/// up to its click count, no more and no fewer.
///
/// Both directions are defects the panel cannot recover from. Too few nodes leave a draft that the
/// gesture cannot finish and no further click was expected to; too many spill past the figure and
/// open a second draft on the release.
#[test]
fn a_drag_completed_tool_fills_its_clicks_exactly() {
    // Every shape of gesture the panel can hand a tool, INCLUDING the degenerate ones: the count is
    // the tool's own contract and may not depend on which way the hand moved. A single sample pair
    // would pass for a tool that returns nothing for, say, a vertical drag.
    let ends = [
        ((0.0, 0.0), (40.0, 30.0)),
        ((40.0, 30.0), (0.0, 0.0)),
        ((0.0, 0.0), (40.0, 0.0)),
        ((0.0, 0.0), (0.0, 40.0)),
        ((7.0, 7.0), (7.0, 7.0)),
    ];
    for def in REGISTRY {
        let Some(rest) = def.drag_rest else { continue };
        for (from, to) in ends {
            let derived = rest(from, to);
            assert_eq!(
                derived.len() + 2,
                def.clicks as usize,
                "{} derives {} node(s) from the drag {from:?}->{to:?} but needs {} click(s)",
                def.key,
                derived.len(),
                def.clicks
            );
        }
    }
}

/// A price BAND is never completed from a drag.
///
/// Sells-to-zone spreads live sell orders across the band it was handed, so both of its prices must
/// be ones a hand pointed at. A band tool that grew a `drag_rest` would put a DERIVED price into
/// that command; the panel refuses one at run time too, and this is the half that fails loudly, at
/// the moment the registry row is written rather than the moment someone drags one.
#[test]
fn no_price_band_is_completed_from_a_drag() {
    for def in REGISTRY {
        let is_band = proto(def.tool).is_some_and(|kind| kind.price_band().is_some());
        assert!(
            !(is_band && def.drag_rest.is_some()),
            "{} is a price band and must not derive nodes from a drag",
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
        t = t.next_allowed(|_| true);
        assert!(!seen.contains(&t), "cycle repeats {t:?} before wrapping");
        seen.push(t);
    }
    assert_eq!(
        t.next_allowed(|_| true),
        first,
        "cycle must wrap to the first tool"
    );
}

/// The cycle skips the tools Moonbot's `HotKey` checkbox switches off, and survives the two
/// refusals a user can produce: the current tool excluded, and every tool excluded.
///
/// Plausible breakage: a filter applied to the STARTING index instead of to the candidates
/// returns an excluded tool whenever the current one is excluded; a `while` that waits for an
/// allowed tool hangs the frame loop when none is.
#[test]
fn tool_cycle_skips_the_tools_left_out_of_the_hotkey() {
    let first = REGISTRY[0].tool;
    let second = REGISTRY[1].tool;
    let third = REGISTRY[2].tool;

    // The immediate neighbour is excluded, so the cycle steps over it.
    assert_eq!(first.next_allowed(|t| t != second), third);
    // The CURRENT tool is excluded: the walk starts one ahead and never has to consider it.
    assert_eq!(second.next_allowed(|t| t != second), third);
    // Nothing is allowed: the tool stays put rather than looping or landing on an excluded one.
    assert_eq!(first.next_allowed(|_| false), first);
    // Exactly one tool is allowed and it is the current one: it stays selected.
    assert_eq!(first.next_allowed(|t| t == first), first);
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
        if matches!(def.tool, FigureTool::FibRetracement | FigureTool::MbFib) {
            assert!(idle > 0, "a scale must name its levels at rest");
        } else {
            assert_eq!(idle, 0, "{} labels an idle chart", def.key);
        }
    }
}

/// A switch stored for a TOOL reaches the next figure drawn with it.
///
/// The two halves of the tool-defaults path meet here: `settings_of` is what the toolbar renders,
/// `apply_settings` is what the draft applies, and if they disagreed the toolbar would offer a
/// switch that changed nothing on the chart. Fibonacci is the tool that has switches; the assertion
/// is on its own `settings()`, not on the bitmask behind them.
#[test]
fn a_switch_stored_for_a_tool_reaches_the_next_figure() {
    let tool = FigureTool::FibRetracement;
    let key = settings_of(tool, &ToolSettings::new())
        .first()
        .expect("a ratio scale offers switches")
        .key
        .clone();

    let mut stored = ToolSettings::new();
    stored.insert(key.clone(), false);
    assert!(
        settings_of(tool, &stored)
            .iter()
            .any(|s| s.key == key && !s.on),
        "the toolbar must show the switch as off"
    );

    let mut kind = (tool.def().make)(&nodes(tool.def().clicks as usize)).expect("builds");
    assert!(
        apply_settings(&mut kind, &stored),
        "the figure must take it"
    );
    assert!(
        kind.shape()
            .settings()
            .iter()
            .any(|s| s.key == key && !s.on),
        "the drawn figure must have the switch off"
    );
}

/// An unknown key in a stored map is dropped rather than panicking or being invented as a switch.
///
/// Stored maps outlive the tools they describe: a tool can lose a switch while a session still
/// holds one for it.
#[test]
fn an_unknown_stored_switch_is_ignored() {
    let mut stored = ToolSettings::new();
    stored.insert("level.99999".to_string(), false);
    stored.insert("nonsense".to_string(), true);
    for def in REGISTRY {
        let before = settings_of(def.tool, &ToolSettings::new());
        assert_eq!(settings_of(def.tool, &stored), before, "{}", def.key);
    }
}

/// Pins which tools are price BANDS, and that a band answers with the prices it was drawn from.
///
/// Plausible breakage: a tool that merely has two handles — a segment, a ray, a Fibonacci — starts
/// answering, and Sells-to-zone then spreads live sell orders across a band the user never drew;
/// or the Zone/Rect answer stops being the two clicked prices and the command addresses a
/// different band than the one on screen.
#[test]
fn only_zone_and_rect_are_price_bands() {
    let bands = [FigureTool::Channel, FigureTool::Rect];
    for def in REGISTRY {
        let drawn = (def.make)(&nodes(def.clicks as usize)).expect("full node set must build");
        let band = drawn.price_band();
        assert_eq!(
            band.is_some(),
            bands.contains(&def.tool),
            "{} disagrees about being a band",
            def.key
        );
        if let Some((a, z)) = band {
            let clicked: Vec<f64> = nodes(def.clicks as usize).iter().map(|n| n.price).collect();
            assert!(
                clicked.contains(&a) && clicked.contains(&z) && a != z,
                "{} answers with prices it was not drawn from",
                def.key
            );
        }
    }
}

/// Every tool can be asked about itself before one has been drawn.
///
/// `settings_of` builds a throwaway figure to do that; a tool whose `make` refused the throwaway
/// would silently offer no switches in the toolbar while offering them on a real figure.
#[test]
fn every_tool_answers_the_toolbar_the_same_as_a_drawn_figure() {
    for def in REGISTRY {
        let drawn = (def.make)(&nodes(def.clicks as usize)).expect("full node set must build");
        assert_eq!(
            settings_of(def.tool, &ToolSettings::new()),
            drawn.shape().settings(),
            "{} describes itself differently before it is drawn",
            def.key
        );
    }
}
