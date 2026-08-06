use super::*;
use crate::figures::tools::HLine;

fn fig() -> Figure {
    Figure::new(
        FigureKind::HLine(HLine { price: 100.0 }),
        DrawStyle::default(),
        0.0,
    )
}

/// A figure's four style fields and the `DrawStyle` view of them are the same value.
///
/// The settings panel edits a `DrawStyle` and writes it back through these two, so a field added to
/// one side and forgotten on the other would silently stop being editable — the control would move
/// and the figure would not. The oracle is a style that differs in EVERY field.
#[test]
fn a_style_survives_the_round_trip_through_a_figure() {
    let style = DrawStyle {
        color: [1, 2, 3, 4],
        thickness: 4.5,
        kind: LineKind::DashDotDot,
        fill: [5, 6, 7, 8],
    };
    let mut f = fig();
    assert!(f.set_style(style), "a different style must be applied");
    assert_eq!(f.style(), style);
    // And every field individually reached the figure, not just the view of it.
    assert_eq!(f.color, style.color);
    assert_eq!(f.thickness, style.thickness);
    assert_eq!(f.line_kind, style.kind);
    assert_eq!(f.fill, style.fill);
}

/// Applying the style a figure already has reports no change.
///
/// The edit paths use that answer to decide whether to save the store and re-upsert an armed
/// figure's blob to the core; a `true` here would send a command per click of a control that moved
/// nothing.
#[test]
fn re_applying_the_same_style_reports_no_change() {
    let mut f = fig();
    let style = f.style();
    assert!(!f.set_style(style));
}
