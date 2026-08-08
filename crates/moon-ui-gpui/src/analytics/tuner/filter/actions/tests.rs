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
