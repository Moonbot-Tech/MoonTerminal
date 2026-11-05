use super::{resolve, screener_rows, search};

/// The whole point of the gate: a process with no bench is every normal run, and a catalog answering
/// there would offer a live core's search markets it does not have.
#[test]
fn nothing_answers_outside_a_capture_run() {
    assert!(search("ace", 10).is_empty());
    assert_eq!(resolve("ACE"), None);
    assert!(screener_rows(1).is_empty());
}
