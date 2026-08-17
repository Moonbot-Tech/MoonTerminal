use super::*;
use crate::figures::tools::FigureTool;

fn window() -> SeedWindow {
    SeedWindow {
        from_ms: 1_700_000_000_000.0,
        to_ms: 1_700_086_400_000.0,
        low: 0.15,
        high: 0.35,
    }
}

/// Every slot must produce the tool it names. A silent mismatch would leave the bench missing one
/// tool and carrying another twice, which is exactly what the bench exists to make visible.
#[test]
fn every_slot_builds_its_own_tool() {
    let w = window();
    let kinds: Vec<FigureTool> = SLOTS
        .iter()
        .enumerate()
        .map(|(i, slot)| figure_for(*slot, &w, 0.05 + 0.10 * i as f64).tool())
        .collect();
    let expected = [
        FigureTool::HLine,
        FigureTool::Segment,
        FigureTool::Ray,
        FigureTool::Rect,
        FigureTool::Triangle,
        FigureTool::Channel,
        FigureTool::FibRetracement,
        FigureTool::MbFib,
        FigureTool::Position,
    ];
    assert_eq!(kinds, expected);
}

/// The set covers every tool exactly once: a tool added to the build and forgotten here would
/// leave the bench unable to show it.
#[test]
fn the_set_has_no_duplicates() {
    let w = window();
    let mut tools: Vec<FigureTool> = SLOTS
        .iter()
        .map(|slot| figure_for(*slot, &w, 0.2).tool())
        .collect();
    let total = tools.len();
    tools.sort_unstable_by_key(|t| format!("{t:?}"));
    tools.dedup();
    assert_eq!(tools.len(), total, "a tool is laid out twice");
}

/// Nodes must land INSIDE the traded window; a figure outside it is off screen when the chart
/// opens on the trades, which is the one place the bench is looked at.
#[test]
fn nodes_stay_inside_the_window() {
    let w = window();
    for (index, slot) in SLOTS.iter().enumerate() {
        let kind = figure_for(*slot, &w, 0.05 + 0.10 * index as f64);
        let shape = kind.shape();
        for node in (0..shape.handle_count()).filter_map(|i| shape.handle(i)) {
            // A price-only tool keeps a node whose time is meaningless; only its price matters.
            let price_only = matches!(kind.tool(), FigureTool::HLine | FigureTool::Channel);
            if !price_only {
                assert!(
                    node.time_ms >= w.from_ms && node.time_ms <= w.to_ms,
                    "{:?}: time {} outside [{}, {}]",
                    kind.tool(),
                    node.time_ms,
                    w.from_ms,
                    w.to_ms
                );
            }
            assert!(
                node.price >= w.low && node.price <= w.high,
                "{:?}: price {} outside [{}, {}]",
                kind.tool(),
                node.price,
                w.low,
                w.high
            );
        }
    }
}

/// Slots are spread across the window rather than stacked: overlapping tools hide each other's
/// handles, and then the bench cannot answer what a tool looks like.
#[test]
fn slots_do_not_share_one_position() {
    let w = window();
    let mut starts: Vec<i64> = SLOTS
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let kind = figure_for(*slot, &w, 0.05 + 0.10 * index as f64);
            // Price-only tools have no meaningful time; they are separated by price instead.
            (!matches!(kind.tool(), FigureTool::HLine | FigureTool::Channel))
                .then(|| kind.shape().handle(0).map(|n| n.time_ms as i64))
                .flatten()
        })
        .collect();
    let total = starts.len();
    starts.sort_unstable();
    starts.dedup();
    assert_eq!(starts.len(), total, "two tools start at the same moment");
}

/// Both filled and unfilled rendering must be on the bench at once, and the styles must vary:
/// a set drawn in one colour and one thickness cannot show whether either is honoured.
#[test]
fn styles_vary_across_the_set() {
    let styles: Vec<DrawStyle> = (0..SLOTS.len()).map(style_for).collect();
    assert!(
        styles.iter().any(|s| s.fill[3] > 0),
        "no filled figure on the bench"
    );
    assert!(
        styles.iter().any(|s| s.fill[3] == 0),
        "no unfilled figure on the bench"
    );
    let mut thicknesses: Vec<u32> = styles.iter().map(|s| s.thickness as u32).collect();
    thicknesses.sort_unstable();
    thicknesses.dedup();
    assert!(thicknesses.len() > 1, "every figure has one thickness");
}
