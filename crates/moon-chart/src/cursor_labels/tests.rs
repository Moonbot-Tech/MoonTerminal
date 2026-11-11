use super::*;
use moon_core::config::CursorLabelSpot;

const ALL_HOMES: CursorLabelHomes = CursorLabelHomes {
    row_volumes: true,
    cross: true,
};

/// The figure's slot, looked up by figure rather than by position in the result.
fn slot(out: &[Option<CursorLabelSlot>; 3], label: CursorLabel) -> Option<CursorLabelSlot> {
    out.iter().flatten().find(|s| s.label == label).copied()
}

/// The default places reproduce the old picture: size on the chart, depth and percent in the
/// book on either side of the line, each nearest its line.
#[test]
fn the_defaults_are_the_old_homes() {
    let out = resolve(&CursorLabelsCfg::default(), |_| true, ALL_HOMES);
    let size = slot(&out, CursorLabel::OrderSize).unwrap();
    let depth = slot(&out, CursorLabel::Depth).unwrap();
    let pct = slot(&out, CursorLabel::Percent).unwrap();
    assert_eq!(
        (size.anchor, size.below, size.rank),
        (CursorLabelAnchor::Chart, false, 0)
    );
    assert_eq!(
        (depth.anchor, depth.below, depth.rank),
        (CursorLabelAnchor::Book, false, 0)
    );
    assert_eq!(
        (pct.anchor, pct.below, pct.rank),
        (CursorLabelAnchor::Book, true, 0)
    );
}

/// Figures sharing a home and a side stack outward in the stored order; the other side does not
/// count.
#[test]
fn a_shared_place_stacks_in_the_stored_order() {
    let cfg = CursorLabelsCfg {
        order_size: CursorLabelSpot::BookAbove,
        depth: CursorLabelSpot::BookAbove,
        percent: CursorLabelSpot::BookBelow,
        order: [
            CursorLabel::Depth,
            CursorLabel::Percent,
            CursorLabel::OrderSize,
        ],
    };
    let out = resolve(&cfg, |_| true, ALL_HOMES);
    assert_eq!(slot(&out, CursorLabel::Depth).unwrap().rank, 0);
    assert_eq!(slot(&out, CursorLabel::OrderSize).unwrap().rank, 1);
    assert_eq!(slot(&out, CursorLabel::Percent).unwrap().rank, 0);
}

/// A hidden or absent figure leaves no gap in the stack.
#[test]
fn a_missing_figure_takes_no_rank() {
    let cfg = CursorLabelsCfg {
        order_size: CursorLabelSpot::BookAbove,
        depth: CursorLabelSpot::BookAbove,
        percent: CursorLabelSpot::BookAbove,
        order: CursorLabel::DEFAULT_ORDER,
    };
    let out = resolve(&cfg, |l| l != CursorLabel::OrderSize, ALL_HOMES);
    assert!(slot(&out, CursorLabel::OrderSize).is_none());
    assert_eq!(slot(&out, CursorLabel::Depth).unwrap().rank, 0);
    assert_eq!(slot(&out, CursorLabel::Percent).unwrap().rank, 1);

    let hidden = CursorLabelsCfg {
        depth: CursorLabelSpot::Hidden,
        ..cfg
    };
    let out = resolve(&hidden, |_| true, ALL_HOMES);
    assert!(slot(&out, CursorLabel::Depth).is_none());
    assert_eq!(slot(&out, CursorLabel::Percent).unwrap().rank, 1);
}

/// A home the frame cannot offer falls back to the book edge and stacks with what is already
/// there, rather than printing over it.
#[test]
fn a_fallback_home_stacks_with_the_book_edge() {
    let cfg = CursorLabelsCfg {
        order_size: CursorLabelSpot::CrossRightAbove,
        depth: CursorLabelSpot::BookAbove,
        percent: CursorLabelSpot::AfterVolumesAbove,
        order: CursorLabel::DEFAULT_ORDER,
    };
    let out = resolve(
        &cfg,
        |_| true,
        CursorLabelHomes {
            row_volumes: false,
            cross: false,
        },
    );
    let ranks: Vec<_> = out
        .iter()
        .flatten()
        .map(|s| (s.label, s.anchor, s.rank))
        .collect();
    assert_eq!(
        ranks,
        vec![
            (CursorLabel::OrderSize, CursorLabelAnchor::Book, 0),
            (CursorLabel::Depth, CursorLabelAnchor::Book, 1),
            (CursorLabel::Percent, CursorLabelAnchor::Book, 2),
        ]
    );
    let offered = resolve(&cfg, |_| true, ALL_HOMES);
    assert_eq!(
        slot(&offered, CursorLabel::OrderSize).unwrap().anchor,
        CursorLabelAnchor::CrossRight
    );
    assert_eq!(
        slot(&offered, CursorLabel::Percent).unwrap().anchor,
        CursorLabelAnchor::AfterVolumes
    );
}

/// The two homes beside the crosshair stack in one column, so a figure the pane edge pulls
/// across the crosshair cannot land on the other one.
#[test]
fn both_crosshair_homes_share_one_column() {
    let cfg = CursorLabelsCfg {
        order_size: CursorLabelSpot::CrossLeftAbove,
        depth: CursorLabelSpot::CrossRightAbove,
        percent: CursorLabelSpot::CrossRightBelow,
        order: CursorLabel::DEFAULT_ORDER,
    };
    let out = resolve(&cfg, |_| true, ALL_HOMES);
    let size = slot(&out, CursorLabel::OrderSize).unwrap();
    let depth = slot(&out, CursorLabel::Depth).unwrap();
    assert_eq!((size.anchor, size.rank), (CursorLabelAnchor::CrossLeft, 0));
    assert_eq!(
        (depth.anchor, depth.rank),
        (CursorLabelAnchor::CrossRight, 1)
    );
    assert_eq!(slot(&out, CursorLabel::Percent).unwrap().rank, 0);
}
