// Explicit imports avoid pulling the parent's `gpui::*`, whose `test` shadows the built-in
// attribute and recursively expands `#[test]`.
use super::{analyzer_stamp, suggestion_inputs};

/// Empty searched sides must clear old zero-off bounds without inventing a Min-only Max.
#[test]
fn suggestion_inputs_clear_open_zero_off_sides() {
    let fields = moon_core::db::tuner::FIELDS;
    let pricebug = fields.iter().find(|f| f.col == "pricebug").unwrap();
    assert_eq!(
        suggestion_inputs(pricebug, None, Some(2.0)),
        ("0".into(), "2".into())
    );
    let detect = fields.iter().find(|f| f.col == "xpricedelta").unwrap();
    assert_eq!(
        suggestion_inputs(detect, Some(1.0), None),
        ("1".into(), "".into())
    );
}

/// A hidden Detect bound must neither serialize an unscored threshold nor keep Save amber.
#[test]
fn seam_scope_expansion_omits_detect_from_the_actual_save_payload() {
    let mut state = super::super::state::TunerState::load(None, None, None, None, None, false);
    let detect = moon_core::db::tuner::FIELDS
        .iter()
        .position(|s| s.col == "xpricedelta")
        .expect("Detect");
    state.bounds[0][detect] = ("3".into(), String::new());
    assert!(state.save_dirty(true));
    assert_eq!(
        state.build_strategy_changes(false, true).0,
        vec![("DropsPriceDelta".into(), "3".into())]
    );
    state.invalidate();
    assert!(state.build_strategy_changes(true, false).0.is_empty());
    assert!(!state.save_dirty(false));
    state.invalidate();
    assert!(state.save_dirty(true));
    assert_eq!(
        state.build_strategy_changes(false, true).0,
        vec![("DropsPriceDelta".into(), "3".into())]
    );
}

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

/// A joint search started off a Drops scope must not search or fill xPriceDelta.
///
/// The completion closure needs an `AnalyticsView`, which these tests do not construct.
/// This pins the two branches that decide it. The scoring of a missing Detect value is
/// covered by the moon-core search tests.
///
/// Breakage this pins: building `locked` from the checkbox alone. The default mask admits
/// every mapped field, so xPriceDelta would be searched on a MoonShot scope and its Min
/// written back into the box.
#[test]
fn a_joint_search_excludes_detect_outside_a_drops_scope() {
    let source = include_str!("../actions.rs").replace("\r\n", "\n");
    let body = source
        .split_once("fn suggest_into_v1(")
        .expect("suggest_into_v1")
        .1
        .split("\n    /// Stop the running joint search")
        .next()
        .expect("suggest_into_v1 body");
    assert!(
        body.contains("if FIELDS[fi].class == FieldClass::Detect && !drops_only {"),
        "a Detect field outside a Drops scope is excluded from the search"
    );
    assert!(
        body.contains("Some((None, None))"),
        "exclusion is the locked (None, None) state, not a fixed numeric bound"
    );
    let completion = body
        .split_once("move |this, sugg, cx|")
        .expect("completion")
        .1;
    assert!(
        completion.contains("if field.class == FieldClass::Detect && !drops_only {"),
        "the completion must keep the scope captured when the search started"
    );
}

/// A single-field click on a hidden Detect row must return before a search starts.
#[test]
fn a_single_field_search_refuses_a_hidden_detect_field() {
    let source = include_str!("../actions.rs").replace("\r\n", "\n");
    let body = source
        .split_once("fn suggest_one_into_v1(")
        .expect("suggest_one_into_v1")
        .1
        .split("\n    /// Copy bounds v1")
        .next()
        .expect("suggest_one body");
    let before_spawn = body.split("self.spawn_db(").next().expect("spawn");
    assert!(
        before_spawn
            .contains("if !super::offer_field(FIELDS[fi].class, self.scope_is_drops_only())"),
        "the refusal must happen before the search is dispatched"
    );
}
