use super::*;

/// A draft of `tool` with its first node already placed, as a press leaves it.
fn started(tool: FigureTool) -> FigDraft {
    let mut d = FigDraft::new(
        0,
        1,
        "GateF/BTC".to_string(),
        tool,
        DrawStyle::default(),
        ToolSettings::new(),
        FigNode::new(0.0, 100.0),
    );
    d.place(FigNode::new(0.0, 100.0));
    d
}

/// A live gesture previews the FINISHED figure, not the tool's partial shape.
///
/// Plausible breakage: routing the preview through `preview` regardless, which shows a triangle's
/// base edge while the release lands a whole triangle — dragging one thing and getting another.
#[test]
fn a_gesture_previews_what_the_release_will_build() {
    let mut d = started(FigureTool::Triangle);
    d.cursor = FigNode::new(10_000.0, 110.0);
    let edge = d.preview().expect("a placed vertex previews something");
    assert!(matches!(edge.kind, FigureKind::Segment(_)));

    assert!(d.set_drag_rest(vec![FigNode::new(5_000.0, 130.0)]));
    let whole = d.preview().expect("a gesture previews its figure");
    assert!(matches!(whole.kind, FigureKind::Triangle(_)));
}

/// The derived nodes are the ones the gesture holds now, and repeating them changes nothing.
#[test]
fn derived_nodes_report_only_real_changes() {
    let mut d = started(FigureTool::Triangle);
    let apex = vec![FigNode::new(5_000.0, 130.0)];
    assert!(d.set_drag_rest(apex.clone()));
    assert!(!d.set_drag_rest(apex), "an unchanged set must not redraw");
    assert!(d.set_drag_rest(Vec::new()), "clearing is a change");
}

/// Placing a node drops the derived set, which the caller has just placed as ordinary nodes.
///
/// Plausible breakage: keeping it, so the apex is previewed a second time on top of the vertex it
/// already became — and, for a tool needing one more click, previewed against the wrong nodes.
#[test]
fn placing_a_node_retires_the_derived_set() {
    let mut d = started(FigureTool::Triangle);
    assert!(d.set_drag_rest(vec![FigNode::new(5_000.0, 130.0)]));
    assert!(d.place(FigNode::new(10_000.0, 100.0)).is_none(), "two of three vertices");
    let preview = d.preview().expect("two vertices preview a triangle");
    assert!(matches!(preview.kind, FigureKind::Triangle(_)));
    // The derived node is gone: clearing it again reports no change.
    assert!(!d.set_drag_rest(Vec::new()));
}
