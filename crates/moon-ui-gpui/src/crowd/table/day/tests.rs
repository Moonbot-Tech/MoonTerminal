use super::shift_mark;

#[test]
fn a_row_that_moved_says_which_way_and_by_how_much() {
    // The one part of the mark that can be read back. A CLIMB is positive because the place
    // number falls as a row goes up, and a mark that said "-3" for climbing three would be read
    // as a loss by everybody who reads the money beside it.
    assert_eq!(shift_mark(0), None, "a row that stayed put wears a mark");
    assert_eq!(shift_mark(3).as_deref(), Some("\u{2191}(+3)"));
    assert_eq!(shift_mark(-5).as_deref(), Some("\u{2193}(-5)"));
    assert_eq!(shift_mark(1).as_deref(), Some("\u{2191}(+1)"));
}
