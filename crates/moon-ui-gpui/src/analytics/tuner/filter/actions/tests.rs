// Explicit imports avoid pulling the parent's `gpui::*`, whose `test` shadows the built-in
// attribute and recursively expands `#[test]`.
use super::analyzer_stamp;

/// Replacing `actions.rs:analyzer_stamp` with the former UTC formatter makes saved and copied
/// strategy comments disagree with the selected Warsaw display zone.
#[test]
fn analyzer_stamp_follows_the_selected_display_zone() {
    assert_eq!(
        analyzer_stamp(1_784_968_010, chrono_tz::Europe::Warsaw),
        "25.07.2026 10:26:50 (Save from analyzer)"
    );
}

/// `filter/actions.rs:suggest_into_v1` must copy the live axis mark onto `SuggestState::Done`.
///
/// The completion closure is the callback `spawn_db` runs after the database worker returns.
/// Driving that closure needs an `AnalyticsView` and a GPUI context, which this binary crate's
/// unit tests do not construct. The gap is that this test does not execute the closure. It pins
/// the copy step itself: the value read from `SuggestState::axis_moved` is the value stored on
/// `Done`.
///
/// Breakage: `let axis_moved = this.tuner.sugg.axis_moved();` becoming `let axis_moved = false`,
/// or the `Done` literal hardcoding `axis_moved: false`. The run finishes and the status band
/// never says the report time axis shifted, so the user treats a fit from the old axis as current.
#[test]
fn a_finished_joint_search_copies_the_axis_move_onto_the_result() {
    let source = include_str!("../actions.rs").replace("\r\n", "\n");
    let body = source
        .split_once("fn suggest_into_v1(")
        .expect("suggest_into_v1")
        .1
        .split("\n    }\n")
        .next()
        .expect("suggest_into_v1 body");
    let completion = body
        .split_once("move |this, sugg, cx|")
        .expect("suggestion completion closure")
        .1;
    assert!(
        completion.contains("let axis_moved = this.tuner.sugg.axis_moved();"),
        "the completion must copy SuggestState::axis_moved, not a constant"
    );
    let done = completion
        .split_once("SuggestState::Done {")
        .expect("Done construction")
        .1
        .split_once("\n                };")
        .expect("Done construction close")
        .0;
    assert!(
        done.contains("\n                    axis_moved,"),
        "Done must store the copied binding"
    );
    assert!(
        !done.contains("axis_moved: false") && !done.contains("axis_moved: true"),
        "hardcoding the mark on Done drops the copy"
    );
}
