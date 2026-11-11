use super::*;

/// A layout written before the crosshair figures could move keeps their old homes.
///
/// Breakage: a default that put the depth anywhere but the book's left edge would move every
/// upgraded reader's crosshair figure on its first start.
#[test]
fn an_old_layout_keeps_the_old_homes() {
    let layout: WindowLayout = toml::from_str("[chart_graphics]\nladder_big_pct = 25\n")
        .expect("a layout without the block loads");
    let labels = layout.chart_graphics.cursor_labels;
    assert_eq!(labels.order_size, CursorLabelSpot::ChartAbove);
    assert_eq!(labels.depth, CursorLabelSpot::BookAbove);
    assert_eq!(labels.percent, CursorLabelSpot::BookBelow);
    assert_eq!(labels.order, CursorLabel::DEFAULT_ORDER);
}

/// The block survives both stores, nested inside a kind default as well as Main's own table.
///
/// Breakage: a nested table written before a scalar of its parent makes TOML refuse the whole
/// layout; a JSON spec in `charts.json` that dropped the block would reset every tab's places.
#[test]
fn the_block_round_trips_through_toml_and_json() {
    let cfg = ChartGraphicsCfg {
        cursor_labels: CursorLabelsCfg {
            order_size: CursorLabelSpot::CrossRightBelow,
            depth: CursorLabelSpot::AfterVolumesAbove,
            percent: CursorLabelSpot::Hidden,
            order: [
                CursorLabel::Percent,
                CursorLabel::OrderSize,
                CursorLabel::Depth,
            ],
        },
        ..ChartGraphicsCfg::default()
    };
    let mut layout = WindowLayout {
        chart_graphics: cfg,
        ..WindowLayout::default()
    };
    layout.chart_defaults_addto.chart_graphics = Some(cfg);
    let text = toml::to_string(&layout).expect("the layout writes");
    let back: WindowLayout = toml::from_str(&text).expect("the layout reads back");
    assert_eq!(back.chart_graphics.cursor_labels, cfg.cursor_labels);
    assert_eq!(
        back.chart_defaults_addto
            .chart_graphics
            .map(|g| g.cursor_labels),
        Some(cfg.cursor_labels)
    );
    let json: ChartGraphicsCfg =
        serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(json.cursor_labels, cfg.cursor_labels);
}

/// A spelling this build does not know costs that one figure its place, nothing else.
///
/// Breakage: a strict reader would reset the whole chart-graphics block — or, without the outer
/// lenient reader, the whole layout — over one hand-edited word.
#[test]
fn an_unknown_spot_reads_as_that_figures_default() {
    let doc = "[chart_graphics]\nladder_big_pct = 75\n\
               [chart_graphics.cursor_labels]\n\
               order_size = \"somewhere\"\n\
               depth = \"cross-left-below\"\n";
    let layout: WindowLayout = toml::from_str(doc).expect("the layout loads");
    let labels = layout.chart_graphics.cursor_labels;
    assert_eq!(labels.order_size, CursorLabelSpot::ChartAbove);
    assert_eq!(labels.depth, CursorLabelSpot::CrossLeftBelow);
    assert_eq!(labels.percent, CursorLabelSpot::BookBelow);
    assert_eq!(layout.chart_graphics.ladder_big_pct, 75);
}

/// An order that repeats a figure reads as the default, so no figure silently stops drawing.
#[test]
fn an_incomplete_order_reads_as_the_default() {
    let doc = "[chart_graphics.cursor_labels]\norder = [\"depth\", \"depth\", \"percent\"]\n";
    let layout: WindowLayout = toml::from_str(doc).expect("the layout loads");
    assert_eq!(
        layout.chart_graphics.cursor_labels.order,
        CursorLabel::DEFAULT_ORDER
    );
}

/// A step toward the line swaps with the neighbour; the ends do not wrap.
#[test]
fn shift_swaps_with_the_neighbour_and_stops_at_the_ends() {
    let mut cfg = CursorLabelsCfg::default();
    cfg.shift(CursorLabel::Depth, true);
    assert_eq!(
        cfg.order,
        [
            CursorLabel::Depth,
            CursorLabel::OrderSize,
            CursorLabel::Percent
        ]
    );
    cfg.shift(CursorLabel::Depth, true);
    assert_eq!(cfg.order[0], CursorLabel::Depth, "the first stays first");
    cfg.shift(CursorLabel::Percent, false);
    assert_eq!(cfg.order[2], CursorLabel::Percent, "the last stays last");
}
