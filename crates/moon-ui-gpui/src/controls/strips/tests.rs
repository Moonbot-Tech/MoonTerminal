//! Regression coverage for preset-strip wheel gating and handler wiring.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use super::{
    SellClickAction, SizeClickAction, StripForm, StripGeometry, collapsed_label,
    collapsed_sell_cell, sell_click_action, sell_click_action_in, sell_labels, sell_menu_action,
    size_click_action, size_labels, wheel_step_dir,
};
use gpui::{Modifiers, Point, ScrollDelta};

/// Build a vertical line-based scroll delta for wheel-gate tests.
///
/// Args:
///     y: Signed vertical line delta.
///
/// Returns:
///     A vertical `ScrollDelta`.
fn lines(y: f32) -> ScrollDelta {
    ScrollDelta::Lines(Point { x: 0.0, y })
}

/// Build modifiers containing only Ctrl.
///
/// Returns:
///     Ctrl-only modifiers.
fn ctrl() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

/// `strips.rs:wheel_step_dir` must reject an unmodified wheel gesture.
///
/// Removing the modifier gate would let ordinary toolbar scrolling silently rewrite an order size
/// or sell percentage.
#[test]
fn bare_wheel_never_changes_a_preset() {
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(1.0)), None);
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(-1.0)), None);
}

/// `strips.rs:wheel_step_dir` must preserve both Ctrl-wheel directions.
///
/// Reversing the Y comparison or rejecting Ctrl-modified input changes a trading value in the
/// wrong direction or leaves it unchanged.
#[test]
fn ctrl_wheel_reports_direction() {
    assert_eq!(wheel_step_dir(ctrl(), lines(1.0)), Some(true));
    assert_eq!(wheel_step_dir(ctrl(), lines(-1.0)), Some(false));
}

/// `strips.rs:wheel_step_dir` must reject a horizontal trackpad gesture.
///
/// Treating zero Y as a downward step would shrink a trading parameter during sideways scrolling.
#[test]
fn horizontal_gesture_is_not_a_downward_step() {
    assert_eq!(wheel_step_dir(ctrl(), lines(0.0)), None);
}

/// `strips.rs:size_click_action` must preserve selection and double-click editing with the native
/// clicked index. Reversing or collapsing the click-count branch selects a preset when the user
/// requested its inline editor.
#[test]
fn order_size_clicks_preserve_selection_and_edit_semantics() {
    assert_eq!(size_click_action(4, 1), SizeClickAction::Select(4));
    assert_eq!(size_click_action(2, 2), SizeClickAction::Edit(2));
    assert_eq!(size_click_action(5, 3), SizeClickAction::Edit(5));
}

/// `strips.rs:sell_click_action` must distinguish fixed-slot selection, active-slot restoration,
/// and double-click editing. Treating the selected slot as zero-based or evaluating it before the
/// double-click branch changes the live take-profit mode instead of opening the editor.
#[test]
fn fixed_sell_clicks_preserve_slot_and_edit_semantics() {
    assert_eq!(
        sell_click_action(3, Some(1), 1),
        SellClickAction::SelectFixed(4)
    );
    assert_eq!(
        sell_click_action(3, Some(4), 1),
        SellClickAction::EngageMain
    );
    assert_eq!(sell_click_action(3, Some(4), 2), SellClickAction::Edit(3));
    // The full strip through the form-aware resolver is the same resolver.
    assert_eq!(
        sell_click_action_in(StripForm::Full, 3, Some(4), 1),
        Some(SellClickAction::EngageMain)
    );
    assert_eq!(
        sell_click_action_in(StripForm::Full, 3, Some(4), 2),
        Some(SellClickAction::Edit(3))
    );
    // Collapsed: the double click's first click must not clear the engaged slot (it would take the
    // cell off the row before the editor mounts); the double click edits that preset.
    assert_eq!(
        sell_click_action_in(StripForm::Collapsed, 3, Some(4), 1),
        None
    );
    assert_eq!(
        sell_click_action_in(StripForm::Collapsed, 3, Some(4), 2),
        Some(SellClickAction::Edit(3))
    );
}

/// The production preset-label path must compact large values without mutating its inputs.
///
/// Routing `size_labels` back through raw adaptive formatting makes the final two literal labels
/// fail while leaving the shared formatter's own tests green.
#[test]
fn order_size_labels_use_shared_compact_formatter() {
    let values = [50.0, 100.0, 250.0, 500.0, 10_000.0, 1_000_000.0];

    assert_eq!(
        size_labels(values),
        ["50", "100", "250", "500", "10k", "1m"].map(str::to_string)
    );
    assert_eq!(values, [50.0, 100.0, 250.0, 500.0, 10_000.0, 1_000_000.0]);
}

/// Both `strips.rs:size_strip` and `sell_strip` must route native MoonUI scroll callbacks through
/// the Ctrl gate.
///
/// Reading the raw delta in either callback would defeat the gate while leaving the pure helper
/// tests green.
#[test]
fn both_wheel_handlers_consult_the_gate_rather_than_the_raw_delta() {
    let source = include_str!("../strips.rs");
    let implementation = source
        .split("#[cfg(test)]\nmod tests;")
        .next()
        .unwrap_or(source);

    assert_eq!(
        implementation
            .matches("wheel_step_dir(event.modifiers, event.delta)")
            .count(),
        2,
        "both preset strips must route native wheel events through wheel_step_dir"
    );
}

/// Geometry shared by the collapse tests: six distinct cells, a 2 px frame and a 20 px caret.
const GEOMETRY: StripGeometry = StripGeometry {
    cells: [40.0, 41.0, 42.0, 43.0, 44.0, 45.0],
    main: 50.0,
    frame: 2.0,
    caret: 20.0,
    caret_gap: 2.0,
};

/// `strips.rs:StripGeometry::collapsed` must cost the WIDEST cell (the main one included), its
/// frame and the caret, whichever cell is selected: costing the selected cell lets a pick flip the
/// toolbar's level under the cursor.
#[test]
fn collapsed_width_is_identical_for_every_selection() {
    assert_eq!(GEOMETRY.collapsed_cell(), 50.0);
    assert_eq!(GEOMETRY.collapsed(), 50.0 + 2.0 + 2.0 + 20.0);
    let sizes = StripGeometry {
        main: 0.0,
        ..GEOMETRY
    };
    assert_eq!(sizes.collapsed(), 45.0 + 2.0 + 2.0 + 20.0);
    assert_eq!(GEOMETRY.full(), 255.0 + 2.0);
}

/// The collapsed cell must read the label of the cell it stands for, and the main take-profit
/// value while no S slot is engaged — never another preset's figure.
#[test]
fn collapsed_label_is_the_selected_cell_label() {
    let sizes = size_labels([50.0, 100.0, 250.0, 500.0, 10_000.0, 1_000_000.0]);
    assert_eq!(collapsed_label(&sizes, Some(2), "1.5%"), "250");
    let sells = sell_labels([0.5, 1.0, 1.5, 2.0, 3.0, 5.0]);
    assert_eq!(
        collapsed_label(&sells, collapsed_sell_cell(Some(4)), "1.5%"),
        sells[3]
    );
    assert_eq!(
        collapsed_label(&sells, collapsed_sell_cell(None), "1.5%"),
        "1.5%"
    );
}

/// The three sell states — an engaged slot, the main take profit engaged, and a manual strategy
/// owning the target (which the toolbar hands over as no slot) — must resolve through the same
/// resolver the full strip uses: picking the engaged row restores the main take profit, any other
/// row engages that slot, and with no slot engaged the collapsed cell stands for no preset.
#[test]
fn three_sell_states_resolve_to_the_full_strip_actions() {
    // Engaged S4: the collapsed cell is S4, and picking its menu row is the full strip's single
    // click on that cell (a single click on the collapsed cell itself does nothing).
    assert_eq!(collapsed_sell_cell(Some(4)), Some(3));
    assert_eq!(
        sell_menu_action(3, Some(4)),
        sell_click_action(3, Some(4), 1)
    );
    assert_eq!(sell_menu_action(3, Some(4)), SellClickAction::EngageMain);
    assert_eq!(
        sell_menu_action(0, Some(4)),
        SellClickAction::SelectFixed(1)
    );
    // Main TP engaged, or a manual strategy owning the sell target: no cell, every row engages.
    assert_eq!(collapsed_sell_cell(None), None);
    for index in 0..6 {
        assert_eq!(
            sell_menu_action(index, None),
            SellClickAction::SelectFixed(index + 1)
        );
        assert_eq!(
            sell_menu_action(index, None),
            sell_click_action(index, None, 1)
        );
    }
    // An out-of-range slot never addresses a cell.
    assert_eq!(collapsed_sell_cell(Some(7)), None);
    assert_eq!(collapsed_sell_cell(Some(0)), None);
}

/// Both forms must install the same handler builders, so the collapsed cell's double-click and
/// Ctrl+wheel reach exactly the full strip's code paths.
#[test]
fn both_forms_share_one_handler_builder_per_strip() {
    let source = include_str!("../strips.rs");
    let implementation = source
        .split("#[cfg(test)]\nmod tests;")
        .next()
        .unwrap_or(source);
    for builder in [
        ".on_click(size_on_click(",
        ".on_scroll(size_on_scroll(",
        ".on_click(sell_on_click(",
        ".on_scroll(sell_on_scroll(",
    ] {
        assert_eq!(
            implementation.matches(builder).count(),
            1,
            "{builder} must be installed once, for both forms"
        );
    }
}
