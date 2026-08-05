use super::*;

use moon_core::figures::tools::{HLine, Segment};
use moon_core::figures::{DrawStyle, FigureKind};

const EPOCH: f64 = 1_700_000_000_000.0;

type Buffers = (
    Vec<ZoneInstance>,
    Vec<LineInstance>,
    Vec<SegInstance>,
    Vec<MarkerInstance>,
    Vec<FigureLabel>,
);

fn buffers() -> Buffers {
    (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
}

fn hline_fig(id: u64) -> Figure {
    let mut f = Figure::new(
        FigureKind::HLine(HLine { price: 100.0 }),
        DrawStyle {
            color: [10, 20, 30, 255],
            thickness: 2.0,
            kind: LineKind::Dash,
        },
        0,
    );
    f.id = id;
    f
}

fn segment_fig(id: u64) -> Figure {
    let mut f = Figure::new(
        FigureKind::Segment(Segment {
            a: FigNode::new(EPOCH, 100.0),
            b: FigNode::new(EPOCH + 60_000.0, 110.0),
        }),
        DrawStyle::default(),
        0,
    );
    f.id = id;
    f
}

/// Runs the builder over one figure in the given interaction state.
fn build_one(fig: &Figure, hovered: Option<u64>, selected: Option<u64>) -> Buffers {
    let (mut zones, mut hlines, mut segs, mut markers, mut labels) = buffers();
    let mut out = FigureBuffers {
        zones: &mut zones,
        hlines: &mut hlines,
        segs: &mut segs,
        markers: &mut markers,
        labels: &mut labels,
    };
    build_figure_geometry(
        std::iter::once(fig),
        None,
        FigureView {
            epoch_ms: EPOCH,
            hovered,
            selected,
            dragging: None,
        },
        &mut out,
    );
    (zones, hlines, segs, markers, labels)
}

#[test]
fn an_idle_figure_is_thin_dimmed_and_keeps_its_line_kind() {
    let fig = hline_fig(1);
    let (_, hlines, _, markers, labels) = build_one(&fig, None, None);
    assert_eq!(hlines.len(), 1);
    assert_eq!(
        hlines[0].thickness, fig.thickness,
        "idle draws at face width"
    );
    assert_eq!(hlines[0].style, 1.0, "dashed maps to style 1");
    assert!(
        hlines[0].color[3] < 1.0,
        "an idle figure must sit behind the data, not on top of it"
    );
    assert!(markers.is_empty());
    assert!(labels.is_empty(), "an idle chart draws no figure text");
}

#[test]
fn arming_and_hovering_each_make_a_figure_stand_out_more_than_idle() {
    let fig = hline_fig(1);
    let mut armed = fig.clone();
    armed.alert = true;
    let (_, idle, ..) = build_one(&fig, None, None);
    let (_, armed_lines, ..) = build_one(&armed, None, None);
    let (_, hovered, ..) = build_one(&fig, Some(1), None);

    assert!(
        armed_lines[0].thickness > idle[0].thickness,
        "an armed figure must read as armed"
    );
    assert!(hovered[0].thickness > idle[0].thickness);
    assert_eq!(armed_lines[0].color[3], 1.0);
    assert_eq!(hovered[0].color[3], 1.0);
    assert_eq!(
        armed_lines[0].style, idle[0].style,
        "arming must not change the line kind"
    );
    assert_eq!(hovered[0].style, 0.0, "hover forces a solid line");
}

#[test]
fn a_hovered_figure_gains_its_readout() {
    let fig = hline_fig(7);
    let (_, _, _, markers, labels) = build_one(&fig, Some(7), None);
    assert!(markers.is_empty(), "hover alone shows no drag knots");
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].text, LabelValue::Price(100.0));
    assert!(
        labels[0].permanent,
        "a figure already on the chart carries permanent readouts, hovered or not"
    );
    assert_eq!(labels[0].place, LabelPlace::RightEdge);
    assert_eq!(labels[0].color, 0x0A_14_1E, "label takes the figure color");
}

#[test]
fn selection_alone_carries_no_readout() {
    // Selection is sticky: a label kept alive by it would be re-formatted and re-shaped by the
    // text pass on every frame for as long as the figure stays selected.
    let fig = segment_fig(3);
    let (_, _, _, markers, labels) = build_one(&fig, None, Some(3));
    assert!(!markers.is_empty(), "a selected figure still shows handles");
    assert!(labels.is_empty());
}

#[test]
fn a_selected_segment_gains_a_knot_per_end_at_epoch_relative_times() {
    let fig = segment_fig(3);
    let (_, _, segs, markers, _) = build_one(&fig, None, Some(3));
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].t0_rel, 0.0);
    assert_eq!(segs[0].t1_rel, 60_000.0, "times are relative to the epoch");
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].t_rel, 0.0);
    assert_eq!(markers[1].t_rel, 60_000.0);
    assert_eq!(markers[0].shape, MARKER_SHAPE_KNOT);
}

#[test]
fn a_draft_draws_bright_and_thick_without_knots() {
    let (mut zones, mut hlines, mut segs, mut markers, mut labels) = buffers();
    let mut out = FigureBuffers {
        zones: &mut zones,
        hlines: &mut hlines,
        segs: &mut segs,
        markers: &mut markers,
        labels: &mut labels,
    };
    let draft = hline_fig(0);
    build_figure_geometry(
        std::iter::empty(),
        Some(&draft),
        FigureView {
            epoch_ms: EPOCH,
            hovered: None,
            selected: None,
            dragging: None,
        },
        &mut out,
    );
    assert_eq!(hlines.len(), 1);
    assert!(
        hlines[0].thickness > draft.thickness,
        "the figure under the cursor must stand out while it is being placed"
    );
    assert_eq!(hlines[0].color[3], 1.0);
    assert!(markers.is_empty());
    assert_eq!(labels.len(), 1, "a draft reads out while it is drawn");
    assert!(
        !labels[0].permanent,
        "the figure being drawn keeps its numbers whatever the label switch says"
    );
}

#[test]
fn the_builder_appends_and_never_clears() {
    let (mut zones, mut hlines, mut segs, mut markers, mut labels) = buffers();
    hlines.push(LineInstance {
        price: 1.0,
        color: [0.0; 4],
        style: 0.0,
        thickness: 1.0,
    });
    let mut out = FigureBuffers {
        zones: &mut zones,
        hlines: &mut hlines,
        segs: &mut segs,
        markers: &mut markers,
        labels: &mut labels,
    };
    let fig = hline_fig(1);
    build_figure_geometry(
        std::iter::once(&fig),
        None,
        FigureView {
            epoch_ms: EPOCH,
            hovered: None,
            selected: None,
            dragging: None,
        },
        &mut out,
    );
    assert_eq!(hlines.len(), 2, "the order layer's line was dropped");
    assert_eq!(hlines[0].price, 1.0);
}

#[test]
fn seg_patterns_collapse_the_three_dashed_kinds() {
    // The shader has one dashed pattern for Moonbot's three, and solid and dotted stay apart.
    let dashed: Vec<f32> = [LineKind::Dash, LineKind::DashDot, LineKind::DashDotDot]
        .into_iter()
        .map(seg_pattern)
        .collect();
    assert!(
        dashed.windows(2).all(|w| w[0] == w[1]),
        "the dashed kinds no longer share one pattern: {dashed:?}"
    );
    let solid = seg_pattern(LineKind::Solid);
    let dot = seg_pattern(LineKind::Dot);
    assert_ne!(solid, dot);
    assert_ne!(solid, dashed[0]);
    assert_ne!(dot, dashed[0]);
}

#[test]
fn a_fill_reaches_the_zone_layer_bounded_in_time() {
    use moon_core::figures::tools::FibRetracement;

    let f = Figure::new(
        FigureKind::FibRetracement(FibRetracement {
            a: FigNode::new(EPOCH, 100.0),
            b: FigNode::new(EPOCH + 60_000.0, 80.0),
        }),
        DrawStyle::default(),
        0,
    );
    let (zones, ..) = build_one(&f, None, None);
    assert!(!zones.is_empty(), "the scale drew no fill");
    for z in &zones {
        assert_eq!(
            (z.t0_rel, z.t1_rel),
            (0.0, 60_000.0),
            "fill is not bounded to the move"
        );
        assert!(
            z.price0 < z.price1,
            "band prices must be ordered for the shader"
        );
        assert!(z.color[3] < 0.5, "a fill must stay behind the candles");
    }
}

#[test]
fn an_order_zone_spans_the_whole_plot_with_a_finite_sentinel() {
    // The shader clamps the sentinel to the plot's edge, so order zones keep looking exactly as
    // they did. It must stay FINITE: the Metal library is built with fast math, where an infinite
    // operand is not guaranteed to survive `clamp`.
    let z = ZoneInstance::full_width(1.0, 2.0, [0.0; 4]);
    assert_eq!(z.t0_rel, -crate::layers::TIME_UNBOUNDED);
    assert_eq!(z.t1_rel, crate::layers::TIME_UNBOUNDED);
    assert!(z.t0_rel.is_finite() && z.t1_rel.is_finite());
}

#[test]
fn a_fill_never_reacts_to_hover_or_selection() {
    // A fill rides the base cache; re-tinting it on hover re-bakes the whole chart background.
    use moon_core::figures::tools::FibRetracement;
    let f = Figure::new(
        FigureKind::FibRetracement(FibRetracement {
            a: FigNode::new(EPOCH, 100.0),
            b: FigNode::new(EPOCH + 60_000.0, 80.0),
        }),
        DrawStyle::default(),
        7,
    );
    let (idle, ..) = build_one(&f, None, None);
    let (hovered, ..) = build_one(&f, Some(7), None);
    let (selected, ..) = build_one(&f, None, Some(7));
    let colors = |z: &[ZoneInstance]| z.iter().map(|z| z.color).collect::<Vec<_>>();
    assert_eq!(colors(&idle), colors(&hovered));
    assert_eq!(colors(&idle), colors(&selected));
}

#[test]
fn a_level_readout_is_rendered_to_text_once_not_every_frame() {
    use moon_core::figures::tools::FibRetracement;
    let f = Figure::new(
        FigureKind::FibRetracement(FibRetracement {
            a: FigNode::new(EPOCH, 63_713.5),
            b: FigNode::new(EPOCH + 60_000.0, 61_240.25),
        }),
        DrawStyle::default(),
        0,
    );
    let (_, _, _, _, labels) = build_one(&f, None, None);
    assert!(!labels.is_empty());
    for l in &labels {
        assert!(
            l.permanent,
            "a scale's readouts are drawn without the pointer on it"
        );
        match &l.text {
            LabelValue::Ready(s) => {
                assert!(s.contains('('), "a level must name its price: {s}");
                // The price axis would round this to one decimal above 1000; a level must not.
                assert!(s.contains('.'), "level price lost its precision: {s}");
            }
            other => panic!("a level readout must arrive finished: {other:?}"),
        }
    }
    // Two neighbouring levels must not print as the same number.
    let texts: Vec<String> = labels
        .iter()
        .map(|l| match &l.text {
            LabelValue::Ready(s) => s.to_string(),
            _ => unreachable!(),
        })
        .collect();
    let mut sorted = texts.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        texts.len(),
        "two levels read alike: {texts:?}"
    );
}

#[test]
fn the_figure_being_dragged_paints_no_fill() {
    // A fill lives in the base cache; one that moved with the cursor would re-bake the whole
    // chart background at mouse-move rate.
    use moon_core::figures::tools::FibRetracement;
    let mut f = Figure::new(
        FigureKind::FibRetracement(FibRetracement {
            a: FigNode::new(EPOCH, 100.0),
            b: FigNode::new(EPOCH + 60_000.0, 80.0),
        }),
        DrawStyle::default(),
        0,
    );
    f.id = 5;
    let (still, ..) = build_one(&f, None, None);
    assert!(!still.is_empty());

    let (mut zones, mut hlines, mut segs, mut markers, mut labels) = buffers();
    let mut out = FigureBuffers {
        zones: &mut zones,
        hlines: &mut hlines,
        segs: &mut segs,
        markers: &mut markers,
        labels: &mut labels,
    };
    build_figure_geometry(
        std::iter::once(&f),
        None,
        FigureView {
            epoch_ms: EPOCH,
            hovered: None,
            selected: Some(5),
            dragging: Some(5),
        },
        &mut out,
    );
    assert!(zones.is_empty(), "a dragged figure re-baked the base cache");
    assert!(!segs.is_empty(), "but its lines must still be drawn");
}
