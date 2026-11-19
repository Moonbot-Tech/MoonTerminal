//! The auto-figures list's approach box.

use super::{DEFAULT_APPROACH_PCT, approach_pct};

/// An empty box is the default distance, a comma is a decimal point, and text that is not a number
/// is not some other distance.
///
/// Breakage: an empty box filtering at 0 % (no coin is ever exactly on a line), `0,8` read as 0 or
/// as 8, or `abc` falling back to the default — the table would then claim to answer a question
/// the user did not type.
#[test]
fn the_approach_box_reads_a_percentage() {
    assert_eq!(approach_pct(""), DEFAULT_APPROACH_PCT);
    assert_eq!(approach_pct("  "), DEFAULT_APPROACH_PCT);
    assert_eq!(approach_pct("0,8"), 0.8);
    assert_eq!(approach_pct(" 1.25 "), 1.25);
    assert!(approach_pct("abc").is_nan());
}
