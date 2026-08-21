//! The one rule in the caption-module editor that is not a layout.

use super::clamped;

/// Removing captions must not leave the settings pane pointing past the list — the pane reads
/// `row.parts[selected]`, and a selection past the end would describe a caption that is gone.
#[test]
fn the_selection_follows_a_shrinking_list() {
    assert_eq!(clamped(3, 4), 3, "a valid selection is left alone");
    assert_eq!(
        clamped(3, 2),
        1,
        "a removal pulls it back to the last caption"
    );
    assert_eq!(
        clamped(0, 0),
        0,
        "an empty module selects where the first caption will land"
    );
}
